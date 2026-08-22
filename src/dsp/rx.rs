use crate::dsp::filter_design::{design_lowpass_hamming, ssb_analytic_taps};
use num_complex::Complex32;
use orion_sdr::IqToAudioChain;
use orion_sdr::core::{Block, WorkReport};
use orion_sdr::demodulate::FmQuadratureDemod;

// -------------------------------------------------------------------------
// Baseband Filtering and Decimation
// -------------------------------------------------------------------------

/// A FIR low-pass filter and decimator for the received complex baseband.
/// Filters out adjacent signals and reduces the sample rate (decimation) to match the audio
/// demodulator's input rate.
pub struct FilterAudio {
    decimation: usize,
    fs_hz: i64,
    cutoff_hz: f32,
    mixer_count: usize,
    history_re: Vec<f32>,
    history_im: Vec<f32>,
    history_idx: usize,
    taps: Vec<f32>,
}

impl FilterAudio {
    pub fn new(decimation: usize, fs_hz: i64, cutoff_hz: f32) -> Self {
        let mut filter = Self {
            decimation,
            fs_hz,
            cutoff_hz,
            mixer_count: 0,
            history_re: Vec::new(),
            history_im: Vec::new(),
            history_idx: 0,
            taps: Vec::new(),
        };
        filter.recompute_taps();
        filter
    }

    fn recompute_taps(&mut self) {
        let mut num_taps = (self.decimation * 6) | 1; // Keep it odd for symmetric filter properties
        num_taps = num_taps.clamp(63, 511);
        let max_cutoff = (self.fs_hz as f32 / self.decimation as f32) * 0.45;
        let effective_cutoff = self.cutoff_hz.min(max_cutoff);
        let fc = effective_cutoff / self.fs_hz as f32; // Low pass cutoff
        self.taps = design_lowpass_hamming(num_taps, fc);

        // The history buffer is allocated to twice the size of the FIR filter taps (num_taps * 2).
        // This is part of the double-buffering optimization used during convolution in `execute()`.
        // By storing each incoming sample at both `h_idx` and `h_idx + num_taps`, we ensure that
        // a contiguous slice of length `num_taps` starting at `h_idx` always represents the last
        // `num_taps` samples in correct chronological order. This eliminates the need to either
        // shift elements (expensive O(N) copy) or handle circular buffer wrap-arounds inside the
        // hot path convolution loop
        let new_len = num_taps * 2;
        if self.history_re.len() != new_len {
            self.history_re = vec![0.0; new_len];
            self.history_im = vec![0.0; new_len];
            self.history_idx = 0;
            self.mixer_count = 0;
        }
    }

    /// Updates the tuning and filtering parameters.
    /// If any parameter affecting the filter shape changes, it triggers `recompute_taps`.
    pub fn set_params(&mut self, decimation: usize, fs_hz: i64, cutoff_hz: f32) {
        let mut changed = false;
        if self.fs_hz != fs_hz {
            self.fs_hz = fs_hz;
            changed = true;
        }
        if self.decimation != decimation {
            self.decimation = decimation;
            changed = true;
        }
        if self.cutoff_hz != cutoff_hz {
            self.cutoff_hz = cutoff_hz;
            changed = true;
        }
        if changed {
            self.recompute_taps();
        }
    }

