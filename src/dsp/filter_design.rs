use num_complex::Complex32 as C32;
use std::f32::consts::PI;

pub const SSB_FIR_TAPS: usize = 255;

/// Designs a Hamming-windowed-sinc low-pass FIR: `num_taps` coefficients with normalized cutoff
/// `fc` (cycles/sample, i.e. `cutoff_hz / fs`), scaled to unity DC gain (sum of taps = 1). Shared by the
/// low-pass filters/decimators in this crate (`FmDecimator`, `FilterAudio`, and the `IqResampler`
/// interpolation prototype, which rescales to sum of = L) so the window/normalization live in one place.
pub fn design_lowpass_hamming(num_taps: usize, fc: f32) -> Vec<f32> {
    let mut taps = vec![0.0f32; num_taps];
    let mut sum = 0.0f32;
    for i in 0..num_taps {
        let n = i as f32 - (num_taps - 1) as f32 / 2.0;
        let sinc = if n == 0.0 {
            2.0 * PI * fc
        } else {
            (2.0 * PI * fc * n).sin() / n
        };
        let window = 0.54 - 0.46 * (2.0 * PI * i as f32 / (num_taps - 1) as f32).cos();
        taps[i] = sinc * window;
        sum += taps[i];
    }
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}

/// Designs the shared complex analytic band-pass FIR taps: a windowed-sinc low-pass prototype
/// (Blackman window) frequency-shifted so the filter passes audio `[f_lo, filter_bw]` on one side of
/// DC and rejects the other sideband. `f_lo = 0.0` gives the original low-pass-shifted `[0, filter_bw]`
/// (used by the RX); the TX passes `f_lo = TX_AUDIO_LOW_CUT_HZ` for the low-cut. `usb=true` shifts up
/// (passes positive frequencies), `usb=false` shifts down (LSB). Normalized to unity passband gain.
/// Used by both the TX modulator (`ComplexSsbFir`) and the RX demod (`AnalyticSsbDemod`) so
/// they stay matched duals (the RX's wider [0, bw] cleanly demodulates the TX's [f_lo, bw]).
pub fn ssb_analytic_taps(fs: f32, filter_bw: f32, usb: bool, f_lo: f32) -> Vec<C32> {
    let ntaps = SSB_FIR_TAPS;
    let m = (ntaps - 1) / 2;
    // Prototype low-pass cutoff = half the passband width; shift = passband center. For f_lo = 0
    // this reduces to fc = filter_bw/2, shift = filter_bw/2 (the original behaviour).
    let center = (f_lo + filter_bw) / 2.0;
    let half = (filter_bw - f_lo) / 2.0;
    let fc = half / fs; // prototype LP cutoff (cycles/sample)
    let shift = (center / fs) * if usb { 1.0 } else { -1.0 };
    let pi = std::f32::consts::PI;

    let mut g = vec![0.0f32; ntaps];
    for i in 0..ntaps {
        let k = i as isize - m as isize;
        let sinc = if k == 0 {
            2.0 * fc
        } else {
            (2.0 * pi * fc * k as f32).sin() / (pi * k as f32)
        };
        let a = 2.0 * pi * i as f32 / (ntaps - 1) as f32;
        let w = 0.42 - 0.5 * a.cos() + 0.08 * (2.0 * a).cos();
        g[i] = sinc * w;
    }
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
