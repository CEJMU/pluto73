use log::debug;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

use crate::device::{PlutoSystem, unpack_iq_words, wait_for_uio_interrupt};
use crate::dsp::{AudioProcessor, Demodulation, FilterAudio};
use crate::state::DemodMode;
use crate::{AUDIO_SAMPLE_RATE, FILTER_BW_MIN_HZ, MIN_SPAN_FM, filter_bw_max_hz};

#[derive(Clone, Debug)]
pub struct AudioConfig {
    pub enabled: bool,
    pub demod_mode: Demodulation,
    pub if_cutoff_hz: f32,
    pub fs_hz: i64,
}

/// Updates the hardware AXI DDS phase increment and the receiver software demodulator settings
/// (such as sample rate, filters, and audio mode) based on user frequency tuning and modulation commands.
pub fn update_audio_tuning(
    playback_hz: i64,
    lo_hz: i64,
    fs_hz: i64,
    demod_mode_enum: DemodMode,
    filter_bw: f32,
    system: &Arc<Mutex<PlutoSystem>>,
    audio_config: &Arc<Mutex<AudioConfig>>,
) {
    // Clamp the tune offset to the captured band (+/-fs/2, i.e. Nyquist).
    let max_offset_hz = fs_hz / 2;
    let target_hz = (playback_hz - lo_hz).clamp(-max_offset_hz, max_offset_hz);

    // The RX DDS NCO is free-running on the fabric clock (`l_clk` = the AD9361 DATA_CLK), so its
    // phase increment is computed against that clock, not the baseband rate. In the Pluto's config
    // (2R2T, CMOS dual-port full-duplex, DDR) DATA_CLK == 2x fs: each sample period carries
    // I1,Q1,I2,Q2 = 4 words at 2 words/clock = 2 clocks/sample. Confirmed by AD9361 UG-570 Table 48
    // (2R2T max fs 30.72 MSPS <-> max DATA_CLK 61.44 MHz); 1R1T would instead be 1x fs.
    {
        let mut sys = system.lock().unwrap();
        sys.rx_set_dds(-target_hz as f64, (fs_hz * 2) as f64);
    }

    // Clamp filter bandwidth to prevent unstable IIR coefficients (Nyquist safety margins)
    let clamped_filter_bw = filter_bw.clamp(FILTER_BW_MIN_HZ, filter_bw_max_hz(demod_mode_enum));

    // Standard SSB convention (matched to the TX complex-FIR modulator, see tx_dsp.rs): the tuned
    // carrier is at baseband DC and the audio occupies ONE side of it - USB at [0, +bw], LSB at
    // [-bw, 0]. The analytic demod (dsp::AnalyticSsbDemod) selects the sideband from the SIGN of
    // bfo_hz (+ = USB, - = LSB); its magnitude is unused. The IF filter must pass the full sideband,
    // so if_cutoff = bw (not bw/2 - that would clip the top half of the audio).
    let (demod_mode, if_cutoff_hz) = match demod_mode_enum {
        DemodMode::USB => (
            Demodulation::SSB {
                fs: AUDIO_SAMPLE_RATE as f32,
                bfo_hz: (clamped_filter_bw / 2.0),
                audio_bw_hz: clamped_filter_bw,
            },
            clamped_filter_bw,
        ),
        DemodMode::LSB => (
            Demodulation::SSB {
                fs: AUDIO_SAMPLE_RATE as f32,
                bfo_hz: -(clamped_filter_bw / 2.0),
                audio_bw_hz: clamped_filter_bw,
            },
            clamped_filter_bw,
        ),
        DemodMode::FM => (
            Demodulation::FM {
                audio_fs: 240_000.0,
                dev_hz: 75_000.0,
                audio_bw_hz: clamped_filter_bw,
            },
            120_000.0,
        ),
    };

    {
        let mut cfg = audio_config.lock().unwrap();
        cfg.demod_mode = demod_mode;
        cfg.if_cutoff_hz = if_cutoff_hz;
        cfg.fs_hz = fs_hz;
    }

    debug!(
        "Audio Tuning: playback={} Hz, LO={} Hz, target={} Hz, DDS shifted to 0 Hz",
        playback_hz, lo_hz, target_hz
    );
}

