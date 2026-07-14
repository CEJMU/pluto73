use pluto::device::{GainMode, MAX_AUDIO_SAMPLES, PlutoDevice, PlutoTxDevice};
use pluto::tx_dsp::{TxMode, TxModulator};
use crate::test::dsp_helpers::{fft_mags_i16, write_wav_i16_stereo};
use std::f64::consts::PI;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn init_channels_cyclic(tx: &mut PlutoTxDevice) -> Result<(), Box<dyn std::error::Error>> {
    tx.buffer = None;
    let (i_name, q_name) = match tx.antenna {
        1 => ("voltage2", "voltage3"),
        _ => ("voltage0", "voltage1"),
    };
    let tx_i = tx
        .dev_tx_stream
        .find_output_channel(i_name)
        .ok_or("TX I Channel not found")?;
    let tx_q = tx
        .dev_tx_stream
        .find_output_channel(q_name)
        .ok_or("TX Q Channel not found")?;
    tx_i.enable();
    tx_q.enable();
    tx.ch_i = Some(tx_i);
    tx.ch_q = Some(tx_q);
    let tx_buffer = tx.dev_tx_stream.create_buffer(tx.buffer_size, true)?;
    tx.buffer = Some(tx_buffer);
    Ok(())
}

fn write_buffer_once(
    tx: &mut PlutoTxDevice,
    i_samples: &[i16],
    q_samples: &[i16],
) -> Result<(), Box<dyn std::error::Error>> {
    let buffer = tx.buffer.as_mut().ok_or("TX Buffer not initialized")?;
    let ch_i = tx.ch_i.as_ref().ok_or("TX I channel not initialized")?;
    let ch_q = tx.ch_q.as_ref().ok_or("TX Q channel not initialized")?;
    ch_i.write(buffer, i_samples)?;
    ch_q.write(buffer, q_samples)?;
    buffer.push()?;
    Ok(())
}

