use std::fs::File;
use std::io::Write;

use industrial_io as iio;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

pub use pluto::AUDIO_SAMPLE_RATE;
pub use pluto::dsp::filter_design::hamming_window;

/// Sets the AD9361 BIST loopback mode via industrial_io attributes (1 = digital TX->RX inside the
/// AD9361, bypassing DAC/RF/LO/ADC; 0 = off). Falls back to the debugfs path when the debug
/// attributes are disabled in the running firmware.
pub fn set_ad9361_loopback(mode: u8) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(ctx) = iio::Context::new() {
        if let Some(dev) = ctx.find_device("ad9361-phy") {
            if dev.attr_write("loopback", mode as i64).is_ok() {
                return Ok(());
            }
        }
    }

    // Fallback to debugfs path if industrial_io debug attributes are disabled
    for entry in std::fs::read_dir("/sys/bus/iio/devices")? {
        let entry = entry?;
        if let Ok(name) = std::fs::read_to_string(entry.path().join("name")) {
            if name.trim() == "ad9361-phy" {
                let dbg = format!(
                    "/sys/kernel/debug/iio/{}/loopback",
                    entry.file_name().to_string_lossy()
                );
                std::fs::write(&dbg, format!("{}\n", mode))?;
                return Ok(());
            }
        }
    }
    Err("ad9361-phy device not found".into())
}

/// Runs `body` with the AD9361 in BIST digital loopback, restoring loopback = 0 afterward
/// Prints `skip_label` and skips the body when the loopback mode cannot be set (e.g. debug attributes disabled in the running firmware).
/// Use this for A/B cases where the loopback capture is the point and there is no meaningful RF fallback.
pub fn with_ad9361_loopback<F>(skip_label: &str, body: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce() -> Result<(), Box<dyn std::error::Error>>,
{
    match set_ad9361_loopback(1) {
        Ok(()) => {
            let result = body();
            let _ = set_ad9361_loopback(0);
            result
        }
        Err(e) => {
            println!("{}: {}", skip_label, e);
            Ok(())
        }
    }
}

/// RAII guard that holds the AD9361 in BIST digital loopback for the duration of a scope and
/// restores loopback = 0 on drop (covering `?` early-returns and panics). Use this for a whole-test
/// `--loopback` toggle: unlike `with_ad9361_loopback`, when the mode is unavailable it prints
/// `skip_label` and leaves the guard inactive so captures fall back to the normal RF path.
pub struct LoopbackGuard {
    active: bool,
}

impl LoopbackGuard {
    /// Enables loopback; on failure prints `skip_label` and yields an inactive (no-op) guard.
    pub fn enable(skip_label: &str) -> Self {
        match set_ad9361_loopback(1) {
            Ok(()) => {
                println!("AD9361 BIST digital loopback ENABLED (TX->RX, RF path bypassed)");
                LoopbackGuard { active: true }
            }
            Err(e) => {
                println!("{}: {}", skip_label, e);
                LoopbackGuard { active: false }
            }
        }
    }
}

impl Drop for LoopbackGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = set_ad9361_loopback(0);
        }
    }
}

/// Reads a WAV file as mono f32 samples.
/// Requires the WAV file to have a sample rate of exactly 48000 Hz.
pub fn read_wav_as_f32_mono(path: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    println!(
        "  WAV: {:?} {}ch {}Hz {}bit",
        spec.sample_format, spec.channels, spec.sample_rate, spec.bits_per_sample
    );

    if spec.sample_rate != AUDIO_SAMPLE_RATE {
        return Err(format!(
            "WAV file must have a sample rate of exactly 48000 Hz (found {} Hz)",
            spec.sample_rate
        )
        .into());
    }

    let channels = spec.channels as usize;
    let mut samples = Vec::new();

    match spec.sample_format {
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            let raw: Vec<i32> = reader.samples::<i32>().collect::<Result<_, _>>()?;
            for frame in raw.chunks(channels) {
                let mono = frame.iter().map(|&s| s as f32 * scale).sum::<f32>() / channels as f32;
                samples.push(mono);
            }
        }
        hound::SampleFormat::Float => {
            let raw: Vec<f32> = reader.samples::<f32>().collect::<Result<_, _>>()?;
            for frame in raw.chunks(channels) {
                let mono = frame.iter().sum::<f32>() / channels as f32;
                samples.push(mono);
            }
        }
    }

    Ok(samples)
}

