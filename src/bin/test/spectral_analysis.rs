use pluto::device::{
    GainMode, MAX_AUDIO_SAMPLES, PlutoDevice, PlutoRxDevice, PlutoSystem, PlutoTxDevice,
};
use pluto::dsp::{AudioProcessor, Demodulation, FilterAudio};
use crate::test::dsp_helpers::{
    apply_hamming_window, dominant_tone, hamming_window, write_wav_f32_mono,
};
use pluto::tx_dsp::{IqResampler, TxMode, TxModulator, tx_dma_audio_fs};
use num_complex::Complex;
use rustfft::FftPlanner;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const AUDIO_SAMPLE_RATE: u32 = 48_000;

/// One point in the parameter sweep.
#[derive(Clone, Copy)]
struct Combo {
    fs_hz: i64,     // span / sample rate
    lo_hz: i64,     // center frequency
    offset_hz: f64, // listening-frequency offset from center (TX and RX tuned here together)
}

/// Sweeps combinations of {span x center-frequency x listening-offset} and runs a USB SSB
/// loopback at each, with TX and RX tuned to the same offset. Recovered audio is analyzed
/// in-process (dominant frequency and spur-suppression ratio) and reported as PASS/FAIL.
///
/// Purpose: prove the backend + FPGA tuning/demod chain is correct across the whole operating space
pub fn run_spec_audio_sweep(
    tone_hz: f32,
    duration_s: f32,
    save_wavs: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== TX/RX COMBINATION SWEEP TEST ===");

    // --- Build the grid ---
    let spans: [i64; 4] = [3_840_000, 7_680_000, 15_360_000, 30_720_000];
    let los: [i64; 2] = [900_000_000, 1_800_000_000];
    let offsets: [f64; 4] = [50_000.0, 300_000.0, -300_000.0, 700_000.0];

    let mut combos: Vec<Combo> = Vec::new();
    for &fs_hz in &spans {
        for &lo_hz in &los {
            for &offset_hz in &offsets {
                combos.push(Combo {
                    fs_hz,
                    lo_hz,
                    offset_hz,
                });
            }
        }
    }
    println!(
        "Grid: {} spans x {} centers x {} offsets = {} combos",
        spans.len(),
        los.len(),
        offsets.len(),
        combos.len()
    );

    println!("Content: {} Hz tone, {:.1}s", tone_hz, duration_s);

    // Open device
    println!("Opening PlutoDevice...");
    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    let mut tx = pluto.tx;
    let mut rx = pluto.rx;
    let system = Arc::new(Mutex::new(pluto.system));

    let antenna: u8 = 0;
    let mut results: Vec<(Combo, (bool, String))> = Vec::new();

    for (i, combo) in combos.iter().enumerate() {
        println!(
            "\n[{}/{}] fs={:.2} MHz  LO={:.0} MHz  offset={:+.0} kHz",
            i + 1,
            combos.len(),
            combo.fs_hz as f64 / 1e6,
            combo.lo_hz as f64 / 1e6,
            combo.offset_hz / 1e3
        );

        let recovered = run_combo(
            &mut tx, &mut rx, &system, antenna, combo, duration_s, tone_hz, 0.0,
        )?;

        let (peak_hz, snr_db) = dominant_tone(&recovered, AUDIO_SAMPLE_RATE as f32);
        let freq_err = (peak_hz - tone_hz).abs();
        let pass = freq_err <= 15.0 && snr_db >= 12.0;
        let detail = format!(
            "peak {:.1} Hz (err {:.1} Hz), spur ratio {:.1} dB",
            peak_hz, freq_err, snr_db
        );
        println!("    -> {}: {}", if pass { "PASS" } else { "FAIL" }, detail);

        if save_wavs {
            let name = format!(
                "sweep_{:.0}M_{:.0}M_{:+.0}k.wav",
                combo.fs_hz as f64 / 1e6,
                combo.lo_hz as f64 / 1e6,
                combo.offset_hz / 1e3
            );
            write_wav_f32_mono(&name, &recovered, AUDIO_SAMPLE_RATE, true)?;
            println!("    saved {}", name);
        }

        results.push((*combo, (pass, detail)));
    }

    // Summary table
    let _ = tx.set_gain(-89.75);
    println!("\n=== SWEEP SUMMARY ===");
    println!(
        "{:<10} {:<10} {:<10}  {:<6}  {}",
        "span", "LO", "offset", "verdict", "detail"
    );
    let mut n_pass = 0;
    for (combo, (pass, detail)) in &results {
        if *pass {
            n_pass += 1;
        }
        println!(
            "{:<10} {:<10} {:<10}  {:<6}  {}",
            format!("{:.2}M", combo.fs_hz as f64 / 1e6),
            format!("{:.0}M", combo.lo_hz as f64 / 1e6),
            format!("{:+.0}k", combo.offset_hz / 1e3),
            if *pass { "PASS" } else { "FAIL" },
            detail
        );
    }
    println!("\n{}/{} combos PASS", n_pass, results.len());
    if n_pass == results.len() {
        println!("ALL PASS - backend + FPGA tuning/demod correct across the whole grid.");
    } else {
        println!("Some combos FAILED - see rows above. The failing pattern localizes the bug.");
    }
    println!("=== SWEEP TEST COMPLETE ===");
    Ok(())
}

