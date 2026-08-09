use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tokio::sync::{broadcast, mpsc};
use warp::filters::ws::{Message, WebSocket};
use warp::http::StatusCode;
use warp::{Filter, Rejection, Reply, ws::Ws};

/// Binary WebSocket frame layout, shared by every stream. Bytes:
/// 0: frame type,
/// 1-3: are reserved/zero,
/// HEADER_BYTES: payload start
pub mod msg_header {
    pub const HEADER_BYTES: usize = 4;
    /// Server -> client: one u8-per-bin waterfall row.
    pub const WATERFALL: u8 = 0;
    /// Server -> client: demodulated RX audio, f32 LE PCM.
    pub const AUDIO: u8 = 1;
    /// Client -> server: TX audio, f32 LE PCM.
    pub const TX_AUDIO: u8 = 2;
    /// Server -> client: raw interleaved i16 LE I/Q (opt-in via SetRxIqStream).
    pub const IQ: u8 = 3;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ControlCommand {
    Start,
    Stop,
    SetRxFrequency {
        hz: u64,
    },
    SetRxCenterFrequency {
        hz: u64,
    },
    /// `seq` is a client-chosen sequence number echoed back as `request_id` in the `Config` that reports this span change applied, so the client can match request to acknowledgement.
    SetRxSpan {
        center_hz: u64,
        span_hz: u64,
        #[serde(default)]
        request_id: u64,
    },
    SetRxAudioEnabled {
        enabled: bool,
    },
    SetRxDemodulation {
        mode: String,
        filter_bw_hz: f32,
    },
    SetRxWaterfallInterval {
        ms: u64,
    },
    SetRxAntenna {
        antenna: u8,
    },
    SetTxState {
        active: bool,
        tx_gain_db: f64,
    },
    SetTxModulation {
        mode: String,
        filter_bw_hz: f32,
    },
    SetTxOffset {
        hz: i64,
    },
    SetRxGainMode {
        mode: String,
    },
    SetRxGain {
        db: f64,
    },
    SetTxGain {
        db: f64,
    },
    SetRxRfBandwidth {
        bw_hz: i64,
    },
    SetRxWaterfallScale {
        min_db: f32,
        max_db: f32,
    },
    SetRxWaterfallFftSize {
        size: usize,
    },
    /// Toggle the optional raw I/Q stream (binary header type 3). Off by default; intended for
    /// alternative frontends that run their own DSP on the wideband capture.
    SetRxIqStream {
        enabled: bool,
    },
    SetRxSquelch {
        threshold_db: f32,
    },
    RequestSync,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "payload")]
pub enum ServerMessage {
    Status {
        state: String,
    },
    /// Reports the actual hardware operating point after a (re)tune
    Config {
        lo_hz: i64,
        sample_rate_hz: i64,
        min_span_hz: i64,
        rf_bandwidth_hz: i64,
        /// Seq of the last completed SetRxSpan (0 before the first).
        request_id: u64,
        audio_sample_rate_hz: u32,
    },
    Telemetry {
        temp_c: f32,
        vccint_v: f32,
        vccoddr_v: f32,
    },
    RxGain {
        gain_db: f64,
        mode: String,
    },
    Settings {
        playback_hz: i64,
        demod_mode: String,
        filter_bw_hz: f32,
        audio_enabled: bool,
        waterfall_interval_ms: u64,
        rx_gain_mode: String,
        rx_gain_db: f64,
        tx_offset_hz: i64,
        waterfall_min_db: f32,
        waterfall_max_db: f32,
        waterfall_fft_size: usize,
        antenna: u8,
        rx_squelch_db: f32,
    },
}

#[derive(Debug, Clone)]
pub struct NetworkServer {
    http_port: u16,
    https_port: u16,
    control_tx: mpsc::UnboundedSender<ControlCommand>,
    tx_audio_tx: mpsc::UnboundedSender<Vec<f32>>,
    rx_waterfall_tx: broadcast::Sender<Vec<u8>>,
    rx_audio_tx: broadcast::Sender<Vec<f32>>,
    rx_iq_stream_tx: broadcast::Sender<Vec<u8>>,
    status_messages_tx: broadcast::Sender<ServerMessage>,
}

