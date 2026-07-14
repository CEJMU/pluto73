use pluto::device::{GainMode, MAX_AUDIO_SAMPLES, PlutoDevice};
use pluto::dsp::{FilterAudio, FmDecimator};
use crate::test::dsp_helpers::{apply_hamming_window, write_wav_f32_mono};
use num_complex::{Complex, Complex32};
use orion_sdr::core::Block;
use orion_sdr::demodulate::FmQuadratureDemod;
use rustfft::FftPlanner;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Reference-free RX quality measurement on a LIVE, over-the-air FM broadcast station (no TX, no
/// loopback). Captures the FM composite baseband and exploits its known structure - a constant
/// 19 kHz stereo pilot and an empty 60-100 kHz band - to produce program-independent link-quality
/// numbers (pilot SNR, ultrasonic noise floor, audio SNR, stereo present, RSSI/gain). Intended to
/// quantify e.g. an RFI before/after figure (Pluto next to the PC vs. moved away).
pub fn run_fm_broadcast_quality(
    station_hz: i64,
    duration_s: f32,
    out_prefix: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== LIVE FM STATION QUALITY TEST ===");
    println!(
        "station {:.3} MHz, {:.1} s capture, AGC-fast (real antenna signal)\n",
        station_hz as f64 / 1e6,
        duration_s
    );

    let fs_hz: i64 = 3_840_000;
    let cic_decimation: u32 = 4;
    let antenna: u8 = 0;
    let dma_fs = (fs_hz / cic_decimation as i64 / 4) as f32; // 240 kHz FM composite rate

    // Off-tune the LO below the station and use the FPGA DDS to bring the station back to baseband
    // DC. This parks the LO/DC leakage spike at -offset (well outside the +/-120 kHz multiplex) so it
    // can't bias the low-frequency audio or the pilot/noise bands. Mirrors the live app, which tunes
    // LO = playback - fs/4 and DDS-shifts by -fs/4 (see audio::update_audio_tuning).
    let offset_hz = fs_hz / 4; // 960 kHz
    let lo_hz = station_hz - offset_hz;

    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));

    let mut rx = pluto.rx;
    let mut system = pluto.system;

    system.rx_apply_dsp_config(antenna, fs_hz);
    system.reset_audio_dma_controller();
    // Station sits at LO+offset; shift it down to DC.
    system.rx_set_dds(-(offset_hz as f64), (fs_hz * 2) as f64);

    rx.set_antenna(antenna)?;
    rx.set_frequencies(lo_hz, fs_hz)?;
    rx.set_rf_bandwidth(fs_hz)?;
    // AGC (NOT Manual): a real over-the-air signal is weak; a leftover manual gain would bury it.
    rx.set_gain(GainMode::AgcFast, None)?;

    println!(
        "LO {:.3} MHz + DDS {:+.0} kHz -> station at DC; composite rate {} kHz",
        lo_hz as f64 / 1e6,
        -(offset_hz as f64) / 1e3,
        dma_fs as i64 / 1000
    );

    // Let the AGC settle on the real signal before capturing.
    thread::sleep(Duration::from_millis(500));

    // Capture N seconds of raw complex IQ from the audio DMA (240 kHz)
    let system = Arc::new(Mutex::new(system));
    let (i_data, q_data) = capture_audio_dma_iq(&system, duration_s)?;
    println!(
        "Captured {} IQ samples ({:.2} s at {} kHz)",
        i_data.len(),
        i_data.len() as f32 / dma_fs,
        dma_fs as i64 / 1000
    );
    if i_data.len() < 240_000 {
        return Err("too few samples captured - no DMA data?".into());
    }

    // Read the AGC-applied gain / RSSI after the AGC has settled on the captured signal.
    let (gain_db, rssi_db) = rx.rx_signal_strength().unwrap_or((0.0, 0.0));

    // Front-end selectivity: same 120 kHz complex LPF the live FM path applies before demod
    // (dsp::FilterAudio, decimation = 1). Rejects adjacent channels so they don't fold into the
    // discriminator. FilterAudio consumes the raw i16 DMA samples and scales them to ~[-1, 1].
    let mut filter = FilterAudio::new(1, dma_fs as i64, 120_000.0);
    let iq: Vec<Complex32> = filter.execute(&i_data, &q_data);

    // FM-demodulate the composite (keep at 240 kHz; the pilot/noise bands live above 15 kHz)
    // Params match the live app's FM demod (audio_bw 100 kHz -> post-LP at 90 kHz).
    let mut demod = FmQuadratureDemod::new(dma_fs, 75_000.0, 100_000.0);
    let mut composite = vec![0.0f32; iq.len()];
    demod.process(&iq, &mut composite);

    // Analyze the composite spectrum
    let m = analyze_composite(&composite, dma_fs);

    println!(
        "\nstation {:.3} MHz, {:.1} s, AGC-fast, RX gain {:.1} dB (RSSI {:.1} dB)",
        station_hz as f64 / 1e6,
        duration_s,
        gain_db,
        rssi_db
    );

    println!("\n========================================");
    if m.pilot_snr_db > 20.0 && m.ultrasonic_db < -30.0 && m.audio_snr_db > 15.0 {
        println!("TEST RESULT: PASS");
    } else {
        println!("TEST RESULT: FAIL");
    }
    println!("========================================");
    println!(
        "- Pilot SNR: {:.1} dB (Threshold: >20.0 dB)",
        m.pilot_snr_db
    );
    println!(
        "- Ultrasonic Noise: {:.1} dB (Threshold: < -30.0 dB)",
        m.ultrasonic_db
    );
    println!(
        "- Audio SNR: {:.1} dB (Threshold: > 15.0 dB)",
        m.audio_snr_db
    );
    println!("- Stereo Detected: {}", if m.stereo { "Yes" } else { "No" });
    println!("========================================\n");

    // --- Optional WAV outputs ---
    if let Some(prefix) = out_prefix {
        // Listening WAV: replicate the live FM audio path (AudioProcessor::FM in dsp.rs)
        let audio = fm_composite_to_audio(&composite);
        let audio_path = format!("{}_audio.wav", prefix);
        write_wav_f32_mono(&audio_path, &audio, 48_000, true)?;
        println!(
            "\nwrote {} ({} samples, 48 kHz mono)",
            audio_path,
            audio.len()
        );

        // Raw 240 kHz composite for offline plotting
        let comp_path = format!("{}_composite.wav", prefix);
        write_wav_f32_mono(&comp_path, &composite, dma_fs as u32, false)?;
        println!(
            "wrote {} ({} samples, {} kHz mono composite)",
            comp_path,
            composite.len(),
            dma_fs as i64 / 1000
        );
    }

    println!("\n=== FM QUALITY TEST COMPLETE ===");
    Ok(())
}