/// Computes magnitudes at specific target frequencies using a single FFT for complex i16 samples.
pub fn fft_mags_i16(i_samples: &[i16], q_samples: &[i16], freqs: &[f64], fs: f64) -> Vec<f64> {
    let n = i_samples.len().min(q_samples.len());
    if n == 0 {
        return vec![0.0; freqs.len()];
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    // Hann-window before transforming.
    let mut buf: Vec<Complex<f32>> = i_samples[..n]
        .iter()
        .zip(&q_samples[..n])
        .enumerate()
        .map(|(k, (&i, &q))| {
            let w = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * k as f64 / (n as f64 - 1.0)).cos());
            Complex::new(i as f32 * w as f32, q as f32 * w as f32)
        })
        .collect();
    fft.process(&mut buf);

    let bin_hz = fs / n as f64;
    freqs
        .iter()
        .map(|&f| {
            let bin = (f / bin_hz).round() as isize;
            let idx = bin.rem_euclid(n as isize) as usize;
            (buf[idx].norm() / n as f32) as f64
        })
        .collect()
}

/// Computes magnitudes at specific target frequencies using a single FFT for real f32 samples.
pub fn fft_mags_f32(samples: &[f32], freqs: &[f64], fs: f64) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return vec![0.0; freqs.len()];
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut buf: Vec<Complex<f32>> = samples.iter().map(|&s| Complex::new(s, 0.0)).collect();
    fft.process(&mut buf);

    let bin_hz = fs / n as f64;
    freqs
        .iter()
        .map(|&f| {
            let bin = (f / bin_hz).round() as isize;
            let idx = bin.rem_euclid(n as isize) as usize;
            (buf[idx].norm() / n as f32) as f64
        })
        .collect()
}

/// Applies a Hamming window to a complex slice in-place.
pub fn apply_hamming_window(buf: &mut [Complex<f32>]) {
    let window = hamming_window(buf.len());
    for (sample, w) in buf.iter_mut().zip(window) {
        *sample *= w;
    }
}

/// Dominant tone frequency (Hz) over the active (non-silent) region + its peak-to-largest-spur ratio.
pub fn dominant_tone(audio: &[f32], fs: f32) -> (f32, f32) {
    if audio.len() < 8192 {
        return (0.0, 0.0);
    }

    // Skip the silent lead-in:
    // Compute a coarse RMS envelope in 20 ms windows and start where it first exceeds 30 % of the peak
    let win = (fs * 0.02) as usize;
    let env: Vec<f32> = audio
        .chunks(win)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let ep = env.iter().cloned().fold(0.0f32, f32::max);
    let a = env.iter().position(|&v| v > ep * 0.3).unwrap_or(0) * win;
    let body = &audio[a.min(audio.len())..];

    // Choose power-of-two FFT length
    let mut n = 1usize;
    while n * 2 <= body.len() && n < 32768 {
        n *= 2;
    }
    if n < 1024 {
        return (0.0, 0.0);
    }

    // Windowed FFT
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut buf: Vec<Complex<f32>> = body[..n].iter().map(|&x| Complex::new(x, 0.0)).collect();
    apply_hamming_window(&mut buf);
    fft.process(&mut buf);

    // Find dominant bin above 100 Hz
    let half = n / 2;
    let mags: Vec<f32> = buf[..half].iter().map(|c| c.norm()).collect();
    let lf = ((100.0 * n as f32 / fs).ceil() as usize).max(2); // low-frequency cutoff bin

    let (mut pb, mut pm) = (lf, 0.0f32);
    for b in lf..half {
        if mags[b] > pm {
            pm = mags[b];
            pb = b;
        }
    }

    // Measure the largest spur >= 5 bins from the peak
    let mut spur = 0.0f32;
    for b in lf..half {
        if (b as isize - pb as isize).abs() > 5 && mags[b] > spur {
            spur = mags[b];
        }
    }

    let snr = 20.0 * (pm / spur.max(1e-9)).log10();
    (pb as f32 * fs / n as f32, snr)
}