    /// Processes a block of raw I/Q samples from the SDR.
    /// For each sample:
    /// 1. Pushes the sample into a circular delay line (history buffer).
    /// 2. If enough samples have accumulated (based on `decimation`), it applies the FIR filter
    ///    to the delay line and outputs a single decimated, low-pass-filtered sample.
    pub fn execute(&mut self, i: &[i16], q: &[i16]) -> Vec<Complex32> {
        let n = std::cmp::min(i.len(), q.len());
        let mut sliced_iq = Vec::with_capacity((n + self.mixer_count) / self.decimation + 1);

        let num_taps = self.taps.len();
        let mut m_count = self.mixer_count;
        let mut h_idx = self.history_idx;
        let inv_32768 = 1.0 / 32768.0;

        for idx in 0..n {
            let re = i[idx] as f32 * inv_32768;
            let im = q[idx] as f32 * inv_32768;

            // Write each sample twice: once at the current index (h_idx) and once at the index offset by
            // num_taps (h_idx + num_taps). Since the buffer is size num_taps * 2 and h_idx wraps at num_taps,
            // this guarantees that for any wrap boundary, the block of size num_taps starting at the new h_idx
            // (after incrementing) forms a contiguous chronologically ordered history slice.
            self.history_re[h_idx] = re;
            self.history_re[h_idx + num_taps] = re;
            self.history_im[h_idx] = im;
            self.history_im[h_idx + num_taps] = im;

            h_idx += 1;
            if h_idx >= num_taps {
                h_idx = 0;
            }

            m_count += 1;

            if m_count >= self.decimation {
                let mut out_i = 0.0;
                let mut out_q = 0.0;

                // Because of the double-buffer layout, we can grab a contiguous slice representing
                // the last num_taps samples in correct order starting directly at the current h_idx.
                // This contiguous access is highly friendly to CPU caches and compilers trying to optimize it
                let window_re = &self.history_re[h_idx..h_idx + num_taps];
                let window_im = &self.history_im[h_idx..h_idx + num_taps];
                let taps = &self.taps[..num_taps];

                for idx_tap in 0..num_taps {
                    out_i += window_re[idx_tap] * taps[idx_tap];
                    out_q += window_im[idx_tap] * taps[idx_tap];
                }

                sliced_iq.push(Complex32::new(out_i, out_q));
                m_count = 0;
            }
        }

        self.history_idx = h_idx;
        self.mixer_count = m_count;
        sliced_iq
    }
}

// -------------------------------------------------------------------------
// High-Level Demodulator Manager
// -------------------------------------------------------------------------

/// Post-DDC audio demodulator.
/// FM: `FmQuadratureDemod` => anti-alias decimate (`FmDecimator`) => one-pole IIR => DC blocker.
/// SSB: `AnalyticSsbDemod` => audio DC blocker.
pub enum AudioProcessor {
    FM {
        chain: IqToAudioChain<FmQuadratureDemod>,
        // Anti-alias decimating FIR for the composite =Y audio step
        decimator: FmDecimator,
        deemphasis_state: f32,
        dc_blocker_x: f32,
        dc_blocker_y: f32,
    },
    SSB {
        // FALLBACK (orion-sdr): to use the orion-sdr product detector instead of the custom analytic
        // demod, change this field type to `IqToAudioChain<SsbProductDemod>`
        chain: IqToAudioChain<AnalyticSsbDemod>,
        dc_blocker_x: f32,
        dc_blocker_y: f32,
    },
}

impl AudioProcessor {
    pub fn new(demod: Demodulation) -> Self {
        match demod {
            Demodulation::FM {
                audio_fs,
                dev_hz,
                audio_bw_hz,
            } => AudioProcessor::FM {
                chain: IqToAudioChain::new(FmQuadratureDemod::new(audio_fs, dev_hz, audio_bw_hz)),
                // Decimate the composite from audio_fs (240 kHz) down to the 48 kHz audio rate.
                decimator: FmDecimator::new(
                    audio_fs,
                    (audio_fs / 48_000.0).round().max(1.0) as usize,
                    15_000.0,
                ),
                deemphasis_state: 0.0,
                dc_blocker_x: 0.0,
                dc_blocker_y: 0.0,
            },

            Demodulation::SSB {
                fs,
                bfo_hz,
                audio_bw_hz,
            } => AudioProcessor::SSB {
                // Sign of bfo_hz selects the sideband (USB = +, LSB = -); its magnitude is unused by
                // the analytic demod (the sideband is chosen by the FIR passband, not a BFO shift).
                chain: IqToAudioChain::new(AnalyticSsbDemod::new(fs, audio_bw_hz, bfo_hz >= 0.0)),
                // -- FALLBACK: orion-sdr product detector (pairs with EITHER TX variant, since both
                // now use the standard convention: carrier at DC, single sideband). Use bfo_hz = 0.0
                // so it recovers the sideband as Re(z). NOTE: unlike the analytic demod it is NOT
                // sideband-selective, so it also passes any residual opposite sideband (fine for the
                // clean complex-FIR TX; the Weaver TX's ~-11..-30 dBc image would leak through). To
                // switch: change the field type above and replace the line with:
                //   chain: IqToAudioChain::new(SsbProductDemod::new(fs, 0.0, audio_bw_hz)),
                dc_blocker_x: 0.0,
                dc_blocker_y: 0.0,
            },
        }
    }

