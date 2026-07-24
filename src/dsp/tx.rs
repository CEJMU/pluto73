use crate::dsp::filter_design::{design_lowpass_hamming, ssb_analytic_taps};
use log::debug;
use num_complex::Complex32 as C32;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::SyncSender,
};
use std::thread;
use std::time::Duration;

/// Audio sample rate the SSB modulator operates at (and the rate the network delivers audio).
const MOD_FS: u32 = crate::AUDIO_SAMPLE_RATE;

/// TX audio low-cut (Hz). The TX analytic FIR passband starts here instead of at DC, so sub-300 Hz
/// content (mic rumble, handling noise, music bass) can't pile onto the suppressed carrier or leak
/// onto the opposite sideband - standard SSB practice
pub const TX_AUDIO_LOW_CUT_HZ: f32 = 300.0;

// -------------------------------------------------------------------------
// Rate Calculation and Resampling
// -------------------------------------------------------------------------

/// The rate the TX DMA must be fed at, given the AD9361/FPGA sample rate `tx_fs`.
///
/// The FPGA interpolates the DMA stream by FIR(4) x CIC, with CIC capped at 64, so the most
/// it can do is x256. `tx_apply_dsp_config` clamps total interpolation to 256 to match. The
/// FPGA therefore drains the DMA at `tx_fs / (4 * cic)` = `max(48000, tx_fs/256)`:
///   3.84/7.68 MHz -> 48 kHz, 15.36 MHz -> 60 kHz, 30.72 MHz -> 120 kHz.
pub fn tx_dma_audio_fs(tx_fs: f32) -> u32 {
    let total = ((tx_fs / MOD_FS as f32).round() as i64).clamp(16, 256);
    let cic = (total / 4).max(1);
    (tx_fs / (4 * cic) as f32).round() as u32
}

/// Streaming L/M polyphase resampler for complex i16 IQ, used to upsample the modulated
/// 48 kHz SSB IQ to the DMA feed rate the FPGA expects at wide spans.
pub struct IqResampler {
    l: usize,
    m: usize,
    taps_per_phase: usize,
    poly: Vec<Vec<f32>>, // poly[phase][k] = prototype[phase + k*L]
    // Double-length history rings (newest last): each input is written at `hist_idx` and `hist_idx + taps_per_phase`,
    // so the last taps_per_phase inputs are always one contiguous chronological slice starting at `hist_idx`.
    hist_i: Vec<f32>,
    hist_q: Vec<f32>,
    hist_idx: usize,
    phase: usize,
}

impl IqResampler {
    /// Builds a resampler for `dma_fs`, or `None` when `dma_fs == MOD_FS`
    pub fn for_dma_fs(dma_fs: u32) -> Option<Self> {
        if dma_fs == MOD_FS {
            return None;
        }

        fn gcd(mut a: u32, mut b: u32) -> u32 {
            while b != 0 {
                let t = b;
                b = a % b;
                a = t;
            }
            a
        }

        let g = gcd(dma_fs, MOD_FS);
        let l = (dma_fs / g) as usize;
        let m = (MOD_FS / g) as usize;

        let taps_per_phase = 16;
        let n = l * taps_per_phase;

        // Windowed-sinc low-pass prototype at the interpolated rate (MOD_FS * L).
        // Cutoff at 90% of the Nyquist of the lower rate.
        let lower_fs = MOD_FS.min(dma_fs);
        let fc = 0.9 * (lower_fs as f32 / 2.0) / (MOD_FS as f32 * l as f32); // cycles/sample
        let mut proto = design_lowpass_hamming(n, fc);
        for t in &mut proto {
            *t *= l as f32; // rescale to Sigma = L
        }

        // Polyphase decomposition: branch p uses taps p, p+L, p+2L, ...
        let mut poly = vec![vec![0.0f32; taps_per_phase]; l];
        for p in 0..l {
            for k in 0..taps_per_phase {
                poly[p][k] = proto[p + k * l];
            }
        }

        Some(Self {
            l,
            m,
            taps_per_phase,
            poly,
            hist_i: vec![0.0; taps_per_phase * 2],
            hist_q: vec![0.0; taps_per_phase * 2],
            hist_idx: 0,
            phase: 0,
        })
    }

