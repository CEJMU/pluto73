use num_complex::Complex32 as C32;
use std::f32::consts::PI;

pub const SSB_FIR_TAPS: usize = 255;

/// Generates a Hamming window of length `size`
pub fn hamming_window(size: usize) -> Vec<f32> {
    if size <= 1 {
        return vec![1.0; size];
    }
    let denom = (size - 1) as f32;
    (0..size)
        .map(|i| 0.54 - 0.46 * (2.0 * PI * i as f32 / denom).cos())
        .collect()
}

/// Designs a Hamming-windowed-sinc low-pass FIR: `num_taps` coefficients with normalized cutoff
/// `fc` (cycles/sample, i.e. `cutoff_hz / fs`), scaled to unity DC gain (sum of taps = 1). Shared by the
/// low-pass filters/decimators in this crate (`FmDecimator`, `FilterAudio`, and the `IqResampler`
/// interpolation prototype, which rescales to sum of = L) so the window/normalization live in one place.
pub fn design_lowpass_hamming(num_taps: usize, fc: f32) -> Vec<f32> {
    let window = hamming_window(num_taps);
    let mut taps = vec![0.0f32; num_taps];
    let mut sum = 0.0f32;
    for i in 0..num_taps {
        let n = i as f32 - (num_taps - 1) as f32 / 2.0;
        let sinc = if n == 0.0 {
            2.0 * PI * fc
        } else {
            (2.0 * PI * fc * n).sin() / n
        };
        taps[i] = sinc * window[i];
        sum += taps[i];
    }
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}

/// Designs the shared complex analytic band-pass FIR taps: a windowed-sinc low-pass prototype
/// (Blackman window) frequency-shifted so the filter passes audio `[f_lo, filter_bw]` on one side of
/// DC and rejects the opposite sideband.
/// - `f_lo = 0.0`: Low-pass-shifted `[0, filter_bw]` (used by RX `AnalyticSsbDemod`).
/// - `f_lo = TX_AUDIO_LOW_CUT_HZ`: Low-cut-shifted `[f_lo, filter_bw]` (used by TX `ComplexSsbFir`).
/// - `usb = true`: Shifts up (USB, positive frequencies); `usb = false`: Shifts down (LSB, negative frequencies).
///
/// Output taps are normalized to unity passband gain (sum of prototype taps = 1).
pub fn ssb_analytic_taps(fs: f32, filter_bw: f32, usb: bool, f_lo: f32) -> Vec<C32> {
    let ntaps = SSB_FIR_TAPS;
    let m = (ntaps - 1) / 2;
    let pi = std::f32::consts::PI;

    // Step 1: Compute prototype low-pass cutoff and complex frequency modulation shift
    let center = (f_lo + filter_bw) / 2.0;
    let half = (filter_bw - f_lo) / 2.0;
    let fc = half / fs; // Prototype low-pass cutoff (cycles per sample)
    let shift = (center / fs) * if usb { 1.0 } else { -1.0 }; // Normalized frequency shift

    // Step 2: Generate real-valued low-pass prototype g[k] using a Blackman window
    let mut g = vec![0.0f32; ntaps];
    for i in 0..ntaps {
        let k = i as isize - m as isize;
        let sinc = if k == 0 {
            2.0 * fc
        } else {
            (2.0 * pi * fc * k as f32).sin() / (pi * k as f32)
        };
        // Blackman window: w(n) = 0.42 - 0.5 * cos(2pi*n/(N-1)) + 0.08 * cos(4pi*n/(N-1))
        let a = 2.0 * pi * i as f32 / (ntaps - 1) as f32;
        let w = 0.42 - 0.5 * a.cos() + 0.08 * (2.0 * a).cos();
        g[i] = sinc * w;
    }

    // Step 3: Normalize prototype gain to unity and apply complex frequency shift e^(j * 2pi * shift * k)
    let sum: f32 = g.iter().sum();
    (0..ntaps)
        .map(|i| {
            let k = i as isize - m as isize;
            let ph = 2.0 * pi * shift * k as f32;
            let gain = g[i] / sum;
            C32::new(gain * ph.cos(), gain * ph.sin())
        })
        .collect()
}