/// Reads raw IQ data from the FPGA audio DMA path and analyzes its spectrum.
/// No demodulation - just dumps what the FPGA audio pipeline produces.
pub fn run_dma_probe() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== AUDIO DMA PROBE ===");
    println!("Reads raw FPGA audio DMA data and analyzes spectrum.");
    println!("Note: RX audio path is permanently wired to TX post-DDS output in this FPGA design.\n");

    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));

    let mut rx = pluto.rx;
    let mut tx = pluto.tx;
    let mut system = pluto.system;

    let lo_hz: i64 = 900_000_000;
    let fs_hz: i64 = 3_840_000;
    let antenna: u8 = 0;
    let cic_decimation: u32 = 4;

    // Configure FPGA
    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();

    // RX DDS at -50 kHz (undo TX DDS)
    system.rx_set_dds(-50_000.0, (fs_hz * 2) as f64);

    // Configure AD9361 RX
    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;

    // Configure TX
    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.set_gain(0.0)?;
    tx.init_channels()?;

    // Base configuration for GPIO RX
    let base_val = 0x01 | (cic_decimation << 4) | ((antenna as u32) << 13);
    system.write_gpio_rx(0x00, base_val);

    // Audio DMA sample rate: fs / cic_decimation / 4 (FIR)
    let dma_fs = fs_hz / cic_decimation as i64 / 4; // 240000
    println!("Audio DMA sample rate: {} kHz", dma_fs as f64 / 1000.0);
    println!("After RX DDS -50 kHz, TX signal should appear at -500 Hz (for 1 kHz SSB tone)\n");

    let system = Arc::new(Mutex::new(system));
    let stop_flag = Arc::new(AtomicBool::new(false));

    // --- Probe: Read DMA with NO TX (baseline noise/carrier) ---
    println!("--- Baseline Capture (No TX) ---");
    let baseline = capture_audio_dma(&system, &stop_flag, Duration::from_secs(2))?;
    analyze_dma_data("Baseline", &baseline.0, &baseline.1, dma_fs as f64);

    // --- Probe: TX raw 1 kHz complex tone (no SSB modulator) ---
    println!("\n--- Raw 1 kHz complex tone (no modulator) ---");
    let stop_tx = Arc::new(AtomicBool::new(false));
    let stop_tx2 = stop_tx.clone();
    let tx_handle = thread::spawn(move || {
        let chunk_size = 4096;
        let w = 2.0 * PI * 1000.0 / 48000.0;
        let mut t = 0u64;
        let mut tx_i = vec![0i16; chunk_size];
        let mut tx_q = vec![0i16; chunk_size];
        let start = Instant::now();

        while !stop_tx2.load(Ordering::Relaxed) {
            for n in 0..chunk_size {
                let angle = w * (t as f64);
                tx_i[n] = (angle.cos() * 25000.0) as i16;
                tx_q[n] = (angle.sin() * 25000.0) as i16;
                t += 1;
            }
            let _ = tx.write_buffer(&tx_i, &tx_q);

        }
        tx
    });

    thread::sleep(Duration::from_millis(500)); // Let TX settle
    let raw_tone = capture_audio_dma(&system, &stop_flag, Duration::from_secs(3))?;
    stop_tx.store(true, Ordering::Relaxed);
    let mut tx = tx_handle.join().map_err(|_| "TX thread panicked")?;

    analyze_dma_data("Raw 1kHz tone", &raw_tone.0, &raw_tone.1, dma_fs as f64);

    // --- Probe: TX SSB-modulated 1 kHz tone ---
    println!("\n--- SSB USB modulated 1 kHz tone ---");
    let stop_tx = Arc::new(AtomicBool::new(false));
    let stop_tx2 = stop_tx.clone();
    let tx_handle = thread::spawn(move || {
        let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, 3_840_000.0);
        let chunk_size = 4096;
        let mut t_audio = 0u64;
        let start = Instant::now();

        while !stop_tx2.load(Ordering::Relaxed) {
            let audio: Vec<f32> = (0..chunk_size)
                .map(|n| {
                    let t = (t_audio + n as u64) as f32 / 48000.0;
                    (2.0 * std::f32::consts::PI * 1000.0 * t).sin()
                })
                .collect();
            t_audio += chunk_size as u64;

            let mut out_i = Vec::new();
            let mut out_q = Vec::new();
            modulator.process_chunk(&audio, &mut out_i, &mut out_q);
            let _ = tx.write_buffer(&out_i, &out_q);

        }
        tx
    });

    thread::sleep(Duration::from_millis(500));
    let ssb_tone = capture_audio_dma(&system, &stop_flag, Duration::from_secs(3))?;
    stop_tx.store(true, Ordering::Relaxed);
    let mut tx = tx_handle.join().map_err(|_| "TX thread panicked")?;

    analyze_dma_data("SSB 1kHz tone", &ssb_tone.0, &ssb_tone.1, dma_fs as f64);

    // Write raw DMA IQ to WAV for external analysis
    let wav_path = "audio_dma_raw.wav";
    write_wav_i16_stereo(wav_path, &ssb_tone.0, &ssb_tone.1, dma_fs as u32)?;
    println!(
        "\nWrote raw DMA IQ to: {} ({} samples, stereo {}kHz)",
        wav_path,
        ssb_tone.0.len(),
        dma_fs / 1000
    );

    let _ = tx.set_gain(-89.75);

    let check_freqs = vec![1000.0, 50000.0];
    let check_mags = fft_mags_i16(&ssb_tone.0, &ssb_tone.1, &check_freqs, dma_fs as f64);
    let ssb_mag = check_mags[0];
    let noise_mag = check_mags[1];
    let snr_db = if noise_mag > 1e-6 {
        20.0 * (ssb_mag / noise_mag).log10()
    } else {
        0.0
    };

    println!("\n========================================");
    if snr_db > 15.0 {
        println!("TEST RESULT: PASS");
    } else {
        println!("TEST RESULT: FAIL");
    }
    println!("========================================");
    println!("- 1 kHz SSB Signal Level: {:.1}", ssb_mag);
    println!("- 50 kHz Noise Floor: {:.1}", noise_mag);
    println!("- Signal-to-Noise Ratio: {:.1} dB (Threshold: >15.0 dB)", snr_db);
    println!("========================================\n");
    Ok(())
}



