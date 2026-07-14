pub mod device;
pub mod dsp;
pub mod state;

pub mod threads;

pub use crate::dsp::tx as tx_dsp;
pub use crate::threads::{audio, network, rx_io, tx_io};

use crate::device::{GainMode, PlutoDevice};
use crate::threads::tx_dsp::spawn_tx_dsp_thread;
use audio::{AudioConfig, spawn_audio_thread, update_audio_tuning};
use rx_io::{IoCommand, spawn_rx_io_thread};
use state::{ControlState, DemodMode};
use tx_io::{TxIoCommand, spawn_tx_io_thread};

use dsp::WaterfallProcessor;
use network::{ControlCommand, NetworkServer};
use num_complex::Complex32;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::{
    thread,
    time::{Duration, Instant},
};
use tokio::sync::broadcast;
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};

use crate::dsp::Demodulation;
use log::{debug, error, info, warn};

// Minimum hardware span required per modulation mode.
pub const MIN_SPAN_SSB: i64 = 768_000;
pub const MIN_SPAN_FM: i64 = 3_840_000;
// Maximum span supported by the AD9361 driver.
pub const MAX_SPAN: i64 = 30_720_000;

fn get_min_span(mode: DemodMode) -> i64 {
    match mode {
        DemodMode::FM => MIN_SPAN_FM,
        DemodMode::USB | DemodMode::LSB => MIN_SPAN_SSB,
    }
}

/// Reads `<flag> <N>` from the CLI args, falling back to `default` if the flag is absent or its
/// value is missing/invalid.
fn parse_port_arg(args: &[String], flag: &str, default: u16) -> u16 {
    match args.iter().position(|a| a == flag) {
        Some(i) => match args.get(i + 1).and_then(|s| s.parse::<u16>().ok()) {
            Some(port) => port,
            None => {
                error!("{flag} requires a valid port number (1-65535); falling back to {default}");
                default
            }
        },
        None => default,
    }
}

pub const WATERFALL_DMA_SIZE: usize = 16384; // Must match FPGA BURST_LEN
pub const WATERFALL_FFT_SIZE: usize = 8192;
pub const TX_DMA_SIZE: usize = 4096;

pub const MIN_TX_GAIN_DB: f64 = -89.75;

pub async fn run_server() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Default to `info`, but quiet warp's own startup chatter
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,warp::server=warn"),
    )
    .init();

    let args: Vec<String> = std::env::args().collect();

    // Optional `--port <N>` / `--tls-port <N>` override the default HTTP (8080) and HTTPS (443)
    // ports. HTTP defaults to 8080 because the device firmware's own httpd already owns port 80.
    let http_port = parse_port_arg(&args, "--port", 8080);
    let https_port = parse_port_arg(&args, "--tls-port", 443);

    info!("Starting Pluto SDR backend");

    // Channels bridging the async network server and the blocking device loop.
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (tx_audio_tx, tx_audio_rx) = mpsc::unbounded_channel::<Vec<f32>>();
    let (rx_waterfall_tx, _) = broadcast::channel(64);
    let (rx_audio_tx, _) = broadcast::channel(256);
    // Optional raw I/Q stream. Small ring: each frame is a full wideband burst (tens of KB), and a
    // lagging client is dropped (`Lagged`) rather than allowed to buffer unbounded memory.
    let (rx_iq_stream_tx, _) = broadcast::channel(8);
    let (status_messages_tx, _) = broadcast::channel(16);

    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let network_shutdown = shutdown_flag.clone();

    // Network server (async task).
    let network_server = NetworkServer::new(
        http_port,
        https_port,
        control_tx,
        tx_audio_tx,
        rx_waterfall_tx.clone(),
        rx_audio_tx.clone(),
        rx_iq_stream_tx.clone(),
        status_messages_tx.clone(),
    );

    let network_handle = tokio::spawn(async move {
        if let Err(err) = network_server.run().await {
            error!("Network server error: {err}");
            network_shutdown.store(true, Ordering::Relaxed);
        }
    });

    // Device loop (blocking task): owns the radio and all DSP/IO threads.
    let device_shutdown = shutdown_flag.clone();
    let device_handle = tokio::task::spawn_blocking(move || {
        if let Err(e) = run_device_loop(
            device_shutdown,
            control_rx,
            tx_audio_rx,
            rx_waterfall_tx,
            rx_audio_tx,
            rx_iq_stream_tx,
            status_messages_tx,
        ) {
            error!("Device loop error: {}", e);
        }
    });

    // Wait for a shutdown signal, then stop cleanly.
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sighup = signal(SignalKind::hangup())?;

    tokio::select! {
        _ = sigterm.recv() => info!("Received SIGTERM"),
        _ = sigint.recv() => info!("Received SIGINT"),
        _ = sighup.recv() => info!("Received SIGHUP"),
        _ = tokio::signal::ctrl_c() => info!("Received Ctrl-C"),
    }

    shutdown_flag.store(true, Ordering::Relaxed);
    network_handle.abort();

    // Brief window for the device loop to reset the FPGA GPIO before force-exit.
    let _ = tokio::time::timeout(Duration::from_secs(2), device_handle).await;

    std::process::exit(0);
}

