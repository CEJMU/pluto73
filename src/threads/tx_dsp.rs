use log::debug;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::SyncSender,
};
use std::thread;
use std::time::Duration;

use crate::dsp::tx::{IqResampler, TxConfig, TxMode, TxModulator, tx_dma_audio_fs};

// --- Jitter buffer / drift-nudge tuning (all in the 48 kHz audio domain) ---
//
// The browser produces audio on its own 48 kHz clock; the Pluto drains the TX DMA on an
// independent 48 kHz clock derived from the AD9361. Those clocks are never synchronised, so over
// a lossy/bursty link (WiFi) we see network jitter on top of a slow steady drift. The jitter buffer
// holds a cushion of audio so short stalls don't underrun the DMA, and the drift nudge slowly
// adds/drops one sample per block to keep the cushion centred on `TARGET_DEPTH` without letting
// latency run away.

/// Modulation block size. One `output_block` produces this many 48 kHz audio samples.
const BLOCK: usize = 512;
/// Target cushion depth (150 ms). Startup/re-prime fills to here before audio starts flowing.
const TARGET_DEPTH: usize = 7_200;
/// Deadband around the target (-+25 ms) inside which no drift nudge is applied.
const DEADBAND: usize = 1_200;
/// Hard cap (350 ms). A burst beyond this is trimmed from the oldest samples back down to the target
const MAX_DEPTH: usize = 16_800;
/// Continuous silence emitted while empty before the thread parks on a blocking recv (250 ms).
const SILENCE_PARK: usize = 12_000;

/// Bounded jitter buffer with drift compensation for browser TX audio. Operates on raw 48 kHz
/// audio samples, ahead of SSB modulation.
struct TxJitterBuffer {
    buf: VecDeque<f32>,
    /// Once primed, audio is drained to the modulator; before that, the cushion is still filling
    /// and only silence is emitted. Re-primes after a park.
    primed: bool,
    /// Consecutive silence samples emitted while the buffer is empty (drives the park decision).
    silence_run: usize,
    /// Consecutive silence blocks emitted while waiting for the buffer to reach TARGET_DEPTH
    /// during priming. Used to detect a stalled source and force a partial prime.
    prime_wait_samples: usize,
}

impl TxJitterBuffer {
    fn new() -> Self {
        Self {
            buf: VecDeque::with_capacity(MAX_DEPTH + BLOCK),
            primed: false,
            silence_run: 0,
            prime_wait_samples: 0,
        }
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.primed = false;
        self.silence_run = 0;
        self.prime_wait_samples = 0;
    }

    /// Appends a network chunk, trimming the oldest samples back to TARGET_DEPTH if the buffer
    /// exceeds MAX_DEPTH. Returns the number of samples dropped (for logging).
    fn push_chunk(&mut self, chunk: Vec<f32>) -> usize {
        self.buf.extend(chunk);
        if self.buf.len() > MAX_DEPTH {
            let drop = self.buf.len() - TARGET_DEPTH;
            self.buf.drain(..drop);
            drop
        } else {
            0
        }
    }

    /// Resturns Drift nudge: 
    /// +1 to consume one extra sample this block,
    /// -1 to consume one fewer
    /// 0 inside the deadband.
    fn nudge(&self) -> i32 {
        let d = self.buf.len();
        if d > TARGET_DEPTH + DEADBAND {
            1
        } else if d + DEADBAND < TARGET_DEPTH {
            -1
        } else {
            0
        }
    }

    /// Pops BLOCK + nudge input samples and resamples them to exactly BLOCK output samples.
    /// A -+1 nudge is applied as a gentle linear resample. A short tail (drain underflow) is silence-padded.
    fn output_block(&mut self, nudge: i32) -> Vec<f32> {
        let want = (BLOCK as i32 + nudge).max(0) as usize;
        let take = want.min(self.buf.len());
        let src: Vec<f32> = self.buf.drain(..take).collect();
        resample_to_block(&src)
    }
}

/// Resamples src to exactly BLOCK samples. Handles the three cases the jitter buffer produces:
/// exact length (copy), a -+1 drift nudge (linear resample), or a short drain tail (pad silence).
fn resample_to_block(src: &[f32]) -> Vec<f32> {
    let l = src.len();
    if l == BLOCK {
        return src.to_vec();
    }
    if l == 0 {
        return vec![0.0; BLOCK];
    }
    if l == BLOCK + 1 || l == BLOCK - 1 {
        return linear_resample(src, BLOCK);
    }
    // Drain underflow: emit what we have, then silence to fill the block.
    let mut out = src.to_vec();
    out.resize(BLOCK, 0.0);
    out
}

