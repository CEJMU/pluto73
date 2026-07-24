use crate::dsp::filter_design::hamming_window;
use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::cmp::min;
use std::sync::Arc;

/// Windowed FFT: dB-scaled magnitude row for the waterfall/spectrum display,
/// Hamming window, fast IEEE-754 log10, DC-spike interpolation, FFT shift
pub struct WaterfallProcessor {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    fft_buffer: Vec<Complex<f32>>,
    fft_size: usize,
    pub min_db: f32,
    pub max_db: f32,
}

impl WaterfallProcessor {
    pub fn new(fft_size: usize) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);

        Self {
            fft,
            window: hamming_window(fft_size),
            fft_buffer: vec![Complex::new(0.0, 0.0); fft_size],
            fft_size,
            min_db: -100.0,
            max_db: -40.0,
        }
    }

    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Convenience wrapper around `process_frame_into` that allocates the output row
    pub fn process_frame(&mut self, samples: &[Complex<f32>]) -> Vec<u8> {
        let mut row = vec![0; self.fft_size];
        self.process_frame_into(samples, &mut row);
        row
    }

    /// Computes one dB-scaled magnitude row into `row` (length must be `fft_size`).
    pub fn process_frame_into(&mut self, samples: &[Complex<f32>], row: &mut [u8]) {
        assert_eq!(row.len(), self.fft_size, "output row length != fft_size");
        let n = min(self.fft_size, samples.len());

        // Apply window
        for i in 0..n {
            self.fft_buffer[i] = samples[i] * self.window[i];
        }

        // Zero-pad if there are fewer samples than the target FFT size
        for i in n..self.fft_size {
            self.fft_buffer[i] = Complex::new(0.0, 0.0);
        }

        self.fft.process(&mut self.fft_buffer);

        let half_size = self.fft_size / 2;

        // Constants for scaling DB values to 0.0 .. 1.0 range
        let range_db = self.max_db - self.min_db;

        let norm_factor_sqr = (self.fft_size as f32).powi(2);

        for i in 0..self.fft_size {
            // Normalize squared magnitude to avoid expensive square root
            let mag_sqr = self.fft_buffer[i].norm_sqr() / norm_factor_sqr;
            let mag_db = if mag_sqr > 1e-14 {
                // Fast 10*log10(x) approximation for visual waterfall using IEEE 754 exponent
                let bits = mag_sqr.to_bits() as f32;
                (bits * 1.1920929e-7 - 127.0) * 3.01029995
            } else {
                self.min_db
            };

            // Clamp and scale to [0.0, 1.0]
            let scaled = ((mag_db - self.min_db) / range_db).clamp(0.0, 1.0);

            // FFT Shift: DC to center
            let shifted_idx = (i + half_size) % self.fft_size;
            row[shifted_idx] = (scaled * 255.0) as u8;
        }

        // Visual DC Spike Removal (probably not needed as the Pluto already does a rather good job of this): Interpolate the center bins to hide the hardware LO leakage
        if self.fft_size >= 5 {
            let center = half_size;
            let interpolated = ((row[center - 2] as u16 + row[center + 2] as u16) / 2) as u8;
            row[center - 1] = interpolated;
            row[center] = interpolated;
            row[center + 1] = interpolated;
        }
    }
}
