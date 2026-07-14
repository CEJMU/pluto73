use pluto::device::{GainMode, MAX_AUDIO_SAMPLES, PlutoDevice};
use pluto::dsp::{AudioProcessor, Demodulation, FilterAudio};
use pluto::tx_dsp::{IqResampler, TxMode, TxModulator, tx_dma_audio_fs};
use crate::test::dsp_helpers::{read_wav_as_f32_mono, write_wav_f32_mono};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use std::f64::consts::PI;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const AUDIO_SAMPLE_RATE: u32 = 48_000;

/// Basic RF loopback verification.
/// Uses a raw tone to verify basic transmission and reception loopback through the waterfall path (raw ADC).
pub fn run_rf_raw_loopback() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== PLUTO SDR RAW RF LOOPBACK TEST ===");
    println!("Opening PlutoDevice...");
    let pluto = PlutoDevice::open(16384, 4096).map_err(|err| err.to_string())?;
    thread::sleep(Duration::from_millis(500));

    let mut rx = pluto.rx;
    let mut tx = pluto.tx;
    let mut system = pluto.system;

    let lo_hz = 900_000_000i64;
    let fs_hz = 3_840_000i64;
    let antenna = 0u8;

    println!("Initializing FPGA configurations...");
    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();

    println!(
        "Configuring RX (LO: {} MHz, Rate: {} MHz)...",
        lo_hz as f64 / 1_000_000.0,
        fs_hz as f64 / 1_000_000.0
    );
    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.init_channels()?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;
    println!("RX set to Manual Gain (40.0 dB)");

    println!("Configuring TX...");
    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.set_gain(0.0)?;
    tx.init_channels()?;

    println!("Starting TX/RX loopback...");
    println!("Transmitting 1 kHz tone at 48 kHz input sample rate...");
    println!(
        "Expected loopback tone at RX: LO + 51 kHz (due to +50 kHz FPGA DDS shift + 1 kHz baseband)"
    );

    let chunk_size = 4096;
    let mut tx_i = vec![0i16; chunk_size];
    let mut tx_q = vec![0i16; chunk_size];
    let mut t = 0u64;
    let w_tx = 2.0 * PI * 1000.0 / 48000.0;
    let num_iterations = 300;

    for i in 0..num_iterations {
        for n in 0..chunk_size {
            let angle = w_tx * (t as f64);
            tx_i[n] = (angle.cos() * 25000.0) as i16;
            tx_q[n] = (angle.sin() * 25000.0) as i16;
            t += 1;
        }

        tx.write_buffer(&tx_i, &tx_q)?;
        system.trigger_waterfall_burst();

        match rx.read_buffer() {
            Ok((rx_i, rx_q)) => {
                if i % 15 == 0 {
                    let fs_rx = 3840000.0;
                    let n = rx_i.len().min(rx_q.len());
                    if n > 0 {
                        let mut planner = FftPlanner::<f32>::new();
                        let fft = planner.plan_fft_forward(n);
                        let mut fft_buf: Vec<Complex<f32>> = rx_i[..n]
                            .iter()
                            .zip(&rx_q[..n])
                            .map(|(&i_val, &q_val)| Complex::new(i_val as f32, q_val as f32))
                            .collect();
                        fft.process(&mut fft_buf);

                        let bin_hz = fs_rx / n as f64;
                        let get_mag = |f_target: f64| -> f64 {
                            let bin = (f_target / bin_hz).round() as isize;
                            let idx = bin.rem_euclid(n as isize) as usize;
                            (fft_buf[idx].norm() / n as f32) as f64
                        };

                        let mag_51k = get_mag(51000.0);
                        let mag_50k = get_mag(50000.0);
                        let mag_1k = get_mag(1000.0);
                        let mag_dc = get_mag(0.0);
                        let mag_noise = get_mag(100000.0);

                        println!(
                            "Iteration {:3} | Carrier 50 kHz: {:7.1} | DUC 51 kHz: {:7.1} | Bypass 1 kHz: {:7.1} | DC: {:7.1} | Noise 100 kHz: {:7.1}",
                            i, mag_50k, mag_51k, mag_1k, mag_dc, mag_noise
                        );
                    }
                }
            }
            Err(err) => {
                println!("Iteration {:3} | No RX data received. Error: {}", i, err);
            }
        }

        thread::sleep(Duration::from_millis(10));
    }

    println!("Loopback test completed successfully. Muting TX...");
    let _ = tx.set_gain(-89.75);
    Ok(())
}