impl NetworkServer {
    pub fn new(
        http_port: u16,
        https_port: u16,
        control_tx: mpsc::UnboundedSender<ControlCommand>,
        tx_audio_tx: mpsc::UnboundedSender<Vec<f32>>,
        rx_waterfall_tx: broadcast::Sender<Vec<u8>>,
        rx_audio_tx: broadcast::Sender<Vec<f32>>,
        rx_iq_stream_tx: broadcast::Sender<Vec<u8>>,
        status_messages_tx: broadcast::Sender<ServerMessage>,
    ) -> Self {
        Self {
            http_port,
            https_port,
            control_tx,
            tx_audio_tx,
            rx_waterfall_tx,
            rx_audio_tx,
            rx_iq_stream_tx,
            status_messages_tx,
        }
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // --- Routes ---
        let control_tx = self.control_tx.clone();
        let tx_audio_tx = self.tx_audio_tx.clone();
        let rx_waterfall_tx = self.rx_waterfall_tx.clone();
        let rx_audio_tx = self.rx_audio_tx.clone();
        let rx_iq_stream_tx = self.rx_iq_stream_tx.clone();
        let status_messages_tx = self.status_messages_tx.clone();

        let ws_route = warp::path("ws")
            .and(warp::ws())
            .and(warp::any().map(move || control_tx.clone()))
            .and(warp::any().map(move || tx_audio_tx.clone()))
            .and(warp::any().map(move || rx_waterfall_tx.clone()))
            .and(warp::any().map(move || rx_audio_tx.clone()))
            .and(warp::any().map(move || rx_iq_stream_tx.clone()))
            .and(warp::any().map(move || status_messages_tx.clone()))
            .map(
                |ws: Ws,
                 control_tx: mpsc::UnboundedSender<ControlCommand>,
                 tx_audio_tx: mpsc::UnboundedSender<Vec<f32>>,
                 rx_waterfall_tx: broadcast::Sender<Vec<u8>>,
                 rx_audio_tx: broadcast::Sender<Vec<f32>>,
                 rx_iq_stream_tx: broadcast::Sender<Vec<u8>>,
                 status_messages_tx: broadcast::Sender<ServerMessage>| {
                    ws.on_upgrade(move |socket| {
                        handle_ws_connection(
                            socket,
                            control_tx,
                            tx_audio_tx,
                            rx_waterfall_tx,
                            rx_audio_tx,
                            rx_iq_stream_tx,
                            status_messages_tx,
                        )
                    })
                    .into_response()
                },
            );

        // Serve the web UI from a `static/` directory next to the executable (files are read at
        // runtime rather than bundled into the binary). `warp::fs::dir` handles mime types and
        // serves `index.html` for directory requests.
        let static_route = warp::fs::dir(static_dir());

        let routes = ws_route.or(static_route).recover(handle_rejection).boxed();

        // --- Servers ---
        // Serve HTTP on `http_port` directly, and HTTPS on `https_port` when cert/key are present.
        let has_certs =
            std::path::Path::new("cert.pem").exists() && std::path::Path::new("key.pem").exists();

        let http_port = self.http_port;
        info!("Network server listening on http://0.0.0.0:{http_port}");
        let http_routes = routes.clone();
        let http_server = tokio::spawn(async move {
            warp::serve(http_routes)
                .run(([0, 0, 0, 0], http_port))
                .await;
        });

        let https_server = if has_certs {
            let https_port = self.https_port;
            info!("Secure HTTPS server listening on https://0.0.0.0:{https_port}");
            let https_routes = routes.clone();
            let handle = tokio::spawn(async move {
                warp::serve(https_routes)
                    .tls()
                    .cert_path("cert.pem")
                    .key_path("key.pem")
                    .run(([0, 0, 0, 0], https_port))
                    .await;
            });
            Some(handle)
        } else {
            None
        };

        // Wait for the servers to complete (they run indefinitely).
        if let Some(https_handle) = https_server {
            let _ = tokio::join!(http_server, https_handle);
        } else {
            let _ = http_server.await;
        }

        Ok(())
    }
}

/// Path to the `static/` directory alongside the running executable, falling back to a relative
/// `static/` if the executable path can't be resolved.
fn static_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("static")))
        .unwrap_or_else(|| std::path::PathBuf::from("static"))
}

/// Sends one framed binary message (see `msg_header` for the layout). Returns false when the
/// socket is gone and the caller should tear the connection down.
async fn send_framed(
    ws_tx: &mut SplitSink<WebSocket, Message>,
    frame_type: u8,
    payload: &[u8],
) -> bool {
    let mut bin = Vec::with_capacity(msg_header::HEADER_BYTES + payload.len());
    bin.extend_from_slice(&[frame_type, 0, 0, 0]);
    bin.extend_from_slice(payload);
    ws_tx.send(Message::binary(bin)).await.is_ok()
}

