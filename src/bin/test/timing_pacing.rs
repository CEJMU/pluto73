use pluto::device::PlutoDevice;
use pluto::tx_dsp::{TxMode, TxModulator};
use std::thread;
use std::time::{Duration, Instant};

/// Measures the actual blocking behavior of TX write_buffer/push().
/// If push() returns in <1ms, the FPGA FIFO is NOT providing backpressure
/// and all our audio data is being lost to overflow.
/// If push() blocks for ~85ms, the DMA is properly paced.
pub fn run_pacing_dma_delay(rate_hz: Option<i64>) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== TX DMA TIMING TEST ===");
    println!("Measures how long write_buffer/push() actually blocks.\n");

    let pluto = PlutoDevice::open(16384, 4096).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(500));

    let mut tx = pluto.tx;
    let mut rx = pluto.rx;
    let mut system = pluto.system;

    let lo_hz: i64 = 900_000_000;
    let fs_hz: i64 = rate_hz.unwrap_or(3_840_000);
    let antenna: u8 = 0;

    let fir = fs_hz < pluto::device::AD9361_MIN_FS_NO_FIR;
    if fir {
        rx.set_bb_rate_fir(fs_hz)?;
    } else {
        rx.disable_bb_fir()?;
    }
    let fs_hz = rx.sampling_frequency;
    println!("Configured rate: {} Hz (FIR: {})", fs_hz, fir);

    system.tx_apply_dsp_config(tx.antenna, fs_hz as f64);

    tx.antenna = antenna;
    tx.set_frequencies(lo_hz, fs_hz)?;
    tx.set_rf_bandwidth(fs_hz)?;
    tx.set_gain(-20.0)?;
    tx.init_channels()?;

    let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
    let chunk_size = 4096;
    let iterations = 100;

    // Generate a 1 kHz tone
    let audio: Vec<f32> = (0..chunk_size)
        .map(|n| (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 48000.0).sin())
        .collect();

    println!("Expected: ~85ms per push (4096 samples at 48 kHz effective rate)");
    println!("If push takes <1ms, the FIFO is overflowing.\n");

    // Backpressure test: No sleep between pushes (raw speed)
    println!("--- Backpressure test: No sleep between pushes ---");
    {
        let mut push_times: Vec<f64> = Vec::with_capacity(iterations);
        let overall_start = Instant::now();

        for _ in 0..iterations {
            let mut out_i = Vec::new();
            let mut out_q = Vec::new();
            modulator.process_chunk(&audio, &mut out_i, &mut out_q);

            let push_start = Instant::now();
            tx.write_buffer(&out_i, &out_q)?;
            let push_elapsed = push_start.elapsed().as_secs_f64() * 1000.0;
            push_times.push(push_elapsed);
        }

        let overall_ms = overall_start.elapsed().as_secs_f64() * 1000.0;
        let expected_ms = iterations as f64 * 4096.0 / 48000.0 * 1000.0;

        push_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = push_times[0];
        let max = push_times[push_times.len() - 1];
        let median = push_times[push_times.len() / 2];
        let mean = push_times.iter().sum::<f64>() / push_times.len() as f64;

        println!(
            "  {} pushes in {:.1}ms (expected {:.1}ms for real-time)",
            iterations, overall_ms, expected_ms
        );
        println!(
            "  Per-push: min={:.2}ms  median={:.2}ms  mean={:.2}ms  max={:.2}ms",
            min, median, mean, max
        );

        if mean < 5.0 {
            println!("  >>> push() is NOT blocking - FIFO overflow confirmed! <<<");
        } else if mean > 50.0 {
            println!("  >>> push() IS blocking - backpressure is working <<<");
        } else {
            println!("  >>> push() partially blocks - some buffering <<<");
        }

        // Show distribution
        let under_1ms = push_times.iter().filter(|&&t| t < 1.0).count();
        let under_10ms = push_times.iter().filter(|&&t| t < 10.0).count();
        let over_50ms = push_times.iter().filter(|&&t| t > 50.0).count();
        let over_80ms = push_times.iter().filter(|&&t| t > 80.0).count();
        println!(
            "  Distribution: <1ms: {}  <10ms: {}  >50ms: {}  >80ms: {}",
            under_1ms, under_10ms, over_50ms, over_80ms
        );
    }

    // Startup behavior: First 20 pushes detailed (shows FIFO filling behavior)
    println!("\n--- Startup behavior: First 20 pushes detailed ---");
    {
        // Recreate modulator to reset state
        let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
        // Re-init TX to flush any queued data
        tx.release_channels();
        thread::sleep(Duration::from_millis(100));
        tx.init_channels()?;

        for i in 0..20 {
            let mut out_i = Vec::new();
            let mut out_q = Vec::new();
            modulator.process_chunk(&audio, &mut out_i, &mut out_q);

            let start = Instant::now();
            tx.write_buffer(&out_i, &out_q)?;
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

            println!("  Push {:2}: {:.2}ms", i, elapsed_ms);
        }
    }

    // Steady-state: Precise drain rate measurement.
    // Once push() is backpressured the FPGA drains continuously, so aggregate
    // throughput (total samples / wall time) = the true drain rate, independent of
    // per-push CPU/copy overhead. Used to pin down l_clk and the correct /N divisor.
    println!("\n--- Steady-state: Drain rate measurement ---");
    {
        let mut modulator = TxModulator::new(TxMode::USB, 3_000.0, fs_hz as f32);
        tx.release_channels();
        thread::sleep(Duration::from_millis(100));
        tx.init_channels()?;

        // Warm up: fill the FIFO so subsequent pushes are drain-limited.
        for _ in 0..10 {
            let mut oi = Vec::new();
            let mut oq = Vec::new();
            modulator.process_chunk(&audio, &mut oi, &mut oq);
            tx.write_buffer(&oi, &oq)?;
        }

        let measure_pushes = 300usize;
        let start = Instant::now();
        for _ in 0..measure_pushes {
            let mut oi = Vec::new();
            let mut oq = Vec::new();
            modulator.process_chunk(&audio, &mut oi, &mut oq);
            tx.write_buffer(&oi, &oq)?;
        }
        let secs = start.elapsed().as_secs_f64();
        let samples = (measure_pushes * chunk_size) as f64;
        let drain = samples / secs;
        let total_interp = (fs_hz as f64 / 48000.0).round().max(16.0).min(256.0);
        let cur_divisor = 5.0 / 2.0; // 2.5 divisor baked into the current bitstream's 2-in-5 strobe
        let l_clk = drain * cur_divisor * total_interp;
        let divisor_for_48k = cur_divisor * drain / 48000.0;

        println!(
            "  {} pushes x {} = {:.0} samples in {:.3}s",
            measure_pushes, chunk_size, samples, secs
        );
        println!("  drain rate      = {:.0} Hz  (target 48000)", drain);
        println!(
            "  implied l_clk   = {:.3} MHz  (= {:.2} x {:.3} MHz)",
            l_clk / 1e6,
            l_clk / fs_hz as f64,
            fs_hz as f64 / 1e6
        );
        println!(
            "  divisor for 48k = {:.2}  (current bitstream uses {:.1})",
            divisor_for_48k, cur_divisor
        );
    }

    let _ = tx.set_gain(-89.75);
    println!("\n=== TX TIMING TEST COMPLETE ===");
    Ok(())
}