/// Confirms whether `read_audio_dma_samples`'s submit-before-read ordering can tear a ping-pong
/// buffer. Uses the RX DDS to turn the always-present DC / LO-leakage carrier into a steady tone,
/// captures the audio DMA, and checks the per-sample phase increment for jumps. A jump in the
/// *middle* of a 16384-sample buffer means the freshly-submitted transfer overwrote the buffer while
/// we were still reading it (tearing); a jump exactly at a buffer boundary means a dropped/seamed
/// buffer instead.
pub fn run_dma_continuity() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== AUDIO DMA CONTINUITY / TEARING TEST ===");
    println!("Cyclic TX tone (continuous, no TX-push seams);");
    println!("checks RX audio-DMA phase continuity for buffer tearing/drops.\n");

    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));

    let mut rx = pluto.rx;
    let mut tx = pluto.tx;
    let mut system = pluto.system;

    let lo_hz: i64 = 900_000_000;
    let fs_hz: i64 = 3_840_000;
    let antenna: u8 = 0;
    let cic_decimation: u32 = 4;
    let dma_fs = (fs_hz / cic_decimation as i64 / 4) as f64; // 240 kHz

    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();
    // Undo the TX DDS +50 kHz so the injected tone lands at its baseband frequency.
    system.rx_set_dds(-50_000.0, (fs_hz * 2) as f64);

    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;

    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.set_gain(0.0)?;

    // GPIO RX setup
    let base_val = 0x01 | (cic_decimation << 4) | ((antenna as u32) << 13);
    system.write_gpio_rx(0x00, base_val);

    // Build a seamlessly-looping complex tone: 3000 Hz completes exactly 256 cycles in the
    // 4096-sample buffer at 48 kHz, so the cyclic wrap has no phase discontinuity.
    let buf_len = 4096usize;
    let tone_tx_hz = 3000.0f64;
    let w = 2.0 * PI * tone_tx_hz / 48000.0;
    let mut ti = vec![0i16; buf_len];
    let mut tq = vec![0i16; buf_len];
    for n in 0..buf_len {
        let a = w * n as f64;
        ti[n] = (a.cos() * 25000.0) as i16;
        tq[n] = (a.sin() * 25000.0) as i16;
    }
    init_channels_cyclic(&mut tx)?;
    write_buffer_once(&mut tx, &ti, &tq)?;
    println!("Cyclic {} Hz tone pushed (loops in hardware).", tone_tx_hz);

    let system = Arc::new(Mutex::new(system));
    let stop = Arc::new(AtomicBool::new(false));

    thread::sleep(Duration::from_millis(500)); // let the loop settle

    let dur = Duration::from_secs(5);
    println!(
        "Capturing {}s of audio DMA at {} kHz...",
        dur.as_secs(),
        dma_fs / 1000.0
    );
    let (i_data, q_data) = capture_audio_dma(&system, &stop, dur)?;
    analyze_continuity(&i_data, &q_data, dma_fs, MAX_AUDIO_SAMPLES);

    let _ = tx.set_gain(-89.75);


    println!("\n=== DMA CONTINUITY TEST COMPLETE ===");
    Ok(())
}

#[inline]
fn wrap_pi(x: f64) -> f64 {
    let mut y = x;
    while y > PI {
        y -= 2.0 * PI;
    }
    while y < -PI {
        y += 2.0 * PI;
    }
    y
}

