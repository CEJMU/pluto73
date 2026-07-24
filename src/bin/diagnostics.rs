// test_runner.rs
// Encapsulates the execution of all standalone diagnostic/measurement test commands.
// Keeping this in a separate file keeps main.rs clean and focused on the live server logic.

#[path = "test"]
pub mod test {
    pub mod dma_diagnostics;
    pub mod dsp_helpers;
    pub mod fm_broadcast;
    pub mod narrowband;
    pub mod rf_loopback;
    pub mod software_dsp;
    pub mod spectral_analysis;
    pub mod timing_pacing;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Optional AD9361 BIST (Built-In Self Test) digital-loopback toggle for the characterization tests that support it
    // (spec-tx-shape, spec-tx-wideband, dma-carrier-offset): isolates digital/FPGA causes from
    // analog/RF ones by looping TX->RX inside the AD9361, bypassing DAC/RF/LO/ADC.
    let loopback = args.iter().any(|arg| arg == "--loopback");
    // --- RF & Hardware Loopback Tests ---
    if args.iter().any(|arg| arg == "--test-rf-raw-loopback") {
        if let Err(err) = test::rf_loopback::run_rf_raw_loopback() {
            eprintln!("Raw RF loopback test failed: {}", err);
        }
        std::process::exit(0);
    }
    if let Some(pos) = args
        .iter()
        .position(|arg| arg == "--test-rf-audio-loopback")
    {
        let input = args.get(pos + 1).unwrap_or_else(|| {
            eprintln!("Usage: --test-rf-audio-loopback <input.wav> <output.wav>");
            std::process::exit(1);
        });
        let output = args.get(pos + 2).unwrap_or_else(|| {
            eprintln!("Usage: --test-rf-audio-loopback <input.wav> <output.wav> [fs_hz]");
            std::process::exit(1);
        });
        let fs_hz: i64 = args
            .get(pos + 3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(3_840_000);
        let rx_gain_db: f64 = args
            .get(pos + 4)
            .and_then(|s| s.parse().ok())
            .unwrap_or(40.0);
        let lo_hz: i64 = args
            .get(pos + 5)
            .and_then(|s| s.parse().ok())
            .unwrap_or(900_000_000);
        // Optional sideband: "usb" (default) or "lsb".
        let usb = !args
            .get(pos + 6)
            .map(|s| s.eq_ignore_ascii_case("lsb"))
            .unwrap_or(false);
        if let Err(err) =
            test::rf_loopback::run_rf_audio_loopback(input, output, fs_hz, rx_gain_db, lo_hz, usb)
        {
            eprintln!("RF audio loopback test failed: {}", err);
        }
        std::process::exit(0);
    }
    if let Some(pos) = args.iter().position(|arg| arg == "--test-rf-tone-loopback") {
        let freq_hz: f32 = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                eprintln!("Usage: --test-rf-tone-loopback <freq_hz> <duration_s> <output.wav>");
                std::process::exit(1);
            });
        let duration_s: f32 = args
            .get(pos + 2)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                eprintln!("Usage: --test-rf-tone-loopback <freq_hz> <duration_s> <output.wav>");
                std::process::exit(1);
            });
        let output = args.get(pos + 3).unwrap_or_else(|| {
            eprintln!("Usage: --test-rf-tone-loopback <freq_hz> <duration_s> <output.wav> [chunk_size] [fs_hz]");
            std::process::exit(1);
        });
        let chunk_size: usize = args
            .get(pos + 4)
            .and_then(|s| s.parse().ok())
            .unwrap_or(4096);
        let fs_hz: i64 = args
            .get(pos + 5)
            .and_then(|s| s.parse().ok())
            .unwrap_or(3_840_000);
        if let Err(err) =
            test::rf_loopback::run_rf_tone_loopback(freq_hz, duration_s, output, chunk_size, fs_hz)
        {
            eprintln!("RF tone loopback test failed: {}", err);
        }
        std::process::exit(0);
    }

    // --- Audio DMA & Continuity Probes ---
    if args.iter().any(|arg| arg == "--test-dma-probe") {
        if let Err(err) = test::dma_diagnostics::run_dma_probe() {
            eprintln!("Audio DMA probe failed: {}", err);
        }
        std::process::exit(0);
    }
    if args.iter().any(|arg| arg == "--test-dma-continuity") {
        if let Err(err) = test::dma_diagnostics::run_dma_continuity() {
            eprintln!("DMA continuity test failed: {}", err);
        }
        std::process::exit(0);
    }
    if args.iter().any(|arg| arg == "--test-dma-carrier-offset") {
        if let Err(err) = test::dma_diagnostics::run_carrier_offset_probe(loopback) {
            eprintln!("Carrier offset probe failed: {}", err);
        }
        std::process::exit(0);
    }

    // --- Narrowband & Low Visual Span Tests ---
    if let Some(pos) = args.iter().position(|arg| arg == "--test-narrowband-rx") {
        let rate_hz: i64 = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(768_000);
        let secs: f32 = args
            .get(pos + 2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0);
        if let Err(err) = test::narrowband::run_narrowband_rx(rate_hz, secs) {
            eprintln!("Narrowband RX test failed: {}", err);
        }
        std::process::exit(0);
    }
    if let Some(pos) = args
        .iter()
        .position(|arg| arg == "--test-narrowband-loopback")
    {
        let rate_hz: i64 = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(768_000);
        let secs: f32 = args
            .get(pos + 2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0);
        if let Err(err) = test::narrowband::run_narrowband_loopback(rate_hz, secs) {
            eprintln!("Narrowband SSB loopback failed: {}", err);
        }
        std::process::exit(0);
    }

    // --- Live Broadcast Quality Metrics ---
    if let Some(pos) = args
        .iter()
        .position(|arg| arg == "--test-fm-broadcast-quality")
    {
        let station_hz: i64 = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                eprintln!(
                    "Usage: --test-fm-broadcast-quality <station_hz> <duration_s> [out_prefix]"
                );
                std::process::exit(1);
            });
        let duration_s: f32 = args
            .get(pos + 2)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                eprintln!(
                    "Usage: --test-fm-broadcast-quality <station_hz> <duration_s> [out_prefix]"
                );
                std::process::exit(1);
            });
        let out_prefix = args.get(pos + 3).map(|s| s.as_str());
        if let Err(err) =
            test::fm_broadcast::run_fm_broadcast_quality(station_hz, duration_s, out_prefix)
        {
            eprintln!("FM broadcast quality test failed: {}", err);
        }
        std::process::exit(0);
    }

    // --- Spectral Purity & Sweeps ---
    if let Some(pos) = args.iter().position(|arg| arg == "--test-spec-audio-sweep") {
        let tone_hz: f32 = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000.0);
        let duration_s: f32 = args
            .get(pos + 2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(2.5);
        let save_wavs = args.iter().any(|a| a == "--save");
        if let Err(err) =
            test::spectral_analysis::run_spec_audio_sweep(tone_hz, duration_s, save_wavs)
        {
            eprintln!("Spectral audio sweep failed: {}", err);
        }
        std::process::exit(0);
    }

    if args.iter().any(|arg| arg == "--test-spec-tx-shape") {
        if let Err(err) = test::spectral_analysis::run_spec_tx_shape(loopback) {
            eprintln!("Spectral TX shape test failed: {}", err);
        }
        std::process::exit(0);
    }
    if args.iter().any(|arg| arg == "--test-spec-tx-wideband") {
        if let Err(err) = test::spectral_analysis::run_spec_tx_wideband(loopback) {
            eprintln!("Spectral TX wideband test failed: {}", err);
        }
        std::process::exit(0);
    }
    if let Some(pos) = args.iter().position(|arg| arg == "--test-spur-probe") {
        let fs_hz: i64 = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(3_840_000);
        if let Err(err) = test::spectral_analysis::run_spur_probe(fs_hz) {
            eprintln!("Spur probe failed: {}", err);
        }
        std::process::exit(0);
    }

    // --- Pure Software DSP Verification ---
    if let Some(pos) = args
        .iter()
        .position(|arg| arg == "--test-soft-ssb-loopback")
    {
        let output = args.get(pos + 1).unwrap_or_else(|| {
            eprintln!("Usage: --test-soft-ssb-loopback <output.wav>");
            std::process::exit(1);
        });
        if let Err(err) = test::software_dsp::run_soft_ssb_loopback(output) {
            eprintln!("Software SSB loopback test failed: {}", err);
        }
        std::process::exit(0);
    }

    // --- Timing & Pacing Tests ---
    if let Some(pos) = args.iter().position(|arg| arg == "--test-pacing-dma-delay") {
        let rate_hz: Option<i64> = args.get(pos + 1).and_then(|s| s.parse().ok());
        if let Err(err) = test::timing_pacing::run_pacing_dma_delay(rate_hz) {
            eprintln!("Pacing DMA delay test failed: {}", err);
        }
        std::process::exit(0);
    }
}