/// Characterizes the TRANSMITTED signal in isolation: transmits an equal-amplitude multitone
/// through the real TX path and captures the RAW 240 kHz IQ (before the RX demod filter), so the
/// measured spectrum reflects the TX chain only (RX CIC droop in this band is <0.1 dB; no FilterAudio
/// rolloff). Reports, per span, the per-tone USB level (audio-band flatness = what a QO-100 listener
/// hears), the opposite-sideband suppression (mirror image at -f), and the carrier suppression (DC).
pub fn run_spec_tx_shape() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== TX SPECTRUM CHARACTERIZATION (transmitted signal, isolated from RX demod) ===\n");

    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    let mut tx = pluto.tx;
    let mut rx = pluto.rx;
    let system = Arc::new(Mutex::new(pluto.system));
    let antenna: u8 = 0;
    let lo_hz: i64 = 900_000_000;
    let offset_hz = 50_000.0f64;
    let tones: Vec<f32> = vec![300.0, 600.0, 1000.0, 1500.0, 2000.0, 2500.0, 2900.0];

    for &fs_hz in &[3_840_000i64, 7_680_000i64] {
        println!("--- fs = {:.2} MHz ---", fs_hz as f64 / 1e6);
        let cic_decimation: u32 = ((fs_hz / 960_000).clamp(4, 32) as u32).next_power_of_two();
        {
            let mut sys = system.lock().unwrap();
            sys.rx_apply_dsp_config(antenna, fs_hz);
            let (rounded_tx_fs, _cic_interp) = sys.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
            sys.reset_audio_dma_controller();
            sys.tx_set_dds(offset_hz, rounded_tx_fs * 2.0);
            sys.rx_set_dds(-offset_hz, (fs_hz * 2) as f64);
        }
        rx.set_antenna(antenna)?;
        rx.set_frequencies(lo_hz, fs_hz)?;
        rx.set_rf_bandwidth(fs_hz)?;
        rx.set_gain(GainMode::Manual, Some(40.0))?;
        tx.antenna = antenna;
        tx.set_frequencies(lo_hz, fs_hz)?;
        tx.set_rf_bandwidth(fs_hz)?;
        tx.set_gain(0.0)?;
        tx.init_channels()?;
        let dma_fs = (fs_hz / cic_decimation as i64 / 4) as f32; // 240 kHz

        // Step one tone at a time (no inter-tone IMD). For each: capture raw IQ, measure the USB
        // line (+f) and its opposite-sideband image (-f).
        println!(
            "  {:>8}   {:>10}   {:>14}",
            "tone", "USB level", "opp-sideband"
        );
        let mut usb_lin: Vec<(f32, f32)> = Vec::new(); // (f, linear magnitude)
        let mut opp_db: Vec<f32> = Vec::new();
        for &f in &tones {
            let iq = capture_tx_iq(&mut tx, &system, fs_hz, f, 1.5)?;
            let (usb, opp) = tone_line_and_image(&iq, dma_fs, f);
            usb_lin.push((f, usb));
            let opp_dbc = 20.0 * ((opp / (usb + 1e-9)) + 1e-12).log10();
            opp_db.push(opp_dbc);
        }
        // USB levels in dB relative to the strongest tone (= audio-band frequency response).
        let ref_mag = usb_lin.iter().map(|x| x.1).fold(0.0f32, f32::max);
        for (i, &(f, m)) in usb_lin.iter().enumerate() {
            let lvl = 20.0 * ((m / (ref_mag + 1e-9)) + 1e-12).log10();
            println!("  {:>6.0}Hz   {:>8.1} dB   {:>10.1} dBc", f, lvl, opp_db[i]);
        }
        let levels: Vec<f32> = usb_lin
            .iter()
            .map(|x| 20.0 * ((x.1 / (ref_mag + 1e-9)) + 1e-12).log10())
            .collect();
        let flat = levels.iter().cloned().fold(f32::MIN, f32::max)
            - levels.iter().cloned().fold(f32::MAX, f32::min);
        let worst_opp = opp_db.iter().cloned().fold(f32::MIN, f32::max);
        println!(
            "  -> audio-band flatness {:.1} dB; worst opp-sideband {:.1} dBc",
            flat, worst_opp
        );
        println!();
    }
    let _ = tx.set_gain(-89.75);
    println!("=== TX SPECTRUM CHARACTERIZATION COMPLETE ===");
    Ok(())
}

