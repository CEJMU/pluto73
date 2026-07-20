use pluto::device::{GainMode, MAX_AUDIO_SAMPLES, PlutoDevice, PlutoTxDevice};
use pluto::dsp::{AudioProcessor, Demodulation, FilterAudio};
use crate::test::dsp_helpers::{apply_hamming_window, dominant_tone};
use pluto::tx_dsp::{TxMode, TxModulator};
use num_complex::Complex;
use rustfft::FftPlanner;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Enables the FIR, sets a low baseband rate, and captures the wideband spectrum to
/// confirm the AD9361 + FPGA still deliver clean samples at that rate, and reports the achieved
/// waterfall bin resolution vs. the 3.84 MHz baseline.
pub fn run_narrowband_rx(rate_hz: i64, secs: f32) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== LOW-SPAN (AD9361 internal FIR) TEST ===");
    println!(
        "target span {:.3} MHz (below the 2.083 MSPS FIR-bypassed floor)\n",
        rate_hz as f64 / 1e6
    );

    let lo_hz = 900_000_000i64;
    let antenna = 0u8;

    let mut pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    pluto.reset_device_state().map_err(|e| e.to_string())?;
    let mut rx = pluto.rx;
    let mut system = pluto.system;

    // Enable the AD9361 internal FIR and set the low rate
    rx.set_bb_rate_fir(rate_hz)?;
    println!(
        "set_bb_rate_fir({}) -> sampling_frequency reads {} Hz",
        rate_hz, rx.sampling_frequency
    );
    if (rx.sampling_frequency - rate_hz).abs() > rate_hz / 20 {
        println!("  WARNING: readback differs from request by >5% (rate may have been coerced).");
    }

    // Configure FPGA DSP + RF frontend for the low rate.
    system.rx_apply_dsp_config(antenna, rx.sampling_frequency);
    system.reset_audio_dma_controller();

    rx.set_frequencies(lo_hz, rx.sampling_frequency)?;
    rx.set_rf_bandwidth(rx.sampling_frequency.max(200_000))?; // tighten the analog/ADC window
    rx.set_gain(GainMode::Manual, Some(40.0))?;
    rx.init_channels()?;

    let fs = rx.sampling_frequency as f32;
    let n = 16384usize;
    let bin_hz = fs / n as f32;
    println!(
        "waterfall: {}-pt FFT over {:.3} MHz -> {:.1} Hz/bin  (vs {:.1} Hz/bin at 3.84 MHz)\n",
        n,
        fs / 1e6,
        bin_hz,
        3_840_000.0 / n as f32
    );

    // Warm up, then average several wideband bursts.
    thread::sleep(Duration::from_millis(300));
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let bursts = ((secs / 0.05).round() as usize).clamp(4, 40);
    let mut acc = vec![0.0f32; n];
    let mut segs = 0usize;
    let mut peak_adc = 0i32;
    for _ in 0..bursts {
        system.trigger_waterfall_burst();
        if let Ok((ri, rq)) = rx.read_buffer() {
            if ri.len().min(rq.len()) >= n {
                for i in 0..n {
                    peak_adc = peak_adc.max((ri[i] as i32).abs()).max((rq[i] as i32).abs());
                }
                let mut buf: Vec<Complex<f32>> = ri[..n]
                    .iter()
                    .zip(&rq[..n])
                    .map(|(&i_val, &q_val)| Complex::new(i_val as f32, q_val as f32))
                    .collect();
                apply_hamming_window(&mut buf);
                fft.process(&mut buf);
                for i in 0..n {
                    acc[i] += buf[i].norm();
                }
                segs += 1;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }

    let dma_fs = rx.sampling_frequency / 16;
    println!(
        "\naudio DMA path: fs/16 = {} Hz (the rate SSB audio would arrive at here)",
        dma_fs
    );
    {
        let mut uio = system.clone_uio_file()?;
        let mut i_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut q_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut total = 0usize;
        let mut sumsq = 0.0f64;
        let start = Instant::now();
        let target_duration = Duration::from_secs_f32(secs);
        while start.elapsed() < target_duration {
            system.ensure_dma_running();
            let mut fds = [libc::pollfd {
                fd: uio.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            }];
            if unsafe { libc::poll(fds.as_mut_ptr(), 1, 200) } <= 0 {
                continue;
            }
            let mut int_info = [0u8; 4];
            if uio.read_exact(&mut int_info).is_err() {
                continue;
            }
            let nread = system
                .read_audio_dma_samples(&mut i_ch, &mut q_ch)
                .unwrap_or(0);
            for &v in i_ch.iter().take(nread) {
                sumsq += (v as f64) * (v as f64);
            }
            total += nread;
            i_ch.clear();
            q_ch.clear();
        }
        let elapsed = start.elapsed().as_secs_f32();
        let rms = if total > 0 {
            (sumsq / total as f64).sqrt()
        } else {
            0.0
        };
        println!(
            "  captured {} samples in {:.3}s (~{:.1} Hz effective), RMS {:.1}",
            total,
            elapsed,
            total as f32 / elapsed,
            rms
        );
        if total > (dma_fs as f32 * secs / 4.0) as usize {
            println!("  audio DMA path OK at low rate.");
        } else {
            println!("  audio DMA path delivered FEW/NO samples - needs investigation.");
        }
    }

    let _ = rx.disable_bb_fir(); // restore the normal FIR-bypassed rate range on the way out

    if segs == 0 {
        println!("RESULT: FAIL - no wideband bursts captured at this rate.");
        return Ok(());
    }
    for v in acc.iter_mut() {
        *v /= segs as f32;
    }

    // Report: DC/LO-leakage spike, noise floor (median), and their ratio - a live RX at low rate
    // shows a clear DC spike well above a flat noise floor.
    let dc = acc[0].max(acc[1]).max(acc[n - 1]);
    let mut sorted = acc.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = sorted[n / 2].max(1e-9);
    let dc_db = 20.0 * (dc / floor).log10();

    println!(
        "captured {} bursts, peak ADC sample {} (12-bit FS +/-2047)",
        segs, peak_adc
    );
    println!("DC/LO spike {:.1} dB over noise floor", dc_db);
    if peak_adc > 0 && dc_db > 12.0 {
        println!(
            "\nRESULT: PASS - AD9361 + FPGA deliver live samples at {:.3} MHz; waterfall bin is\n        {:.1} Hz ({:.1}x finer than 3.84 MHz).",
            fs / 1e6,
            bin_hz,
            3_840_000.0 / fs
        );
    } else {
        println!("\nRESULT: SUSPECT - samples flowed but no clear DC spike; inspect the chain.");
    }
    println!("=== LOW-SPAN TEST COMPLETE ===");
    Ok(())
}

/// End-to-end SSB TX->RX loopback at a low (AD9361-FIR) span. Transmits a
/// 1 kHz USB tone through the TX path (TxModulator + FPGA DUC + AD9361 TX FIR), receives it
/// through the audio path (audio DMA + FilterAudio + AnalyticSsbDemod), and checks the recovered
/// audio frequency. If the recovered tone is ~1 kHz and clean, SSB works end-to-end at low span.
/// Needs the TX -> attenuator -> RX loopback cable.
pub fn run_narrowband_loopback(rate_hz: i64, _secs: f32) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== LOW-SPAN SSB TX->RX LOOPBACK (AD9361 FIR) ===\n");

    let lo = 900_000_000i64;
    let antenna = 0u8;
    let offset = 50_000.0f64;
    let tone_hz = 1000.0f32;
    const AFS: f32 = 48_000.0;

    let mut pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    pluto.reset_device_state().map_err(|e| e.to_string())?;
    let mut tx = pluto.tx;
    let mut rx = pluto.rx;

    let fir = rate_hz < pluto::device::AD9361_MIN_FS_NO_FIR;
    if fir {
        rx.set_bb_rate_fir(rate_hz)?;
    } else {
        rx.disable_bb_fir()?;
        rx.set_frequencies(lo, rate_hz)?; // baseline control must actually set the requested rate
    }
    let fs = rx.sampling_frequency;
    let dma_fs = fs / 16; // audio DMA rate (cic=4)
    println!(
        "rate {} Hz (FIR {}), audio DMA {} Hz\n",
        fs,
        if fir { "ON" } else { "off" },
        dma_fs
    );

    let system = Arc::new(Mutex::new(pluto.system));
    {
        let mut sys = system.lock().unwrap();
        sys.rx_apply_dsp_config(antenna, fs);
        let (rounded_tx_fs, _cic_interp) = sys.tx_apply_dsp_config(tx.antenna, fs as f64);
        sys.reset_audio_dma_controller();
        // TX carrier at LO+offset; RX DDS undoes it -> recovered tone lands near baseband +tone.
        sys.tx_set_dds(offset, rounded_tx_fs * 2.0);
        sys.rx_set_dds(-offset, (fs * 2) as f64);
    }

    rx.set_frequencies(lo, fs)?;
    rx.set_rf_bandwidth(fs.max(200_000))?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;

    tx.antenna = antenna;
    tx.set_frequencies(lo, fs)?;
    tx.set_rf_bandwidth(fs.max(200_000))?;
    tx.set_gain(0.0)?;
    tx.init_channels()?;

    // RX capture + demod thread (mirrors the live audio path).
    let dma_fs_i = dma_fs;
    let sw_dec = ((dma_fs_i as f64 / AFS as f64).round() as usize).max(1);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_rx = stop.clone();
    let sys_rx = system.clone();
    let rx_handle = thread::spawn(move || -> Vec<f32> {
        let mut uio = { sys_rx.lock().unwrap().clone_uio_file().expect("uio") };
        let mut filt = FilterAudio::new(sw_dec, dma_fs_i, 3_000.0);
        let mut proc = AudioProcessor::new(Demodulation::SSB {
            fs: AFS,
            bfo_hz: 1500.0,
            audio_bw_hz: 3_000.0,
        });
        let mut audio: Vec<f32> = Vec::new();
        let mut i_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut q_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        while !stop_rx.load(Ordering::Relaxed) {
            {
                sys_rx.lock().unwrap().ensure_dma_running();
            }
            let mut fds = [libc::pollfd {
                fd: uio.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            }];
            if unsafe { libc::poll(fds.as_mut_ptr(), 1, 100) } <= 0 {
                continue;
            }
            let mut ii = [0u8; 4];
            if uio.read_exact(&mut ii).is_err() {
                continue;
            }
            let nread = {
                sys_rx
                    .lock()
                    .unwrap()
                    .read_audio_dma_samples(&mut i_ch, &mut q_ch)
                    .unwrap_or(0)
            };
            if nread == 0 {
                continue;
            }
            let iq = filt.execute(&i_ch, &q_ch);
            i_ch.clear();
            q_ch.clear();
            if !iq.is_empty() {
                proc.process(iq, &mut audio);
            }
        }
        audio
    });

    thread::sleep(Duration::from_millis(300));
    transmit_usb_tone(&mut tx, fs, tone_hz, 2.0)?;
    thread::sleep(Duration::from_millis(200));
    let _ = tx.set_gain(-89.75);
    stop.store(true, Ordering::Relaxed);
    let audio = rx_handle.join().map_err(|_| "RX thread panicked")?;
    {
        let mut sys = system.lock().unwrap();
        sys.rx_set_dds(0.0, (fs * 2) as f64);
    }
    let _ = rx.disable_bb_fir();

    // Analyze recovered audio: dominant frequency over the active region.
    println!(
        "recovered {} audio samples ({:.2}s @ {} Hz)",
        audio.len(),
        audio.len() as f32 / AFS,
        AFS as i64
    );
    let (peak_hz, snr_db) = dominant_tone(&audio, AFS);
    let err = (peak_hz - tone_hz).abs();
    println!(
        "dominant recovered tone: {:.1} Hz (expected {:.0} Hz, err {:.1} Hz), peak/spur {:.1} dB",
        peak_hz, tone_hz, err, snr_db
    );
    println!("\n========================================");
    if err < 60.0 && snr_db > 12.0 {
        println!("TEST RESULT: PASS");
    } else {
        println!("TEST RESULT: FAIL");
    }
    println!("========================================");
    println!("- Target Span: {:.3} MHz", fs as f64 / 1e6);
    println!("- Dominant Tone: {:.1} Hz (Expected: {:.1} Hz)", peak_hz, tone_hz);
    println!("- Frequency Error: {:.1} Hz", err);
    println!("- Peak/Spur Ratio: {:.1} dB (Threshold: >12.0 dB)", snr_db);
    println!("========================================\n");
    Ok(())
}