/// The peak-to-spur figure of `dominant_tone` together with the frequencies of the strongest
/// spurs behind it.
///
/// Returns `(peak_hz, snr_db, [(spur_hz, level_dbc), ...])`, strongest spur first, with spurs separated by at least ten bins so one line is not reported repeatedly.
/// `exclude_hz` sets how far either side of the tone is ignored.
pub fn dominant_tone_spurs(
    audio: &[f32],
    fs: f32,
    want: usize,
    exclude_hz: f32,
) -> (f32, f32, Vec<(f32, f32)>) {
    let (peak_hz, snr) = dominant_tone(audio, fs);
    if peak_hz == 0.0 {
        return (0.0, 0.0, Vec::new());
    }

    // Reproduce dominant_tone's framing:
    // The bins must line up with the SNR figure being explained, so the same envelope gating and power-of-two selection is repeated here.
    let win = (fs * 0.02) as usize;
    let env: Vec<f32> = audio
        .chunks(win)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let ep = env.iter().cloned().fold(0.0f32, f32::max);
    let a = env.iter().position(|&v| v > ep * 0.3).unwrap_or(0) * win;
    let body = &audio[a.min(audio.len())..];

    let mut n = 1usize;
    while n * 2 <= body.len() && n < 32768 {
        n *= 2;
    }

    // Windowed FFT
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut buf: Vec<Complex<f32>> = body[..n].iter().map(|&x| Complex::new(x, 0.0)).collect();
    apply_hamming_window(&mut buf);
    fft.process(&mut buf);

    let half = n / 2;
    let mags: Vec<f32> = buf[..half].iter().map(|c| c.norm()).collect();
    let lf = ((100.0 * n as f32 / fs).ceil() as usize).max(2); // low-frequency cutoff bin

    // Peak bin and magnitude (looked up from the frequency returned by dominant_tone).
    let pb = (peak_hz * n as f32 / fs).round() as usize;
    let pm = mags[pb.min(half - 1)];

    // Collect spur candidates outside the exclusion zone:
    // `exclude_hz` converts to at least 5 bins so the main-lobe skirt of the Hamming window is not counted as a spur.
    let excl = ((exclude_hz * n as f32 / fs).round() as isize).max(5);

    let mut cand: Vec<(usize, f32)> = (lf..half)
        .filter(|&b| (b as isize - pb as isize).abs() > excl)
        .map(|b| (b, mags[b]))
        .collect();
    cand.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap());

    //  Pick the strongest spurs >= 10 bins apart
    let mut out: Vec<(f32, f32)> = Vec::new();
    for (b, m) in cand {
        if out.len() >= want {
            break;
        }
        let f = b as f32 * fs / n as f32;
        if out
            .iter()
            .any(|&(g, _)| (g - f).abs() < 10.0 * fs / n as f32)
        {
            continue;
        }
        out.push((f, 20.0 * (m / pm.max(1e-9)).log10()));
    }

    (peak_hz, snr, out)
}