async fn handle_ws_connection(
    ws: WebSocket,
    control_tx: mpsc::UnboundedSender<ControlCommand>,
    tx_audio_tx: mpsc::UnboundedSender<Vec<f32>>,
    rx_waterfall_tx: broadcast::Sender<Vec<u8>>,
    rx_audio_tx: broadcast::Sender<Vec<f32>>,
    rx_iq_stream_tx: broadcast::Sender<Vec<u8>>,
    status_messages_tx: broadcast::Sender<ServerMessage>,
) {
    // --- Connection setup ---
    let (mut ws_tx, mut ws_rx) = ws.split();
    let mut rx_waterfall_sub = rx_waterfall_tx.subscribe();
    let mut rx_audio_sub = rx_audio_tx.subscribe();
    let mut rx_iq_stream_sub = rx_iq_stream_tx.subscribe();
    let mut status_messages_sub = status_messages_tx.subscribe();

    let mut tx_chunk_count = 0u64;

    let status_msg = ServerMessage::Status {
        state: String::from("connected"),
    };
    if let Ok(text) = serde_json::to_string(&status_msg) {
        let _ = ws_tx.send(Message::text(text)).await;
    }

    let _ = control_tx.send(ControlCommand::RequestSync);

    // --- Event loop: inbound client messages + outbound RX/status streams ---
    loop {
        tokio::select! {
            ws_event = ws_rx.next() => {
                match ws_event {
                    Some(Ok(message)) => {
                        if message.is_binary() {
                            let bytes = message.as_bytes();
                            if bytes.len() >= msg_header::HEADER_BYTES {
                                let header = bytes[0];
                                if header == msg_header::TX_AUDIO {
                                    let payload = &bytes[msg_header::HEADER_BYTES..];
                                    if payload.len() % 4 == 0 {
                                        let num_samples = payload.len() / 4;
                                        let mut pcm = Vec::with_capacity(num_samples);
                                        for chunk in payload.chunks_exact(4) {
                                            let sample = f32::from_le_bytes(chunk.try_into().unwrap());
                                            pcm.push(sample);
                                        }
                                        tx_chunk_count += 1;
                                        if tx_chunk_count == 1 {
                                            debug!("[WS] Received FIRST binary TX audio chunk from client ({} samples)", pcm.len());
                                        }
                                        if tx_chunk_count % 100 == 0 {
                                            debug!("[WS] Received {} binary TX audio chunks from client ({} samples)", tx_chunk_count, pcm.len());
                                        }
                                        if let Err(err) = tx_audio_tx.send(pcm) {
                                            error!("[WS] Failed to send PCM data to tx_audio channel: {}", err);
                                        }
                                    } else {
                                        warn!("[WS] Received binary payload length {} not divisible by 4", payload.len());
                                    }
                                } else {
                                    warn!("[WS] Received binary message with unknown header ID: {}", header);
                                }
                            } else {
                                warn!("[WS] Received short binary packet of size {} bytes", bytes.len());
                            }
                        } else if let Ok(text) = message.to_str() {
                            match serde_json::from_str::<ControlCommand>(text) {
                                Ok(command) => {
                                    debug!("[WS] Parsed control command: {:?}", command);
                                    let _ = control_tx.send(command);
                                }
                                Err(err) => {
                                    error!(
                                        "[WS] Failed to deserialize ControlCommand from string: '{}'. Error: {:?}",
                                        text, err
                                    );
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }

            waterfall = rx_waterfall_sub.recv() => {
                match waterfall {
                    Ok(values) => {
                        if !send_framed(&mut ws_tx, msg_header::WATERFALL, &values).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("[WS] Waterfall channel lagged! Skipped {} messages", skipped);
                        continue;
                    }
                    Err(_) => break,
                }
            }

            audio = rx_audio_sub.recv() => {
                match audio {
                    Ok(pcm) => {
                        let mut payload = Vec::with_capacity(pcm.len() * 4);
                        for sample in pcm {
                            payload.extend_from_slice(&sample.to_le_bytes());
                        }
                        if !send_framed(&mut ws_tx, msg_header::AUDIO, &payload).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("[WS] Audio channel lagged! Skipped {} messages", skipped);
                        continue;
                    }
                    Err(_) => break,
                }
            }

            iq = rx_iq_stream_sub.recv() => {
                match iq {
                    Ok(samples) => {
                        // `samples` is already interleaved i16 LE I/Q.
                        if !send_framed(&mut ws_tx, msg_header::IQ, &samples).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("[WS] IQ stream channel lagged! Skipped {} messages", skipped);
                        continue;
                    }
                    Err(_) => break,
                }
            }

            msg = status_messages_sub.recv() => {
                match msg {
                    Ok(m) => {
                        if let Ok(text) = serde_json::to_string(&m) {
                            if ws_tx.send(Message::text(text)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("[WS] Status messages channel lagged! Skipped {} messages", skipped);
                        continue;
                    }
                    Err(_) => break,
                }
            }
        }
    }

    // Client gone: force TX off so a dropped connection mid-transmission can't leave the radio keyed on air with nobody feeding it audio.
    let _ = control_tx.send(ControlCommand::SetTxState {
        active: false,
        tx_gain_db: crate::MIN_TX_GAIN_DB,
    });
}

async fn handle_rejection(err: Rejection) -> Result<warp::reply::Response, Infallible> {
    let code = if err.is_not_found() {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let message = format!("Error: {:?}", err);
    Ok(warp::reply::with_status(message, code).into_response())
}