/// Clean TX characterization via the WIDEBAND raw ADC (waterfall/burst path).
/// Transmits a 1 kHz USB tone via the real modulator path, captures bursts with
/// TX on and TX off, and reports: signal line, carrier suppression, opposite-sideband suppression,
/// and a SPUR SCAN (peaks that shouldn't be there), flagging which spurs are TX-generated by
/// comparing against the TX-off baseline.
pub fn run_spec_tx_wideband() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== TX WIDEBAND SPECTRUM (raw ADC, true transmitted RF) ===\n");

    let fs_hz: i64 = 3_840_000;
    let lo_hz: i64 = 900_000_000;
    let antenna: u8 = 0;
    let cic_decimation: u32 = 4;
    let tone_hz = 1000.0f32;

    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    let mut tx = pluto.tx;
    let mut rx = pluto.rx;
    let mut system = pluto.system;

    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();

    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.init_channels()?;
    rx.set_gain(GainMode::Manual, Some(20.0))?; // modest RX gain to keep the ADC well below clip

    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.init_channels()?;

    let n = 16384usize;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);

    // Baseline: TX OFF (RX-side / environmental spurs only)
    tx.set_gain(-89.75)?;
    thread::sleep(Duration::from_millis(100));
    let off_spec = capture_wideband_avg(&mut rx, &mut system, antenna, cic_decimation, &fft, n, 20);

    // TX ON: 1 kHz USB tone via the real modulator path
    tx.set_gain(-20.0)?; // backed off to avoid overdriving the RX ADC (clipping makes fake spurs)
    let mut peak_adc = 0i32;
    let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
    let dma_audio_fs = tx_dma_audio_fs(fs_hz as f32);
    let mut resampler = IqResampler::for_dma_fs(dma_audio_fs);
    let chunk_size = 4096usize;
    let mut t: u64 = 0;
    let (mut dc_pi, mut dc_pq, mut dc_oi, mut dc_oq) = (0i32, 0i32, 0i32, 0i32);
    let mut on_acc = vec![0.0f32; n];
    let mut on_segs = 0usize;
    // Prime the TX FIFO, then capture bursts while keeping it fed.
    for iter in 0..26 {
        let chunk: Vec<f32> = (0..chunk_size)
            .map(|k| {
                let tt = (t + k as u64) as f32 / AUDIO_SAMPLE_RATE as f32;
                (2.0 * std::f32::consts::PI * tone_hz * tt).sin()
            })
            .collect();
        t += chunk_size as u64;
        let mut mi = Vec::new();
        let mut mq = Vec::new();
        modulator.process_chunk(&chunk, &mut mi, &mut mq);
        for s in mi.iter_mut() {
            let x = *s as i32;
            dc_oi = x - dc_pi + (dc_oi * 998 / 1000);
            dc_pi = x;
            *s = dc_oi.clamp(-32768, 32767) as i16;
        }
        for s in mq.iter_mut() {
            let x = *s as i32;
            dc_oq = x - dc_pq + (dc_oq * 998 / 1000);
            dc_pq = x;
            *s = dc_oq.clamp(-32768, 32767) as i16;
        }
        match resampler.as_mut() {
            Some(r) => {
                let mut oi = Vec::new();
                let mut oq = Vec::new();
                r.process(&mi, &mq, &mut oi, &mut oq);
                tx.write_buffer(&oi, &oq)?;
            }
            None => tx.write_buffer(&mi, &mq)?,
        }
        if iter < 4 {
            continue; // let the tone fill the chain before capturing
        }
        system.trigger_waterfall_burst();
        if let Ok((ri, rq)) = rx.read_buffer() {
            let m = ri.len().min(rq.len()).min(n);
            if m >= n {
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
                    on_acc[i] += buf[i].norm();
                }
                on_segs += 1;
            }
        }
    }
    let _ = tx.set_gain(-89.75);
    if on_segs == 0 {
        println!("No TX-on bursts captured.");
        return Ok(());
    }
    for v in on_acc.iter_mut() {
        *v /= on_segs as f32;
    }
    // 12-bit ADC -> full scale is +/-2047. Flag if we're anywhere near clipping
    println!(
        "Peak ADC sample: {} (12-bit full scale +/-2047 -> {:.0}% of FS){}\n",
        peak_adc,
        peak_adc as f32 / 2047.0 * 100.0,
        if peak_adc > 1500 {
            "  <-- near clip, results suspect"
        } else {
            "  (headroom ok)"
        }
    );

    analyze_wideband(&on_acc, &off_spec, fs_hz as f32, n, lo_hz);

    println!("\n=== TX WIDEBAND COMPLETE ===");
    Ok(())
}

