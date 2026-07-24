use std::fmt;
use std::str::FromStr;

use crate::device::GainMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemodMode {
    FM,
    USB,
    LSB,
}

impl DemodMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DemodMode::FM => "FM",
            DemodMode::USB => "USB",
            DemodMode::LSB => "LSB",
        }
    }
}

impl fmt::Display for DemodMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for DemodMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "FM" => Ok(DemodMode::FM),
            "USB" => Ok(DemodMode::USB),
            "LSB" => Ok(DemodMode::LSB),
            _ => Err(format!("Unknown demodulation mode: {}", s)),
        }
    }
}

/// Control-plane view of the session: the settings the user has requested. Distinct from the
/// hardware register shadow in `device.rs` (owned by the IO threads) and the DSP snapshot in
/// `AudioConfig` (owned by the audio thread). Lives only on the control loop.
pub struct ControlState {
    pub is_running: bool,
    pub antenna: u8,
    pub audio_enabled: bool,
    pub playback_hz: i64,
    pub demod_mode: DemodMode,
    pub filter_bw: f32,
    pub waterfall_interval_ms: u64,
    pub visual_span_hz: i64,
    pub rx_gain_mode: GainMode,
    pub rx_gain_db: f64,
    pub rf_bandwidth_hz: i64,
    pub tx_offset_hz: i64,
    /// When set, the wideband waterfall burst is also streamed to clients as raw i16 I/Q. Opt-in
    /// (off by default) so the normal web UI pays no bandwidth cost; enabled by `SetRxIqStream`.
    pub iq_stream_enabled: bool,
}

impl ControlState {
    pub fn new(initial_playback_hz: i64) -> Self {
        Self {
            is_running: true,
            antenna: 0,
            audio_enabled: false,
            playback_hz: initial_playback_hz,
            demod_mode: DemodMode::FM,
            filter_bw: 15_000.0,
            waterfall_interval_ms: 50,
            visual_span_hz: crate::MIN_SPAN_FM,
            rx_gain_mode: GainMode::AgcSlow,
            rx_gain_db: 30.0,
            rf_bandwidth_hz: 0,
            tx_offset_hz: crate::DEFAULT_TX_OFFSET_HZ,
            iq_stream_enabled: false,
        }
    }
}