    pub fn process(
        &mut self,
        samples: Vec<Complex32>,
        audio_buffer: &mut Vec<f32>,
        squelch_threshold_db: f32,
    ) {
        // Squelch
        let squelch_mute = if squelch_threshold_db > -99.0 && !samples.is_empty() {
            let p_raw = samples.iter().map(|c| c.norm_sqr()).sum::<f32>() / samples.len() as f32;
            let power_db = 10.0 * (p_raw + 1e-12).log10();
            power_db < squelch_threshold_db
        } else {
            false
        };

        match self {
            AudioProcessor::FM {
                chain,
                decimator,
                deemphasis_state,
                dc_blocker_x,
                dc_blocker_y,
            } => {
                let audio = chain.process(samples);

                for &sample in &audio {
                    // Anti-alias low-pass + decimate (240 => 48 kHz). Emits one audio sample per
                    // decimation factor; the FIR rejects the 19 kHz pilot and the stereo/RDS
                    // subcarriers so they don't alias into the audio band.
                    if let Some(decimated) = decimator.process(sample) {
                        // Apply De-emphasis filter for Broadcast FM (alpha ~ 0.7575 at 48kHz)
                        *deemphasis_state = *deemphasis_state * 0.7575 + decimated * (1.0 - 0.7575);

                        // Boost volume
                        let base_audio = *deemphasis_state * 4000.0;

                        let dc_blocked = base_audio - *dc_blocker_x + 0.995 * *dc_blocker_y;
                        *dc_blocker_x = base_audio;
                        *dc_blocker_y = dc_blocked;

                        let final_audio = if squelch_mute { 0.0 } else { dc_blocked };
                        audio_buffer.push(final_audio);
                    }
                }
            }
            AudioProcessor::SSB {
                chain,
                dc_blocker_x,
                dc_blocker_y,
            } => {
                let audio = chain.process(samples);
                for sample in audio {
                    let base_audio = -sample * 16000.0;

                    let dc_blocked = base_audio - *dc_blocker_x + 0.995 * *dc_blocker_y;
                    *dc_blocker_x = base_audio;
                    *dc_blocker_y = dc_blocked;

                    let final_audio = if squelch_mute { 0.0 } else { dc_blocked };
                    audio_buffer.push(final_audio);
                }
            }
        }
    }

    pub fn copy_state_from(&mut self, old: &AudioProcessor) {
        match (self, old) {
            (
                AudioProcessor::FM {
                    deemphasis_state: new_de,
                    dc_blocker_x: new_dx,
                    dc_blocker_y: new_dy,
                    ..
                },
                AudioProcessor::FM {
                    deemphasis_state: old_de,
                    dc_blocker_x: old_dx,
                    dc_blocker_y: old_dy,
                    ..
                },
            ) => {
                *new_de = *old_de;
                *new_dx = *old_dx;
                *new_dy = *old_dy;
            }
            (
                AudioProcessor::SSB {
                    dc_blocker_x: new_dx,
                    dc_blocker_y: new_dy,
                    ..
                },
                AudioProcessor::SSB {
                    dc_blocker_x: old_dx,
                    dc_blocker_y: old_dy,
                    ..
                },
            ) => {
                *new_dx = *old_dx;
                *new_dy = *old_dy;
            }
            _ => {}
        }
    }
}

// -------------------------------------------------------------------------
// Demodulation Modes
// -------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Demodulation {
    FM {
        audio_fs: f32,
        dev_hz: f32,
        audio_bw_hz: f32,
    },
    SSB {
        fs: f32,
        bfo_hz: f32,
        audio_bw_hz: f32,
    },
}

