use log::error;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

use crate::device::{GainMode, PlutoRxDevice, PlutoSystem};
use crate::threads::network;
use crate::threads::tx_io::TxIoCommand;

pub enum IoCommand {
    SetCenterFrequency(i64),
    SetSpan {
        center_hz: i64,
        span_hz: i64,
    },
    SetAntenna(u8),
    SetTxState {
        active: bool,
        tx_gain_db: f64,
        playback_hz: i64,
        rx_lo_hz: i64,
    },
    SetRxGainMode(String),
    SetRxGain(f64),
    SetTxGain(f64),
    SetRfBandwidth(i64),
    /// Keeps the TX LO following the listening frequency while TX is active (e.g. on plain
    /// `SetRxFrequency` retunes, which otherwise never reach the TX thread). No-op if TX is off.
    SetTxPlaybackFrequency(i64),
}

pub fn spawn_rx_io_thread(
    mut device: PlutoRxDevice,
    shutdown_io: Arc<AtomicBool>,
    is_running_io: Arc<AtomicBool>,
    io_cmd_rx: std::sync::mpsc::Receiver<IoCommand>,
    config_tx: std::sync::mpsc::Sender<(i64, i64)>,
    iq_tx: std::sync::mpsc::SyncSender<(Vec<i16>, Vec<i16>)>,
    tx_io_cmd_tx: std::sync::mpsc::Sender<TxIoCommand>,
    system: Arc<Mutex<PlutoSystem>>,
    _tx_fs_atomic: Arc<AtomicU32>,
    status_messages_tx: broadcast::Sender<network::ServerMessage>,
    initial_fs_hz: i64,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // --- Thread state ---
        let mut is_tx_active = false;
        let mut current_tx_lo = 0i64;
        let mut last_telemetry_time = Instant::now();
        let mut actual_span = initial_fs_hz;

        // --- Telemetry + command + RX-read loop ---
        while !shutdown_io.load(Ordering::Relaxed) {
            if !is_running_io.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            // Emit telemetry every 2s.
            if last_telemetry_time.elapsed() >= Duration::from_secs(2) {
                last_telemetry_time = Instant::now();
                if let Ok((temp, vccint, vccoddr)) = device.read_telemetry() {
                    let _ = status_messages_tx.send(network::ServerMessage::Telemetry {
                        temp_c: temp,
                        vccint_v: vccint,
                        vccoddr_v: vccoddr,
                    });
                }
            }

            // Apply one pending config command.
            if let Ok(cmd) = io_cmd_rx.try_recv() {
                // Lightweight TX LO retune: skips the ConfigureStart/ConfigureEnd pause used below,
                // which briefly mutes/releases TX channels and would glitch a live transmission
                // every time the listening frequency moves.
                if let IoCommand::SetTxPlaybackFrequency(playback_hz) = cmd {
                    if is_tx_active {
                        current_tx_lo = playback_hz - 50_000;
                        let _ = tx_io_cmd_tx.send(TxIoCommand::SetTxFrequencies {
                            lo_hz: current_tx_lo,
                            fs_hz: actual_span,
                        });
                    }
                } else {
                    // Tell the TX IO thread to drop the TX buffer during clock changes
                    let _ = tx_io_cmd_tx.send(TxIoCommand::ConfigureStart);
                    thread::sleep(Duration::from_millis(15)); // Give TX thread a brief moment to release the DMA

                    match cmd {
                        IoCommand::SetCenterFrequency(new_lo) => {
                            match device.set_frequencies(new_lo, actual_span) {
                                Ok((actual_lo, fs)) => {
                                    let _ = config_tx.send((actual_lo, fs));
                                    if is_tx_active {
                                        let _ = tx_io_cmd_tx.send(TxIoCommand::SetTxFrequencies {
                                            lo_hz: current_tx_lo,
                                            fs_hz: actual_span,
                                        });
                                    }
                                }
                                Err(err) => {
                                    error!("[RX IO Error] Failed to set center frequency: {}", err);
                                }
                            }
                        }
                        IoCommand::SetSpan { center_hz, span_hz } => {
                            match device.set_frequencies(center_hz, span_hz) {
                                Ok((actual_lo, fs)) => {
                                    if let Err(err) = device.set_rf_bandwidth(span_hz) {
                                        error!(
                                            "[RX IO Error] Failed to set RF bandwidth to {} Hz: {}",
                                            span_hz, err
                                        );
                                    }
                                    if let Err(err) = device.init_channels() {
                                        error!(
                                            "[RX IO Error] Failed to initialize channels: {}",
                                            err
                                        );
                                    }
                                    actual_span = fs;
                                    let _ = config_tx.send((actual_lo, fs));
                                    if is_tx_active {
                                        let _ = tx_io_cmd_tx.send(TxIoCommand::SetTxFrequencies {
                                            lo_hz: current_tx_lo,
                                            fs_hz: fs,
                                        });
                                    }
                                }
                                Err(err) => {
                                    error!("[RX IO Error] Failed to set frequencies: {}", err);
                                }
                            }
                        }
                        IoCommand::SetAntenna(antenna) => {
                            match device.set_antenna(antenna) {
                                Ok(_) => {
                                    let _ = config_tx
                                        .send((device.frequency, device.sampling_frequency));
                                }
                                Err(err) => {
                                    error!("[RX IO Error] Failed to set antenna: {}", err);
                                }
                            }
                            {
                                let mut sys = system.lock().unwrap();
                                sys.rx_update_gpio_antenna(antenna);
                            }
                            let _ = tx_io_cmd_tx.send(TxIoCommand::SetAntenna(antenna));
                        }
                        IoCommand::SetTxState {
                            active,
                            tx_gain_db,
                            playback_hz,
                            rx_lo_hz,
                        } => {
                            is_tx_active = active;
                            if active {
                                // Apply the user's chosen TX gain before enabling the transmitter.
                                let _ = tx_io_cmd_tx.send(TxIoCommand::SetTxGain(tx_gain_db));

                                // TX LO is tuned 50 kHz below playback_hz. The FPGA TX DDS (fixed
                                // +50 kHz offset, see `tx_apply_dsp_config`) then shifts the
                                // modulated baseband back up by exactly that amount, so the
                                // transmitted signal lands precisely at playback_hz and the user
                                // can hear themselves when monitoring. TxModulator's own NCO is
                                // disabled (rf_offset_hz = 0.0, see tx_dsp.rs) to avoid double-shifting.
                                current_tx_lo = playback_hz - 50_000;
                                let _ = tx_io_cmd_tx.send(TxIoCommand::SetTxFrequencies {
                                    lo_hz: current_tx_lo,
                                    fs_hz: actual_span,
                                });
                                let _ = tx_io_cmd_tx.send(TxIoCommand::TxStart);
                            } else {
                                // Restore original TX LO frequency to match RX LO
                                let _ = tx_io_cmd_tx.send(TxIoCommand::SetTxFrequencies {
                                    lo_hz: rx_lo_hz,
                                    fs_hz: actual_span,
                                });
                                let _ = tx_io_cmd_tx.send(TxIoCommand::TxStop);
                            }
                        }
                        IoCommand::SetRxGainMode(mode) => {
                            if let Ok(g_mode) = mode.parse() {
                                if let Err(err) = device.set_gain(g_mode, None) {
                                    error!(
                                        "[RX IO Error] Failed to set RX gain mode to {}: {}",
                                        g_mode, err
                                    );
                                }
                            }
                        }
                        IoCommand::SetRxGain(db) => {
                            if let Err(err) = device.set_gain(GainMode::Manual, Some(db)) {
                                error!(
                                    "[RX IO Error] Failed to set manual RX gain to {} dB: {}",
                                    db, err
                                );
                            }
                        }
                        IoCommand::SetTxGain(db) => {
                            let _ = tx_io_cmd_tx.send(TxIoCommand::SetTxGain(db));
                        }
                        IoCommand::SetRfBandwidth(bw_hz) => {
                            if let Err(err) = device.set_rf_bandwidth(bw_hz) {
                                error!(
                                    "[RX IO Error] Failed to set RF bandwidth to {} Hz: {}",
                                    bw_hz, err
                                );
                            }
                        }
                        // Already handled
                        IoCommand::SetTxPlaybackFrequency(_) => unreachable!(),
                    }

                    // Tell the TX IO thread configuration is done and it can recreate the TX buffer
                    let _ = tx_io_cmd_tx.send(TxIoCommand::ConfigureEnd);
                }
            }


            if let Ok((i_samples, q_samples)) = device.read_buffer() {
                if iq_tx.send((i_samples, q_samples)).is_err() {
                    break;
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    })
}
