use log::debug;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::SyncSender,
};
use std::thread;
use std::time::Duration;

use crate::dsp::tx::{IqResampler, TxConfig, TxMode, TxModulator, tx_dma_audio_fs};

/// Spawns the processing thread dedicated to pulling input audio chunks,
/// running SSB modulation, and queueing output IQ buffers to the hardware TX IO thread.
pub fn spawn_tx_dsp_thread(
    shutdown_flag: Arc<AtomicBool>,
    mut tx_audio_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<f32>>,
    iq_tx: SyncSender<(Vec<i16>, Vec<i16>)>,
    tx_fs_atomic: Arc<AtomicU32>,
    tx_config: Arc<Mutex<TxConfig>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // --- Thread state ---
        let mut modulator = TxModulator::new(
            TxMode::USB,
            3_000.0,
            tx_fs_atomic.load(Ordering::Relaxed) as f32,
        );
        let mut current_fs = modulator.tx_fs;
        let mut current_mode = modulator.mode.clone();
        let mut current_bw = modulator.filter_bw;
        // Upsamples the 48 kHz modulated IQ to the rate the FPGA drains the DMA at.
        let mut resampler = IqResampler::for_dma_fs(tx_dma_audio_fs(current_fs));
        let mut tx_processed_count = 0u64;

        // --- Modulation loop ---
        while !shutdown_flag.load(Ordering::Relaxed) {
            let active = {
                let cfg = tx_config.lock().unwrap();
                cfg.active
            };

            // Drain and idle while the transmitter is disabled.
            if !active {
                while let Ok(_) = tx_audio_rx.try_recv() {}
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            if let Some(mut audio_chunk) = tx_audio_rx.blocking_recv() {
                if tx_processed_count == 0 {
                    debug!(
                        "[TX DSP Debug] Received FIRST audio chunk from network. Starting modulation."
                    );
                }

                // Queue depth guard: if we have multiple chunks waiting, we are lagging.
                // We drain all available chunks to find the most recent ones and keep latency low.
                let mut queued_chunks = Vec::with_capacity(8);
                while let Ok(chunk) = tx_audio_rx.try_recv() {
                    queued_chunks.push(chunk);
                }

                let total_chunks = queued_chunks.len() + 1;
                let max_buffered = 5;
                if total_chunks > max_buffered {
                    let drop_count = total_chunks - max_buffered;
                    debug!(
                        "[TX DSP] Queue depth guard triggered: dropping {} stale audio chunks to minimize latency (total queued: {})",
                        drop_count, total_chunks
                    );
                    audio_chunk = queued_chunks[drop_count - 1].clone();
                    queued_chunks = queued_chunks[drop_count..].to_vec();
                }

                // Recheck active status and configuration after waking up
                let (mode, desired_bw, active) = {
                    let cfg = tx_config.lock().unwrap();
                    (cfg.mode.clone(), cfg.filter_bw, cfg.active)
                };
                if !active {
                    continue;
                }

                let desired_fs = tx_fs_atomic.load(Ordering::Relaxed) as f32;
                if mode != current_mode || desired_bw != current_bw || desired_fs != current_fs {
                    debug!(
                        "[TX DSP Debug] Re-creating modulator: mode={:?} (was {:?}), bw={} (was {}), rate={} (was {})",
                        mode, current_mode, desired_bw, current_bw, desired_fs, current_fs
                    );
                    modulator = TxModulator::new(mode.clone(), desired_bw, desired_fs);
                    let dma_fs = tx_dma_audio_fs(desired_fs);
                    resampler = IqResampler::for_dma_fs(dma_fs);
                    debug!(
                        "[TX DSP Debug] DMA feed rate = {} Hz (resampler {})",
                        dma_fs,
                        if resampler.is_some() {
                            "active"
                        } else {
                            "bypassed"
                        }
                    );
                    current_mode = mode;
                    current_bw = desired_bw;
                    current_fs = desired_fs;
                }

                // Modulate the primary chunk (+ resample to the DMA feed rate) and send.
                modulate_and_send(&mut modulator, &mut resampler, &audio_chunk, &iq_tx);
                tx_processed_count += 1;

                // Process remaining safe queued chunks
                for chunk in queued_chunks {
                    modulate_and_send(&mut modulator, &mut resampler, &chunk, &iq_tx);
                    tx_processed_count += 1;
                }

                if tx_processed_count % 100 < total_chunks as u64 {
                    debug!(
                        "[TX DSP Debug] Modulated {} chunks (mode: {:?}, bw/dev: {} Hz, rate: {} Hz, samples: {})",
                        tx_processed_count,
                        modulator.mode,
                        modulator.filter_bw,
                        current_fs,
                        audio_chunk.len()
                    );
                }
            } else {
                break; // Channel closed
            }
        }
    })
}

/// Modulates one audio chunk to SSB IQ, optionally upsamples it to the DMA feed rate,
/// and sends it to the TX IO thread.
fn modulate_and_send(
    modulator: &mut TxModulator,
    resampler: &mut Option<IqResampler>,
    chunk: &[f32],
    iq_tx: &SyncSender<(Vec<i16>, Vec<i16>)>,
) {
    let mut mi = Vec::with_capacity(chunk.len());
    let mut mq = Vec::with_capacity(chunk.len());
    modulator.process_chunk(chunk, &mut mi, &mut mq);
    match resampler.as_mut() {
        Some(r) => {
            let mut ui = Vec::with_capacity(chunk.len() * 3 + 8);
            let mut uq = Vec::with_capacity(chunk.len() * 3 + 8);
            r.process(&mi, &mq, &mut ui, &mut uq);
            let _ = iq_tx.send((ui, uq));
        }
        None => {
            let _ = iq_tx.send((mi, mq));
        }
    }
}
