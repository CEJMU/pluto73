use std::f32::consts::PI;
use std::f64::consts::PI as PI64;
use std::fs::{self, File};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use num_complex::Complex;
use rustfft::FftPlanner;

use crate::test::dma_diagnostics::{init_channels_cyclic, write_buffer_once};
use crate::test::dsp_helpers::{
    AUDIO_SAMPLE_RATE, LoopbackGuard, apply_hamming_window, capture_tag, dominant_tone_spurs,
    hamming_window, print_resolution_verdict, print_run_config, with_ad9361_loopback,
    write_wav_f32_mono,
};
use pluto::device::{
    GainMode, MAX_AUDIO_SAMPLES, PlutoDevice, PlutoRxDevice, PlutoSystem, PlutoTxDevice,
    rx_cic_decimation_for_rate, wait_for_uio_interrupt,
};
use pluto::dsp::{AudioProcessor, Demodulation, FilterAudio};
use pluto::tx_dsp::{IqResampler, TxMode, TxModulator, tx_dma_audio_fs};

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

        // Excluding +-100 Hz rather than dominant_tone's five bins: at this transform length a
        // Hamming window still has -42 dB of skirt at the sixth bin, so the narrow exclusion
        // reports the window rather than the chain and reads 43 dB regardless of the radio.
        let (peak_hz, _win_limited, spurs) =
            dominant_tone_spurs(&recovered, AUDIO_SAMPLE_RATE as f32, 1, 100.0);
        let snr_db = spurs.first().map(|&(_, d)| -d).unwrap_or(0.0);
        let spur_hz = spurs.first().map(|&(f, _)| f).unwrap_or(0.0);
        let freq_err = (peak_hz - tone_hz).abs();
        let pass = freq_err <= 15.0 && snr_db >= 12.0;
        let detail = format!(
            "peak {:.1} Hz (err {:.1} Hz), spur ratio {:.1} dB (limit at {:.0} Hz)",
            peak_hz, freq_err, snr_db, spur_hz
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
pub fn run_spec_tx_shape(loopback: bool) -> Result<(), Box<dyn std::error::Error>> {
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

    let _loopback = loopback.then(|| {
        LoopbackGuard::enable(
            "--loopback requested but AD9361 loopback unavailable; capturing over RF",
        )
    });

    for &fs_hz in &[3_840_000i64, 7_680_000i64] {
        println!("--- fs = {:.2} MHz ---", fs_hz as f64 / 1e6);
        let cic_decimation: u32 = rx_cic_decimation_for_rate(fs_hz);
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
pub fn run_spec_tx_wideband(loopback: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== TX WIDEBAND SPECTRUM (raw ADC, true transmitted RF) ===\n");

    let fs_hz: i64 = 3_840_000;
    // 2400.100 MHz, inside the QO-100 narrowband uplink passband
    let lo_hz: i64 = 2_400_100_000;
    let antenna: u8 = 0;
    let cic_decimation: u32 = 4;
    let tone_hz = 1000.0f32;
    print_run_config(lo_hz, fs_hz, 1_000_000.0, 40.0, loopback);

    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    let mut tx = pluto.tx;
    let mut rx = pluto.rx;
    let mut system = pluto.system;

    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();
    // analyze_wideband below probes fixed 999/1000/1001 kHz bins that assume +1 MHz.
    system.tx_set_dds(1_000_000.0, (fs_hz * 2) as f64);

    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.init_channels()?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;

    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.init_channels()?;

    let _loopback = loopback.then(|| {
        LoopbackGuard::enable(
            "--loopback requested but AD9361 loopback unavailable; capturing over RF",
        )
    });

    let n = 16384usize;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);

    // Baseline: TX OFF (RX-side / environmental spurs only)
    tx.set_gain(-89.75)?;
    thread::sleep(Duration::from_millis(100));
    let off_spec = capture_wideband_avg(&mut rx, &mut system, antenna, cic_decimation, &fft, n, 20);

    // TX ON: 1 kHz USB tone via the real modulator path
    tx.set_gain(0.0)?;
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
                (2.0 * PI * tone_hz * tt).sin()
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

    analyze_wideband(
        &on_acc,
        &off_spec,
        fs_hz as f32,
        n,
        lo_hz,
        capture_tag(loopback),
    );

    println!("\n=== TX WIDEBAND COMPLETE ===");
    Ok(())
}

/// Characterizes transmitter linearity using a 700 Hz + 1900 Hz two-tone audio test signal at 0.5 amplitude each.
/// Measures 3rd-order intermodulation distortion (IMD3) products at 2*f1 - f2 (-500 Hz) and 2*f2 - f1 (+3.1 kHz).
pub fn run_spec_tx_twotone(loopback: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== TX TWO-TONE IMD3 LINEARITY TEST (700 Hz + 1900 Hz, raw ADC wideband) ===\n");

    let fs_hz: i64 = 3_840_000;
    // 2400.100 MHz, inside the QO-100 narrowband uplink passband
    let lo_hz: i64 = 2_400_100_000;
    let antenna: u8 = 0;
    let f1 = 700.0f32;
    let f2 = 1900.0f32;
    print_run_config(lo_hz, fs_hz, 1_000_000.0, 40.0, loopback);

    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    let mut tx = pluto.tx;
    let mut rx = pluto.rx;
    let mut system = pluto.system;

    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();
    system.tx_set_dds(1_000_000.0, (fs_hz * 2) as f64);

    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.init_channels()?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;

    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.init_channels()?;

    let _loopback = loopback.then(|| {
        LoopbackGuard::enable(
            "--loopback requested but AD9361 loopback unavailable; capturing over RF",
        )
    });

    let n = 16384usize;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);

    // TX ON: Two-tone 700 Hz + 1900 Hz (0.5 peak amplitude each -> peak sum 1.0)
    tx.set_gain(0.0)?;
    let mut peak_adc = 0i32;
    let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
    let dma_audio_fs = tx_dma_audio_fs(fs_hz as f32);
    let mut resampler = IqResampler::for_dma_fs(dma_audio_fs);
    let chunk_size = 4096usize;
    let mut t: u64 = 0;
    let (mut dc_pi, mut dc_pq, mut dc_oi, mut dc_oq) = (0i32, 0i32, 0i32, 0i32);
    let mut on_acc = vec![0.0f32; n];
    let mut on_segs = 0usize;

    for iter in 0..26 {
        // Create Tones
        let chunk: Vec<f32> = (0..chunk_size)
            .map(|k| {
                let tt = (t + k as u64) as f32 / AUDIO_SAMPLE_RATE as f32;
                0.5 * (2.0 * PI * f1 * tt).sin() + 0.5 * (2.0 * PI * f2 * tt).sin()
            })
            .collect();
        t += chunk_size as u64;

        // Software TX
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
            continue;
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

    println!(
        "Peak ADC sample: {} (12-bit full scale +/-2047 -> {:.0}% of FS)\n",
        peak_adc,
        peak_adc as f32 / 2047.0 * 100.0
    );

    analyze_twotone(&on_acc, fs_hz as f32, n, capture_tag(loopback));

    println!("\n=== TX TWO-TONE IMD3 TEST COMPLETE ===");
    Ok(())
}

fn analyze_twotone(on: &[f32], fs: f32, n: usize, tag: &str) {
    // Bin/frequency conversion helpers
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
    // Tight (+/-1 bin) peak search so 1 kHz-spaced lines don't smear.
    let mag1 = |spec: &[f32], f: f32| -> f32 {
        let c = freq_bin(f) as isize;
        let mut m = 0.0f32;
        for d in -1..=1 {
            m = m.max(spec[(c + d).rem_euclid(n as isize) as usize]);
        }
        m
    };

    // Probe the expected lines
    let carrier_hz = 1_000_000.0f32;
    let f1_hz = carrier_hz + 700.0;
    let f2_hz = carrier_hz + 1900.0;
    let imd3_low_hz = carrier_hz - 500.0; // 2*f1 - f2
    let imd3_up_hz = carrier_hz + 3100.0; // 2*f2 - f1

    let m_f1 = mag1(on, f1_hz);
    let m_f2 = mag1(on, f2_hz);
    let ref_mag = m_f1.max(m_f2).max(1e-9);

    let m_carrier = mag1(on, carrier_hz);
    let m_imd3_low = mag1(on, imd3_low_hz);
    let m_imd3_up = mag1(on, imd3_up_hz);

    let dbc = |m: f32| 20.0 * ((m / ref_mag) + 1e-12).log10();

    let imd3_low_dbc = dbc(m_imd3_low);
    let imd3_up_dbc = dbc(m_imd3_up);
    let worst_imd3 = imd3_low_dbc.max(imd3_up_dbc);

    print_resolution_verdict(700.0, fs / n as f32, "carrier and IMD3 probes");

    println!("Two-Tone Signals & Intermodulation Products:");
    println!(
        "  Fundamental f1 (+700 Hz)   : LO+1000.70 kHz  {:>6.1} dBc",
        dbc(m_f1)
    );
    println!(
        "  Fundamental f2 (+1900 Hz)  : LO+1001.90 kHz  {:>6.1} dBc",
        dbc(m_f2)
    );
    println!(
        "  Carrier Residual (0 Hz)    : LO+1000.00 kHz  {:>6.1} dBc",
        dbc(m_carrier)
    );
    println!(
        "  IMD3 Lower (2*f1-f2, -500Hz): LO+999.50 kHz   {:>6.1} dBc",
        imd3_low_dbc
    );
    println!(
        "  IMD3 Upper (2*f2-f1, +3.1k) : LO+1003.10 kHz  {:>6.1} dBc",
        imd3_up_dbc
    );

    // ---- 4. Verdict ----
    println!("\nIMD3 Linearity Evaluation:");
    if worst_imd3 <= -40.0 {
        println!("  VERDICT: PASS (EXCELLENT LINEARITY: IMD3 <= -40.0 dBc)");
    } else if worst_imd3 <= -25.0 {
        println!("  VERDICT: PASS (GOOD AMATEUR SSB LINEARITY: IMD3 <= -25.0 dBc)");
    } else {
        println!("  VERDICT: FAIL (POOR LINEARITY: IMD3 > -25.0 dBc)");
    }

    // ---- 5. Export spectrum CSVs ----
    if let Ok(mut csv_file) = File::create(&format!("ssb_twotone_spectrum_{}.csv", tag)) {
        let _ = writeln!(csv_file, "freq_khz,power_dbc");
        let mut pts: Vec<(f32, f32)> = (0..n)
            .map(|b| {
                let freq_hz = bin_freq(b);
                let rel_freq_khz = (freq_hz - carrier_hz) / 1000.0;
                let power_dbc = 20.0 * ((on[b] / ref_mag).max(1e-6)).log10();
                (rel_freq_khz, power_dbc)
            })
            .filter(|&(f, _)| f >= -5.0 && f <= 5.0)
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (freq_khz, pwr) in pts {
            let _ = writeln!(csv_file, "{:.4},{:.2}", freq_khz, pwr);
        }
        println!(
            "\nExported two-tone 16k spectrum CSV to ssb_twotone_spectrum_{}.csv",
            tag
        );
    }

    if let Ok(mut csv_file) = File::create(&format!("ssb_twotone_wideband_spectrum_{}.csv", tag)) {
        let _ = writeln!(csv_file, "freq_khz,power_dbc");
        let mut pts: Vec<(f32, f32)> = (0..n)
            .map(|b| {
                let freq_hz = bin_freq(b);
                let rel_freq_khz = (freq_hz - carrier_hz) / 1000.0;
                let power_dbc = 20.0 * ((on[b] / ref_mag).max(1e-6)).log10();
                (rel_freq_khz, power_dbc)
            })
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (freq_khz, pwr) in pts {
            let _ = writeln!(csv_file, "{:.4},{:.2}", freq_khz, pwr);
        }
        println!(
            "Exported two-tone 16k full wideband spectrum CSV to ssb_twotone_wideband_spectrum_{}.csv",
            tag
        );
    }
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
fn analyze_wideband(on: &[f32], off: &[f32], fs: f32, n: usize, lo_hz: i64, tag: &str) {
    // bins [0, n/2) = positive frequencies, [n/2, n) = negative (aliased)
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

    // Wide (+/-2 bin) probe for isolated lines.
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

    // Probe the signal cluster:
    // carrier at LO+1000 kHz (NCO carrier), wanted USB tone at +1001 kHz, opposite (LSB) image at +999 kHz.
    let carrier = mag1(on, 1_000_000.0);
    let usb = mag1(on, 1_001_000.0);
    let lsb = mag1(on, 999_000.0);

    let (tone, opp, which) = if usb >= lsb {
        (usb, lsb, "USB (+1001 kHz) - correct for USB mode")
    } else {
        (lsb, usb, "LSB (+999 kHz) - WRONG sideband for USB mode!")
    };
    let dbc = |m: f32| 20.0 * ((m / (tone + 1e-9)) + 1e-12).log10();

    println!("Wanted tone is on: {}", which);
    print_resolution_verdict(
        1000.0,
        fs / n as f32,
        "carrier and opposite-sideband probes",
    );

    println!("  tone                 0.0 dBc (reference)");
    println!(
        "  carrier (LO+1000 kHz): {:.1} dBc   {}",
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

    // Fine spectrum profile around the signal: Exact +/-1 kHz lines visible as a text bar chart.
    println!("\nSpectrum around the signal (dBc vs the tone):");
    let mut f = 994_000.0f32;
    while f <= 1_006_000.0 {
        let d = dbc(mag1(on, f));
        let bar = "#".repeat(((d + 60.0).max(0.0) / 3.0) as usize);
        let label = match f as i32 {
            999_000 => " <lsb",
            1_000_000 => " <CARRIER",
            1_001_000 => " <usb(wanted)",
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

    if let Ok(mut csv_file) = File::create(&format!("ssb_16k_spectrum_{}.csv", tag)) {
        let _ = writeln!(csv_file, "freq_khz,power_dbc");
        let mut pts: Vec<(f32, f32)> = (0..n)
            .map(|b| {
                let freq_hz = bin_freq(b);
                let rel_freq_khz = (freq_hz - 1_000_000.0) / 1000.0;
                let power_dbc = 20.0 * ((on[b] / (tone + 1e-9)).max(1e-6)).log10();
                (rel_freq_khz, power_dbc)
            })
            .filter(|&(f, _)| f >= -5.0 && f <= 5.0)
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (freq_khz, pwr) in pts {
            let _ = writeln!(csv_file, "{:.4},{:.2}", freq_khz, pwr);
        }
        println!(
            "\nExported raw ADC 16k close-in spectrum CSV to ssb_16k_spectrum_{}.csv",
            tag
        );
    }

    if let Ok(mut csv_file) = File::create(&format!("ssb_16k_wideband_spectrum_{}.csv", tag)) {
        let _ = writeln!(csv_file, "freq_khz,power_dbc");
        let mut pts: Vec<(f32, f32)> = (0..n)
            .map(|b| {
                let freq_hz = bin_freq(b);
                let rel_freq_khz = (freq_hz - 1_000_000.0) / 1000.0;
                let power_dbc = 20.0 * ((on[b] / (tone + 1e-9)).max(1e-6)).log10();
                (rel_freq_khz, power_dbc)
            })
            .collect();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (freq_khz, pwr) in pts {
            let _ = writeln!(csv_file, "{:.4},{:.2}", freq_khz, pwr);
        }
        println!(
            "Exported raw ADC 16k full wideband spectrum CSV to ssb_16k_wideband_spectrum_{}.csv",
            tag
        );
    }

    // Spur scan: Noise floor = median of the TX-on spectrum.
    let mut sorted: Vec<f32> = on.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = sorted[n / 2].max(1e-6);

    // Local-maxima bins well above the floor, excluding the wanted signal cluster (999-1001 kHz) and DC.  Reports offset from LO, level vs signal, and whether the line is also present with TX off.
    println!(
        "\nSpur scan (peaks > 20 dB over noise floor, excluding the wanted 999-1001 kHz signal):"
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
        if (f - 1_000_000.0).abs() < 2_500.0 {
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
            match wait_for_uio_interrupt(&mut uio_file, 100) {
                Ok(Some(_)) => {}
                _ => continue,
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
    let cic_decimation: u32 = rx_cic_decimation_for_rate(fs_hz);

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
            match wait_for_uio_interrupt(&mut uio_file, 100) {
                Ok(Some(_)) => {}
                _ => {
                    if last_packet.elapsed().as_secs() > 3 {
                        let mut sys = system_rx.lock().unwrap();
                        sys.reset_audio_dma_controller();
                        last_packet = Instant::now();
                    }
                    continue;
                }
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
                (2.0 * PI * tone_hz * tt).sin()
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

    let _tx_start = Instant::now();
    for chunk in all_chunks.iter() {
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

/// Spectral summary of one raw capture:
/// Hann window over the whole block, locate the wanted tone, then read the sideband pairs the fabric strobe would produce if it were non-uniform.
fn report_pacing_sidebands(ri: &[i16], rq: &[i16], fs: f64) {
    let n = ri.len().min(rq.len());
    if n < 1024 {
        return;
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let scale = 2.0 * PI / n as f32;
    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|k| {
            let w = 0.5 - 0.5 * (scale * k as f32).cos();
            Complex::new(ri[k] as f32 * w, rq[k] as f32 * w)
        })
        .collect();
    fft.process(&mut buf);
    let mag: Vec<f32> = buf.iter().map(|c| c.norm()).collect();
    let bin = fs / n as f64;

    // Wanted tone: strongest line between 1 kHz and 500 kHz, where the DDS places it.
    let lo = ((1_000.0 / bin) as usize).max(1);
    let hi = ((500_000.0 / bin) as usize).min(n / 2);
    let mut pk = lo;
    for b in lo..hi {
        if mag[b] > mag[pk] {
            pk = b;
        }
    }
    let pv = mag[pk];
    if pv <= 0.0 {
        return;
    }
    println!(
        "    tone at {:.1} kHz; sidebands relative to it:",
        pk as f64 * bin / 1e3
    );
    for off in [-19_200.0f64, -9_600.0, 9_600.0, 19_200.0] {
        let c = ((pk as f64 * bin + off) / bin).round() as isize;
        let guard = (150.0 / bin).ceil() as isize;
        let mut m = 0.0f32;
        for d in -guard..=guard {
            m = m.max(mag[(c + d).rem_euclid(n as isize) as usize]);
        }
        println!(
            "      {:+6.1} kHz : {:6.1} dBc",
            off / 1e3,
            20.0 * ((m / pv) + 1e-15).log10()
        );
    }
    let mut sorted = mag.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "      floor    : {:6.1} dBc (median bin)",
        20.0 * ((sorted[n / 2] / pv) + 1e-15).log10()
    );
}

/// Continuous ungated raw-ADC capture for localizing the +-9.6 kHz spur pair.
/// Bypasses the burst gate (GPIO RX bit 2 = 0) so the wideband DMA streams the ADC
/// continuously, then dumps one contiguous 2^20-sample block of interleaved i16 IQ per TX
/// case to /root/spur_<case>.bin for host-side FFT analysis:
///   a) DDS +50 kHz, full drive        (reference)
///   b) DDS +37 kHz, full drive        (do the sidebands track the tone or stay at fixed f?)
///   c) DDS +50 kHz, -20 dB drive      (constant dBc = modulation, worse dBc = intermod)
///   d) TX muted                       (additive RX-side floor)
///   e) AD9361 BIST digital loopback   (TX digital data looped to RX inside the AD9361 bypasses DAC/RF/LO/ADC; spur present => digital/FPGA cause, absent => analog/electrical)
pub fn run_spur_probe(fs_hz: i64) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== SPUR PROBE (continuous ungated raw ADC) ===\n");
    println!("  Using sample rate: {} Hz", fs_hz);

    let lo_hz: i64 = 900_000_000;
    let antenna: u8 = 0;
    let n: usize = 1 << 20; // one contiguous block

    let pluto = PlutoDevice::open(n, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    let mut tx = pluto.tx;
    let mut rx = pluto.rx;
    let mut system = pluto.system;

    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(antenna, fs_hz as f64);
    system.reset_audio_dma_controller();
    // (DEFAULT_TX_OFFSET_HZ); cases below assume exactly +50 kHz per its own "DDS +50 kHz" label and the fixed capture/analysis convention.
    system.tx_set_dds(50_000.0, (fs_hz * 2) as f64);

    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.init_channels()?;
    rx.set_gain(GainMode::Manual, Some(40.0))?;

    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.init_channels()?;

    // Bypass the burst gate: the ADC then streams to the wideband DMA continuously.
    system.set_rx_burst_gate_enabled(false);

    let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
    let dma_audio_fs = tx_dma_audio_fs(fs_hz as f32);
    let mut resampler = IqResampler::for_dma_fs(dma_audio_fs);
    let mut t: u64 = 0;

    /// Modulates and pushes `chunks` x 4096 samples of a `tone_hz` tone (blocking on backpressure).
    fn feed(
        tx: &mut PlutoTxDevice,
        modulator: &mut TxModulator,
        resampler: &mut Option<IqResampler>,
        t: &mut u64,
        chunks: usize,
        tone_hz: f32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let chunk_size = 4096usize;
        for _ in 0..chunks {
            let chunk: Vec<f32> = (0..chunk_size)
                .map(|k| {
                    let tt = (*t + k as u64) as f32 / AUDIO_SAMPLE_RATE as f32;
                    (2.0 * PI * tone_hz * tt).sin()
                })
                .collect();
            *t += chunk_size as u64;
            let mut mi = Vec::new();
            let mut mq = Vec::new();
            modulator.process_chunk(&chunk, &mut mi, &mut mq);
            match resampler.as_mut() {
                Some(r) => {
                    let mut oi = Vec::new();
                    let mut oq = Vec::new();
                    r.process(&mi, &mq, &mut oi, &mut oq);
                    tx.write_buffer(&oi, &oq)?;
                }
                None => tx.write_buffer(&mi, &mq)?,
            }
        }
        Ok(())
    }

    /// Discards stale queued blocks, then writes one contiguous block as interleaved LE i16 IQ.
    fn capture_block(
        rx: &mut PlutoRxDevice,
        tx: &mut PlutoTxDevice,
        modulator: &mut TxModulator,
        resampler: &mut Option<IqResampler>,
        t: &mut u64,
        feed_tx: bool,
        tone_hz: f32,
        path: &str,
        fs: f64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..4 {
            if feed_tx {
                feed(tx, modulator, resampler, t, 4, tone_hz)?;
            }
            let _ = rx.read_buffer()?;
        }
        if feed_tx {
            feed(tx, modulator, resampler, t, 4, tone_hz)?;
        }
        let (ri, rq) = rx.read_buffer()?;
        let m = ri.len().min(rq.len());
        let mut bytes = Vec::with_capacity(m * 4);
        let mut peak = 0i32;
        for k in 0..m {
            bytes.extend_from_slice(&ri[k].to_le_bytes());
            bytes.extend_from_slice(&rq[k].to_le_bytes());
            peak = peak.max((ri[k] as i32).abs()).max((rq[k] as i32).abs());
        }
        fs::write(path, &bytes)?;
        println!("  wrote {} ({} samples, peak ADC {})", path, m, peak);
        report_pacing_sidebands(&ri, &rq, fs);
        Ok(())
    }

    tx.set_gain(0.0)?;
    println!("case a: DDS +50 kHz, full drive (tone at LO+51 kHz)");
    capture_block(
        &mut rx,
        &mut tx,
        &mut modulator,
        &mut resampler,
        &mut t,
        true,
        1000.0,
        "/root/spur_a.bin",
        fs_hz as f64,
    )?;

    system.tx_set_dds(37_000.0, (fs_hz * 2) as f64);
    println!("case b: DDS +37 kHz, full drive (tone at LO+38 kHz)");
    capture_block(
        &mut rx,
        &mut tx,
        &mut modulator,
        &mut resampler,
        &mut t,
        true,
        1000.0,
        "/root/spur_b.bin",
        fs_hz as f64,
    )?;

    system.tx_set_dds(50_000.0, (fs_hz * 2) as f64);
    tx.set_gain(-20.0)?;
    println!("case c: DDS +50 kHz, -20 dB drive");
    capture_block(
        &mut rx,
        &mut tx,
        &mut modulator,
        &mut resampler,
        &mut t,
        true,
        1000.0,
        "/root/spur_c.bin",
        fs_hz as f64,
    )?;

    tx.set_gain(-89.75)?;
    println!("case d: TX muted (RX-side floor)");
    capture_block(
        &mut rx,
        &mut tx,
        &mut modulator,
        &mut resampler,
        &mut t,
        false,
        1000.0,
        "/root/spur_d.bin",
        fs_hz as f64,
    )?;

    tx.set_gain(0.0)?;
    with_ad9361_loopback("case e SKIPPED: cannot set AD9361 loopback", || {
        println!("case e: AD9361 digital loopback (BIST), DDS +50 kHz, full drive");
        capture_block(
            &mut rx,
            &mut tx,
            &mut modulator,
            &mut resampler,
            &mut t,
            true,
            1000.0,
            "/root/spur_e.bin",
            fs_hz as f64,
        )
    })?;

    /// Pushes `chunks` x 4096 samples of constant DC I/Q (bypasses modulator + resampler math).
    fn feed_dc(tx: &mut PlutoTxDevice, chunks: usize) -> Result<(), Box<dyn std::error::Error>> {
        let di = vec![12000i16; 4096];
        let dq = vec![0i16; 4096];
        for _ in 0..chunks {
            tx.write_buffer(&di, &dq)?;
        }
        Ok(())
    }

    /// Same as capture_block but with the DC feed.
    fn capture_block_dc(
        rx: &mut PlutoRxDevice,
        tx: &mut PlutoTxDevice,
        path: &str,
        fs: f64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..4 {
            feed_dc(tx, 4)?;
            let _ = rx.read_buffer()?;
        }
        feed_dc(tx, 4)?;
        let (ri, rq) = rx.read_buffer()?;
        let m = ri.len().min(rq.len());
        let mut bytes = Vec::with_capacity(m * 4);
        for k in 0..m {
            bytes.extend_from_slice(&ri[k].to_le_bytes());
            bytes.extend_from_slice(&rq[k].to_le_bytes());
        }
        fs::write(path, &bytes)?;
        println!("  wrote {} ({} samples)", path, m);
        report_pacing_sidebands(&ri, &rq, fs);
        Ok(())
    }

    println!("case f: constant DC I/Q feed (no modulator), RF path, carrier at LO+50 kHz");
    capture_block_dc(&mut rx, &mut tx, "/root/spur_f.bin", fs_hz as f64)?;

    with_ad9361_loopback("case g SKIPPED: cannot set AD9361 loopback", || {
        println!("case g: constant DC I/Q feed, AD9361 digital loopback");
        capture_block_dc(&mut rx, &mut tx, "/root/spur_g.bin", fs_hz as f64)
    })?;

    with_ad9361_loopback("cases h/i SKIPPED: cannot set AD9361 loopback", || {
        println!("case h: 2 kHz audio tone, AD9361 digital loopback (PM-depth-vs-f_audio scaling)");
        capture_block(
            &mut rx,
            &mut tx,
            &mut modulator,
            &mut resampler,
            &mut t,
            true,
            2000.0,
            "/root/spur_h.bin",
            fs_hz as f64,
        )?;
        println!("case i: 500 Hz audio tone, AD9361 digital loopback");
        capture_block(
            &mut rx,
            &mut tx,
            &mut modulator,
            &mut resampler,
            &mut t,
            true,
            500.0,
            "/root/spur_i.bin",
            fs_hz as f64,
        )
    })?;

    with_ad9361_loopback("case j SKIPPED: cannot set AD9361 loopback", || {
        println!("case j: DSP BYPASS mode (enabled=0), AD9361 digital loopback, 1 kHz tone");
        system.set_tx_dsp_enabled(false);
        let result = capture_block(
            &mut rx,
            &mut tx,
            &mut modulator,
            &mut resampler,
            &mut t,
            true,
            1000.0,
            "/root/spur_j.bin",
            fs_hz as f64,
        );
        system.set_tx_dsp_enabled(true);
        result
    })?;

    println!("case k: DSP BYPASS mode (enabled=0), RF loopback path, 1 kHz tone");
    system.set_tx_dsp_enabled(false);
    capture_block(
        &mut rx,
        &mut tx,
        &mut modulator,
        &mut resampler,
        &mut t,
        true,
        1000.0,
        "/root/spur_k.bin",
        fs_hz as f64,
    )?;
    system.set_tx_dsp_enabled(true);

    println!("\n=== SPUR PROBE COMPLETE ===");
    println!("scp the /root/spur_*.bin files to the host for FFT analysis.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Radiated-signal characterisation
// ---------------------------------------------------------------------------

/// Renders exactly one seamless cyclic-buffer period of `tones` through the real TX modulator.
fn render_tx_loop(fs_hz: i64, loop_len: usize, tones: &[(f64, f32)]) -> (Vec<i16>, Vec<i16>) {
    let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
    let audio: Vec<f32> = (0..loop_len * 5)
        .map(|k| {
            let t = k as f64 / AUDIO_SAMPLE_RATE as f64;
            tones
                .iter()
                .map(|&(f, a)| a * ((2.0 * PI64 * f * t).sin() as f32))
                .sum::<f32>()
        })
        .collect();
    let (mut mi, mut mq) = (Vec::new(), Vec::new());
    modulator.process_chunk(&audio, &mut mi, &mut mq);
    let start = mi.len() - loop_len;
    (mi[start..].to_vec(), mq[start..].to_vec())
}

/// Loads one cyclic period into the TX DMA and captures one contiguous raw wideband block.
fn radiated_case(
    tx: &mut PlutoTxDevice,
    rx: &mut PlutoRxDevice,
    fft: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    fs_hz: i64,
    loop_len: usize,
    n: usize,
    tones: &[(f64, f32)],
) -> Result<(Vec<f32>, i32), Box<dyn std::error::Error>> {
    let (li, lq) = render_tx_loop(fs_hz, loop_len, tones);
    init_channels_cyclic(tx)?;
    write_buffer_once(tx, &li, &lq)?;
    // Long enough for the interpolator chain to fill and for the receiver's tracking quadrature correction to converge on this signal.
    thread::sleep(Duration::from_millis(2500));

    // Discard queued blocks so the capture cannot contain the previous case's signal.
    for _ in 0..4 {
        let _ = rx.read_buffer()?;
    }
    let (ri, rq) = rx.read_buffer()?;
    if ri.len().min(rq.len()) < n {
        return Err(format!("short RX block: {} < {}", ri.len().min(rq.len()), n).into());
    }

    let mut peak = 0i32;
    let mut buf: Vec<Complex<f32>> = Vec::with_capacity(n);
    let w_scale = 2.0 * PI / n as f32;
    for k in 0..n {
        peak = peak.max((ri[k] as i32).abs()).max((rq[k] as i32).abs());
        let w = 0.5 - 0.5 * (w_scale * k as f32).cos(); // Hann: -31 dB sidelobes, 1.42 dB scalloping
        buf.push(Complex::new(ri[k] as f32 * w, rq[k] as f32 * w));
    }
    fft.process(&mut buf);
    Ok((buf.iter().map(|c| c.norm()).collect(), peak))
}

/// Characterises the signal the antenna actually radiates.
///
/// Chain under test, end to end, identical to what the application transmits:
///     audio -> `TxModulator` (software analytic SSB FIR) -> TX DMA -> FPGA CIC interpolator -> FIR -> DDS mixer -> DAC -> antenna
/// Readback is the stock ADI wideband path the waterfall uses with the burst gate bypassed so the ADC streams continuously:
///     ADC -> cpack -> wideband DMA
///
/// Reports carrier suppression, opposite-sideband suppression, two-tone IMD3/IMD5, the analog quadrature image mirrored about the LO (which nothing else in the suite measures), and a wideband spur scan across the full span.
pub fn run_spec_tx_radiated(loopback: bool) -> Result<(), Box<dyn std::error::Error>> {
    let fs_hz: i64 = 3_840_000;
    // Inside the QO-100 narrowband uplink passband
    let lo_hz: i64 = 2_400_100_000;
    let antenna: u8 = 0;
    let off = 1_000_000.0f64; // fabric DDS offset, matches DEFAULT_TX_OFFSET_HZ
    let rx_gain = 40.0f64;
    let loop_len = 4800usize; // 100 ms at 48 kHz -> seamless for any multiple of 10 Hz
    let n = 1usize << 20; // 273 ms -> 3.66 Hz bins

    println!("=== RADIATED TX SIGNAL (software modulator -> FPGA TX -> raw wideband ADC) ===\n");
    println!("TX: real modulator + FPGA interpolator/DDS/DAC, cyclic DMA (cannot starve)");
    println!("RX: stock wideband path, burst gate bypassed - no custom receive DSP in the loop\n");
    print_run_config(lo_hz, fs_hz, off, rx_gain, loopback);

    let dma_fs = tx_dma_audio_fs(fs_hz as f32);
    if dma_fs != AUDIO_SAMPLE_RATE {
        return Err(format!(
            "TX DMA feed rate is {} Hz, not {} Hz; the cyclic loop would need resampling and \
             would no longer wrap seamlessly",
            dma_fs, AUDIO_SAMPLE_RATE
        )
        .into());
    }

    let pluto = PlutoDevice::open(n, loop_len).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));
    let mut tx = pluto.tx;
    let mut rx = pluto.rx;
    let mut system = pluto.system;

    system.rx_apply_dsp_config(antenna, fs_hz);
    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);
    system.reset_audio_dma_controller();
    system.tx_set_dds(off, (fs_hz * 2) as f64);

    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    rx.init_channels()?;
    rx.set_gain(GainMode::Manual, Some(rx_gain))?;

    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.set_gain(0.0)?;

    // Settle the receiver's quadrature calibration before measuring anything
    rx.set_gain(GainMode::Manual, Some(rx_gain - 10.0))?;
    thread::sleep(Duration::from_millis(300));
    rx.set_gain(GainMode::Manual, Some(rx_gain))?;
    thread::sleep(Duration::from_millis(500));

    let _loopback = loopback.then(|| {
        LoopbackGuard::enable("--loopback requested but AD9361 loopback unavailable; over RF")
    });

    // Bypass the burst gate so the wideband DMA streams the ADC continuously.
    system.set_rx_burst_gate_enabled(false);

    let bin_hz = fs_hz as f64 / n as f64;
    print_resolution_verdict(1000.0, bin_hz as f32, "carrier and sideband probes");

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);

    println!("\nCapturing (5 x ~1.4 s)...");
    let (sil_spec, sil_peak) = radiated_case(&mut tx, &mut rx, &fft, fs_hz, loop_len, n, &[])?;
    let (one_spec, one_peak) =
        radiated_case(&mut tx, &mut rx, &fft, fs_hz, loop_len, n, &[(1000.0, 1.0)])?;
    let (two_spec, two_peak) = radiated_case(
        &mut tx,
        &mut rx,
        &fft,
        fs_hz,
        loop_len,
        n,
        &[(700.0, 0.5), (1900.0, 0.5)],
    )?;

    // Baseline with the transmitter muted: anything still present is additive (receiver noise, ADC/clock spurs, ambient RF) and is not being radiated by this chain at all.
    tx.set_gain(-89.75)?;
    let (off_spec, off_peak) = radiated_case(&mut tx, &mut rx, &fft, fs_hz, loop_len, n, &[])?;
    tx.set_gain(0.0)?;

    // Attribution capture: drop the receiver gain 10 dB, leaving the RF at the antenna untouched.
    // A product that is already on the incoming signal keeps the same ratio to it.
    // One the  receiver manufactures from that signal is driven by the level inside the receiver, so backing the gain off improves its ratio.
    let gain_step = 10.0f64;
    rx.set_gain(GainMode::Manual, Some(rx_gain - gain_step))?;
    thread::sleep(Duration::from_millis(200));
    let (lowg_spec, lowg_peak) =
        radiated_case(&mut tx, &mut rx, &fft, fs_hz, loop_len, n, &[(1000.0, 1.0)])?;
    rx.set_gain(GainMode::Manual, Some(rx_gain))?;

    // Peak search spans +-3 bins so a line that falls between bins is not under-read.
    let probe = |spec: &[f32], f_hz: f64| -> f32 {
        let c = (f_hz / bin_hz).round() as isize;
        let mut m = 0.0f32;
        for d in -3..=3 {
            m = m.max(spec[(c + d).rem_euclid(n as isize) as usize]);
        }
        m
    };

    println!(
        "\nPeak ADC: silence {}, single {} ({:.1}% FS), two-tone {} ({:.1}% FS), TX off {}",
        sil_peak,
        one_peak,
        one_peak as f32 / 20.48,
        two_peak,
        two_peak as f32 / 20.48,
        off_peak
    );

    let reference = probe(&one_spec, off + 1000.0);
    let dbc = |m: f32| 20.0 * ((m / (reference + 1e-9)) + 1e-12).log10();
    println!(
        "Reference (wanted tone, single-tone case): {:.1} magnitude\n",
        reference
    );

    println!("--- SILENCE (modulator fed zeros; what radiates with no audio) ---");
    println!(
        "  carrier   @ LO+{:.0} kHz : {:>6.1} dBc",
        off / 1e3,
        dbc(probe(&sil_spec, off))
    );
    println!(
        "  LO leak   @ LO          : {:>6.1} dBc",
        dbc(probe(&sil_spec, 0.0))
    );
    println!(
        "  noise ref @ LO+{:.0} kHz : {:>6.1} dBc",
        (off + 20_000.0) / 1e3,
        dbc(probe(&sil_spec, off + 20_000.0))
    );

    println!("\n--- SINGLE TONE (1 kHz USB) ---");
    println!(
        "  wanted    @ LO+{:.1} kHz : {:>6.1} dBc  (reference)",
        (off + 1000.0) / 1e3,
        0.0
    );
    println!(
        "  carrier   @ LO+{:.1} kHz : {:>6.1} dBc",
        off / 1e3,
        dbc(probe(&one_spec, off))
    );
    println!(
        "  opp sideb @ LO+{:.1} kHz : {:>6.1} dBc",
        (off - 1000.0) / 1e3,
        dbc(probe(&one_spec, off - 1000.0))
    );
    println!(
        "  LO image  @ LO-{:.1} kHz : {:>6.1} dBc  <-- analog quadrature image",
        (off + 1000.0) / 1e3,
        dbc(probe(&one_spec, -(off + 1000.0)))
    );
    println!(
        "  img carr  @ LO-{:.1} kHz : {:>6.1} dBc",
        off / 1e3,
        dbc(probe(&one_spec, -off))
    );
    println!(
        "  noise ref @ LO+{:.0} kHz : {:>6.1} dBc",
        (off + 20_000.0) / 1e3,
        dbc(probe(&one_spec, off + 20_000.0))
    );

    let ref2 = probe(&two_spec, off + 700.0);
    let dbc2 = |m: f32| 20.0 * ((m / (ref2 + 1e-9)) + 1e-12).log10();
    println!("\n--- TWO TONE (700 Hz + 1900 Hz, each at half amplitude) ---");
    println!(
        "  f1        @ LO+{:.1} kHz : {:>6.1} dBc  (reference)",
        (off + 700.0) / 1e3,
        0.0
    );
    println!(
        "  f2        @ LO+{:.1} kHz : {:>6.1} dBc",
        (off + 1900.0) / 1e3,
        dbc2(probe(&two_spec, off + 1900.0))
    );
    println!(
        "  carrier   @ LO+{:.1} kHz : {:>6.1} dBc",
        off / 1e3,
        dbc2(probe(&two_spec, off))
    );
    println!(
        "  IMD3 low  @ LO+{:.1} kHz : {:>6.1} dBc",
        (off - 500.0) / 1e3,
        dbc2(probe(&two_spec, off - 500.0))
    );
    println!(
        "  IMD3 high @ LO+{:.1} kHz : {:>6.1} dBc",
        (off + 3100.0) / 1e3,
        dbc2(probe(&two_spec, off + 3100.0))
    );
    println!(
        "  IMD5 low  @ LO+{:.1} kHz : {:>6.1} dBc",
        (off - 1700.0) / 1e3,
        dbc2(probe(&two_spec, off - 1700.0))
    );
    println!(
        "  IMD5 high @ LO+{:.1} kHz : {:>6.1} dBc",
        (off + 4300.0) / 1e3,
        dbc2(probe(&two_spec, off + 4300.0))
    );
    println!(
        "  noise ref @ LO+{:.0} kHz : {:>6.1} dBc",
        (off + 20_000.0) / 1e3,
        dbc2(probe(&two_spec, off + 20_000.0))
    );

    // Attribution: is it leaving the antenna, or is the receiver inventing it?
    // Referenced to the wanted tone within its own capture, so the 10 dB gain change itself cannot show up as a level difference - only a change in ratio counts.
    let lowg_ref = probe(&lowg_spec, off + 1000.0);
    let dbc_l = |m: f32| 20.0 * ((m / (lowg_ref + 1e-9)) + 1e-12).log10();

    println!(
        "\n--- SPUR ATTRIBUTION (RX gain {:.0} dB vs {:.0} dB, peak ADC {} vs {}) ---",
        rx_gain,
        rx_gain - gain_step,
        one_peak,
        lowg_peak
    );
    if loopback {
        println!(
            "  Digital loopback: DAC, mixer, PA, ADC and both LOs are out of the path, so every"
        );
        println!(
            "  line below was produced by the software modulator or the FPGA, and the gain step"
        );
        println!("  is not in the signal path - no radiated/receiver split applies here.");
    } else {
        println!("  Ratio unchanged => the product is already on the incoming RF, i.e. RADIATED.");
        println!("  Ratio improves  => the receiver made it from the signal, not transmitted.");
    }
    println!(
        "  {:>12} {:>9} {:>9} {:>9} {:>8}  {}",
        "offset", "level", "TX-muted", "low-gain", "delta", "verdict"
    );

    let bin_freq = |i: usize| -> f64 {
        if i < n / 2 {
            i as f64 * bin_hz
        } else {
            (i as f64 - n as f64) * bin_hz
        }
    };
    // The image is reported first by name; the scan then finds whatever else is out there.
    let mut targets: Vec<(f64, &str)> = vec![(-(off + 1000.0), "quadrature image")];
    let mut candidates: Vec<(f32, f64)> = Vec::new();
    for i in 0..n {
        let f = bin_freq(i);
        // Exclude the occupied channel and the DC/LO region; both are measured explicitly above.
        if (f - off).abs() < 20_000.0 || f.abs() < 20_000.0 {
            continue;
        }
        candidates.push((one_spec[i], f));
    }
    candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (_, f) in candidates {
        if targets.len() >= 8 {
            break;
        }
        if targets.iter().any(|&(s, _)| (s - f).abs() < 5_000.0) {
            continue; // same peak, adjacent bin, or already listed by name
        }
        targets.push((f, ""));
    }
    for (f, label) in &targets {
        let level = dbc(probe(&one_spec, *f));
        let muted = dbc(probe(&off_spec, *f));
        let low = dbc_l(probe(&lowg_spec, *f));
        let delta = low - level;
        let verdict = if loopback {
            "digital origin"
        } else if muted > level - 6.0 {
            "additive (present with TX muted)"
        } else if delta < -5.0 {
            "receiver artefact - NOT transmitted"
        } else if delta > 5.0 {
            "inconclusive (worse at low gain)"
        } else {
            "RADIATED"
        };
        println!(
            "  {:>+9.1} kHz {:>8.1} {:>9.1} {:>9.1} {:>+8.1}  {} {}",
            f / 1e3,
            level,
            muted,
            low,
            delta,
            verdict,
            label
        );
    }

    // Repeatability: the same measurement five times, nothing reconfigured between runs.
    println!("\n--- REPEATABILITY (5 consecutive single-tone captures) ---");
    let watch: [(f64, &str); 4] = [
        (off, "carrier"),
        (off - 1000.0, "opp sideband"),
        (-(off + 1000.0), "quadrature image"),
        (
            targets.get(1).map(|t| t.0).unwrap_or(off + 600_000.0),
            "largest other spur",
        ),
    ];
    let mut runs: Vec<Vec<f32>> = Vec::new();
    for _ in 0..5 {
        let (s, _) = radiated_case(&mut tx, &mut rx, &fft, fs_hz, loop_len, n, &[(1000.0, 1.0)])?;
        let r = probe(&s, off + 1000.0);
        runs.push(
            watch
                .iter()
                .map(|&(f, _)| 20.0 * ((probe(&s, f) / (r + 1e-9)) + 1e-12).log10())
                .collect(),
        );
    }
    println!(
        "  {:>20} {:>8} {:>8} {:>8}   runs",
        "line", "min", "max", "spread"
    );
    for (j, (f, name)) in watch.iter().enumerate() {
        let vals: Vec<f32> = runs.iter().map(|r| r[j]).collect();
        let lo_v = vals.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi_v = vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let list: Vec<String> = vals.iter().map(|v| format!("{:.1}", v)).collect();
        println!(
            "  {:>20} {:>8.1} {:>8.1} {:>8.1}   [{}]  @ {:+.1} kHz",
            name,
            lo_v,
            hi_v,
            hi_v - lo_v,
            list.join(", "),
            f / 1e3
        );
    }

    // Spectra for plotting. Each case gets two exports:
    // the full span for the spur survey, and a close-in window around the suppressed carrier.
    // The close-in one is needed because at full-span decimation the carrier, the wanted tone and the opposite sideband are 1 kHz apart and would collapse into a single row.
    let tag = capture_tag(loopback);
    let window = |spec: &[f32],
                  reference: f32,
                  f_lo: f64,
                  f_hi: f64,
                  rows: usize,
                  f_ref: f64,
                  khz: bool|
     -> String {
        let step = (f_hi - f_lo) / rows as f64;
        let mut out = String::from(if khz {
            "freq_khz,dbc\n"
        } else {
            "freq_hz,dbc\n"
        });
        for r in 0..rows {
            let fa = f_lo + r as f64 * step;
            let ia = (fa / bin_hz).round() as isize;
            let ib = ((fa + step) / bin_hz).round() as isize;
            let mut m = 0.0f32;
            for i in ia..ib.max(ia + 1) {
                m = m.max(spec[i.rem_euclid(n as isize) as usize]);
            }
            let v = 20.0 * ((m / (reference + 1e-9)) + 1e-12).log10();
            let f = fa - f_ref;
            if khz {
                out.push_str(&format!("{:.4},{:.2}\n", f / 1e3, v));
            } else {
                out.push_str(&format!("{:.1},{:.2}\n", f, v));
            }
        }
        out
    };

    let half = fs_hz as f64 / 2.0;
    for (spec, r, kind) in [(&one_spec, reference, "tone"), (&two_spec, ref2, "twotone")] {
        for (body, span, name) in [
            (
                window(spec, r, -half, half, 4000, 0.0, false),
                "full span",
                format!("radiated_{}_{}.csv", kind, tag),
            ),
            (
                window(spec, r, off - 10_000.0, off + 10_000.0, 2000, off, true),
                "+-10 kHz",
                format!("radiated_closein_{}_{}.csv", kind, tag),
            ),
        ] {
            let path = format!("/root/{}", name);
            fs::write(&path, body)?;
            println!(
                "Exported {} ({}, RBW {:.2} Hz, max-hold)",
                path, span, bin_hz
            );
        }
    }

    println!("\n=== RADIATED TX CHARACTERISATION COMPLETE ===");
    Ok(())
}