    #[inline]
    fn branch(&self, p: usize, hist: &[f32]) -> f32 {
        let k = self.taps_per_phase;
        let coeffs = &self.poly[p];
        // Contiguous chronological window of the last k inputs (see the hist_i field comment).
        let window = &hist[self.hist_idx..self.hist_idx + k];
        let mut acc = 0.0f32;
        for t in 0..k {
            // tap t multiplies the t-th most recent input = window[k-1-t]
            acc += coeffs[t] * window[k - 1 - t];
        }
        acc
    }

    /// Resamples a chunk of i16 IQ, appending the output to `out_i`/`out_q`.
    pub fn process(
        &mut self,
        in_i: &[i16],
        in_q: &[i16],
        out_i: &mut Vec<i16>,
        out_q: &mut Vec<i16>,
    ) {
        let n = in_i.len().min(in_q.len());
        let k = self.taps_per_phase;
        for idx in 0..n {
            // Write each input twice so the convolution window stays contiguous.
            let xi = in_i[idx] as f32;
            let xq = in_q[idx] as f32;
            self.hist_i[self.hist_idx] = xi;
            self.hist_i[self.hist_idx + k] = xi;
            self.hist_q[self.hist_idx] = xq;
            self.hist_q[self.hist_idx + k] = xq;
            self.hist_idx += 1;
            if self.hist_idx >= k {
                self.hist_idx = 0;
            }
            while self.phase < self.l {
                let yi = self.branch(self.phase, &self.hist_i);
                let yq = self.branch(self.phase, &self.hist_q);
                out_i.push(yi.round().clamp(-32768.0, 32767.0) as i16);
                out_q.push(yq.round().clamp(-32768.0, 32767.0) as i16);
                self.phase += self.m;
            }
            self.phase -= self.l;
        }
    }
}

// -------------------------------------------------------------------------
// High-Level Modulator & Processing Pipeline
// -------------------------------------------------------------------------

/// A stateful Single Sideband (SSB) modulator producing carrier-suppressed analytic IQ.
/// Uses a complex analytic band-pass FIR ([`ComplexSsbFir`]) so audio maps to `+f_a` (USB) with the
/// suppressed carrier at DC - the standard SSB convention, with no audio energy folded onto DC.
pub struct TxModulator {
    pub mode: TxMode,
    pub filter_bw: f32,
    ssb_mod: ComplexSsbFir,
    pub tx_fs: f32,
    c32_buf: Vec<C32>,
}

impl TxModulator {
    pub fn new(mode: TxMode, filter_bw: f32, tx_fs: f32) -> Self {
        let actual_mod_fs = MOD_FS as f32;
        let clamped_bw = filter_bw.clamp(crate::FILTER_BW_MIN_HZ, 20_000.0);
        let is_usb = mode == TxMode::USB;

        // Complex analytic band-pass FIR: carrier-suppressed by construction, -83 dBc opposite
        // sideband, standard convention (carrier at DC, USB audio at +f_a). The FPGA +50 kHz DDS
        // does the RF offset, so no software rf shift is needed here.
        let ssb_mod = ComplexSsbFir::new(actual_mod_fs, clamped_bw, is_usb);

        // -- FALLBACK: carrier-fixed orion-sdr Weaver modulator (cheaper CPU, but worse opposite-
        // sideband: ~-11..-30 dBc vs -83 dBc). Produces the SAME standard convention as the FIR
        // above (carrier at DC, audio at +f_a)
        // To switch: change the `ssb_mod` field type to `orion_sdr::modulate::SsbPhasingMod`,
        // add `use orion_sdr::core::Block;`, replace the line above with the block below, and use
        // `self.ssb_mod.process(audio, &mut self.c32_buf)` (pre-sized) in `process_chunk`.
        // rf_hz = +/-audio_if (NOT 0) together with the flipped usb flag places the carrier at DC
        // rather than mid-audio (audio_if):
        //   let audio_if = clamped_bw / 2.0;
        //   let (rf_hz, weaver_usb) = if is_usb { (audio_if, false) } else { (-audio_if, true) };
        //   let ssb_mod = SsbPhasingMod::new(actual_mod_fs, audio_if / 0.9, audio_if, rf_hz, weaver_usb);

        Self {
            mode,
            filter_bw: clamped_bw,
            ssb_mod,
            tx_fs,
            c32_buf: Vec::with_capacity(crate::TX_DMA_SIZE),
        }
    }

