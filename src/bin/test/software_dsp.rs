use pluto::dsp::{AudioProcessor, Demodulation, FilterAudio};
use crate::test::dsp_helpers::{fft_mags_f32, fft_mags_i16, write_wav_f32_mono};
use pluto::tx_dsp::{TxMode, TxModulator};
use std::f32::consts::PI;

/// Pure software test of the SSB modulation-demodulation chain.
/// No hardware, no FPGA, no DMA. Just: modulate -> simulate FPGA shifts -> demodulate.
/// If the output WAV contains the original audio, the softwares DSP chain is correct.
pub fn run_soft_ssb_loopback(output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== SSB DEMOD SOFTWARE TEST ===");
    println!("Pure software: modulate -> simulate FPGA -> demodulate -> WAV");
    println!("No hardware involved.\n");

    let audio_fs = 48_000.0f32;
    let filter_bw = 3_000.0f32;

    // --- Generate test audio (500 Hz + 1500 Hz) ---
    let duration_s = 2.0;
    let num_samples = (audio_fs * duration_s) as usize;
    let input_audio: Vec<f32> = (0..num_samples)
        .map(|n| {
            let t = n as f32 / audio_fs;
            0.5 * (2.0 * PI * 500.0 * t).sin() + 0.5 * (2.0 * PI * 1500.0 * t).sin()
        })
        .collect();

    println!(
        "Input: {} samples ({:.1}s), tones at 500 Hz + 1500 Hz",
        num_samples, duration_s
    );

    // SSB modulate (same as TX path)
    let mut modulator = TxModulator::new(TxMode::USB, filter_bw, 3_840_000.0);
    let chunk_size = 4096;

    let mut mod_i_all: Vec<i16> = Vec::new();
    let mut mod_q_all: Vec<i16> = Vec::new();

    for chunk in input_audio.chunks(chunk_size) {
        let mut padded = chunk.to_vec();
        padded.resize(chunk_size, 0.0);

        let mut out_i = Vec::new();
        let mut out_q = Vec::new();
        modulator.process_chunk(&padded, &mut out_i, &mut out_q);
        mod_i_all.extend_from_slice(&out_i);
        mod_q_all.extend_from_slice(&out_q);
    }

    println!(
        "Modulator output: {} IQ samples at {} kHz",
        mod_i_all.len(),
        audio_fs / 1000.0
    );

    // Analyze modulator output spectrum
    println!("\nModulator output spectrum:");
    let probe_freqs: Vec<f64> = vec![
        -2000.0, -1500.0, -1000.0, -500.0, 0.0, 500.0, 1000.0, 1500.0, 2000.0,
    ];
    let n_mod = mod_i_all.len().min(mod_q_all.len()).min(48000);
    let mags = fft_mags_i16(
        &mod_i_all[..n_mod],
        &mod_q_all[..n_mod],
        &probe_freqs,
        audio_fs as f64,
    );
    for (idx, &f) in probe_freqs.iter().enumerate() {
        let mag = mags[idx];
        let bar_len = (mag / 500.0).min(30.0) as usize;
        println!("  {:+6.0} Hz: {:8.1}  {}", f, mag, "#".repeat(bar_len));
    }

    // Simulate the FPGA
    // TX DDS shifts by +50 kHz, RX DDS shifts by -50 kHz -> no shift.
    // CIC+FIR decimation preserves the baseband signal.
    // So the audio DMA data ~ modulator output (at 48 kHz, after FPGA round-trip).
    //
    // But the FPGA CIC decimates from 3.84 MHz to 240 kHz (by 4+4=16x).
    // Then software decimates from 240 kHz to 48 kHz (by 5x).
    //
    // For this software test, we simulate the FPGA by treating the modulator output
    // as if it arrived at 240 kHz (interpolating by 5) so FilterAudio can decimate by 5.

    println!("\nSimulating FPGA round-trip...");

    // Interpolate by 5 (48 kHz -> 240 kHz) to simulate DMA rate
    let interp = 5;
    let dma_fs = audio_fs as i64 * interp as i64; // 240000
    let mut dma_i: Vec<i16> = Vec::with_capacity(mod_i_all.len() * interp);
    let mut dma_q: Vec<i16> = Vec::with_capacity(mod_q_all.len() * interp);
    for idx in 0..mod_i_all.len() {
        dma_i.push(mod_i_all[idx]);
        dma_q.push(mod_q_all[idx]);
        for _ in 1..interp {
            dma_i.push(0);
            dma_q.push(0);
        }
    }

    println!(
        "  Simulated DMA: {} samples at {} kHz",
        dma_i.len(),
        dma_fs / 1000
    );

    // Demodulate (same as audio_thread.rs SSB path)
    let if_cutoff_hz = filter_bw; // pass the full one-sided sideband [0, bw] (was bw/2)
    let bfo_hz = filter_bw / 2.0; // sign only: + = USB (analytic demod ignores the magnitude)
    let sw_decimation = interp; // 5

    let mut audio_filter = FilterAudio::new(sw_decimation, dma_fs, if_cutoff_hz);
    let demod = Demodulation::SSB {
        fs: audio_fs,
        bfo_hz,
        audio_bw_hz: filter_bw,
    };
    let mut audio_processor = AudioProcessor::new(demod);
    let mut audio_output: Vec<f32> = Vec::new();

    // Process in chunks (same as audio_thread)
    for chunk_start in (0..dma_i.len()).step_by(16384) {
        let chunk_end = (chunk_start + 16384).min(dma_i.len());
        let i_chunk = &dma_i[chunk_start..chunk_end];
        let q_chunk = &dma_q[chunk_start..chunk_end];

        let sliced_iq = audio_filter.execute(i_chunk, q_chunk);
        if !sliced_iq.is_empty() {
            audio_processor.process(sliced_iq, &mut audio_output);
        }
    }

    println!(
        "  Demodulated: {} audio samples at {} kHz",
        audio_output.len(),
        audio_fs / 1000.0
    );

    // Analyze output spectrum
    println!("\nDemodulated output spectrum:");
    let out_probes = [
        0.0, 250.0, 500.0, 750.0, 1000.0, 1250.0, 1500.0, 1750.0, 2000.0, 2500.0, 3000.0, 5000.0,
    ];
    let n_out = audio_output.len().min(48000);
    let freqs_f64: Vec<f64> = out_probes.iter().map(|&f| f as f64).collect();
    let mags = fft_mags_f32(&audio_output[..n_out], &freqs_f64, audio_fs as f64);
    let max_mag = mags.iter().copied().fold(0.0f64, f64::max);

    for (idx, &f) in out_probes.iter().enumerate() {
        let mag = mags[idx];
        let db = if mag > 0.0 && max_mag > 0.0 {
            20.0 * (mag / max_mag).log10()
        } else {
            -99.0
        };
        let bar_len = if max_mag > 0.0 {
            ((mag / max_mag) * 30.0) as usize
        } else {
            0
        };
        println!(
            "  {:6.0} Hz: {:10.1}  {:5.1} dB  {}",
            f,
            mag,
            db,
            "#".repeat(bar_len.min(30))
        );
    }

    // Write output WAV
    let peak = audio_output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    println!("\nOutput: peak={:.4}", peak);

    write_wav_f32_mono(output_path, &audio_output, audio_fs as u32, true)?;
    println!("Wrote: {}", output_path);

    // Also write the INPUT for comparison
    let input_path = output_path.replace(".wav", "_input.wav");
    write_wav_f32_mono(&input_path, &input_audio, audio_fs as u32, false)?;
    println!("Wrote input reference: {}", input_path);

    println!("\n=== SSB DEMOD TEST COMPLETE ===");
    println!(
        "Compare {} (demodulated) vs {} (original)",
        output_path, input_path
    );
    println!("Expected: tones at 500 Hz and 1500 Hz in both files.");
    Ok(())
}