/// Writes mono f32 samples to a 16-bit WAV file. If `normalize`, scales the peak to 0.9.
pub fn write_wav_f32_mono(
    path: &str,
    samples: &[f32],
    sample_rate: u32,
    normalize: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let gain = if normalize {
        let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        if peak > 1e-6 { 0.9 / peak } else { 1.0 }
    } else {
        1.0
    };
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        let v = (s * gain).clamp(-1.0, 1.0);
        writer.write_sample((v * 32767.0) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Writes stereo i16 samples to a 16-bit WAV file.
pub fn write_wav_i16_stereo(
    path: &str,
    i_samples: &[i16],
    q_samples: &[i16],
    sample_rate: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let len = i_samples.len().min(q_samples.len());
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for i in 0..len {
        writer.write_sample(i_samples[i])?;
        writer.write_sample(q_samples[i])?;
    }
    writer.finalize()?;
    Ok(())
}

/// Exports a 16,384-point FFT spectrum curve to CSV for complex i16 samples (e.g. DMA captures).
pub fn export_16k_ssb_spectrum_csv(
    i_samples: &[i16],
    q_samples: &[i16],
    fs_hz: f64,
    carrier_freq_hz: f64,
    output_csv: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Transform the WHOLE capture, exactly as fft_mags_i16 does for the printed figures
    let n = i_samples.len().min(q_samples.len());
    if n < 1024 {
        return Err("Insufficient samples for spectrum export".into());
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);

    let mut buf: Vec<Complex<f32>> = i_samples[..n]
        .iter()
        .zip(&q_samples[..n])
        .enumerate()
        .map(|(k, (&i, &q))| {
            let w = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * k as f64 / (n as f64 - 1.0)).cos());
            Complex::new(i as f32 * w as f32, q as f32 * w as f32)
        })
        .collect();

    fft.process(&mut buf);

    // Normalise magnitudes and extract the +/-5 kHz window
    let bin_hz = fs_hz / n as f64;
    let mut max_mag = 1e-9f32;
    for c in &buf {
        if c.norm() > max_mag {
            max_mag = c.norm();
        }
    }

    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(n / 8);
    for k in 0..n {
        let freq = if k < n / 2 {
            k as f64 * bin_hz
        } else {
            (k as f64 - n as f64) * bin_hz
        };

        let rel_freq_khz = ((freq - carrier_freq_hz) / 1000.0) as f32;
        if !(-5.0..=5.0).contains(&rel_freq_khz) {
            continue;
        }

        let mag = buf[k].norm();
        pts.push((rel_freq_khz, 20.0 * (mag / max_mag).max(1e-6).log10()));
    }
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    // Max-hold decimation for the written file. A full-length transform over +/-5 kHz is tens of
    // thousands of points, more than a plot needs; taking the maximum of each group rather than
    // every n-th sample keeps every peak at its true height instead of thinning narrow lines.
    const MAX_ROWS: usize = 4000;
    let group = (pts.len() / MAX_ROWS).max(1);
    let decimated: Vec<(f32, f32)> = pts
        .chunks(group)
        .map(|c| {
            let f = c[c.len() / 2].0;
            let m = c.iter().fold(f32::NEG_INFINITY, |a, &(_, d)| a.max(d));
            (f, m)
        })
        .collect();

    let mut file = File::create(output_csv)?;
    writeln!(file, "freq_khz,power_dbc")?;
    for (freq_khz, pwr) in &decimated {
        writeln!(file, "{:.4},{:.2}", freq_khz, pwr)?;
    }

    println!(
        "Exported spectrum to {} ({} rows, RBW {:.3} Hz, {} bins max-held per row)",
        output_csv,
        decimated.len(),
        bin_hz,
        group
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Run provenance and measurement-resolution reporting
// ---------------------------------------------------------------------------

pub fn capture_tag(loopback: bool) -> &'static str {
    if loopback { "bist" } else { "rf" }
}

/// Prints the measurement configuration at the head of a diagnostic run.
pub fn print_run_config(
    lo_hz: i64,
    fs_hz: i64,
    tx_dds_offset_hz: f64,
    rx_gain_db: f64,
    loopback: bool,
) {
    println!("--- Run configuration ---");
    println!("  TX/RX LO      : {:.3} MHz", lo_hz as f64 / 1e6);
    println!("  Sample rate   : {:.3} MHz", fs_hz as f64 / 1e6);
    println!("  TX DDS offset : {:+.3} kHz", tx_dds_offset_hz / 1e3);
    println!("  RX gain       : {:.1} dB (manual)", rx_gain_db);
    println!(
        "  Signal path   : {}",
        if loopback {
            "AD9361 BIST digital loopback (DAC/analog/ADC bypassed)"
        } else {
            "RF cable loopback"
        }
    );
    println!("  Build         : pluto v{}", env!("CARGO_PKG_VERSION"));
    println!();
}

/// States whether probes `separation_hz` from a full-scale line are resolved at `bin_hz`, and,
/// when they are not, says so explicitly and names the instrument that can resolve them.
///
/// A burst FFT over 16384 samples at 3.84 MHz gives 234 Hz bins, so an in-channel product a few
/// hundred hertz from the wanted tone sits 2-5 bins away and reads 30 dB above its true level.
pub fn print_resolution_verdict(separation_hz: f32, bin_hz: f32, what: &str) {
    let bins = separation_hz / bin_hz;
    if bins >= 50.0 {
        println!(
            "  Resolution: {:.1} bins at {:.1} Hz/bin - {} resolved.\n",
            bins, bin_hz, what
        );
    } else {
        println!(
            "Resolution: {:.1} bins at {:.1} Hz/bin - {} NOT resolved.\n  \
             The values below are the analysis window's leakage floor, i.e. UPPER BOUNDS, not measurements. \
             Use --test-dma-carrier-offset (multi-second capture, sub-Hz bins, -100 dBc floor) for the true in-channel figures.\n",
            bins, bin_hz, what
        );
    }
}

// ---------------------------------------------------------------------------
// Envelope comparison for speech loopback
// ---------------------------------------------------------------------------

/// Zero-phase band-limit by zeroing FFT bins outside `[lo_hz, hi_hz]`.
///
/// The transmit chain deliberately discards everything outside its 300-3000 Hz passband, so a
/// reference that still contains that energy would be compared against a signal which never
/// carried it, and the resulting correlation would measure the passband rather than the transport.
fn band_limit(x: &[f32], fs: f32, lo_hz: f32, hi_hz: f32) -> Vec<f32> {
    // Zero-pad to the next power of two
    let mut n = 1usize;
    while n < x.len() {
        n *= 2;
    }

    let mut buf: Vec<Complex<f32>> = x
        .iter()
        .map(|&v| Complex::new(v, 0.0))
        .chain(std::iter::repeat(Complex::new(0.0, 0.0)))
        .take(n)
        .collect();

    // FFT
    let mut planner = FftPlanner::<f32>::new();
    planner.plan_fft_forward(n).process(&mut buf);

    // Zero out bins outside [lo_hz, hi_hz]
    // Both the positive- and negative-frequency halves of the spectrum
    // are handled: for k > N/2 the physical frequency is (N - k) * bin.
    let bin = fs / n as f32;
    for (k, c) in buf.iter_mut().enumerate() {
        let f = if k <= n / 2 {
            k as f32 * bin
        } else {
            (n - k) as f32 * bin
        };
        if f < lo_hz || f > hi_hz {
            *c = Complex::new(0.0, 0.0);
        }
    }

    // Inverse FFT and normalise
    planner.plan_fft_inverse(n).process(&mut buf);
    let s = 1.0 / n as f32;
    buf[..x.len()].iter().map(|c| c.re * s).collect()
}

/// RMS envelope in `win_ms` blocks - the coarse amplitude contour a listener perceives.
fn envelope(x: &[f32], fs: f32, win_ms: f32) -> Vec<f32> {
    let w = ((fs * win_ms / 1000.0) as usize).max(1);
    x.chunks(w)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect()
}

/// Pearson correlation of two envelopes at their best alignment, returning `(r, lag_frames)`.
/// The loopback path delays the audio by its own prefill and filter latency, so comparing without searching the lag would report that delay as if it were distortion.
fn best_corr(a: &[f32], b: &[f32], max_lag: usize) -> (f32, isize) {
    // Pearson r for a single alignment
    let pearson = |p: &[f32], q: &[f32]| -> f32 {
        let n = p.len().min(q.len());
        if n < 4 {
            return 0.0;
        }

        // Mean-centre both series.
        let (mp, mq) = (
            p[..n].iter().sum::<f32>() / n as f32,
            q[..n].iter().sum::<f32>() / n as f32,
        );

        // Accumulate numerator and per-series energy.
        let (mut num, mut dp, mut dq) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let (u, v) = (p[i] - mp, q[i] - mq);
            num += u * v;
            dp += u * u;
            dq += v * v;
        }

        if dp <= 0.0 || dq <= 0.0 {
            return 0.0;
        }
        num / (dp.sqrt() * dq.sqrt())
    };

    // Sweep over candidate lags and keep the best
    let (mut best, mut best_lag) = (-1.0f32, 0isize);
    for lag in 0..=max_lag.min(b.len().saturating_sub(4)) {
        let r = pearson(a, &b[lag..]);
        if r > best {
            best = r;
            best_lag = lag as isize;
        }
    }

    (best, best_lag)
}

