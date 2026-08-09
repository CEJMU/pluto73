use crate::test::dsp_helpers::{
    dominant_tone_spurs, fft_mags_f32, fft_mags_i16, write_wav_f32_mono,
};
use pluto::dsp::{AudioProcessor, Demodulation, FilterAudio};
use pluto::tx_dsp::{TxMode, TxModulator};
use std::f32::consts::PI;

const CHUNK: usize = pluto::TX_DMA_SIZE;
const FILTER_BW: f32 = 3_000.0;
/// The audio DMA carries five samples per audio sample, so the receive chain's decimator expects its input at that rate.
const INTERP: usize = 5;
/// How far either side of a wanted tone the spur search ignores.
///
/// This has to be wide because `dominant_tone_spurs` applies a Hamming window:
/// its first sidelobe is about -43 dB and the skirt then falls off slowly, so close to a full-scale tone the analysis window dominates whatever the chain itself produced.
/// With a 10 Hz exclusion the search duly reported "spurs" at 514 Hz and 989 Hz at roughly -47 dBc, both 14 Hz from a tone, both pure window skirt.
/// At 250 Hz the reading is a real one: 749 Hz at -72 dBc for the single-tone case.
///
/// The cost is that anything closer than this to a tone is simply out of reach here, and that the dual-tone case can find nothing at all to report: two full-scale tones 1 kHz apart, each with a 250 Hz exclusion around it, leave too little clear spectrum for this window.
/// Measuring close-in components needs a window with a faster-decaying skirt than the shared helper applies.
const SPUR_EXCLUDE_HZ: f32 = 250.0;

/// Pure software test of the SSB modulation-demodulation chain.
/// No hardware, no FPGA, no DMA. Just: modulate -> match the DMA rate -> demodulate.
///
/// Two drive conditions are run. The dual tone exercises the chain with two components present at once.
/// The single 1 kHz tone is the same as `--test-spec-tx-radiated` uses, so the software and hardware image figures describe the same quantity and can be compared directly.
/// Levels are reported in dB against the wanted tone, so the suppression figures can be read directly and compared with the radiated ones.
pub fn run_soft_ssb_loopback(output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== SSB DEMOD SOFTWARE TEST ===");
    println!("Pure software: modulate -> match DMA rate -> demodulate -> WAV");
    println!("No hardware involved.\n");

    run_case(
        "dual tone (500 Hz + 1500 Hz)",
        &[500.0, 1500.0],
        Some(output_path),
    )?;
    run_case(
        "single tone (1 kHz, same stimulus as the radiated measurement)",
        &[1000.0],
        None,
    )?;

    println!("=== SSB DEMOD TEST COMPLETE ===");
    Ok(())
}