/// Averages the magnitude spectrum of several wideband bursts (TX state left as-is by caller).
fn capture_wideband_avg(
    rx: &mut PlutoRxDevice,
    system: &mut PlutoSystem,
    _antenna: u8,
    _cic_decimation: u32,
    fft: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    n: usize,
    bursts: usize,
) -> Vec<f32> {
    let mut acc = vec![0.0f32; n];
    let mut segs = 0usize;
    for _ in 0..bursts {
        system.trigger_waterfall_burst();
        if let Ok((ri, rq)) = rx.read_buffer() {
            if ri.len().min(rq.len()) >= n {
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
    if segs > 0 {
        for v in acc.iter_mut() {
            *v /= segs as f32;
        }
    }
    acc
}

/// Reports signal/carrier/opposite-sideband and a spur scan, comparing TX-on vs TX-off.
fn analyze_wideband(on: &[f32], off: &[f32], fs: f32, n: usize, lo_hz: i64) {
    // bin <-> baseband frequency (Hz, signed): bins [0,n/2) = +f, [n/2,n) = -(n-bin)
    let bin_freq = |b: usize| -> f32 {
        if b < n / 2 {
            b as f32 * fs / n as f32
        } else {
            (b as f32 - n as f32) * fs / n as f32
        }
    };
    let freq_bin = |f: f32| -> usize {
        ((f / fs * n as f32).round() as isize).rem_euclid(n as isize) as usize
    };
    let mag = |spec: &[f32], f: f32| -> f32 {
        let c = freq_bin(f) as isize;
        let mut m = 0.0f32;
        for d in -2..=2 {
            m = m.max(spec[(c + d).rem_euclid(n as isize) as usize]);
        }
        m
    };
    // Tight (+/-1 bin) probe so 1 kHz-spaced lines (~4 bins apart) don't smear together.
    let mag1 = |spec: &[f32], f: f32| -> f32 {
        let c = freq_bin(f) as isize;
        let mut m = 0.0f32;
        for d in -1..=1 {
            m = m.max(spec[(c + d).rem_euclid(n as isize) as usize]);
        }
        m
    };
    // Explicit probes: carrier at LO+50 kHz (DAC DC leak), the wanted USB tone at +51 kHz, and
    // the opposite (LSB) image at +49 kHz.
    let carrier = mag1(on, 50_000.0);
    let usb = mag1(on, 51_000.0);
    let lsb = mag1(on, 49_000.0);
    let (tone, opp, which) = if usb >= lsb {
        (usb, lsb, "USB (+51 kHz) - correct for USB mode")
    } else {
        (lsb, usb, "LSB (+49 kHz) - WRONG sideband for USB mode!")
    };
    let dbc = |m: f32| 20.0 * ((m / (tone + 1e-9)) + 1e-12).log10();
    println!("Wanted tone is on: {}", which);
    println!("  tone                 0.0 dBc (reference)");
    println!(
        "  carrier  (LO+50 kHz): {:.1} dBc   {}",
        dbc(carrier),
        if dbc(carrier) > -25.0 {
            "<-- POOR carrier suppression (DAC DC leak)"
        } else {
            "ok"
        }
    );
    println!(
        "  opp-sideband         : {:.1} dBc   {}",
        dbc(opp),
        if dbc(opp) > -30.0 {
            "<-- POOR sideband suppression"
        } else {
            "ok"
        }
    );

    // Fine spectrum profile around the signal (exact +/-1 kHz lines visible).
    println!("\nSpectrum around the signal (dBc vs the tone):");
    let mut f = 44_000.0f32;
    while f <= 56_000.0 {
        let d = dbc(mag1(on, f));
        let bar = "#".repeat(((d + 60.0).max(0.0) / 3.0) as usize);
        let label = match f as i32 {
            49_000 => " <lsb",
            50_000 => " <CARRIER",
            51_000 => " <usb(wanted)",
            _ => "",
        };
        println!(
            "  LO{:+6.0} kHz  {:>6.1} dBc {}{}",
            f / 1000.0,
            d,
            bar,
            label
        );
        f += 1_000.0;
    }

    // Noise floor = median of the TX-on spectrum.
    let mut sorted: Vec<f32> = on.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = sorted[n / 2].max(1e-6);

    // Spur scan: local-maxima bins well above the floor, excluding the wanted signal cluster
    // (49-51 kHz) and DC. Report offset from LO, level vs signal, and whether it's also present
    // with TX off
    println!(
        "\nSpur scan (peaks > 20 dB over noise floor, excluding the wanted 49-51 kHz signal):"
    );
    println!(
        "  {:>14}   {:>9}   {:>10}",
        "offset from LO", "level", "source"
    );
    let thresh = floor * 10.0; // +20 dB
    let mut spurs: Vec<(f32, f32, bool)> = Vec::new();
    for b in 2..n - 2 {
        let m = on[b];
        if m < thresh {
            continue;
        }
        if !(m >= on[b - 1] && m >= on[b + 1]) {
            continue;
        }
        let f = bin_freq(b);
        if f.abs() < 3000.0 {
            continue; // DC region
        }
        if (f - 50_000.0).abs() < 2_500.0 {
            continue; // wanted signal/carrier/sideband cluster
        }
        // Present with TX off? (within 6 dB) -> not TX-generated.
        let off_m = mag(off, f);
        let tx_generated = m > off_m * 2.0; // >6 dB stronger than baseline
        spurs.push((f, dbc(m), tx_generated));
    }
    // Keep the strongest ~15, sorted by level.
    spurs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    spurs.truncate(15);
    if spurs.is_empty() {
        println!("  (none above threshold - clean)");
    }
    for (f, lvl, txg) in &spurs {
        let _ = lo_hz;
        println!(
            "  {:>+11.1} kHz   {:>6.1} dBc   {}",
            f / 1000.0,
            lvl,
            if *txg { "TX-generated" } else { "RX/baseline" }
        );
    }
}

/// Transmits a tone while capturing the raw complex IQ from the audio DMA (no FilterAudio/demod).
fn capture_tx_iq(
    tx: &mut PlutoTxDevice,
    system: &Arc<Mutex<PlutoSystem>>,
    fs_hz: i64,
    tone_hz: f32,
    dur_s: f32,
) -> Result<Vec<Complex<f32>>, Box<dyn std::error::Error>> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let system_rx = system.clone();
    let stop_rx = stop_flag.clone();
    let rx_handle = thread::spawn(move || -> Vec<Complex<f32>> {
        let mut uio_file = {
            let sys = system_rx.lock().unwrap();
            sys.clone_uio_file().expect("clone UIO file")
        };
        let mut iq: Vec<Complex<f32>> = Vec::with_capacity(240_000 * 3);
        let mut i_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut q_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
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
            if unsafe { libc::poll(fds.as_mut_ptr(), 1, 100) } <= 0 {
                continue;
            }
            let mut int_info = [0u8; 4];
            if uio_file.read_exact(&mut int_info).is_err() {
                continue;
            }
            let n = {
                let mut sys = system_rx.lock().unwrap();
                sys.read_audio_dma_samples(&mut i_ch, &mut q_ch)
                    .unwrap_or(0)
            };
            if n == 0 {
                thread::sleep(Duration::from_micros(100));
                continue;
            }
            for (i, q) in i_ch.iter().zip(q_ch.iter()) {
                iq.push(Complex::new(*i as f32, *q as f32));
            }
            i_ch.clear();
            q_ch.clear();
        }
        iq
    });
    thread::sleep(Duration::from_millis(150));
    transmit(tx, fs_hz, dur_s, tone_hz)?;
    thread::sleep(Duration::from_millis(150));
    let _ = tx.set_gain(-89.75);
    tx.set_gain(0.0)?; // restore for the next tone
    stop_flag.store(true, Ordering::Relaxed);
    rx_handle.join().map_err(|_| "RX thread panicked".into())
}

/// Averaged complex-FFT magnitude at +f (USB line) and -f (opposite-sideband image).
fn tone_line_and_image(iq: &[Complex<f32>], fs: f32, f_hz: f32) -> (f32, f32) {
    if iq.len() < 8192 {
        return (0.0, 0.0);
    }
    let n = 8192usize;
    let lead = iq.len() / 5;
    let hann = hamming_window(n);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut acc = vec![0.0f32; n];
    let mut segs = 0usize;
    let mut start = lead;
    while start + n <= iq.len() {
        let mut buf: Vec<Complex<f32>> = (0..n).map(|i| iq[start + i] * hann[i]).collect();
        fft.process(&mut buf);
        for i in 0..n {
            acc[i] += buf[i].norm();
        }
        segs += 1;
        start += n / 2;
    }
    if segs == 0 {
        return (0.0, 0.0);
    }
    let peak = |f: f32| -> f32 {
        let c = (f / fs * n as f32).round() as isize;
        let mut m = 0.0f32;
        for d in -2..=2 {
            let b = (c + d).rem_euclid(n as isize) as usize;
            m = m.max(acc[b]);
        }
        m
    };
    (peak(f_hz), peak(-f_hz))
}

/// Configures the FPGA/RF for one combo, runs a single TX/RX loopback cycle, and returns the
/// recovered demodulated audio.
fn run_combo(
    tx: &mut PlutoTxDevice,
    rx: &mut PlutoRxDevice,
    system: &Arc<Mutex<PlutoSystem>>,
    antenna: u8,
    combo: &Combo,
    duration_s: f32,
    tone_hz: f32,
    rx_detune_hz: f64,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let fs_hz = combo.fs_hz;
    let lo_hz = combo.lo_hz;
    let offset_hz = combo.offset_hz;

    // RX audio CIC scales with fs so the audio-DMA rate stays ~240 kHz at any span.
    let cic_decimation: u32 = ((fs_hz / 960_000).clamp(4, 32) as u32).next_power_of_two();

    // Configure FPGA DSP + tune TX and RX DDS to the same offset
    {
        let mut sys = system.lock().unwrap();
        sys.rx_apply_dsp_config(antenna, fs_hz);
        let (rounded_tx_fs, _cic_interp) = sys.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
        sys.reset_audio_dma_controller();
        // Place the TX carrier at LO+offset and tune RX to bring LO+offset back to DC.
        // (tx_apply_dsp_config already set +50 kHz; override it with the swept offset.)
        sys.tx_set_dds(offset_hz, rounded_tx_fs * 2.0);
        // rx_detune_hz deliberately mis-tunes RX relative to TX (0 in the normal sweep). Used by
        // the DDS-scale measurement to observe how far the recovered tone shifts per Hz of detune.
        sys.rx_set_dds(-offset_hz - rx_detune_hz, (fs_hz * 2) as f64);
    }

    // Configure RF frontend
    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;

    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.set_gain(0.0)?;
    tx.init_channels()?;

    // --- Audio DMA reader thread (same pipeline as rf_loopback) ---
    let stop_flag = Arc::new(AtomicBool::new(false));
    let filter_bw = 3_000.0f32;
    let bfo_hz = filter_bw / 2.0; // sign only: + = USB (analytic demod ignores the magnitude)
    let if_cutoff_hz = filter_bw; // pass the full one-sided sideband [0, bw] (was bw/2)
    let demod = Demodulation::SSB {
        fs: AUDIO_SAMPLE_RATE as f32,
        bfo_hz,
        audio_bw_hz: filter_bw,
    };
    let dma_fs = fs_hz / cic_decimation as i64 / 4;
    let sw_decimation = ((dma_fs as f64 / AUDIO_SAMPLE_RATE as f64).round() as usize).max(1);

    let system_rx = system.clone();
    let stop_rx = stop_flag.clone();
    let rx_handle = thread::spawn(move || -> Vec<f32> {
        let mut uio_file = {
            let sys = system_rx.lock().unwrap();
            sys.clone_uio_file().expect("clone UIO file")
        };
        let mut audio_filter = FilterAudio::new(sw_decimation, dma_fs, if_cutoff_hz);
        let mut audio_processor = AudioProcessor::new(demod);
        let mut all_audio: Vec<f32> = Vec::with_capacity(AUDIO_SAMPLE_RATE as usize * 8);
        let mut audio_buffer: Vec<f32> = Vec::with_capacity(8192);
        let mut i_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
        let mut q_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
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
                    sys.reset_audio_dma_controller();
                    last_packet = Instant::now();
                }
                continue;
            }
            let mut int_info = [0u8; 4];
            if uio_file.read_exact(&mut int_info).is_err() {
                continue;
            }
            let total_read = {
                let mut sys = system_rx.lock().unwrap();
                sys.read_audio_dma_samples(&mut i_ch, &mut q_ch)
                    .unwrap_or(0)
            };
            if total_read == 0 {
                thread::sleep(Duration::from_micros(100));
                continue;
            }
            last_packet = Instant::now();
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
        }
        all_audio.extend_from_slice(&audio_buffer);
        all_audio
    });

    thread::sleep(Duration::from_millis(200));

    // --- Transmit (clock-paced, same TX path as the live app) ---
    transmit(tx, fs_hz, duration_s, tone_hz)?;
    thread::sleep(Duration::from_millis(300));

    let _ = tx.set_gain(-89.75);
    stop_flag.store(true, Ordering::Relaxed);
    let recovered = rx_handle.join().map_err(|_| "RX thread panicked")?;
    Ok(recovered)
}