fn run_device_loop(
    shutdown_flag: Arc<AtomicBool>,
    mut control_rx: mpsc::UnboundedReceiver<ControlCommand>,
    tx_audio_rx: mpsc::UnboundedReceiver<Vec<f32>>,
    rx_waterfall_tx: broadcast::Sender<Vec<u8>>,
    rx_audio_tx: broadcast::Sender<Vec<f32>>,
    rx_iq_stream_tx: broadcast::Sender<Vec<u8>>,
    status_messages_tx: broadcast::Sender<network::ServerMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // --- Startup configuration ---
    let initial_fs_hz = MIN_SPAN_FM;
    let initial_bw_hz = MIN_SPAN_FM;
    let offset_hz: i64 = MIN_SPAN_FM / 4;
    let initial_lo_hz = 99_300_000 - offset_hz;
    let initial_antenna = 0;

    let mut state = ControlState::new(99_300_000);

    // --- Open and configure the radio ---
    let pluto =
        PlutoDevice::open(WATERFALL_DMA_SIZE, TX_DMA_SIZE).map_err(|err| err.to_string())?;
    thread::sleep(Duration::from_millis(500));

    let mut device = pluto.rx;
    let mut tx_device = pluto.tx;
    let mut system_device = pluto.system;

    device.disable_bb_fir().ok();
    device.sampling_frequency = initial_fs_hz;

    system_device.rx_apply_dsp_config(initial_antenna, initial_fs_hz);
    system_device.tx_apply_dsp_config(tx_device.antenna, initial_fs_hz as f64);
    system_device.reset_audio_dma_controller();

    let system = Arc::new(Mutex::new(system_device));

    device
        .set_antenna(initial_antenna)
        .map_err(|err| format!("Config error: {}", err))?;
    device
        .set_frequencies(initial_lo_hz, initial_fs_hz)
        .map_err(|err| format!("Config error: {}", err))?;
    device
        .set_rf_bandwidth(initial_bw_hz)
        .map_err(|err| format!("Config error: {}", err))?;
    device
        .init_channels()
        .map_err(|err| format!("Failed to init channels: {}", err))?;
    tx_device
        .set_gain(MIN_TX_GAIN_DB)
        .map_err(|err| format!("Failed to apply safe TX gain: {}", err))?;

    // --- Processing state ---
    let mut fft = WaterfallProcessor::new(WATERFALL_FFT_SIZE);
    let mut last_waterfall_time = Instant::now();

    let mut current_lo_hz = initial_lo_hz;
    let mut current_fs_hz = initial_fs_hz;
    // In-flight hardware reconfigs awaiting confirmation from the RX IO thread.
    let mut pending_configs: usize = 0;
    let mut pending_dsp_reset: usize = 0;

    let audio_config = Arc::new(Mutex::new(AudioConfig {
        enabled: false,
        demod_mode: Demodulation::FM {
            audio_fs: 240_000.0,
            dev_hz: 75_000.0,
            audio_bw_hz: 120_000.0,
        },
        if_cutoff_hz: 120_000.0,
        fs_hz: initial_fs_hz,
        is_configuring: false,
    }));

    info!("Device loop initialized");

    // --- Inter-thread channels and shared state ---
    let is_running = Arc::new(AtomicBool::new(true));
    let is_running_io = is_running.clone();
    let shutdown_io = shutdown_flag.clone();
    let shutdown_audio = shutdown_flag.clone();

    // RX: raw IQ, hardware -> main (waterfall).
    let (rx_iq_tx, rx_iq_rx) = std::sync::mpsc::sync_channel::<(Vec<i16>, Vec<i16>)>(128);
    // RX: config commands, main -> hardware.
    let (rx_io_cmd_tx, rx_io_cmd_rx) = std::sync::mpsc::channel::<IoCommand>();
    // RX: confirmed LO/sample-rate, hardware -> main.
    let (rx_actual_config_tx, rx_actual_config_rx) = std::sync::mpsc::channel::<(i64, i64)>();

    // TX: modulated IQ, DSP -> hardware.
    let (tx_iq_tx, tx_iq_rx) = std::sync::mpsc::sync_channel::<(Vec<i16>, Vec<i16>)>(5);
    // TX: tuning commands, rx-io -> tx-io.
    let (tx_io_cmd_tx, tx_io_cmd_rx) = std::sync::mpsc::channel::<TxIoCommand>();
    // TX: modulator config, shared with the TX DSP thread.
    let tx_config = Arc::new(std::sync::Mutex::new(tx_dsp::TxConfig {
        mode: tx_dsp::TxMode::USB,
        filter_bw: 3_000.0,
        active: false,
    }));
    // Sample rate driving the transmitter and FPGA DUC.
    let tx_fs_atomic = Arc::new(AtomicU32::new(initial_fs_hz as u32));

    // --- Spawn worker threads ---
    let _tx_dsp_thread = spawn_tx_dsp_thread(
        shutdown_flag.clone(),
        tx_audio_rx,
        tx_iq_tx,
        tx_fs_atomic.clone(),
        tx_config.clone(),
    );

    update_audio_tuning(
        state.playback_hz,
        current_lo_hz,
        current_fs_hz,
        state.demod_mode,
        state.filter_bw,
        &system,
        &audio_config,
    );
    let _audio_thread = spawn_audio_thread(
        shutdown_audio,
        is_running.clone(),
        audio_config.clone(),
        system.clone(),
        rx_audio_tx.clone(),
    );

    // TX IO thread: writes IQ into the TX DMA buffers.
    let _tx_io_thread = spawn_tx_io_thread(
        tx_device,
        shutdown_io.clone(),
        system.clone(),
        tx_io_cmd_rx,
        tx_iq_rx,
    );

    // RX IO thread: reads raw RX DMA buffers and telemetry.
    let _rx_io_thread = spawn_rx_io_thread(
        device,
        shutdown_io,
        is_running_io,
        rx_io_cmd_rx,
        rx_actual_config_tx,
        rx_iq_tx,

        tx_io_cmd_tx.clone(),
        system.clone(),
        tx_fs_atomic.clone(),
        status_messages_tx.clone(),
        initial_fs_hz,
    );

    // --- Main control loop ---
    while !shutdown_flag.load(Ordering::Relaxed) {
        // Handle control commands from the network.
        while let Ok(command) = control_rx.try_recv() {
            debug!("Control command: {:?}", command);

            match command {
                ControlCommand::Start => {
                    state.is_running = true;
                    is_running.store(true, Ordering::Relaxed);
                }
                ControlCommand::Stop => {
                    state.is_running = false;
                    is_running.store(false, Ordering::Relaxed);
                    // Deactivate and stop the TX processing path
                    {
                        let mut cfg = tx_config.lock().unwrap();
                        cfg.active = false;
                    }
                    let _ = rx_io_cmd_tx.send(IoCommand::SetTxState {
                        active: false,
                        tx_gain_db: MIN_TX_GAIN_DB, // Safe minimum gain
                        playback_hz: state.playback_hz,
                        rx_lo_hz: current_lo_hz,
                    });
                }
                ControlCommand::SetRxFrequency { hz } => {
                    state.playback_hz = hz as i64;

                    update_audio_tuning(
                        state.playback_hz,
                        current_lo_hz,
                        current_fs_hz,
                        state.demod_mode,
                        state.filter_bw,
                        &system,
                        &audio_config,
                    );

                    // Keep the TX LO following the listening frequency while transmitting;
                    // otherwise TX stays at the frequency active when it last started.
                    if tx_config.lock().unwrap().active {
                        let _ =
                            rx_io_cmd_tx.send(IoCommand::SetTxPlaybackFrequency(state.playback_hz));
                    }
                }
                ControlCommand::SetRxCenterFrequency { hz } => {
                    current_lo_hz = hz as i64;
                    pending_configs += 1;

                    {
                        let mut sys = system.lock().unwrap();
                        sys.is_configuring = true;
                    }
                    {
                        let mut cfg = audio_config.lock().unwrap();
                        cfg.is_configuring = true;
                    }
                    let _ = rx_io_cmd_tx.send(IoCommand::SetCenterFrequency(current_lo_hz));
                }
                ControlCommand::SetRxSpan { center_hz, span_hz } => {
                    let requested_span = span_hz as i64;
                    state.visual_span_hz = requested_span;

                    let target_hardware_span = requested_span;

                    let is_ssb =
                        state.demod_mode == DemodMode::USB || state.demod_mode == DemodMode::LSB;
                    let mut rounded_span = MAX_SPAN;
                    let spans = if is_ssb {
                        vec![768_000, 1_536_000, MIN_SPAN_FM, 7_680_000, 15_360_000]
                    } else {
                        vec![MIN_SPAN_FM, 7_680_000, 15_360_000]
                    };
                    for span in spans {
                        if target_hardware_span <= span {
                            rounded_span = span;
                            break;
                        }
                    }
                    pending_configs += 1;
                    pending_dsp_reset += 1;
                    {
                        let mut sys = system.lock().unwrap();
                        sys.is_configuring = true;
                    }
                    {
                        let mut cfg = audio_config.lock().unwrap();
                        cfg.is_configuring = true;
                    }
                    let _ = rx_io_cmd_tx.send(IoCommand::SetSpan {
                        center_hz: center_hz as i64,
                        span_hz: rounded_span,
                    });
                }
                ControlCommand::SetRxAudioEnabled { enabled } => {
                    state.audio_enabled = enabled;
                    let mut cfg = audio_config.lock().unwrap();
                    cfg.enabled = enabled;
                }
                ControlCommand::SetRxDemodulation { mode, filter_bw_hz } => {
                    state.demod_mode = mode.parse().unwrap_or(DemodMode::FM);
                    state.filter_bw = filter_bw_hz;

                    let min_span = get_min_span(state.demod_mode);
                    if current_fs_hz < min_span {
                        pending_configs += 1;
                        pending_dsp_reset += 1;
                        {
                            let mut sys = system.lock().unwrap();
                            sys.is_configuring = true;
                        }
                        {
                            let mut cfg = audio_config.lock().unwrap();
                            cfg.is_configuring = true;
                        }
                        let _ = rx_io_cmd_tx.send(IoCommand::SetSpan {
                            center_hz: current_lo_hz,
                            span_hz: min_span,
                        });
                    }

                    let _ = status_messages_tx.send(network::ServerMessage::Config {
                        lo_hz: current_lo_hz,
                        sample_rate_hz: current_fs_hz,
                        min_span_hz: min_span,
                    });

                    update_audio_tuning(
                        state.playback_hz,
                        current_lo_hz,
                        current_fs_hz,
                        state.demod_mode,
                        state.filter_bw,
                        &system,
                        &audio_config,
                    );
                }
                ControlCommand::SetRxWaterfallInterval { ms } => {
                    state.waterfall_interval_ms = ms;
                }
                ControlCommand::SetRxIqStream { enabled } => {
                    state.iq_stream_enabled = enabled;
                }
                ControlCommand::SetRxAntenna { antenna } => {
                    pending_configs += 1;
                    pending_dsp_reset += 1;
                    {
                        let mut sys = system.lock().unwrap();
                        sys.is_configuring = true;
                    }
                    {
                        let mut cfg = audio_config.lock().unwrap();
                        cfg.is_configuring = true;
                    }
                    let _ = rx_io_cmd_tx.send(IoCommand::SetAntenna(antenna));
                }
                ControlCommand::SetTxState { active, tx_gain_db } => {
                    {
                        let mut cfg = tx_config.lock().unwrap();
                        cfg.active = active;
                    }
                    let _ = rx_io_cmd_tx.send(IoCommand::SetTxState {
                        active,
                        tx_gain_db,
                        playback_hz: state.playback_hz,
                        rx_lo_hz: current_lo_hz,
                    });
                }
                ControlCommand::SetTxModulation { mode, filter_bw_hz } => {
                    let new_mode = match mode.as_str() {
                        "USB" => tx_dsp::TxMode::USB,
                        "LSB" => tx_dsp::TxMode::LSB,
                        _ => tx_dsp::TxMode::USB,
                    };
                    let mut cfg = tx_config.lock().unwrap();
                    cfg.mode = new_mode;
                    cfg.filter_bw = filter_bw_hz;
                }
                ControlCommand::SetRxGainMode { mode } => {
                    state.rx_gain_mode = mode.parse().unwrap_or(GainMode::AgcSlow);
                    let _ = rx_io_cmd_tx.send(IoCommand::SetRxGainMode(mode));
                }
                ControlCommand::SetRxGain { db } => {
                    state.rx_gain_db = db;
                    let _ = rx_io_cmd_tx.send(IoCommand::SetRxGain(db));
                }
                ControlCommand::SetTxGain { db } => {
                    let _ = rx_io_cmd_tx.send(IoCommand::SetTxGain(db));
                }
                ControlCommand::SetRxRfBandwidth { bw_hz } => {
                    state.rf_bandwidth_hz = bw_hz;
                    let _ = rx_io_cmd_tx.send(IoCommand::SetRfBandwidth(bw_hz));
                }
                ControlCommand::SetRxWaterfallScale { min_db, max_db } => {
                    fft.min_db = min_db;
                    fft.max_db = max_db;
                }
                ControlCommand::SetRxWaterfallFftSize { size } => {
                    // FFT size = frequency resolution. Must be a power of two (fast FFT path)
                    // and fit within the burst length (that many samples are captured).
                    if size.is_power_of_two() && (256..=WATERFALL_DMA_SIZE).contains(&size) {
                        if size != fft.fft_size() {
                            let (min_db, max_db) = (fft.min_db, fft.max_db);
                            fft = WaterfallProcessor::new(size);
                            fft.min_db = min_db;
                            fft.max_db = max_db;
                        }
                    } else {
                        warn!("Ignoring invalid waterfall FFT size: {}", size);
                    }
                }
                ControlCommand::RequestSync => {
                    let min_span = get_min_span(state.demod_mode);
                    let _ = status_messages_tx.send(network::ServerMessage::Config {
                        lo_hz: current_lo_hz,
                        sample_rate_hz: current_fs_hz,
                        min_span_hz: min_span,
                    });
                    let _ = status_messages_tx.send(network::ServerMessage::Settings {
                        playback_hz: state.playback_hz,
                        demod_mode: state.demod_mode.to_string(),
                        filter_bw_hz: state.filter_bw,
                        audio_enabled: state.audio_enabled,
                        waterfall_interval_ms: state.waterfall_interval_ms,
                        rx_gain_mode: state.rx_gain_mode.to_string(),
                        rx_gain_db: state.rx_gain_db,
                        rf_bandwidth_hz: state.rf_bandwidth_hz,
                        waterfall_min_db: fft.min_db,
                        waterfall_max_db: fft.max_db,
                        waterfall_fft_size: fft.fft_size(),
                    });
                }
            }
        }

        // While stopped: recycle stale RX buffers (unblocking the IO thread) and idle.
        if !state.is_running {
            thread::sleep(Duration::from_millis(100));
            while let Ok(_) = rx_iq_rx.try_recv() {}
            continue;
        }

        // Apply confirmed LO/sample-rate updates reported back by the RX IO thread.
        while let Ok((actual_lo, actual_fs)) = rx_actual_config_rx.try_recv() {
            current_lo_hz = actual_lo;
            current_fs_hz = actual_fs;
            tx_fs_atomic.store(actual_fs as u32, Ordering::Relaxed);

            {
                let mut cfg = audio_config.lock().unwrap();
                cfg.fs_hz = actual_fs;
            }

            if pending_configs > 0 {
                pending_configs -= 1;
            }
            if pending_dsp_reset > 0 {
                pending_dsp_reset -= 1;
            }

            let min_span = get_min_span(state.demod_mode);
            let _ = status_messages_tx.send(network::ServerMessage::Config {
                lo_hz: actual_lo,
                sample_rate_hz: actual_fs,
                min_span_hz: min_span,
            });

            // Flush stale buffers so we don't display old frequencies.
            while let Ok(_) = rx_iq_rx.try_recv() {}

            if pending_configs == 0 && pending_dsp_reset == 0 {
                let mut sys = system.lock().unwrap();
                // Re-apply DSP config for the new rate, keeping the stored antenna.
                let rx_antenna = sys.rx_antenna;
                sys.rx_apply_dsp_config(rx_antenna, actual_fs);
                // Round to what the FPGA DUC is actually clocked at.
                let rounded_tx_fs = (current_fs_hz as f64 / 192000.0).round() * 192000.0;
                tx_fs_atomic.store(rounded_tx_fs as u32, Ordering::Relaxed);
                let _ = tx_io_cmd_tx.send(TxIoCommand::SetTxFrequencies {
                    lo_hz: state.playback_hz - 50_000,
                    fs_hz: current_fs_hz,
                });
                sys.reset_audio_dma_controller();
                sys.is_configuring = false;

                let mut cfg = audio_config.lock().unwrap();
                cfg.is_configuring = false;
            }

            if pending_configs == 0 {
                update_audio_tuning(
                    state.playback_hz,
                    current_lo_hz,
                    current_fs_hz,
                    state.demod_mode,
                    state.filter_bw,
                    &system,
                    &audio_config,
                );
            }
        }

        // Waterfall: once per interval, trigger a burst and emit one FFT row.
        let elapsed_since_last = last_waterfall_time.elapsed();
        if elapsed_since_last >= Duration::from_millis(state.waterfall_interval_ms) {
            // Recycle the backlog first: the waterfall only needs the freshest snapshot, and the
            // 128-deep channel would otherwise make the displayed row seconds old.
            while let Ok(_) = rx_iq_rx.try_recv() {}

            // Trigger one wideband burst (antenna/decimation from PlutoSystem state).
            {
                let mut sys = system.lock().unwrap();
                sys.trigger_waterfall_burst();
            }

            // Wait for the burst data from the IO thread.
            if let Ok((i, q)) = rx_iq_rx.recv_timeout(Duration::from_millis(500)) {
                let n = std::cmp::min(i.len(), q.len());
                let wf_fft_size = fft.fft_size();
                let mut wf_iq = Vec::with_capacity(wf_fft_size);
                let scale = 1.0 / 32768.0;
                for idx in 0..std::cmp::min(wf_fft_size, n) {
                    let i_wf = i[idx] as f32 * scale;
                    let q_wf = q[idx] as f32 * scale;
                    wf_iq.push(Complex32::new(i_wf, q_wf));
                }

                let fft_start = Instant::now();
                let row = fft.process_frame(&wf_iq);
                let fft_duration = fft_start.elapsed();

                if state.waterfall_interval_ms > 0
                    && fft_duration >= Duration::from_millis(state.waterfall_interval_ms)
                {
                    warn!(
                        "Waterfall FFT calculation is too slow! Took {}ms (Target interval: {}ms)",
                        fft_duration.as_millis(),
                        state.waterfall_interval_ms
                    );
                }

                let _ = rx_waterfall_tx.send(row);
                last_waterfall_time = Instant::now();

                // Optional raw I/Q stream: forward the same wideband burst as interleaved i16 LE
                // (I0,Q0,I1,Q1,...). Serialized only when a frontend has opted in, so the normal
                // path pays nothing. Clients read sample-rate/LO from the `Config` message.
                if state.iq_stream_enabled {
                    let mut iq_bytes = Vec::with_capacity(n * 4);
                    for idx in 0..n {
                        iq_bytes.extend_from_slice(&i[idx].to_le_bytes());
                        iq_bytes.extend_from_slice(&q[idx].to_le_bytes());
                    }
                    let _ = rx_iq_stream_tx.send(iq_bytes);
                }

            }
        } else {
            // Avoid busy-looping until the next waterfall interval.
            thread::sleep(Duration::from_millis(2));
        }
    }

    // --- Shutdown ---
    // Restore FPGA GPIO to reset state so a stale CIC rate / antenna / unreset core doesn't persist into the next run.
    system.lock().unwrap().reset_gpio_to_default();
    info!("FPGA GPIO reset to default state");

    Ok(())
}