// -------------------------------------------------------------------------
// Demodulation: FM
// -------------------------------------------------------------------------

/// Decimating anti-alias low-pass FIR for the FM composite => audio step (e.g. 240 kHz -> 48 kHz).
pub struct FmDecimator {
    taps: Vec<f32>,
    // Double-length history ring (same layout as FilterAudio): each sample is written at `idx` and
    // `idx + N` so the most recent N samples are always a contiguous slice starting at `idx`.
    hist: Vec<f32>,
    idx: usize,
    phase: usize,
    factor: usize,
}

impl FmDecimator {
    pub fn new(fs_in: f32, factor: usize, cutoff_hz: f32) -> Self {
        let num_taps = 161usize; // ~15 kHz passband with >60 dB rejection at the 19 kHz pilot
        let taps = design_lowpass_hamming(num_taps, cutoff_hz / fs_in);

        Self {
            hist: vec![0.0; num_taps * 2],
            taps,
            idx: 0,
            phase: 0,
            factor: factor.max(1),
        }
    }

    /// Feeds one composite sample; returns `Some(audio)` on every `factor`-th sample (the decimated,
    /// anti-alias-filtered output), `None` otherwise.
    pub fn process(&mut self, x: f32) -> Option<f32> {
        let n = self.taps.len();
        self.hist[self.idx] = x;
        self.hist[self.idx + n] = x;
        self.idx += 1;
        if self.idx >= n {
            self.idx = 0;
        }

        self.phase += 1;
        if self.phase < self.factor {
            return None;
        }
        self.phase = 0;

        let window = &self.hist[self.idx..self.idx + n];
        let mut acc = 0.0f32;
        for k in 0..n {
            acc += window[k] * self.taps[k];
        }
        Some(acc)
    }
}

// -------------------------------------------------------------------------
// Demodulation: SSB
// -------------------------------------------------------------------------

/// Analytic single-sideband demodulator - the RX dual of `tx_dsp::ComplexSsbFir`.
///
/// Convolves the received complex baseband with the same analytic band-pass FIR taps (passing the
/// wanted sideband, rejecting the other by ~83 dB) and takes the real part = recovered audio. Works
/// on the **standard** SSB convention where the tuned carrier is at baseband DC and USB audio sits
/// at `+f_a` (LSB at `-f_a`). `usb` is taken from the sign of `bfo_hz` (`>= 0` => USB) to keep the
/// caller convention.
#[derive(Clone)]
pub struct AnalyticSsbDemod {
    taps: Vec<Complex32>,
    hist: Vec<Complex32>, // double-length circular buffer
    head_idx: usize,
}

impl AnalyticSsbDemod {
    pub fn new(fs: f32, audio_bw_hz: f32, usb: bool) -> Self {
        // RX passes the full [0, bw] (f_lo = 0.0); the TX-side low-cut is what shapes the audio.
        let taps = ssb_analytic_taps(fs, audio_bw_hz, usb, 0.0);
        Self {
            hist: vec![Complex32::new(0.0, 0.0); taps.len() * 2],
            taps,
            head_idx: 0,
        }
    }
}

impl Block for AnalyticSsbDemod {
    type In = Complex32;
    type Out = f32;

    fn process(&mut self, input: &[Complex32], output: &mut [f32]) -> WorkReport {
        let n = input.len().min(output.len());
        let nt = self.taps.len();
        let mut head = self.head_idx;
        for i in 0..n {
            let sample = input[i];
            self.hist[head] = sample;
            self.hist[head + nt] = sample;
            let window = &self.hist[head + 1..head + 1 + nt];
            let mut acc = Complex32::new(0.0, 0.0);
            for t in 0..nt {
                acc += self.taps[t] * window[nt - 1 - t];
            }
            output[i] = acc.re;

            head += 1;
            if head >= nt {
                head = 0;
            }
        }
        self.head_idx = head;
        WorkReport {
            in_read: n,
            out_written: n,
        }
    }
}