/// End-to-end audio loopback test.
/// Sends a modulated SSB audio signal from a WAV file, routes it through RF loopback,
/// captures it via the RX audio DMA path, demodulates it, and writes the output to a WAV file.
pub fn run_rf_audio_loopback(
    input_path: &str,
    output_path: &str,
    fs_hz: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== FPGA RF AUDIO LOOPBACK TEST ===");
    println!("Uses the proven FPGA audio pipeline (same as FM reception).\n");

    let audio_samples = read_wav_as_f32_mono(input_path)?;
    let duration_s = audio_samples.len() as f64 / AUDIO_SAMPLE_RATE as f64;
    println!(
        "Input: {} samples ({:.2}s at {} Hz)",
        audio_samples.len(),
        duration_s,
        AUDIO_SAMPLE_RATE
    );

    println!("Opening PlutoDevice...");
    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));

    let mut tx = pluto.tx;
    let mut system = pluto.system;

    let lo_hz: i64 = 900_000_000;
    let antenna: u8 = 0;
    let cic_decimation: u32 = ((fs_hz / 960_000).clamp(4, 32) as u32).next_power_of_two();

    println!("Configuring FPGA...");
    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();
    system.rx_set_dds(-50_000.0, (fs_hz * 2) as f64);

    let mut rx = pluto.rx;
    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.set_gain(GainMode::Manual, Some(30.0))?;

    println!("TX: LO={} MHz, +50 kHz DDS offset", lo_hz as f64 / 1e6);
    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.set_gain(-15.0)?;
    tx.init_channels()?;

    let system = Arc::new(Mutex::new(system));
    let stop_flag = Arc::new(AtomicBool::new(false));

    let filter_bw = 3_000.0f32;
    let bfo_hz = filter_bw / 2.0;
    let if_cutoff_hz = filter_bw;
    let demod = Demodulation::SSB {
        fs: AUDIO_SAMPLE_RATE as f32,
        bfo_hz,
        audio_bw_hz: filter_bw,
    };

    let dma_fs = fs_hz / cic_decimation as i64 / 4;
    let target_audio_fs = 48_000.0f32;
    let sw_decimation = ((dma_fs as f64 / target_audio_fs as f64).round() as usize).max(1);

    println!(
        "Audio DMA: {} kHz -> software decimate by {} -> {} kHz",
        dma_fs as f64 / 1000.0,
        sw_decimation,
        target_audio_fs / 1000.0
    );
    println!(
        "SSB USB demod: BFO={} Hz, IF cutoff={} Hz",
        bfo_hz, if_cutoff_hz
    );

    let system_rx = system.clone();
    let stop_rx = stop_flag.clone();
    let rx_handle = thread::spawn(move || -> Vec<f32> {
        let mut uio_file = {
            let sys = system_rx.lock().unwrap();
            sys.clone_uio_file()
                .expect("Failed to clone UIO file handle")
        };

        let mut audio_filter = FilterAudio::new(sw_decimation, dma_fs, if_cutoff_hz);
        let mut audio_processor = AudioProcessor::new(demod);
        let mut all_audio: Vec<f32> = Vec::with_capacity(AUDIO_SAMPLE_RATE as usize * 10);
        let mut audio_buffer: Vec<f32> = Vec::with_capacity(8192);
        let mut i_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut q_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut dma_reads = 0u64;
        let mut last_packet = Instant::now();

        while !stop_rx.load(Ordering::Relaxed) {
            {
                let mut sys = system_rx.lock().unwrap();
                sys.ensure_dma_running();
            }

            let raw_fd = uio_file.as_raw_fd();
            let mut fds = [libc::pollfd {
                fd: raw_fd,
                events: libc::POLLIN,
                revents: 0,
            }];
            let poll_ret = unsafe { libc::poll(fds.as_mut_ptr(), 1, 100) };
            if poll_ret <= 0 {
                if last_packet.elapsed().as_secs() > 3 {
                    let mut sys = system_rx.lock().unwrap();
                    sys.rx_apply_dsp_config(antenna, fs_hz);
                    sys.rx_set_dds(-50_000.0, (fs_hz * 2) as f64);
                    sys.reset_audio_dma_controller();
                    last_packet = Instant::now();
                    println!("  [RX] DMA reset (no data for 3s)");
                }
                continue;
            }

            let mut int_info = [0u8; 4];
            if uio_file.read_exact(&mut int_info).is_err() {
                continue;
            }

            let total_read;
            {
                let mut sys = system_rx.lock().unwrap();
                total_read = sys
                    .read_audio_dma_samples(&mut i_ch, &mut q_ch)
                    .unwrap_or(0);
            }

            if total_read == 0 {
                thread::sleep(Duration::from_micros(100));
                continue;
            }
            last_packet = Instant::now();
            dma_reads += 1;

            let sliced_iq = audio_filter.execute(&i_ch, &q_ch);
            i_ch.clear();
            q_ch.clear();

            if !sliced_iq.is_empty() {
                audio_processor.process(sliced_iq, &mut audio_buffer);
            }

            if audio_buffer.len() >= 4096 {
                all_audio.extend_from_slice(&audio_buffer);
                audio_buffer.clear();
            }

            if dma_reads % 50 == 0 {
                println!(
                    "  [RX] {} DMA reads, {} audio samples so far",
                    dma_reads,
                    all_audio.len()
                );
            }
        }

        all_audio.extend_from_slice(&audio_buffer);
        println!(
            "  [RX] Done: {} DMA reads, {} audio samples ({:.2}s)",
            dma_reads,
            all_audio.len(),
            all_audio.len() as f64 / AUDIO_SAMPLE_RATE as f64
        );
        all_audio
    });

    thread::sleep(Duration::from_millis(200));

    println!("Transmitting audio (clock-paced)...");
    let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
    let dma_audio_fs = tx_dma_audio_fs(fs_hz as f32);
    let mut resampler = IqResampler::for_dma_fs(dma_audio_fs);
    println!(
        "TX DMA feed rate = {} Hz (resampler {})",
        dma_audio_fs,
        if resampler.is_some() {
            "active - resampler enabled"
        } else {
            "bypassed"
        }
    );
    let chunk_size = 4096usize;
    let samples_per_sec = AUDIO_SAMPLE_RATE as f64;
    let secs_per_chunk = chunk_size as f64 / samples_per_sec;
    let prefill_chunks = 2;

    let silence = vec![0.0f32; chunk_size];
    let mut all_chunks: Vec<Vec<f32>> = Vec::new();
    for _ in 0..prefill_chunks {
        all_chunks.push(silence.clone());
    }
    for raw_chunk in audio_samples.chunks(chunk_size) {
        let mut chunk = raw_chunk.to_vec();
        chunk.resize(chunk_size, 0.0);
        all_chunks.push(chunk);
    }
    for _ in 0..3 {
        all_chunks.push(silence.clone());
    }

    let total_chunks = all_chunks.len();
    let tx_start = Instant::now();

    for (idx, chunk) in all_chunks.iter().enumerate() {
        if idx >= prefill_chunks {
            let target_time =
                Duration::from_secs_f64((idx - prefill_chunks) as f64 * secs_per_chunk);
        }

        let mut mod_i = Vec::new();
        let mut mod_q = Vec::new();
        modulator.process_chunk(chunk, &mut mod_i, &mut mod_q);

        match resampler.as_mut() {
            Some(r) => {
                let mut out_i = Vec::with_capacity(mod_i.len() * 3 + 8);
                let mut out_q = Vec::with_capacity(mod_q.len() * 3 + 8);
                r.process(&mod_i, &mod_q, &mut out_i, &mut out_q);
                tx.write_buffer(&out_i, &out_q)?;
            }
            None => {
                tx.write_buffer(&mod_i, &mod_q)?;
            }
        }

        if idx % 10 == 0 {
            println!(
                "  [TX] {}/{} ({:.1}s)",
                idx,
                total_chunks,
                idx as f64 * secs_per_chunk
            );
        }
    }

    thread::sleep(Duration::from_millis(500));

    println!("TX done. Stopping RX...");
    let _ = tx.set_gain(-89.75);
    stop_flag.store(true, Ordering::Relaxed);
    let all_audio = rx_handle.join().map_err(|_| "RX thread panicked")?;

    if all_audio.is_empty() {
        println!("WARNING: No audio captured! DMA may not be running.");
        return Ok(());
    }

    let peak = all_audio.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let gain = if peak > 1e-6 { 0.9 / peak } else { 1.0 };
    println!(
        "Output: {} samples ({:.2}s), peak={:.6}, gain={:.1}",
        all_audio.len(),
        all_audio.len() as f64 / AUDIO_SAMPLE_RATE as f64,
        peak,
        gain
    );

    write_wav_f32_mono(output_path, &all_audio, AUDIO_SAMPLE_RATE, true)?;
    println!("Wrote: {}", output_path);

    println!("=== FPGA RF AUDIO LOOPBACK TEST COMPLETE ===");
    Ok(())
}