    /// Modulates an input slice of f32 audio samples, outputting matched in-phase and quadrature i16 buffers.
    pub fn process_chunk(&mut self, audio: &[f32], out_i: &mut Vec<i16>, out_q: &mut Vec<i16>) {
        self.c32_buf.clear();

        // Process modulation using the stored stateful modulator (appends to c32_buf).
        self.ssb_mod.process(audio, &mut self.c32_buf);

        // Convert modulated complex samples directly to i16 without software upsampling.
        out_i.reserve(self.c32_buf.len());
        out_q.reserve(self.c32_buf.len());

        // Scale to 30000.0 instead of 32767.0 to leave an 8.5% safety margin/headroom.
        // This prevents DAC clipping and integer overflow wrapping on filter overshoot or transient baseband spikes.
        for c in &self.c32_buf {
            let i_val = (c.re * 30000.0) as i16;
            let q_val = (c.im * 30000.0) as i16;
            out_i.push(i_val);
            out_q.push(q_val);
        }
    }
}

// -------------------------------------------------------------------------
// Mode / Configuration Structs
// -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum TxMode {
    USB,
    LSB,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TxConfig {
    pub mode: TxMode,
    pub filter_bw: f32,
    pub active: bool,
}

// -------------------------------------------------------------------------
// Modulation Mode Components
// -------------------------------------------------------------------------

/// A single **complex analytic band-pass FIR** SSB modulator.
///
/// It convolves the real audio with a complex FIR whose response is a low-pass prototype (cutoff
/// `filter_bw/2`) frequency-shifted up by `filter_bw/2`, so it passes audio `[0, filter_bw]` on the
/// positive-frequency side and rejects the negatives. The output is therefore a carrier-suppressed
/// analytic (single-sideband) baseband: audio tone `f_a` lands at baseband `+f_a` (USB) with the
/// suppressed carrier at DC - the standard SSB convention. `usb=false` conjugates the taps for LSB.
///
/// A cheaper Weaver-modulator alternative (`orion_sdr::modulate::SsbPhasingMod`, carrier-fixed but
/// with worse opposite-sideband rejection) is kept commented in `TxModulator::new` for A/B testing.
pub struct ComplexSsbFir {
    taps: Vec<C32>,
    hist: Vec<f32>, // double-length real circular buffer
    head_idx: usize,
}

impl ComplexSsbFir {
    pub fn new(fs: f32, filter_bw: f32, usb: bool) -> Self {
        let taps = ssb_analytic_taps(fs, filter_bw, usb, TX_AUDIO_LOW_CUT_HZ);
        Self {
            hist: vec![0.0; taps.len() * 2],
            taps,
            head_idx: 0,
        }
    }

    /// Modulates real audio to analytic IQ, appending to `out`.
    pub fn process(&mut self, audio: &[f32], out: &mut Vec<C32>) {
        let nt = self.taps.len();
        out.reserve(audio.len());
        let mut head = self.head_idx;
        for &x in audio {
            self.hist[head] = x;
            self.hist[head + nt] = x;
            let window = &self.hist[head + 1..head + 1 + nt];
            let mut acc = C32::new(0.0, 0.0);
            for t in 0..nt {
                acc += self.taps[t] * window[nt - 1 - t];
            }
            out.push(acc);

            head += 1;
            if head >= nt {
                head = 0;
            }
        }
        self.head_idx = head;
    }
}
