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
    let mut buf: Vec<Complex<f32>> = i_samples[..n]
        .iter()
        .zip(&q_samples[..n])
        .map(|(&i, &q)| Complex::new(i as f32, q as f32))
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
    // Skip the silent lead-in: start where the envelope first exceeds 30% of peak.
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
    if n < 1024 {
        return (0.0, 0.0);
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut buf: Vec<Complex<f32>> = body[..n].iter().map(|&x| Complex::new(x, 0.0)).collect();
    apply_hamming_window(&mut buf);
    fft.process(&mut buf);
    let half = n / 2;
    let mags: Vec<f32> = buf[..half].iter().map(|c| c.norm()).collect();
    let lf = ((100.0 * n as f32 / fs).ceil() as usize).max(2);
    let (mut pb, mut pm) = (lf, 0.0f32);
    for b in lf..half {
        if mags[b] > pm {
            pm = mags[b];
            pb = b;
        }
    }
    let mut spur = 0.0f32;
    for b in lf..half {
        if (b as isize - pb as isize).abs() > 5 && mags[b] > spur {
            spur = mags[b];
        }
    }
    let snr = 20.0 * (pm / spur.max(1e-9)).log10();
    (pb as f32 * fs / n as f32, snr)
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