/// Spawns the processing thread dedicated to fetching DMA samples,
/// running software demodulation, decimation, and filtering, and broadcasting playback buffers.
pub fn spawn_audio_thread(
    shutdown_audio: Arc<AtomicBool>,
    is_running: Arc<AtomicBool>,
    audio_config: Arc<Mutex<AudioConfig>>,
    system: Arc<Mutex<PlutoSystem>>,
    rx_audio_tx: broadcast::Sender<Vec<f32>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // --- Thread state ---
        let mut uio_file = {
            let sys = system.lock().unwrap();
            sys.clone_uio_file()
                .expect("Failed to clone UIO file handle")
        };

        let mut audio_processor: Option<AudioProcessor> = None;
        let initial_cic_decimation = { system.lock().unwrap().rx_cic_decimation };
        let initial_fs = MIN_SPAN_FM / initial_cic_decimation as i64;
        let initial_decimation = (initial_fs as f32 / 240_000.0).round() as usize;
        let mut audio_filter = FilterAudio::new(initial_decimation.max(1), initial_fs, 120_000.0);
        let mut current_mode: Option<Demodulation> = None;

        let mut audio_buffer: Vec<f32> = Vec::with_capacity(8192);
        let mut last_packet_time = Instant::now();

        let mut i_ch: Vec<i16> = Vec::with_capacity(crate::device::MAX_AUDIO_SAMPLES);
        let mut q_ch: Vec<i16> = Vec::with_capacity(crate::device::MAX_AUDIO_SAMPLES);
        // Reused DMA copy scratch: grown once, refilled every cycle (avoids a 64 KB alloc + zero-fill per DMA interrupt).
        let mut dma_words: Vec<u32> = Vec::with_capacity(crate::device::MAX_AUDIO_SAMPLES);

        // --- Processing loop ---
        while !shutdown_audio.load(Ordering::Relaxed) {
            // Skip while globally paused.
            if !is_running.load(Ordering::Relaxed) {
                i_ch.clear();
                q_ch.clear();
                audio_buffer.clear();
                last_packet_time = Instant::now();
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            let config = {
                let cfg = audio_config.lock().unwrap();
                cfg.clone()
            };
            // RX CIC decimation and the reconfiguring flag both live on PlutoSystem: read them in the same lock
            let (rx_cic_decimation, is_configuring) = {
                let sys = system.lock().unwrap();
                (sys.rx_cic_decimation, sys.is_configuring)
            };

            // Skip while audio is disabled.
            if !config.enabled {
                i_ch.clear();
                q_ch.clear();
                last_packet_time = Instant::now();
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            // Ensure the DMA hardware transfer is running.
            // Mutex is locked briefly to start DMA if needed.
            {
                let mut sys = system.lock().unwrap();
                sys.ensure_dma_running();
            }

            // Block on the UIO hardware interrupt
            match wait_for_uio_interrupt(&mut uio_file, 50) {
                Ok(Some(_)) => {}
                Ok(None) => continue, // timeout or EINTR: retry
                Err(_) => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
            }

            // Clear the interrupt and get DMA read pointer.
            let total_read;
            let ram_ptr;
            {
                let mut sys = system.lock().unwrap();
                if let Some((count, ptr)) = sys.prepare_audio_dma_read() {
                    total_read = count;
                    ram_ptr = ptr;
                } else {
                    total_read = 0;
                    ram_ptr = std::ptr::null();
                }
            }

            if total_read > 0 && !ram_ptr.is_null() {
                // Copy the entire block in bulk into the reused scratch vector first.
                // This uses optimized memcpy (AXI burst reads) which is much faster.
                dma_words.clear();
                dma_words.reserve(total_read);
                // SAFETY: ram_ptr points at `total_read` packed samples in the mmapped DMA buffer. dma_words has just reserved at least that much capacity.
                unsafe {
                    std::ptr::copy_nonoverlapping(ram_ptr, dma_words.as_mut_ptr(), total_read);
                    dma_words.set_len(total_read);
                }

                unpack_iq_words(&dma_words, &mut i_ch, &mut q_ch);

                // Mute the audio stream while the hardware is configuring
                if is_configuring {
                    i_ch.clear();
                    q_ch.clear();
                    last_packet_time = Instant::now();
                    continue;
                }
            }

            if total_read == 0 {
                // Failsafe: If we get no samples for 5s, reset hardware DMA controller
                if last_packet_time.elapsed().as_secs() > 5 {
                    let mut sys = system.lock().unwrap();
                    if !sys.is_configuring {
                        // Re-assert the current fabric config (for config.fs_hz) as a stall failsafe.
                        let rx_antenna = sys.rx_antenna;
                        sys.rx_apply_dsp_config(rx_antenna, config.fs_hz);
                        sys.reset_audio_dma_controller();
                    }
                    last_packet_time = Instant::now();
                }
                thread::sleep(Duration::from_micros(100));
                continue;
            }
            last_packet_time = Instant::now();

            // Update demod processor if mode changed
            if current_mode.as_ref() != Some(&config.demod_mode) {
                let mut new_proc = AudioProcessor::new(config.demod_mode.clone());
                if let Some(ref old_proc) = audio_processor {
                    new_proc.copy_state_from(old_proc);
                }
                audio_processor = Some(new_proc);
                current_mode = Some(config.demod_mode.clone());
            }

            // Update filter parameters
            let target_audio_fs = match &config.demod_mode {
                Demodulation::FM { audio_fs, .. } => *audio_fs,
                Demodulation::SSB { fs, .. } => *fs,
            };

            // The FPGA has a hardware CIC filter and a Decimate-by-4 FIR filter.
            let actual_dma_fs = (config.fs_hz / rx_cic_decimation as i64) / 4;

            let decimation =
                ((actual_dma_fs as f64 / target_audio_fs as f64).round() as usize).max(1);
            audio_filter.set_params(decimation, actual_dma_fs, config.if_cutoff_hz);

            // Process all accumulated samples in one call
            let sliced_iq = audio_filter.execute(&i_ch, &q_ch);
            i_ch.clear();
            q_ch.clear();

            if !sliced_iq.is_empty() {
                if let Some(processor) = &mut audio_processor {
                    processor.process(sliced_iq, &mut audio_buffer);
                }
            }

            // Buffer audio and broadcast once we reach target chunk size
            if audio_buffer.len() >= 4096 {
                let send_buf = std::mem::replace(&mut audio_buffer, Vec::with_capacity(8192));
                let _ = rx_audio_tx.send(send_buf);
            }
        }
    })
}
