use log::{debug, error};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::device::{PlutoSystem, PlutoTxDevice};

/// Operational commands sent from the receiver control logic to manage the transmitter thread state.
pub enum TxIoCommand {
    /// Notify that the device is reconfiguring (temporarily release channels).
    ConfigureStart,
    /// Notify that configuration has completed (re-init channels if transmitter is active).
    ConfigureEnd,
    /// Initialize the transmitter channels and start streaming data.
    TxStart,
    /// Disable transmitter channels and drop the streaming buffer.
    TxStop,
    SetAntenna(u8),
    /// Sets TX LO, sample rate, and RF analog bandwidth.
    SetTxFrequencies {
        lo_hz: i64,
        fs_hz: i64,
    },
    SetTxGain(f64),
}

/// Spawns the low-level hardware transceiver thread dedicated to writing modulated I/Q samples
/// to the DMA transmission buffer and managing TX settings.
pub fn spawn_tx_io_thread(
    mut tx_device: PlutoTxDevice,
    shutdown_io: Arc<AtomicBool>,
    system: Arc<Mutex<PlutoSystem>>,
    tx_io_cmd_rx: std::sync::mpsc::Receiver<TxIoCommand>,
    tx_iq_rx: std::sync::mpsc::Receiver<(Vec<i16>, Vec<i16>)>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // --- Thread state ---
        let mut tx_write_count = 0u64;
        let mut is_configuring = false;
        let mut is_tx_active = false;
        let mut desired_gain = tx_device.gain;

        // Mute TX at startup (-89.75 dB) to prevent DC spike/noise leakage while inactive.
        if let Err(err) = tx_device.set_gain(-89.75) {
            error!("[TX IO Error] Failed to mute TX on startup: {}", err);
        }

        // --- Command + TX-write loop ---
        while !shutdown_io.load(Ordering::Relaxed) {
            // Drain all queued hardware/config commands.
            while let Ok(cmd) = tx_io_cmd_rx.try_recv() {
                match cmd {
                    TxIoCommand::ConfigureStart => {
                        is_configuring = true;
                        let _ = tx_device.set_gain(-89.75);
                        tx_device.release_channels();
                    }
                    TxIoCommand::ConfigureEnd => {
                        is_configuring = false;
                        if is_tx_active {
                            let _ = tx_device.set_gain(desired_gain);
                            if let Err(err) = tx_device.init_channels() {
                                error!(
                                    "[TX IO Error] Failed to re-initialize channels after configuration: {}",
                                    err
                                );
                            }
                        }
                    }
                    TxIoCommand::TxStart => {
                        debug!(
                            "[TX IO Debug] Command: TxStart. Initializing TX channels on hardware."
                        );
                        is_tx_active = true;
                        if let Err(err) = tx_device.set_gain(desired_gain) {
                            error!("[TX IO Error] Failed to restore TX gain on start: {}", err);
                        }
                        if !is_configuring {
                            if let Err(err) = tx_device.init_channels() {
                                error!("[TX IO Error] Failed to initialize TX channels: {}", err);
                            }
                        }
                    }
                    TxIoCommand::TxStop => {
                        debug!("[TX IO Debug] Command: TxStop. Releasing TX channels.");
                        is_tx_active = false;
                        if let Err(err) = tx_device.set_gain(-89.75) {
                            error!("[TX IO Error] Failed to mute TX on stop: {}", err);
                        }
                        tx_device.release_channels();
                    }
                    TxIoCommand::SetAntenna(antenna) => {
                        tx_device.antenna = antenna;
                        {
                            let mut sys = system.lock().unwrap();
                            sys.tx_update_gpio_antenna(antenna);
                        }
                        if is_tx_active && !is_configuring {
                            tx_device.release_channels();
                            let _ = tx_device.set_gain(desired_gain);
                            if let Err(err) = tx_device.init_channels() {
                                error!(
                                    "[TX IO Error] Failed to re-initialize TX channels after antenna switch: {}",
                                    err
                                );
                            }
                        } else {
                            let _ = tx_device.set_gain(-89.75);
                        }
                    }
                    TxIoCommand::SetTxGain(db) => {
                        debug!("[TX IO Debug] Command: SetTxGain ({} dB)", db);
                        desired_gain = db;
                        if is_tx_active {
                            if let Err(err) = tx_device.set_gain(db) {
                                error!(
                                    "[TX IO Error] Failed to set TX attenuation to {:.2} dB: {}",
                                    db, err
                                );
                            }
                        }
                    }
                    TxIoCommand::SetTxFrequencies { lo_hz, fs_hz } => {
                        debug!(
                            "[TX IO Debug] Command: SetTxFrequencies (LO={} Hz, rate={} Hz)",
                            lo_hz, fs_hz
                        );
                        if let Err(err) = tx_device.set_frequencies(lo_hz, fs_hz) {
                            error!(
                                "[TX IO Error] Failed to tune TX LO to {} Hz and rate {} Hz: {}",
                                lo_hz, fs_hz, err
                            );
                        }
                        if let Err(err) = tx_device.set_rf_bandwidth(fs_hz) {
                            error!(
                                "[TX IO Error] Failed to set TX RF bandwidth to {} Hz: {}",
                                fs_hz, err
                            );
                        }
                        {
                            let mut sys = system.lock().unwrap();
                            sys.tx_apply_dsp_config(tx_device.antenna, fs_hz as f64);
                        }
                    }
                }
            }

            // While inactive/configuring: drain incoming IQ to avoid backlog, then idle.
            if is_configuring || !is_tx_active {
                while let Ok(_) = tx_iq_rx.try_recv() {}
                thread::sleep(Duration::from_millis(10));
                continue;
            }

            // Write ONE chunk per outer iteration (not an inner drain loop) so the command queue is
            // re-checked between chunks - otherwise a continuous stream starves commands like
            // SetTxFrequencies until streaming pauses. Backpressure comes from the FPGA
            // `tx_sample_enable` strobe (push() blocks inside write_buffer), so no software pacing
            // is needed.
            match tx_iq_rx.try_recv() {
                Ok((i_tx, q_tx)) => {
                    tx_write_count += 1;
                    if tx_write_count == 1 {
                        debug!(
                            "[TX IO Debug] Wrote FIRST modulated I/Q buffer to Pluto TX DMA buffer ({} samples)",
                            i_tx.len()
                        );
                    }
                    if tx_write_count % 100 == 0 {
                        debug!(
                            "[TX IO Debug] Wrote {} chunks to Pluto TX DMA buffer ({} samples)",
                            tx_write_count,
                            i_tx.len()
                        );
                    }

                    let n = std::cmp::min(i_tx.len(), q_tx.len());
                    if let Err(err) = tx_device.write_buffer(&i_tx[..n], &q_tx[..n]) {
                        error!("[TX IO Error] Failed writing to TX DMA buffer: {}", err);
                    }
                }
                // Sleep briefly if no data was received to prevent busy-looping
                Err(_) => {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }

        // --- Shutdown ---
        // Mute TX to prevent leakage after exit.
        debug!("[TX IO Debug] Thread shutting down. Muting TX hardware.");
        let _ = tx_device.set_gain(-89.75);
    })
}