/// Captures `duration_s` of complex IQ from the FPGA audio DMA path (240 kHz), returning the raw
/// i16 I/Q streams.
fn capture_audio_dma_iq(
    system: &Arc<Mutex<pluto::device::PlutoSystem>>,
    duration_s: f32,
) -> Result<(Vec<i16>, Vec<i16>), Box<dyn std::error::Error>> {
    let mut uio_file = {
        let sys = system.lock().unwrap();
        sys.clone_uio_file()?
    };

    let cap = (duration_s * 240_000.0) as usize + MAX_AUDIO_SAMPLES;
    let mut all_i: Vec<i16> = Vec::with_capacity(cap);
    let mut all_q: Vec<i16> = Vec::with_capacity(cap);
    let mut i_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);
    let mut q_ch: Vec<i16> = Vec::with_capacity(MAX_AUDIO_SAMPLES);

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs_f32(duration_s) {
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
        if unsafe { libc::poll(fds.as_mut_ptr(), 1, 200) } <= 0 {
            continue;
        }
        let mut int_info = [0u8; 4];
        if uio_file.read_exact(&mut int_info).is_err() {
            continue;
        }
        let n = {
            let mut sys = system.lock().unwrap();
            sys.read_audio_dma_samples(&mut i_ch, &mut q_ch)
                .unwrap_or(0)
        };
        if n == 0 {
            thread::sleep(Duration::from_micros(100));
            continue;
        }
        all_i.extend_from_slice(&i_ch);
        all_q.extend_from_slice(&q_ch);
        i_ch.clear();
        q_ch.clear();
    }
    Ok((all_i, all_q))
}