/// Continuous tone loopback test.
/// Transmits a continuous single-frequency tone through the RF loopback path.
/// Used to isolate content-independent timing/buffering dropouts.
pub fn run_rf_tone_loopback(
    freq_hz: f32,
    duration_s: f32,
    output_path: &str,
    chunk_size: usize,
    fs_hz: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== FPGA RF TONE LOOPBACK TEST ===");
    println!(
        "Transmits a continuous {} Hz tone for {} s (TX chunk_size={}, fs={} Hz).\n",
        freq_hz, duration_s, chunk_size, fs_hz
    );

    let total_samples = (duration_s * AUDIO_SAMPLE_RATE as f32) as usize;

    println!("Opening PlutoDevice...");
    let pluto = PlutoDevice::open(16384, chunk_size).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));

    let mut tx = pluto.tx;
    let mut system = pluto.system;

    let lo_hz: i64 = 900_000_000;
    let antenna: u8 = 0;
    let cic_decimation: u32 = ((fs_hz / 960_000).clamp(4, 32) as u32).next_power_of_two();

    println!("Configuring FPGA...");
    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();
    system.rx_set_dds(-50_000.0, (fs_hz * 2) as f64);

    let mut rx = pluto.rx;
    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;

    println!("TX: LO={} MHz, +50 kHz DDS offset", lo_hz as f64 / 1e6);
    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.set_gain(0.0)?;
    tx.init_channels()?;

    let system = Arc::new(Mutex::new(system));
    let stop_flag = Arc::new(AtomicBool::new(false));

    let filter_bw = 3_000.0f32;
    let bfo_hz = filter_bw / 2.0;
    let if_cutoff_hz = filter_bw;
    let demod = Demodulation::SSB {
        fs: AUDIO_SAMPLE_RATE as f32,
        bfo_hz,
        audio_bw_hz: filter_bw,
    };

    let dma_fs = fs_hz / cic_decimation as i64 / 4;
    let target_audio_fs = 48_000.0f32;
    let sw_decimation = ((dma_fs as f64 / target_audio_fs as f64).round() as usize).max(1);

    println!(
        "Audio DMA: {} kHz -> software decimate by {} -> {} kHz",
        dma_fs as f64 / 1000.0,
        sw_decimation,
        target_audio_fs / 1000.0
    );

    let system_rx = system.clone();
    let stop_rx = stop_flag.clone();
    let rx_handle = thread::spawn(move || -> Vec<f32> {
        let mut uio_file = {
            let sys = system_rx.lock().unwrap();
            sys.clone_uio_file()
                .expect("Failed to clone UIO file handle")
        };

        let mut audio_filter = FilterAudio::new(sw_decimation, dma_fs, if_cutoff_hz);
        let mut audio_processor = AudioProcessor::new(demod);
        let mut all_audio: Vec<f32> = Vec::with_capacity(AUDIO_SAMPLE_RATE as usize * 10);
        let mut audio_buffer: Vec<f32> = Vec::with_capacity(8192);
        let mut i_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut q_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut dma_reads = 0u64;
        let mut last_packet = Instant::now();

        while !stop_rx.load(Ordering::Relaxed) {
            {
                let mut sys = system_rx.lock().unwrap();
                sys.ensure_dma_running();
            }

            let raw_fd = uio_file.as_raw_fd();
            let mut fds = [libc::pollfd {
                fd: raw_fd,
                events: libc::POLLIN,
                revents: 0,
            }];
            let poll_ret = unsafe { libc::poll(fds.as_mut_ptr(), 1, 100) };
            if poll_ret <= 0 {
                if last_packet.elapsed().as_secs() > 3 {
                    let mut sys = system_rx.lock().unwrap();
                    sys.rx_apply_dsp_config(antenna, fs_hz);
                    sys.rx_set_dds(-50_000.0, (fs_hz * 2) as f64);
                    sys.reset_audio_dma_controller();
                    last_packet = Instant::now();
                    println!("  [RX] DMA reset (no data for 3s)");
                }
                continue;
            }

            let mut int_info = [0u8; 4];
            if uio_file.read_exact(&mut int_info).is_err() {
                continue;
            }

            let total_read;
            {
                let mut sys = system_rx.lock().unwrap();
                total_read = sys
                    .read_audio_dma_samples(&mut i_ch, &mut q_ch)
                    .unwrap_or(0);
            }

            if total_read == 0 {
                thread::sleep(Duration::from_micros(100));
                continue;
            }
            last_packet = Instant::now();
            dma_reads += 1;

            let sliced_iq = audio_filter.execute(&i_ch, &q_ch);
            i_ch.clear();
            q_ch.clear();

            if !sliced_iq.is_empty() {
                audio_processor.process(sliced_iq, &mut audio_buffer);
            }

            if audio_buffer.len() >= 4096 {
                all_audio.extend_from_slice(&audio_buffer);
                audio_buffer.clear();
            }

            if dma_reads % 50 == 0 {
                println!(
                    "  [RX] {} DMA reads, {} audio samples so far",
                    dma_reads,
                    all_audio.len()
                );
            }
        }

        all_audio.extend_from_slice(&audio_buffer);
        println!(
            "  [RX] Done: {} DMA reads, {} audio samples ({:.2}s)",
            dma_reads,
            all_audio.len(),
            all_audio.len() as f64 / AUDIO_SAMPLE_RATE as f64
        );
        all_audio
    });

    thread::sleep(Duration::from_millis(200));

    println!("Transmitting continuous tone (clock-paced)...");
    let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
    let dma_audio_fs = tx_dma_audio_fs(fs_hz as f32);
    let mut resampler = IqResampler::for_dma_fs(dma_audio_fs);
    println!(
        "TX DMA feed rate = {} Hz (resampler {})",
        dma_audio_fs,
        if resampler.is_some() {
            "active - resampler enabled"
        } else {
            "bypassed"
        }
    );
    let samples_per_sec = AUDIO_SAMPLE_RATE as f64;
    let secs_per_chunk = chunk_size as f64 / samples_per_sec;
    let prefill_chunks = 2;

    let silence = vec![0.0f32; chunk_size];
    let mut all_chunks: Vec<Vec<f32>> = Vec::new();
    for _ in 0..prefill_chunks {
        all_chunks.push(silence.clone());
    }

    let mut t_audio = 0u64;
    let mut remaining = total_samples;
    while remaining > 0 {
        let n = remaining.min(chunk_size);
        let mut chunk: Vec<f32> = (0..n)
            .map(|k| {
                let t = (t_audio + k as u64) as f32 / AUDIO_SAMPLE_RATE as f32;
                (2.0 * std::f32::consts::PI * freq_hz * t).sin()
            })
            .collect();
        chunk.resize(chunk_size, 0.0);
        t_audio += n as u64;
        remaining -= n;
        all_chunks.push(chunk);
    }

    for _ in 0..3 {
        all_chunks.push(silence.clone());
    }

    let total_chunks = all_chunks.len();
    let tx_start = Instant::now();

    for (idx, chunk) in all_chunks.iter().enumerate() {
        if idx >= prefill_chunks {
            let target_time =
                Duration::from_secs_f64((idx - prefill_chunks) as f64 * secs_per_chunk);
        }

        let mut mod_i = Vec::new();
        let mut mod_q = Vec::new();
        modulator.process_chunk(chunk, &mut mod_i, &mut mod_q);

        match resampler.as_mut() {
            Some(r) => {
                let mut out_i = Vec::with_capacity(mod_i.len() * 3 + 8);
                let mut out_q = Vec::with_capacity(mod_q.len() * 3 + 8);
                r.process(&mod_i, &mod_q, &mut out_i, &mut out_q);
                tx.write_buffer(&out_i, &out_q)?;
            }
            None => {
                tx.write_buffer(&mod_i, &mod_q)?;
            }
        }

        if idx % 10 == 0 {
            println!(
                "  [TX] {}/{} ({:.1}s)",
                idx,
                total_chunks,
                idx as f64 * secs_per_chunk
            );
        }
    }

    thread::sleep(Duration::from_millis(500));

    println!("TX done. Stopping RX...");
    let _ = tx.set_gain(-89.75);
    stop_flag.store(true, Ordering::Relaxed);
    let all_audio = rx_handle.join().map_err(|_| "RX thread panicked")?;

    if all_audio.is_empty() {
        println!("WARNING: No audio captured! DMA may not be running.");
        return Ok(());
    }

    let peak = all_audio.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let gain = if peak > 1e-6 { 0.9 / peak } else { 1.0 };
    println!(
        "Output: {} samples ({:.2}s), peak={:.6}, gain={:.4}",
        all_audio.len(),
        all_audio.len() as f64 / AUDIO_SAMPLE_RATE as f64,
        peak,
        gain
    );

    write_wav_f32_mono(output_path, &all_audio, AUDIO_SAMPLE_RATE, true)?;
    println!("Wrote: {}", output_path);

    println!("=== FPGA RF TONE LOOPBACK TEST COMPLETE ===");
    Ok(())
}