/// Runs one modulate -> rate-match -> demodulate pass over `tones`, reporting the modulator's image and carrier suppression and the demodulated audio's tone recovery and worst spur.
/// When `wav_path` is given, the demodulated audio and the input reference are written alongside.
fn run_case(
    label: &str,
    tones: &[f32],
    wav_path: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let audio_fs = pluto::AUDIO_SAMPLE_RATE as f32;
    let duration_s = 2.0;
    let num_samples = (audio_fs * duration_s) as usize;
    // Full scale split across the tones, so every case presents the same peak envelope.
    let amplitude = 1.0 / tones.len() as f32;
    let input_audio: Vec<f32> = (0..num_samples)
        .map(|n| {
            let t = n as f32 / audio_fs;
            tones
                .iter()
                .map(|&f| amplitude * (2.0 * PI * f * t).sin())
                .sum()
        })
        .collect();

    println!("--- {} ---", label);

    let mut modulator = TxModulator::new(TxMode::USB, FILTER_BW, 3_840_000.0);
    let mut mod_i: Vec<i16> = Vec::new();
    let mut mod_q: Vec<i16> = Vec::new();
    for chunk in input_audio.chunks(CHUNK) {
        let mut padded = chunk.to_vec();
        padded.resize(CHUNK, 0.0);
        let mut out_i = Vec::new();
        let mut out_q = Vec::new();
        modulator.process_chunk(&padded, &mut out_i, &mut out_q);
        mod_i.extend_from_slice(&out_i);
        mod_q.extend_from_slice(&out_q);
    }

    // Modulator output: what the analytic bandpass leaves at the image and at DC
    // Each wanted tone, its mirror, and the suppressed-carrier position, in one transform.
    let n_mod = mod_i.len().min(mod_q.len());
    let probes: Vec<f64> = tones
        .iter()
        .map(|&f| f as f64)
        .chain(tones.iter().map(|&f| -(f as f64)))
        .chain(std::iter::once(0.0))
        .collect();
    let mod_mags = fft_mags_i16(&mod_i[..n_mod], &mod_q[..n_mod], &probes, audio_fs as f64);
    let mod_ref = mod_mags[..tones.len()].iter().copied().fold(0.0, f64::max);

    println!(
        "  modulator output: {} IQ samples at {:.0} kHz",
        n_mod,
        audio_fs / 1000.0
    );
    for (idx, &f) in tones.iter().enumerate() {
        println!(
            "    wanted  {:+7.0} Hz : {:6.1} dB",
            f,
            to_db(mod_mags[idx], mod_ref)
        );
    }
    for (idx, &f) in tones.iter().enumerate() {
        println!(
            "    image   {:+7.0} Hz : {:6.1} dB",
            -f,
            to_db(mod_mags[tones.len() + idx], mod_ref)
        );
    }
    println!(
        "    carrier {:+7.0} Hz : {:6.1} dB",
        0.0,
        to_db(mod_mags[2 * tones.len()], mod_ref)
    );

    // Rate match
    let dma_fs = audio_fs as i64 * INTERP as i64;
    let mut dma_i: Vec<i16> = Vec::with_capacity(n_mod * INTERP);
    let mut dma_q: Vec<i16> = Vec::with_capacity(n_mod * INTERP);
    for idx in 0..n_mod {
        dma_i.push(mod_i[idx]);
        dma_q.push(mod_q[idx]);
        for _ in 1..INTERP {
            dma_i.push(0);
            dma_q.push(0);
        }
    }

    // Demodulate with the production receive chain
    let demod = Demodulation::SSB {
        fs: audio_fs,
        // Sign only: + selects USB, the analytic demodulator ignores the magnitude.
        bfo_hz: FILTER_BW / 2.0,
        audio_bw_hz: FILTER_BW,
    };
    let mut audio_filter = FilterAudio::new(INTERP, dma_fs, FILTER_BW);
    let mut audio_processor = AudioProcessor::new(demod);
    let mut audio_output: Vec<f32> = Vec::new();

    for chunk_start in (0..dma_i.len()).step_by(16384) {
        let chunk_end = (chunk_start + 16384).min(dma_i.len());
        let sliced_iq = audio_filter.execute(
            &dma_i[chunk_start..chunk_end],
            &dma_q[chunk_start..chunk_end],
        );
        if !sliced_iq.is_empty() {
            audio_processor.process(sliced_iq, &mut audio_output, -100.0);
        }
    }

    // Demodulated audio: tone recovery and the worst line that is not a wanted tone
    let out_probes: Vec<f64> = tones
        .iter()
        .map(|&f| f as f64)
        .chain(std::iter::once(0.0))
        .collect();
    let out_mags = fft_mags_f32(&audio_output, &out_probes, audio_fs as f64);
    let out_ref = out_mags[..tones.len()].iter().copied().fold(0.0, f64::max);

    println!(
        "  demodulated audio: {} samples at {:.0} kHz",
        audio_output.len(),
        audio_fs / 1000.0
    );
    for (idx, &f) in tones.iter().enumerate() {
        println!(
            "    tone    {:7.0} Hz : {:6.1} dB",
            f,
            to_db(out_mags[idx], out_ref)
        );
    }
    println!(
        "    DC residual      : {:6.1} dB",
        to_db(out_mags[tones.len()], out_ref)
    );

    // `dominant_tone_spurs` searches the whole band, but masks only the one tone it locks onto.
    // With two tones present the second would be returned as the worst spur, so ask for several and drop any that sits on a tone we put there.
    const SPUR_CANDIDATES: usize = 16;
    let (_, _, spurs) =
        dominant_tone_spurs(&audio_output, audio_fs, SPUR_CANDIDATES, SPUR_EXCLUDE_HZ);
    match spurs
        .iter()
        .find(|(hz, _)| tones.iter().all(|&t| (hz - t).abs() > SPUR_EXCLUDE_HZ))
    {
        Some((hz, dbc)) => println!(
            "    worst spur beyond {:.0} Hz of a tone: {:.0} Hz at {:.1} dBc",
            SPUR_EXCLUDE_HZ, hz, dbc
        ),
        None => println!(
            "    worst spur: none clear of a tone among the {} strongest",
            SPUR_CANDIDATES
        ),
    }

    let peak = audio_output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    println!("    output peak: {:.4}", peak);

    if let Some(path) = wav_path {
        write_wav_f32_mono(path, &audio_output, audio_fs as u32, true)?;
        let input_path = path.replace(".wav", "_input.wav");
        write_wav_f32_mono(&input_path, &input_audio, audio_fs as u32, false)?;
        println!("    wrote {} and {}", path, input_path);
    }
    println!();

    Ok(())
}

fn to_db(mag: f64, reference: f64) -> f64 {
    if mag > 0.0 && reference > 0.0 {
        20.0 * (mag / reference).log10()
    } else {
        -200.0
    }
}