/// Compares the amplitude contour of transmitted and received audio and prints the result.
///
/// Reports the correlation over the transmit passband as a whole and split at 800 Hz
pub fn report_envelope_correlation(reference: &[f32], received: &[f32], fs: u32) {
    let fsf = fs as f32;
    if reference.len() < fs as usize || received.len() < fs as usize {
        println!("envelope correlation: skipped (need at least one second of audio)");
        return;
    }
    let max_lag = envelope(received, fsf, 50.0).len().min(60); // up to 3 s of search
    println!("\n--- Envelope correlation (50 ms RMS windows, best alignment) ---");
    println!("  reference band-limited to the 300-3000 Hz transmit passband before comparison");
    for (lo, hi, label) in [
        (300.0f32, 3000.0f32, "full passband"),
        (300.0, 800.0, "below 800 Hz"),
        (800.0, 3000.0, "above 800 Hz"),
    ] {
        let a = envelope(&band_limit(reference, fsf, lo, hi), fsf, 50.0);
        let b = envelope(&band_limit(received, fsf, lo, hi), fsf, 50.0);
        let (r, lag) = best_corr(&a, &b, max_lag);
        println!(
            "  {:<14} r = {:.3}   (best lag {:.2} s)",
            label,
            r,
            lag as f32 * 0.05
        );
    }
}