/// Modulates the input tone (USB), DC-blocks, resamples to the TX DMA sample rate, and pushes to the TX DMA, paced to
/// real time. Mirrors `rf_loopback` functions.
fn transmit(
    tx: &mut PlutoTxDevice,
    fs_hz: i64,
    duration_s: f32,
    tone_hz: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
    let dma_audio_fs = tx_dma_audio_fs(fs_hz as f32);
    let mut resampler = IqResampler::for_dma_fs(dma_audio_fs);

    let chunk_size = 4096usize;
    let secs_per_chunk = chunk_size as f64 / AUDIO_SAMPLE_RATE as f64;
    let prefill_chunks = 2usize;
    let silence = vec![0.0f32; chunk_size];

    let mut all_chunks: Vec<Vec<f32>> = Vec::new();
    for _ in 0..prefill_chunks {
        all_chunks.push(silence.clone());
    }

    let total = (duration_s * AUDIO_SAMPLE_RATE as f32) as usize;
    let mut t: u64 = 0;
    let mut remaining = total;
    while remaining > 0 {
        let n = remaining.min(chunk_size);
        let mut chunk: Vec<f32> = (0..n)
            .map(|k| {
                let tt = (t + k as u64) as f32 / AUDIO_SAMPLE_RATE as f32;
                (2.0 * std::f32::consts::PI * tone_hz * tt).sin()
            })
            .collect();
        chunk.resize(chunk_size, 0.0);
        t += n as u64;
        remaining -= n;
        all_chunks.push(chunk);
    }
    for _ in 0..3 {
        all_chunks.push(silence.clone());
    }

    let tx_start = Instant::now();
    for (idx, chunk) in all_chunks.iter().enumerate() {
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
    }
    Ok(())
}