/// Linear-resamples src (length >= 2) to n output samples.
fn linear_resample(src: &[f32], n: usize) -> Vec<f32> {
    let l = src.len();
    if l < 2 || n == 0 {
        let mut out = src.to_vec();
        out.resize(n, 0.0);
        return out;
    }
    let mut out = Vec::with_capacity(n);
    let step = (l - 1) as f32 / (n - 1) as f32;
    for j in 0..n {
        let pos = j as f32 * step;
        let i = pos.floor() as usize;
        if i + 1 < l {
            let frac = pos - i as f32;
            out.push(src[i] * (1.0 - frac) + src[i + 1] * frac);
        } else {
            out.push(src[l - 1]);
        }
    }
    out
}

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

        let mut jitter = TxJitterBuffer::new();
        let silence_block = vec![0.0f32; BLOCK];
        let mut tx_block_count = 0u64;
        // When TX is deactivated, we drain the remaining buffered audio before resetting.
        let mut draining = false;

        // --- Modulation loop ---
        while !shutdown_flag.load(Ordering::Relaxed) {
            let active = {
                let cfg = tx_config.lock().unwrap();
                cfg.active
            };

            // When TX is deactivated, drain the remaining buffered audio through the modulator so the tail of the transmission is not truncated, then reset for the next keying.
            if !active {
                if !jitter.buf.is_empty() && jitter.primed {
                    draining = true;
                } else if draining && jitter.buf.is_empty() {
                    // Drain complete: discard any late-arriving chunks and reset.
                    draining = false;
                    while tx_audio_rx.try_recv().is_ok() {}
                    jitter.reset();
                    thread::sleep(Duration::from_millis(50));
                    continue;
                } else if !draining {
                    // Nothing buffered (or never primed): discard and idle immediately.
                    while tx_audio_rx.try_recv().is_ok() {}
                    jitter.reset();
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
                // draining == true: fall through to the modulation path below to drain the buffer.
                // Don't accept new audio while draining.
            } else {
                draining = false;
            }

            // Re-create the modulator/resampler if mode, bandwidth, or sample rate changed.
            let (mode, desired_bw) = {
                let cfg = tx_config.lock().unwrap();
                (cfg.mode.clone(), cfg.filter_bw)
            };
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

            // Drain all currently-available network chunks into the jitter buffer. Skip if we're draining the tail after TX deactivation.
            if !draining {
                while let Ok(chunk) = tx_audio_rx.try_recv() {
                    let dropped = jitter.push_chunk(chunk);
                    if dropped > 0 {
                        debug!(
                            "[TX DSP] Jitter buffer over {} samples: trimmed {} stale samples back to target (~{} ms)",
                            MAX_DEPTH,
                            dropped,
                            TARGET_DEPTH / 48
                        );
                    }
                }
            }

            // Nothing buffered: bridge short gaps with defined silence (keeps the DMA fed and the pipeline clocked), or park on a real drought.
            if jitter.buf.is_empty() {
                jitter.silence_run += BLOCK;
                if jitter.silence_run >= SILENCE_PARK {
                    jitter.primed = false;
                    match tx_audio_rx.blocking_recv() {
                        Some(chunk) => {
                            jitter.push_chunk(chunk);
                            jitter.silence_run = 0;
                        }
                        None => break, // channel closed
                    }
                    continue;
                }
                if !modulate_and_send(&mut modulator, &mut resampler, &silence_block, &iq_tx) {
                    break;
                }
                continue;
            }
            jitter.silence_run = 0;

            // Still filling the cushion: hold audio, feed silence, until primed to TARGET_DEPTH.
            if !jitter.primed {
                if jitter.buf.len() >= TARGET_DEPTH {
                    jitter.primed = true;
                    jitter.prime_wait_samples = 0;
                    debug!(
                        "[TX DSP] Jitter buffer primed at {} samples (~{} ms); starting audio.",
                        jitter.buf.len(),
                        jitter.buf.len() / 48
                    );
                } else {
                    // Track how long we've been waiting with a non-empty buffer that isn't growing. 
                    // After 250ms of silence (SILENCE_PARK samples) force a partial prime so the buffered audio isn't stranded forever.
                    jitter.prime_wait_samples += BLOCK;
                    if jitter.prime_wait_samples >= SILENCE_PARK && !jitter.buf.is_empty() {
                        jitter.primed = true;
                        jitter.prime_wait_samples = 0;
                        debug!(
                            "[TX DSP] Jitter buffer partial-primed at {} samples (~{} ms) after stall.",
                            jitter.buf.len(),
                            jitter.buf.len() / 48
                        );
                    } else {
                        if !modulate_and_send(&mut modulator, &mut resampler, &silence_block, &iq_tx) {
                            break;
                        }
                        continue;
                    }
                }
            }

            // Primed with audio available: emit one block, drift-nudged toward the target depth.
            let nudge = jitter.nudge();
            let block = jitter.output_block(nudge);
            if !modulate_and_send(&mut modulator, &mut resampler, &block, &iq_tx) {
                break;
            }

            tx_block_count += 1;
            if tx_block_count % 200 == 0 {
                debug!(
                    "[TX DSP Debug] Modulated {} blocks (mode: {:?}, bw/dev: {} Hz, rate: {} Hz, depth: {} samples, nudge: {})",
                    tx_block_count,
                    modulator.mode,
                    modulator.filter_bw,
                    current_fs,
                    jitter.buf.len(),
                    nudge
                );
            }
        }
    })
}

/// Modulates one audio chunk to SSB IQ, optionally upsamples it to the DMA feed rate,
/// and sends it to the TX IO thread. Returns false if the channel is disconnected, signalling the caller to break out of its loop.
fn modulate_and_send(
    modulator: &mut TxModulator,
    resampler: &mut Option<IqResampler>,
    chunk: &[f32],
    iq_tx: &SyncSender<(Vec<i16>, Vec<i16>)>,
) -> bool {
    let mut mi = Vec::with_capacity(chunk.len());
    let mut mq = Vec::with_capacity(chunk.len());
    modulator.process_chunk(chunk, &mut mi, &mut mq);
    let result = match resampler.as_mut() {
        Some(r) => {
            let mut ui = Vec::with_capacity(chunk.len() * 3 + 8);
            let mut uq = Vec::with_capacity(chunk.len() * 3 + 8);
            r.process(&mi, &mq, &mut ui, &mut uq);
            iq_tx.send((ui, uq))
        }
        None => {
            iq_tx.send((mi, mq))
        }
    };
    if result.is_err() {
        debug!("[TX DSP] IQ channel disconnected; TX IO thread likely exited. Stopping.");
        return false;
    }
    true
}