/// Characterises a recovered single tone: its frequency, its harmonic distortion, the flatness of
/// its amplitude over the capture, and the worst unrelated line.
pub fn report_tone_quality(audio: &[f32], fs: u32, tone_hz: f32) {
    let fsf = fs as f32;
    if audio.len() < 2 * fs as usize {
        println!("tone quality: skipped (need at least two seconds of audio)");
        return;
    }

    // Isolate the keyed (non-silent) region:
    // The receiver keeps capturing after the transmitter stops, so a fixed trim would leave silence in the window and report it as amplitude variation.
    // Find where the 50 ms RMS envelope is above half the peak, then step a further 0.25 s inward from each end to clear the key-up and key-down ramps.
    let w = (fsf * 0.05) as usize;
    let coarse: Vec<f32> = audio
        .chunks(w)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();

    let cpk = coarse.iter().cloned().fold(0.0f32, f32::max);
    let first = coarse.iter().position(|&v| v > cpk * 0.5).unwrap_or(0);
    let last = coarse
        .iter()
        .rposition(|&v| v > cpk * 0.5)
        .unwrap_or(coarse.len() - 1);

    let margin = (fs as usize / 4) / w; // 0.25 s expressed in envelope frames
    let (a, b) = (
        (first + margin) * w,
        ((last.saturating_sub(margin)) * w).min(audio.len()),
    );
    if b <= a + fs as usize {
        println!("tone quality: skipped (keyed region too short)");
        return;
    }
    let body = &audio[a..b];

    // Spectral analysis: harmonics and spurs
    let (peak_hz, _, spurs) = dominant_tone_spurs(body, fsf, 1, 100.0);

    let mags = fft_mags_f32(
        body,
        &[tone_hz as f64, 2.0 * tone_hz as f64, 3.0 * tone_hz as f64],
        fsf as f64,
    );
    let dbc = |m: f64| 20.0 * (m / mags[0].max(1e-12)).log10();

    // Envelope flatness
    // A constant-amplitude carrier should produce a nearly flat RMS envelope; periodic dropouts, buffer seams, or pacing glitches show up as peaks in the spread.
    let win = (fsf * 0.05) as usize;
    let env: Vec<f32> = body
        .chunks(win)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();

    let mean = env.iter().sum::<f32>() / env.len() as f32;
    let (lo, hi) = env
        .iter()
        .fold((f32::MAX, 0.0f32), |(l, h), &v| (l.min(v), h.max(v)));
    let flat_pct = if mean > 0.0 {
        100.0 * (hi - lo) / mean
    } else {
        0.0
    };

    println!("\n--- Recovered tone quality (steady-state region) ---");
    println!(
        "  frequency        : {:.1} Hz (nominal {:.0} Hz)",
        peak_hz, tone_hz
    );
    println!("  2nd harmonic     : {:>6.1} dBc", dbc(mags[1]));
    println!("  3rd harmonic     : {:>6.1} dBc", dbc(mags[2]));
    if let Some(&(f, d)) = spurs.first() {
        println!("  worst other line : {:>6.1} dBc at {:.0} Hz", d, f);
    }
    println!(
        "  envelope spread  : {:.2} % of mean over {:.1} s (peak-to-trough, 50 ms windows)",
        flat_pct,
        body.len() as f32 / fsf
    );
}