fn analyze_continuity(i_data: &[i16], q_data: &[i16], fs: f64, buf_samples: usize) {
    let n = i_data.len().min(q_data.len());
    if n < 3 {
        println!("  NO DATA");
        return;
    }

    let rms: f64 = (i_data[..n].iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / n as f64).sqrt();

    // Per-sample phase increment: arg(z[k] * conj(z[k-1])). Constant for a pure tone.
    let mut incs = Vec::with_capacity(n - 1);
    for k in 1..n {
        let (i0, q0) = (i_data[k - 1] as f64, q_data[k - 1] as f64);
        let (i1, q1) = (i_data[k] as f64, q_data[k] as f64);
        let re = i1 * i0 + q1 * q0;
        let im = q1 * i0 - i1 * q0;
        incs.push(im.atan2(re));
    }

    let mut sorted = incs.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];

    let mut absdev: Vec<f64> = incs.iter().map(|&x| wrap_pi(x - median).abs()).collect();
    absdev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mad = absdev[absdev.len() / 2];
    let sigma = 1.4826 * mad;
    let threshold = (12.0 * sigma).max(0.35); // 12-sigma, or 0.35 rad floor

    println!(
        "  samples={}, RMS={:.0}, tone={:.0} Hz (median inc {:.5} rad/samp), phase-noise sigma={:.5} rad, threshold={:.4} rad",
        n,
        rms,
        median * fs / (2.0 * PI),
        median,
        sigma,
        threshold
    );

    let num_buffers = n / buf_samples;
    // Skip the first two buffers: DMA/loopback startup settles there and would masquerade as tears.
    let skip = (2 * buf_samples).min(n);

    let mut boundary = 0usize;
    let mut mid = 0usize;
    let mut buffers_with_mid = std::collections::BTreeSet::new();
    let mut pos_hist = [0usize; 16];
    for (k, &x) in incs.iter().enumerate().skip(skip) {
        let jump = wrap_pi(x - median);
        if jump.abs() <= threshold {
            continue;
        }
        let idx = k + 1;
        let pos = idx % buf_samples;
        let near_boundary = pos <= 2 || pos >= buf_samples - 2;
        if near_boundary {
            boundary += 1;
        } else {
            mid += 1;
            buffers_with_mid.insert(idx / buf_samples);
            pos_hist[(pos * 16 / buf_samples).min(15)] += 1;
        }
    }

    println!(
        "  {} buffers captured; ignoring first 2 (startup settling).",
        num_buffers
    );
    println!(
        "  discontinuities (after settle): boundary-aligned={}  mid-buffer={}  (mid spread over {}/{} buffers)",
        boundary,
        mid,
        buffers_with_mid.len(),
        num_buffers.saturating_sub(2)
    );
    if mid > 0 {
        print!("  mid-buffer position histogram (16 bins across the buffer): ");
        for b in &pos_hist {
            print!("{} ", b);
        }
        println!();
        let sample: Vec<usize> = buffers_with_mid.iter().take(20).copied().collect();
        println!(
            "  buffers containing mid-buffer jumps (first 20): {:?}",
            sample
        );
    }
    println!("\n========================================");
    if mid == 0 && boundary == 0 {
        println!("TEST RESULT: PASS");
    } else {
        println!("TEST RESULT: FAIL");
    }
    println!("========================================");
    println!("- Tearing events (mid-buffer): {}", mid);
    println!("- Dropped frames (boundary-aligned): {}", boundary);
    println!("- Distinct affected buffers: {}/{}", buffers_with_mid.len(), num_buffers.saturating_sub(2));
    println!("========================================\n");
}