struct Metrics {
    pilot_snr_db: f32,
    ultrasonic_db: f32,
    audio_snr_db: f32,
    stereo: bool,
}

/// Spectral analysis of the real 240 kHz composite using a Hamming-windowed FFT. All metrics are program-content
/// independent except the raw audio-band level (reported as secondary).
fn analyze_composite(composite: &[f32], fs: f32) -> Metrics {
    let n = 16384usize;
    if composite.len() < n {
        return Metrics {
            pilot_snr_db: 0.0,
            ultrasonic_db: -99.0,
            audio_snr_db: 0.0,
            stereo: false,
        };
    }

    let mut buf: Vec<Complex<f32>> = composite[..n]
        .iter()
        .map(|&x| Complex::new(x, 0.0))
        .collect();
    apply_hamming_window(&mut buf);

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    fft.process(&mut buf);

    let bin_bw = fs / n as f32; // Hz per bin
    let f_to_bin = |f: f32| ((f / bin_bw).round() as usize).min(n / 2);
    let mags: Vec<f32> = buf[..=n / 2].iter().map(|c| c.norm() / n as f32).collect();

    let (plo, phi) = (f_to_bin(18850.0), f_to_bin(19150.0));
    let pilot_peak = (plo..=phi).map(|k| mags[k]).fold(0.0f32, f32::max);
    let pilot_noise = (f_to_bin(16000.0)..=f_to_bin(18000.0))
        .map(|k| mags[k])
        .sum::<f32>()
        / (f_to_bin(18000.0) - f_to_bin(16000.0) + 1).max(1) as f32;
    let pilot_snr_db = if pilot_noise > 1e-6 {
        20.0 * (pilot_peak / pilot_noise).log10()
    } else {
        0.0
    };
    let stereo = pilot_snr_db > 20.0;

    let noise_bin_start = f_to_bin(60000.0);
    let noise_bin_end = f_to_bin(85000.0);
    let avg_noise = (noise_bin_start..=noise_bin_end)
        .map(|k| mags[k])
        .sum::<f32>()
        / (noise_bin_end - noise_bin_start + 1).max(1) as f32;
    let ultrasonic_db = 20.0 * avg_noise.max(1e-6).log10();

    let audio_bin_start = f_to_bin(300.0);
    let audio_bin_end = f_to_bin(15000.0);
    let avg_audio = (audio_bin_start..=audio_bin_end)
        .map(|k| mags[k])
        .sum::<f32>()
        / (audio_bin_end - audio_bin_start + 1).max(1) as f32;
    let audio_snr_db = if avg_noise > 1e-6 {
        20.0 * (avg_audio / avg_noise).log10()
    } else {
        0.0
    };

    Metrics {
        pilot_snr_db,
        ultrasonic_db,
        audio_snr_db,
        stereo,
    }
}

/// Replicates the live FM listening path (dsp::AudioProcessor::FM)
fn fm_composite_to_audio(composite: &[f32]) -> Vec<f32> {
    let mut decimator = FmDecimator::new(240_000.0, 5, 15_000.0);
    let mut out = Vec::with_capacity(composite.len() / 5 + 1);
    let mut deemph = 0.0f32;
    let mut dc_x = 0.0f32;
    let mut dc_y = 0.0f32;
    for &s in composite {
        if let Some(decimated) = decimator.process(s) {
            deemph = deemph * 0.7575 + decimated * (1.0 - 0.7575);
            let base = deemph * 4000.0;
            let blocked = base - dc_x + 0.995 * dc_y;
            dc_x = base;
            dc_y = blocked;
            out.push(blocked);
        }
    }
    out
}