/// Modulates a continuous USB tone and pushes it to the TX DMA, clock-paced (uses resampler if dma_fs != 48 kHz).
/// Mirrors the live TX path's modulator + resampler + DC-block stages.
fn transmit_usb_tone(
    tx: &mut PlutoTxDevice,
    fs: i64,
    tone_hz: f32,
    secs: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut m = TxModulator::new(TxMode::USB, 3_000.0, fs as f32);
    let dma_fs = pluto::tx_dsp::tx_dma_audio_fs(fs as f32);
    let mut resampler = pluto::tx_dsp::IqResampler::for_dma_fs(dma_fs);
    println!(
        "  transmit_usb_tone: fs = {} Hz, dma_fs = {} Hz (resampler {})",
        fs,
        dma_fs,
        if resampler.is_some() {
            "active"
        } else {
            "bypassed"
        }
    );

    let chunk = 4096usize;
    let total = (secs * 48_000.0) as usize;
    let mut t = 0u64;
    let start = Instant::now();
    let mut done = 0usize;
    let mut sent_samples = 0usize;

    while done < total {
        let audio: Vec<f32> = (0..chunk)
            .map(|k| {
                (2.0 * std::f32::consts::PI * tone_hz * (t + k as u64) as f32 / 48_000.0).sin()
            })
            .collect();
        t += chunk as u64;
        done += chunk;
        let mut mi = Vec::new();
        let mut mq = Vec::new();
        m.process_chunk(&audio, &mut mi, &mut mq);

        if let Some(ref mut r) = resampler {
            let mut ri = Vec::new();
            let mut rq = Vec::new();
            r.process(&mi, &mq, &mut ri, &mut rq);
            if !ri.is_empty() {
                tx.write_buffer(&ri, &rq)?;
                sent_samples += ri.len();
            }
        } else {
            tx.write_buffer(&mi, &mq)?;
            sent_samples += mi.len();
        }

    }
    Ok(())
}