fn capture_audio_dma(
    system: &Arc<Mutex<pluto::device::PlutoSystem>>,
    _stop: &Arc<AtomicBool>,
    duration: Duration,
) -> Result<(Vec<i16>, Vec<i16>), Box<dyn std::error::Error>> {
    let mut uio_file = {
        let sys = system.lock().unwrap();
        sys.clone_uio_file()?
    };

    let mut all_i: Vec<i16> = Vec::new();
    let mut all_q: Vec<i16> = Vec::new();
    let mut i_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
    let mut q_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
    let mut reads = 0u32;

    let start = Instant::now();
    while start.elapsed() < duration {
        {
            let mut sys = system.lock().unwrap();
            sys.ensure_dma_running();
        }

        let raw_fd = uio_file.as_raw_fd();
        let mut fds = [libc::pollfd {
            fd: raw_fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        let poll_ret = unsafe { libc::poll(fds.as_mut_ptr(), 1, 200) };
        if poll_ret <= 0 {
            continue;
        }

        let mut int_info = [0u8; 4];
        if uio_file.read_exact(&mut int_info).is_err() {
            continue;
        }

        let total_read;
        {
            let mut sys = system.lock().unwrap();
            total_read = sys
                .read_audio_dma_samples(&mut i_ch, &mut q_ch)
                .unwrap_or(0);
        }

        if total_read > 0 {
            all_i.extend_from_slice(&i_ch);
            all_q.extend_from_slice(&q_ch);
            i_ch.clear();
            q_ch.clear();
            reads += 1;
        }
    }

    println!(
        "  Captured {} DMA reads, {} samples ({:.2}s at {} kHz)",
        reads,
        all_i.len(),
        all_i.len() as f64 / 240000.0,
        240
    );
    Ok((all_i, all_q))
}

fn analyze_dma_data(label: &str, i_data: &[i16], q_data: &[i16], fs: f64) {
    let n = std::cmp::min(i_data.len(), q_data.len());
    if n == 0 {
        println!("  {} - NO DATA", label);
        return;
    }

    // RMS amplitude
    let rms: f64 = (i_data.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
    let peak_i = i_data.iter().map(|s| s.abs()).max().unwrap_or(0);
    let peak_q = q_data.iter().map(|s| s.abs()).max().unwrap_or(0);

    println!(
        "  {} - {} samples, RMS={:.1}, peak I={} Q={}",
        label, n, rms, peak_i, peak_q
    );

    // Spectrum analysis at key frequencies
    // After RX DDS at -50 kHz:
    //   Raw 1 kHz tone: should appear at +1 kHz (51-50=1 kHz)
    //   SSB 1 kHz tone (under new FIR architecture): should appear at +1 kHz (51-50=1 kHz)
    //   DDS carrier: at DC (50-50=0 kHz)
    let probes = [
        (0.0, "DC (carrier)"),
        (100.0, "100 Hz"),
        (500.0, "500 Hz"),
        (-500.0, "-500 Hz"),
        (1000.0, "1 kHz (raw or SSB tone expected)"),
        (-1000.0, "-1 kHz"),
        (1500.0, "1.5 kHz"),
        (-1500.0, "-1.5 kHz"),
        (2000.0, "2 kHz"),
        (5000.0, "5 kHz"),
        (10000.0, "10 kHz"),
        (50000.0, "50 kHz (noise floor)"),
    ];

    let len = std::cmp::min(i_data.len(), q_data.len()).min(240000);
    let i_slice = &i_data[..len];
    let q_slice = &q_data[..len];

    let target_freqs: Vec<f64> = probes.iter().map(|(f, _)| *f).collect();
    let mags = fft_mags_i16(i_slice, q_slice, &target_freqs, fs);
    let max_mag = mags.iter().copied().fold(0.0f64, f64::max);

    println!("  {:>10}  {:>10}  {:>6}  {}", "Freq", "Magnitude", "dB", "");
    for (idx, (freq, label)) in probes.iter().enumerate() {
        let mag = mags[idx];
        let db = if mag > 0.0 && max_mag > 0.0 {
            20.0 * (mag / max_mag).log10()
        } else {
            -999.0
        };
        let bar_len = if max_mag > 0.0 {
            ((mag / max_mag) * 20.0) as usize
        } else {
            0
        };
        let bar: String = "#".repeat(bar_len.min(20));
        println!(
            "  {:>8.0} Hz  {:>10.1}  {:>5.1}  {} {}",
            freq, mag, db, label, bar
        );
    }
}


