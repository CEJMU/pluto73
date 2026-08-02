use log::{debug, warn};
use memmap2::MmapOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::ptr::{read_volatile, write_volatile};
use std::time::Instant;
use std::{fs::OpenOptions, thread, time::Duration};

use industrial_io as iio;

// --- FPGA DMA Controller Register Offsets ---
// Controls starting, stopping, and resetting the DMA engine
pub const DMA_REG_CTRL: usize = 0x400;
// Trigger register to initiate a new DMA transfer request
pub const DMA_REG_START_TRANSFER: usize = 0x408;
// Physical memory address destination in system RAM
pub const DMA_REG_DEST_ADDRESS: usize = 0x410;
// Size of the DMA transfer in bytes
pub const DMA_REG_X_LENGTH: usize = 0x418;
// Interrupt request enable/mask register
pub const DMA_REG_IRQ_MASK: usize = 0x80;
// Interrupt status / clear pending interrupts register
pub const DMA_REG_IRQ_PENDING: usize = 0x84;

// --- Physical Device Paths & Memory Map Base Addresses ---
// Special file mapping the processor's absolute physical memory space
const DEV_MEM: &str = "/dev/mem";
// Userspace I/O driver used to block/wait for DMA interrupt triggers
const DEV_UIO: &str = "/dev/uio0";
// Reserved physical DDR RAM region where FPGA writes incoming ADC samples
const RAM_PHYS_ADDR: u64 = 0x1FC0_0000;
// Register base of the AXI GPIO controller for the RX DSP pipeline
const GPIO_PHYS_ADDR: u64 = 0x4121_0000;
// Register base of the AXI GPIO controller for the TX DSP pipeline
const GPIO_TX_PHYS_ADDR: u64 = 0x4120_0000;
// Register base address of the AD9361 RF transceiver interface core
const AD9361_PHYS_ADDR: u64 = 0x7902_0000;

pub const MAX_AUDIO_SAMPLES: usize = 16384;

/// AD9361 minimum baseband sample rate. Below this the internal FIR must be enabled.
pub const AD9361_MIN_FS_NO_FIR: i64 = 2_083_333;

/// 128-tap /4 RX/TX FIR coefficients from libad9361
const FIR_128_4: [i16; 128] = [
    -15, -27, -23, -6, 17, 33, 31, 9, -23, -47, -45, -13, 34, 69, 67, 21, -49, -102, -99, -32, 69,
    146, 143, 48, -96, -204, -200, -69, 129, 278, 275, 97, -170, -372, -371, -135, 222, 494, 497,
    187, -288, -654, -665, -258, 376, 875, 902, 363, -500, -1201, -1265, -530, 699, 1748, 1906,
    845, -1089, -2922, -3424, -1697, 2326, 7714, 12821, 15921, 15921, 12821, 7714, 2326, -1697,
    -3424, -2922, -1089, 845, 1906, 1748, 699, -530, -1265, -1201, -500, 363, 902, 875, 376, -258,
    -665, -654, -288, 187, 497, 494, 222, -135, -371, -372, -170, 97, 275, 278, 129, -69, -200,
    -204, -96, 48, 143, 146, 69, -32, -99, -102, -49, 21, 67, 69, 34, -13, -45, -47, -23, 9, 31,
    33, 17, -6, -23, -27, -15,
];

/// Represents the gain control modes for the AD9361 receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GainMode {
    AgcSlow,
    AgcFast,
    Manual,
}

impl GainMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            GainMode::AgcSlow => "slow_attack",
            GainMode::AgcFast => "fast_attack",
            GainMode::Manual => "manual",
        }
    }
}

impl std::fmt::Display for GainMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for GainMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "slow_attack" => Ok(GainMode::AgcSlow),
            "fast_attack" => Ok(GainMode::AgcFast),
            "manual" => Ok(GainMode::Manual),
            _ => Err(format!("Unknown gain control mode: {}", s)),
        }
    }
}

/// Top-level handle for the Pluto SDR device, grouping rx, tx, and system register interfaces.
pub struct PlutoDevice {
    pub rx: PlutoRxDevice,
    pub tx: PlutoTxDevice,
    pub system: PlutoSystem,
}

/// Controller for the AD9361 receiver RF front-end and AXI stream RX DMA.
pub struct PlutoRxDevice {
    pub context: iio::Context,
    pub dev_phy: iio::Device,
    pub dev_rx_stream: iio::Device,

    pub ch_i: Option<iio::Channel>,
    pub ch_q: Option<iio::Channel>,
    pub buffer: Option<iio::Buffer>,
    pub buffer_size: usize,

    pub antenna: u8,
    pub frequency: i64,
    pub sampling_frequency: i64,
    pub rf_bandwidth: i64,
    pub gain_mode: GainMode,
    pub gain: Option<f64>,
}

/// Controller for the AD9361 transmitter RF front-end and AXI stream TX DMA.
pub struct PlutoTxDevice {
    pub dev_phy: iio::Device,
    pub dev_tx_stream: iio::Device,

    pub ch_i: Option<iio::Channel>,
    pub ch_q: Option<iio::Channel>,
    pub buffer: Option<iio::Buffer>,
    pub buffer_size: usize,
    pub acc_i: Vec<i16>,
    pub acc_q: Vec<i16>,

    pub antenna: u8,
    pub frequency: i64,
    pub sampling_frequency: i64,
    pub rf_bandwidth: i64,
    pub gain: f64,
}

/// Interface for memory-mapped AXI DMA, GPIO, AD9361 registers, and system DDR RAM.
pub struct PlutoSystem {
    dma_regs: memmap2::MmapMut,
    ram_buf: memmap2::MmapMut,
    gpio_rx_regs: memmap2::MmapMut,
    gpio_tx_regs: memmap2::MmapMut,
    ad9361_regs: memmap2::MmapMut,
    uio_file: std::fs::File,

    // FPGA-fabric DSP register shadow. Mirrors the exact hardware state
    pub rx_antenna: u8,
    pub rx_cic_decimation: u32,
    pub rx_burst_gate: bool,
    pub tx_antenna: u8,
    pub tx_cic_interpolation: u32,
    pub tx_phase_inc: u32,
    pub tx_dds_offset_hz: f64,
    pub tx_dsp_enabled: bool,

    pub is_configuring: bool,
    pub dma_running: bool,
    pub ping_pong: bool,
}

// SAFETY: The underlying `industrial-io` (libiio) bindings wrap raw C pointers which do not
// automatically implement `Send`. We manually implement `Send` under the following strict safety invariants:
// 1. Thread Ownership: Each device (`PlutoRxDevice`, `PlutoTxDevice`) is moved into exactly one
//    worker thread upon application startup and is exclusively owned/accessed by that thread.
// 2. Mutual Exclusion: `PlutoSystem` is wrapped inside an `Arc<Mutex<PlutoSystem>>` to guarantee
//    synchronized, mutually exclusive access across threads.
//
// CAUTION: libiio contexts and devices are not thread-safe. Future modifications must never share
// references to these structs across thread boundaries concurrently without explicit synchronization
// (e.g., via a Mutex), as doing so would cause undefined behavior.
unsafe impl Send for PlutoRxDevice {}
unsafe impl Send for PlutoTxDevice {}
unsafe impl Send for PlutoSystem {}

impl PlutoDevice {
    /// Opens the AD9361 SDR device context and maps all associated memory-mapped FPGA systems.
    /// Returns the instantiated `PlutoDevice` handle.
    pub fn open(
        rx_buffer_size: usize,
        tx_buffer_size: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let context = iio::Context::new()?;

        let dev_phy_rx = context
            .find_device("ad9361-phy")
            .ok_or("Device not found")?;
        let dev_phy_tx = context
            .find_device("ad9361-phy")
            .ok_or("Device not found")?;
        let dev_rx_stream = context
            .find_device("cf-ad9361-lpc")
            .ok_or("Device not found")?;
        let dev_tx_stream = context
            .find_device("cf-ad9361-dds-core-lpc")
            .ok_or("TX Device not found")?;

        let rx_frequency = dev_phy_rx
            .find_output_channel("altvoltage0")
            .ok_or("rx lo not found")?
            .attr_read_int("frequency")?;
        let rx_ch = dev_phy_rx
            .find_input_channel("voltage0")
            .ok_or("voltage0 not found")?;
        let rx_sampling_frequency = rx_ch.attr_read_int("sampling_frequency")?;
        let rx_rf_bandwidth = rx_ch.attr_read_int("rf_bandwidth")?;
        let rx_gain_mode: GainMode = rx_ch.attr_read_str("gain_control_mode")?.parse()?;
        let rx_gain = rx_ch.attr_read_float("hardwaregain")?;

        let tx_ch_alt = dev_phy_tx
            .find_output_channel("altvoltage1")
            .ok_or("altvoltage1 not found")?;
        let tx_ch = dev_phy_tx
            .find_output_channel("voltage0")
            .ok_or("voltage0 not found")?;
        let tx_frequency = tx_ch_alt.attr_read_int("frequency")?;
        let tx_sampling_frequency = tx_ch.attr_read_int("sampling_frequency")?;
        let tx_rf_bandwidth = tx_ch.attr_read_int("rf_bandwidth")?;
        let tx_gain = tx_ch.attr_read_float("hardwaregain")?;

        let system = init_mem_system()?;

        Ok(PlutoDevice {
            rx: PlutoRxDevice {
                context,
                dev_phy: dev_phy_rx,
                dev_rx_stream,
                ch_i: None,
                ch_q: None,
                buffer: None,
                buffer_size: rx_buffer_size,
                antenna: 0,
                frequency: rx_frequency,
                sampling_frequency: rx_sampling_frequency,
                rf_bandwidth: rx_rf_bandwidth,
                gain_mode: rx_gain_mode,
                gain: Some(rx_gain),
            },
            tx: PlutoTxDevice {
                dev_phy: dev_phy_tx,
                dev_tx_stream,
                ch_i: None,
                ch_q: None,
                buffer: None,
                buffer_size: tx_buffer_size,
                acc_i: Vec::with_capacity(tx_buffer_size),
                acc_q: Vec::with_capacity(tx_buffer_size),
                antenna: 0,
                frequency: tx_frequency,
                sampling_frequency: tx_sampling_frequency,
                rf_bandwidth: tx_rf_bandwidth,
                gain: tx_gain,
            },
            system,
        })
    }

    /// Brings the hardware to a known-good baseline before a fresh configuration.
    /// Mutes TX, resets FPGA DSP/DDS/DMA, and returns the AD9361 to the 3.84 MHz FIR-bypassed rate.
    pub fn reset_device_state(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.tx.set_gain(-89.75);

        self.system.reset_gpio_to_default();
        self.system.reset_audio_dma_controller();

        self.rx.disable_bb_fir()?;
        let lo_hz = self.rx.frequency;
        self.rx.set_frequencies(lo_hz, 3_840_000)?;

        // Let the BBPLL / interface clock relock before the caller retunes.
        thread::sleep(Duration::from_millis(50));
        Ok(())
    }
}

/// Reads back the RX and TX `sampling_frequency` attributes for `ch_name` on the shared
/// `ad9361-phy` device and warns (debug_assert in debug builds) if they differ. RX and TX baseband
/// sample rates are hardware-coupled on this AD9361 - one shared BBPLL/LVDS `DATA_CLK` - so they
/// should always read equal regardless of which side (`PlutoRxDevice`/`PlutoTxDevice::
/// set_frequencies`) wrote last. This is a sanity check, not a fix: callers must still only ever
/// pass the one shared rate.
fn assert_shared_sample_rate(dev_phy: &iio::Device, ch_name: &str) {
    let rx_fs = dev_phy
        .find_input_channel(ch_name)
        .and_then(|ch| ch.attr_read_int("sampling_frequency").ok());
    let tx_fs = dev_phy
        .find_output_channel(ch_name)
        .and_then(|ch| ch.attr_read_int("sampling_frequency").ok());

    if let (Some(rx_fs), Some(tx_fs)) = (rx_fs, tx_fs) {
        if rx_fs != tx_fs {
            warn!(
                "RX/TX sampling_frequency diverged on {}: RX={} Hz, TX={} Hz \
                 (AD9361 has a single shared baseband clock; this should never happen)",
                ch_name, rx_fs, tx_fs
            );
        }
        debug_assert_eq!(
            rx_fs, tx_fs,
            "RX/TX sampling_frequency diverged on {}",
            ch_name
        );
    }
}

impl PlutoRxDevice {
    /// Sets the local oscillator center frequency and baseband sampling frequency for the receiver.
    /// Returns the active center frequency and sampling frequency configured by the hardware.
    ///
    /// Note: the AD9361 on this board exposes only one baseband sample rate, shared by RX and TX.
    /// Setting it here also moves `PlutoTxDevice`'s rate.
    pub fn set_frequencies(
        &mut self,
        frequency: i64,
        sampling_frequency: i64,
    ) -> Result<(i64, i64), Box<dyn std::error::Error>> {
        let ch = self
            .dev_phy
            .find_output_channel("altvoltage0")
            .ok_or("Channel not found: altvoltage0")?;

        let _ = ch.attr_write_int("frequency", frequency)?;
        let dev_frequency = ch.attr_read_int("frequency")?;
        self.frequency = dev_frequency;

        if sampling_frequency < AD9361_MIN_FS_NO_FIR {
            self.set_bb_rate_fir(sampling_frequency)?;
        } else {
            self.disable_bb_fir()?;
            let ch_name = format!("voltage{}", self.antenna);
            let ch = self
                .dev_phy
                .find_input_channel(&ch_name)
                .ok_or_else(|| format!("Channel {} not found", ch_name))?;

            let _ = ch.attr_write_int("sampling_frequency", sampling_frequency)?;
            let dev_sampling_frequency = ch.attr_read_int("sampling_frequency")?;
            self.sampling_frequency = dev_sampling_frequency;

            assert_shared_sample_rate(&self.dev_phy, &ch_name);
        }

        Ok((dev_frequency, self.sampling_frequency))
    }

    /// Sets the RF analog bandwidth of the receiver anti-aliasing filters.
    /// Returns the active bandwidth configured by the hardware.
    pub fn set_rf_bandwidth(
        &mut self,
        rf_bandwidth: i64,
    ) -> Result<i64, Box<dyn std::error::Error>> {
        let ch_name = format!("voltage{}", self.antenna);
        let ch = self
            .dev_phy
            .find_input_channel(&ch_name)
            .ok_or_else(|| format!("Channel {} not found", ch_name))?;

        let _ = ch.attr_write_int("rf_bandwidth", rf_bandwidth.min(self.sampling_frequency))?;
        let dev_bandwidth = ch.attr_read_int("rf_bandwidth")?;
        self.rf_bandwidth = dev_bandwidth;

        Ok(dev_bandwidth)
    }

    /// Configures the gain control mode and manual hardware gain for the receiver.
    /// Returns the selected gain mode and the configured hardware gain.
    pub fn set_gain(
        &mut self,
        gain_mode: GainMode,
        gain: Option<f64>,
    ) -> Result<(GainMode, Option<f64>), Box<dyn std::error::Error>> {
        let ch_name = format!("voltage{}", self.antenna);
        let ch = self
            .dev_phy
            .find_input_channel(&ch_name)
            .ok_or_else(|| format!("Channel {} not found", ch_name))?;

        let _ = ch.attr_write_str("gain_control_mode", gain_mode.as_str())?;
        self.gain_mode = gain_mode;

        if let GainMode::Manual = self.gain_mode {
            if let Some(gain) = gain {
                let _ = ch.attr_write_str("hardwaregain", &format!("{:.2}", gain))?;
                let dev_gain = ch.attr_read_float("hardwaregain")?;
                self.gain = Some(dev_gain);
            }
        }

        Ok((gain_mode, self.gain))
    }

    /// Reads the AD9361's applied hardware gain (dB) and measured RSSI (dB) for the active RX
    /// channel. With AGC enabled the `hardwaregain` value reflects the gain the AGC settled on.
    /// `rssi` is the chip's own received-signal-strength estimate.
    pub fn rx_signal_strength(&self) -> Result<(f64, f64), Box<dyn std::error::Error>> {
        let ch_name = format!("voltage{}", self.antenna);
        let ch = self
            .dev_phy
            .find_input_channel(&ch_name)
            .ok_or_else(|| format!("Channel {} not found", ch_name))?;
        let gain = ch.attr_read_float("hardwaregain")?;
        let rssi = ch.attr_read_float("rssi").unwrap_or(0.0);
        Ok((gain, rssi))
    }

    /// Enables the AD9361's internal 128-tap 4x FIR and sets a baseband sample rate.
    /// Faithfully replicates libad9361 `ad9361_set_bb_rate`, including the enable/rate sequencing.
    /// Returns the active RX sampling frequency configured by the hardware.
    pub fn set_bb_rate_fir(&mut self, rate: i64) -> Result<i64, Box<dyn std::error::Error>> {
        let taps = &FIR_128_4;
        let ch_name = format!("voltage{}", self.antenna);
        let ch = self
            .dev_phy
            .find_input_channel(&ch_name)
            .ok_or_else(|| format!("Channel {} not found", ch_name))?;

        let current_rate = ch.attr_read_int("sampling_frequency")?;
        let enabled = self
            .dev_phy
            .attr_read_bool("in_out_voltage_filter_fir_en")
            .unwrap_or(false);

        // Must disable the FIR before rewriting its coefficients
        if enabled {
            if current_rate <= AD9361_MIN_FS_NO_FIR {
                let _ = ch.attr_write_int("sampling_frequency", 3_000_000);
            }
            self.dev_phy
                .attr_write_bool("in_out_voltage_filter_fir_en", false)?;
        }

        // filter_fir_config blob, same layout libad9361 writes (one coef per line; RX = TX).
        let mut cfg = String::with_capacity(taps.len() * 12 + 64);
        cfg.push_str("RX 3 GAIN -6 DEC 4\n");
        cfg.push_str("TX 3 GAIN 0 INT 4\n");
        for &c in taps.iter() {
            cfg.push_str(&format!("{},{}\n", c, c));
        }
        cfg.push('\n');
        self.dev_phy.attr_write_str("filter_fir_config", &cfg)?;

        // Enable the FIR, then set the low rate. At very low rates the driver needs enough DAC/
        // TXSAMP headroom while enabling. Mirror libad9361 by bumping to 3 MHz first if short.
        if rate <= AD9361_MIN_FS_NO_FIR {
            if let Ok(rates) = self.dev_phy.attr_read_str("tx_path_rates") {
                let field = |key: &str| -> Option<i64> {
                    rates
                        .split_whitespace()
                        .find_map(|t| t.strip_prefix(key))
                        .and_then(|v| v.parse().ok())
                };
                if let (Some(dac), Some(txsamp)) = (field("DAC:"), field("TXSAMP:")) {
                    if txsamp > 0 && (dac / txsamp) * 16 < taps.len() as i64 {
                        let _ = ch.attr_write_int("sampling_frequency", 3_000_000);
                    }
                }
            }
            self.dev_phy
                .attr_write_bool("in_out_voltage_filter_fir_en", true)?;
            ch.attr_write_int("sampling_frequency", rate)?;
        } else {
            ch.attr_write_int("sampling_frequency", rate)?;
            self.dev_phy
                .attr_write_bool("in_out_voltage_filter_fir_en", true)?;
        }

        let dev_rate = ch.attr_read_int("sampling_frequency")?;
        self.sampling_frequency = dev_rate;
        Ok(dev_rate)
    }

    /// Disables the AD9361 internal FIR. Returns the active RX sampling frequency after the change.
    pub fn disable_bb_fir(&mut self) -> Result<i64, Box<dyn std::error::Error>> {
        if !self
            .dev_phy
            .attr_read_bool("in_out_voltage_filter_fir_en")
            .unwrap_or(false)
        {
            return Ok(self.sampling_frequency);
        }
        // If currently at a sub-floor rate, bump to a valid rate BEFORE disabling, otherwise the
        // FIR-off rate range would exclude the current rate and leave the AD9361 in a bad state.
        let ch_name = format!("voltage{}", self.antenna);
        let ch = self
            .dev_phy
            .find_input_channel(&ch_name)
            .ok_or_else(|| format!("Channel {} not found", ch_name))?;

        if ch.attr_read_int("sampling_frequency").unwrap_or(0) < AD9361_MIN_FS_NO_FIR {
            let _ = ch.attr_write_int("sampling_frequency", 3_000_000);
        }
        self.dev_phy
            .attr_write_bool("in_out_voltage_filter_fir_en", false)?;

        let dev_rate = ch.attr_read_int("sampling_frequency")?;
        self.sampling_frequency = dev_rate;
        Ok(dev_rate)
    }

    /// Selects the receiver antenna input (0 or 1). Drops the active buffer and
    /// reinitializes input channels mapped to the selected port.
    pub fn set_antenna(&mut self, antenna: u8) -> Result<(), Box<dyn std::error::Error>> {
        if antenna >= 2 {
            return Err(format!(
                "Invalid settings: antenna has to be 0 or 1, was {}",
                antenna
            )
            .into());
        }

        self.antenna = antenna;

        let ch_name = format!("voltage{}", self.antenna);
        let ch = self
            .dev_phy
            .find_input_channel(&ch_name)
            .ok_or_else(|| format!("Channel {} not found", ch_name))?;

        let current_fs = ch
            .attr_read_int("sampling_frequency")
            .unwrap_or(self.sampling_frequency);
        if self.sampling_frequency > current_fs {
            let _ = ch.attr_write_int("sampling_frequency", self.sampling_frequency)?;
            let _ = ch.attr_write_int("rf_bandwidth", self.rf_bandwidth)?;
        } else {
            let _ = ch.attr_write_int("rf_bandwidth", self.rf_bandwidth)?;
            let _ = ch.attr_write_int("sampling_frequency", self.sampling_frequency)?;
        }

        let dev_sampling_frequency = ch.attr_read_int("sampling_frequency")?;
        let dev_bandwidth = ch.attr_read_int("rf_bandwidth")?;
        self.sampling_frequency = dev_sampling_frequency;
        self.rf_bandwidth = dev_bandwidth;

        let _ = ch.attr_write_str("gain_control_mode", self.gain_mode.as_str())?;

        if let GainMode::Manual = self.gain_mode {
            if let Some(gain) = self.gain {
                let _ = ch.attr_write_str("hardwaregain", &format!("{:.2}", gain))?;
                let dev_gain = ch.attr_read_float("hardwaregain")?;
                self.gain = Some(dev_gain);
            }
        }

        // Drop buffer and disable channels
        self.buffer = None;

        if let Some(rx_i) = &self.ch_i {
            rx_i.disable();
        }
        if let Some(rx_q) = &self.ch_q {
            rx_q.disable();
        }

        // Rebind channels and buffer to new antenna
        self.init_channels()?;

        Ok(())
    }

    /// Enables the I/Q input channels for the selected receiver antenna and initializes
    /// a new IIO stream buffer of `buffer_size` samples.
    pub fn init_channels(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.buffer = None; // Drop any active buffer to release the resource and prevent EBUSY
        let (i_name, q_name) = match self.antenna {
            1 => ("voltage2", "voltage3"),
            _ => ("voltage0", "voltage1"),
        };
        let rx_i = self
            .dev_rx_stream
            .find_input_channel(i_name)
            .ok_or("Channel not found")?;
        let rx_q = self
            .dev_rx_stream
            .find_input_channel(q_name)
            .ok_or("Channel not found")?;

        rx_i.enable();
        rx_q.enable();

        self.ch_i = Some(rx_i);
        self.ch_q = Some(rx_q);

        if self.buffer_size > 0 {
            let buffer = self.dev_rx_stream.create_buffer(self.buffer_size, false)?;
            self.buffer = Some(buffer);
        }

        Ok(())
    }

    /// Refills the receiver stream buffer from hardware and reads the parsed I and Q i16 sample vectors.
    pub fn read_buffer(&mut self) -> Result<(Vec<i16>, Vec<i16>), Box<dyn std::error::Error>> {
        let buffer = self.buffer.as_mut().ok_or("Buffer not initialized")?;

        buffer.refill()?;

        let ch_i = self.ch_i.as_ref().ok_or("RX I channel not initialized")?;
        let ch_q = self.ch_q.as_ref().ok_or("RX Q channel not initialized")?;

        let i_samples = ch_i.read(buffer)?;
        let q_samples = ch_q.read(buffer)?;

        Ok((i_samples, q_samples))
    }

    /// Reads internal device telemetry sensors (processor temperature, internal logic voltage VCCINT,
    /// and memory voltage VCCODDR) via the XADC interface.
    pub fn read_telemetry(&self) -> Result<(f32, f32, f32), Box<dyn std::error::Error>> {
        if let Some(dev_xadc) = self.context.find_device("xadc") {
            // Temperature
            let temp_ch = dev_xadc
                .find_input_channel("temp0")
                .ok_or("temp0 channel not found")?;
            let temp_raw = temp_ch.attr_read_int("raw")? as f32;
            let temp_offset = temp_ch.attr_read_int("offset")? as f32;
            let temp_scale = temp_ch.attr_read_float("scale")? as f32;
            let temp_c = (temp_raw + temp_offset) * temp_scale / 1000.0;

            // VCCINT
            let vccint_ch = dev_xadc
                .find_input_channel("voltage0")
                .ok_or("vccint channel not found")?;
            let vccint_raw = vccint_ch.attr_read_int("raw")? as f32;
            let vccint_scale = vccint_ch.attr_read_float("scale")? as f32;
            let vccint_v = vccint_raw * vccint_scale / 1000.0;

            // VCCODDR
            let vccoddr_ch = dev_xadc
                .find_input_channel("voltage5")
                .ok_or("vccoddr channel not found")?;
            let vccoddr_raw = vccoddr_ch.attr_read_int("raw")? as f32;
            let vccoddr_scale = vccoddr_ch.attr_read_float("scale")? as f32;
            let vccoddr_v = vccoddr_raw * vccoddr_scale / 1000.0;

            Ok((temp_c, vccint_v, vccoddr_v))
        } else {
            // Fallback to ad9361-phy temp0 input
            if let Some(temp_ch) = self.dev_phy.find_input_channel("temp0") {
                let temp_raw = temp_ch.attr_read_int("input")? as f32;
                let temp_c = temp_raw / 1000.0;
                Ok((temp_c, 0.0, 0.0))
            } else {
                Err("No temperature sensor device found".into())
            }
        }
    }
}

impl PlutoTxDevice {
    /// Sets the local oscillator center frequency and baseband sampling frequency for the
    /// transmitter. Returns the active center frequency and sampling frequency configured by the
    /// hardware.
    ///
    /// Note: the AD9361 on this board exposes only one baseband sample rate, shared by RX and TX.
    /// Setting it here also moves `PlutoRxDevice`'s rate.
    pub fn set_frequencies(
        &mut self,
        frequency: i64,
        sampling_frequency: i64,
    ) -> Result<(i64, i64), Box<dyn std::error::Error>> {
        let ch_lo = self
            .dev_phy
            .find_output_channel("altvoltage1")
            .ok_or("Channel altvoltage1 not found")?;
        ch_lo.attr_write_int("frequency", frequency)?;
        let dev_frequency = ch_lo.attr_read_int("frequency")?;
        self.frequency = dev_frequency;

        let ch_name = format!("voltage{}", self.antenna);
        let ch_out = self
            .dev_phy
            .find_output_channel(&ch_name)
            .ok_or_else(|| format!("TX Channel {} not found", ch_name))?;
        ch_out.attr_write_int("sampling_frequency", sampling_frequency)?;
        let dev_sampling_frequency = ch_out.attr_read_int("sampling_frequency")?;
        self.sampling_frequency = dev_sampling_frequency;

        assert_shared_sample_rate(&self.dev_phy, &ch_name);

        Ok((dev_frequency, dev_sampling_frequency))
    }

    /// Sets the RF analog bandwidth of the transmitter filters.
    /// Returns the active bandwidth configured by the hardware.
    pub fn set_rf_bandwidth(
        &mut self,
        rf_bandwidth: i64,
    ) -> Result<i64, Box<dyn std::error::Error>> {
        let ch_name = format!("voltage{}", self.antenna);
        let ch_out = self
            .dev_phy
            .find_output_channel(&ch_name)
            .ok_or_else(|| format!("TX Channel {} not found", ch_name))?;

        ch_out.attr_write_int("rf_bandwidth", rf_bandwidth)?;
        let dev_bandwidth = ch_out.attr_read_int("rf_bandwidth")?;
        self.rf_bandwidth = dev_bandwidth;

        Ok(dev_bandwidth)
    }

    /// Sets the manual hardware transmission attenuation gain (in dB, between -89.75 and 0.0).
    /// Returns the active gain configured by the hardware.
    pub fn set_gain(&mut self, gain: f64) -> Result<f64, Box<dyn std::error::Error>> {
        let ch_name = format!("voltage{}", self.antenna);
        let ch = self
            .dev_phy
            .find_output_channel(&ch_name)
            .ok_or_else(|| format!("TX Channel {} not found", ch_name))?;

        let clamped_gain = gain.clamp(-89.75, 0.0);
        let _ = ch.attr_write_str("hardwaregain", &format!("{:.2}", clamped_gain))?;
        let dev_gain = ch.attr_read_float("hardwaregain")?;
        self.gain = dev_gain;

        Ok(self.gain)
    }

    /// Enables the I/Q output channels for the selected transmitter antenna and initializes
    /// a new IIO stream buffer of `buffer_size` samples.
    pub fn init_channels(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.buffer = None; // Drop any active buffer to release the resource and prevent EBUSY
        let (i_name, q_name) = match self.antenna {
            1 => ("voltage2", "voltage3"),
            _ => ("voltage0", "voltage1"),
        };
        let tx_i = self
            .dev_tx_stream
            .find_output_channel(i_name)
            .ok_or("TX I Channel not found")?;
        let tx_q = self
            .dev_tx_stream
            .find_output_channel(q_name)
            .ok_or("TX Q Channel not found")?;

        tx_i.enable();
        tx_q.enable();

        self.ch_i = Some(tx_i);
        self.ch_q = Some(tx_q);

        if self.buffer_size > 0 {
            let tx_buffer = self.dev_tx_stream.create_buffer(self.buffer_size, false)?;
            self.buffer = Some(tx_buffer);
        }

        Ok(())
    }

    /// Drops the active transmission stream buffer and disables the transmitter I/Q channels.
    pub fn release_channels(&mut self) {
        self.buffer = None;
        if let Some(ch) = &self.ch_i {
            ch.disable();
        }
        if let Some(ch) = &self.ch_q {
            ch.disable();
        }
        self.ch_i = None;
        self.ch_q = None;
        self.acc_i.clear();
        self.acc_q.clear();
    }

    /// Buffers raw I/Q samples into an internal accumulator. Once `buffer_size` samples are
    /// accumulated, they are pushed directly to the AXI stream DMA hardware buffer.
    pub fn write_buffer(
        &mut self,
        mut i_samples: &[i16],
        mut q_samples: &[i16],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let buffer = self.buffer.as_mut().ok_or("TX Buffer not initialized")?;
        let ch_i = self.ch_i.as_ref().ok_or("TX I channel not initialized")?;
        let ch_q = self.ch_q.as_ref().ok_or("TX Q channel not initialized")?;

        while !i_samples.is_empty() && !q_samples.is_empty() {
            let needed = self.buffer_size - self.acc_i.len();
            let take = std::cmp::min(needed, i_samples.len()).min(q_samples.len());

            self.acc_i.extend_from_slice(&i_samples[..take]);
            self.acc_q.extend_from_slice(&q_samples[..take]);

            i_samples = &i_samples[take..];
            q_samples = &q_samples[take..];

            if self.acc_i.len() == self.buffer_size {
                ch_i.write(buffer, &self.acc_i)?;
                ch_q.write(buffer, &self.acc_q)?;
                buffer.push()?;

                self.acc_i.clear();
                self.acc_q.clear();
            }
        }

        Ok(())
    }
}

impl PlutoSystem {
    fn rx_base_val(&self) -> u32 {
        // Bit 0: Active-low reset (1 = run, 0 = reset)
        // Bit 1: Waterfall burst trigger, pulsed on offset 0x00 by `trigger_waterfall_burst`, defaults to 0 here
        // Bit 2: Gated burst mode enabled (1 = enabled, 0 = bypass direct DMA-to-ADC)
        // Bit 3: rx_cic_config_valid is pulsed on offset 0x00 by `rx_apply_dsp_config`, defaults to 0 here
        // Bits [11:4]: RX CIC Decimation Rate
        // Bit 12: rx_dds_valid is pulsed on offset 0x00 by `rx_set_dds`, defaults to 0 here
        // Bit 13: RX antenna select (0 = Channel 1, 1 = Channel 2)
        (self.rx_cic_decimation << 4)
            | 0x01
            | ((self.rx_burst_gate as u32) << 2)
            | ((self.rx_antenna as u32) << 13)
    }

    /// Stores the active RX antenna selection and writes it (with the current decimation) to the AXI GPIO controller.
    pub fn rx_update_gpio_antenna(&mut self, rx_antenna: u8) {
        self.rx_antenna = rx_antenna;
        let rx_val = self.rx_base_val();
        self.write_gpio_rx(0x00, rx_val);
    }

    /// Enables (true) or bypasses (false) the RX burst gate (bit 2). When bypassed the wideband ADC
    /// streams continuously to the DMA instead of in triggered bursts.
    pub fn set_rx_burst_gate_enabled(&mut self, enabled: bool) {
        self.rx_burst_gate = enabled;
        let base_val = self.rx_base_val();
        self.write_gpio_rx(0x00, base_val);
    }

    /// Triggers a single gated burst transfer of ADC samples to DDR memory for the waterfall display.
    pub fn trigger_waterfall_burst(&mut self) {
        let base_val = self.rx_base_val();
        // Pulse trigger (bit 1 = 1)
        self.write_gpio_rx(0x00, base_val | 0x02);
        thread::sleep(Duration::from_micros(10));
        // Bring trigger back low
        self.write_gpio_rx(0x00, base_val);
    }

    /// Computes the RX FPGA CIC decimation for `rx_fs`, stores it (the source of truth for later
    /// GPIO re-pulses), resets the pipeline, and locks in the decimation rate and antenna.
    pub fn rx_apply_dsp_config(&mut self, rx_antenna: u8, rx_fs: i64) {
        self.rx_antenna = rx_antenna;
        self.rx_cic_decimation = rx_cic_decimation_for_rate(rx_fs);

        // Release AD9361 core resets by writing 3 to offset 0x40
        self.write_ad9361(0x40, 3);

        // Configure Channel 1,2 as outputs: write 0x0 to the Tri-state register at offset x
        self.write_gpio_rx(0x04, 0x0);
        self.write_gpio_rx(0x0C, 0x0);

        let base_val = self.rx_base_val();
        let reset_val = (self.rx_antenna as u32) << 13; // Reset active (Bit 0 = 0), burst gate disabled/bypassed (Bit 2 = 0)

        // Assert reset and keep burst gate bypassed/disabled
        self.write_gpio_rx(0x00, reset_val);
        thread::sleep(Duration::from_millis(10));

        // Release reset and enable gated burst mode
        self.write_gpio_rx(0x00, base_val);

        // Pulse CIC config valid (bit 3 = 1) to lock in the decimation rate
        self.write_gpio_rx(0x00, base_val | 0x08);
        thread::sleep(Duration::from_micros(10));
        self.write_gpio_rx(0x00, base_val);
    }

    /// Tunes the AXI DDS Compiler phase increment offset to shift the RX spectrum.
    pub fn rx_set_dds(&mut self, offset_hz: f64, sample_rate_hz: f64) {
        // Calculate the 32-bit Phase Increment for the DDS Compiler
        // phase_inc = (offset_hz / sample_rate) * 2^32
        // We use i64 to properly handle two's complement for negative frequencies
        let phase_inc = ((offset_hz / sample_rate_hz) * (1u64 << 32) as f64) as i64 as u32;
        // Write Phase Increment to Channel 2 (Offset 0x08)
        self.write_gpio_rx(0x08, phase_inc);

        // Pulse the tvalid signal (Bit 12) on Channel 1 (Offset 0x00)
        let base_val = self.rx_base_val();
        self.write_gpio_rx(0x00, base_val | (1 << 12));
        thread::sleep(Duration::from_micros(10));
        self.write_gpio_rx(0x00, base_val);

        debug!(
            "FPGA DDS Tuned: offset={} Hz (PINC={:#010X})",
            offset_hz, phase_inc
        );
    }

    fn tx_base_val(&self) -> u32 {
        // Bit 0: dsp_mux_tx enabled (1 = custom DSP active, 0 = bypass DMA->DAC)
        // Bit 1: TX antenna select
        // Bit 2: tx_dds_valid is pulsed on offset 0x00 by `tx_set_dds`, defaults to 0 here
        // Bit 3: tx_cic_config_valid is pulsed on offset 0x00 by `tx_apply_dsp_config`, defaults to 0 here
        // Bits [11:4]: CIC interpolation rate
        // Bits [27:12]: tx_strobe_gen phase increment
        //
        // The fabric (`dsp_mux_tx`) computes `4 * cic_interp - 1` from bits [11:4]; a value of 0
        // wraps that threshold to 0xFFF and starves the TX feed strobe, so never present less
        // than the hardware CIC's minimum rate of 4 to the fabric.
        debug_assert!(
            self.tx_cic_interpolation >= 4,
            "tx_cic_interpolation must be >= 4 (the CIC minimum) before any GPIO write"
        );
        (self.tx_dsp_enabled as u32)
            | ((self.tx_antenna as u32) << 1)
            | (self.tx_cic_interpolation.max(4) << 4)
            | (self.tx_phase_inc << 12)
    }

    /// Computes the TX FPGA DSP rates for `tx_fs`, resets the pipeline to lock in the
    /// interpolation rate/antenna/strobe rate, and tunes the TX DDS Compiler to the configured
    /// offset (`tx_dds_offset_hz`, default +50 kHz) to avoid the center DC spike.
    /// Returns the rounded TX sample rate and the CIC interpolation factor.
    pub fn tx_apply_dsp_config(&mut self, tx_antenna: u8, tx_fs: f64) -> (f64, u32) {
        let rounded_tx_fs = tx_rounded_fs(tx_fs);
        let cic_interpolation = tx_cic_interpolation(rounded_tx_fs);
        let tx_phase_inc = tx_strobe_phase_inc(rounded_tx_fs);

        self.tx_antenna = tx_antenna;
        self.tx_cic_interpolation = cic_interpolation;
        self.tx_phase_inc = tx_phase_inc;

        // Configure Channel 1 as output: write 0x0 to the Tri-state register
        self.write_gpio_tx(0x04, 0x0);

        let base_val = self.tx_base_val();
        self.write_gpio_tx(0x00, base_val);
        thread::sleep(Duration::from_micros(10));

        // Pulse CIC config valid (bit 3 = 1)
        self.write_gpio_tx(0x00, base_val | 0x08);
        thread::sleep(Duration::from_micros(10));
        self.write_gpio_tx(0x00, base_val);

        debug!(
            "FPGA TX DSP config applied: interpolation rate = {}, antenna = {}.",
            cic_interpolation, tx_antenna
        );

        // Configure the TX DDS Compiler to the configured frequency offset (default +50 kHz)
        self.tx_set_dds(self.tx_dds_offset_hz, rounded_tx_fs * 2.0);

        (rounded_tx_fs, cic_interpolation)
    }

    /// Tunes the AXI DDS Compiler phase increment offset to shift the TX spectrum.
    pub fn tx_set_dds(&mut self, offset_hz: f64, sample_rate_hz: f64) {
        // Calculate the 32-bit Phase Increment for the DDS Compiler
        let phase_inc = ((offset_hz / sample_rate_hz) * (1u64 << 32) as f64) as i64 as u32;
        // Write Phase Increment to Channel 2 of the AXI GPIO controller for TX (Offset 0x08)
        self.write_gpio_tx(0x08, phase_inc);

        // Pulse the tvalid signal (Bit 2) on Channel 1 (Offset 0x00). Bits [27:12] carry the
        // tx_strobe_gen phase increment and must be kept set across the pulse.
        let base_val = self.tx_base_val();
        self.write_gpio_tx(0x00, base_val | (1 << 2));
        thread::sleep(Duration::from_micros(10));
        self.write_gpio_tx(0x00, base_val);

        debug!(
            "FPGA TX DDS Tuned: offset={} Hz (PINC={:#010X})",
            offset_hz, phase_inc
        );
    }

    /// Stores the active TX antenna selection and writes it (with the current rates) to the AXI GPIO controller.
    pub fn tx_update_gpio_antenna(&mut self, tx_antenna: u8) {
        self.tx_antenna = tx_antenna;
        let base_val = self.tx_base_val();
        self.write_gpio_tx(0x00, base_val);
    }

    /// Enables (true) or bypasses (false) the custom FPGA TX DSP pipeline (dsp_mux_tx enabled bit 0).
    pub fn set_tx_dsp_enabled(&mut self, enabled: bool) {
        self.tx_dsp_enabled = enabled;
        let base_val = self.tx_base_val();
        self.write_gpio_tx(0x00, base_val);
    }

    /// Restores both AXI GPIO controllers (RX and TX DSP pipelines) to their power-on-reset state
    pub fn reset_gpio_to_default(&mut self) {
        self.write_gpio_rx(0x08, 0x0);
        self.write_gpio_rx(0x00, 0x0);
        self.write_gpio_tx(0x08, 0x0);
        self.write_gpio_tx(0x00, 0x0);
    }

    /// Drains any stale pending interrupts from the Linux kernel UIO driver.
    pub fn drain_uio_interrupts(&mut self) {
        let raw_fd = self.uio_file.as_raw_fd();
        let mut fds = [libc::pollfd {
            fd: raw_fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        unsafe {
            // Non-blocking poll (timeout = 0)
            while libc::poll(fds.as_mut_ptr(), 1, 0) > 0 {
                let mut int_info = [0u8; 4];
                let _ = self.uio_file.read(&mut int_info);
                fds[0].revents = 0;
            }
        }
    }

    /// Restarts the AXI DMA engine and resets the interrupt mask/status registers to clear any pending triggers.
    pub fn reset_audio_dma_controller(&mut self) {
        // Stop DMA
        self.write_dma(DMA_REG_CTRL, 0x0);
        thread::sleep(Duration::from_micros(10));
        self.write_dma(DMA_REG_CTRL, 0x1);
        self.dma_running = false;
        self.ping_pong = false;

        // Clear any old interrupts
        self.write_dma(DMA_REG_IRQ_PENDING, 0xFFFFFFFF);

        // Unmask EOT (Bit 1 = 0), Mask SOT (Bit 0 = 1)
        // This ensures UIO only wakes us up when the transfer is DONE!
        self.write_dma(DMA_REG_IRQ_MASK, 0x01);

        // Drain any stale UIO interrupts from the kernel driver
        self.drain_uio_interrupts();
    }

    /// Queues initial ping-pong DMA transfer descriptors if the DMA engine is currently stopped.
    pub fn ensure_dma_running(&mut self) {
        let max_bytes: u32 = (MAX_AUDIO_SAMPLES * 4) as u32;
        let current_offset = if self.ping_pong { max_bytes } else { 0 };
        let next_offset = if !self.ping_pong { max_bytes } else { 0 };

        if !self.dma_running {
            self.write_dma(DMA_REG_IRQ_PENDING, 0xFFFFFFFF);

            // Queue both ping-pong transfers; leave the DMA stopped if the DMAC won't accept them.
            if !self.submit_dma_transfer(current_offset, max_bytes)
                || !self.submit_dma_transfer(next_offset, max_bytes)
            {
                return;
            }

            let _ = self.uio_file.write_all(&1u32.to_ne_bytes());
            self.dma_running = true;
        }
    }

    /// Reads memory-mapped ADC samples from the current page of the DMA buffer, unpacks them into
    /// separate I/Q vectors, and queues a new transfer request to keep the DMA pipeline full.
    ///
    /// WARNING: This is a blocking convenience wrapper intended ONLY for single-threaded scripts
    /// In a multi-threaded context (like the main application), calling  this function while
    /// holding a `Mutex` lock on `PlutoSystem` will cause severe lock contention.
    /// Instead, audio threads should call `prepare_audio_dma_read()` to acquire the raw
    /// pointer, immediately drop the lock, and perform the `unsafe` copy and `unpack_iq_words()`
    /// entirely outside of the critical section.
    pub fn read_audio_dma_samples(
        &mut self,
        i_buf: &mut Vec<i16>,
        q_buf: &mut Vec<i16>,
    ) -> Option<usize> {
        let (count, ram_ptr) = self.prepare_audio_dma_read()?;
        let mut words = vec![0u32; count];
        // SAFETY: ram_ptr points at `count` packed u32 samples inside the mmapped DMA buffer
        // (see prepare_audio_dma_read); `words` was just allocated with that length.
        unsafe {
            std::ptr::copy_nonoverlapping(ram_ptr, words.as_mut_ptr(), count);
        }
        unpack_iq_words(&words, i_buf, q_buf);
        Some(count)
    }

    /// Prepares the DMA for the next read and returns the raw pointer to the mmapped RAM buffer
    /// corresponding to the completed transfer, allowing copy/unpack to run outside the lock.
    ///
    /// The submit-before-read ordering here is critical: the next transfer is queued into the just-finished
    /// buffer BEFORE it is read, keeping the 2-deep hardware queue full so it never underruns.
    pub fn prepare_audio_dma_read(&mut self) -> Option<(usize, *const u32)> {
        if !self.dma_running {
            return None;
        }

        let max_bytes: u32 = (MAX_AUDIO_SAMPLES * 4) as u32;
        let current_offset = if self.ping_pong { max_bytes } else { 0 };

        // Clear DMA Interrupt unconditionally
        self.write_dma(DMA_REG_IRQ_PENDING, 0xFFFFFFFF);

        // Queue the next transfer into current_offset (the buffer we just finished) BEFORE reading
        // it, to keep the 2-deep hardware queue full and never underrun
        if !self.submit_dma_transfer(current_offset, max_bytes) {
            return None;
        }

        let _ = self.uio_file.write_all(&1u32.to_ne_bytes());

        let ram_ptr = unsafe {
            (self.ram_buf.as_ptr() as *const u8).add(current_offset as usize) as *const u32
        };

        self.ping_pong = !self.ping_pong;
        Some((MAX_AUDIO_SAMPLES, ram_ptr))
    }

    /// Submits one DMA transfer descriptor (dest address + length) to the AXI DMAC's
    /// submission queue, then kicks it off via START_TRANSFER. Returns false (without
    /// submitting) if the DMAC does not accept the descriptor within 100 ms.
    ///
    /// The DMAC's START_TRANSFER register reads back as 1 until the hardware has
    /// internally accepted the descriptor into its (2-deep) queue; writing a new
    /// descriptor while it still reads 1 races the hardware and the write is lost
    /// (this is the same handshake the kernel's axi-dmac driver performs before calling
    /// axi_dmac_start_transfer - see drivers/dma/dma-axi-dmac.c). Skipping this check
    /// silently drops every other submitted buffer, desyncing the ping-pong tracking
    /// from the hardware and producing periodic dropouts in the captured audio.
    fn submit_dma_transfer(&mut self, dest_offset: u32, max_bytes: u32) -> bool {
        let deadline = Instant::now() + Duration::from_millis(100);
        while self.read_dma(DMA_REG_START_TRANSFER) != 0 {
            if Instant::now() >= deadline {
                warn!("AXI DMAC did not accept a transfer descriptor within 100 ms; skipping");
                return false;
            }
        }
        self.write_dma(DMA_REG_DEST_ADDRESS, RAM_PHYS_ADDR as u32 + dest_offset);
        self.write_dma(DMA_REG_X_LENGTH, max_bytes - 1);
        self.write_dma(DMA_REG_START_TRANSFER, 0x1);
        true
    }

    /// Clones the file descriptor handle of the UIO device file.
    pub fn clone_uio_file(&self) -> std::io::Result<std::fs::File> {
        self.uio_file.try_clone()
    }

    /// Safe wrapper to write a 32-bit value to a specific offset in the GPIO RX registers.
    /// Alignment is guaranteed because the mapped physical registers are page-aligned (4KB bounds),
    /// and the register offsets used are all 32-bit aligned.
    #[inline(always)]
    pub fn write_gpio_rx(&mut self, offset_bytes: usize, value: u32) {
        unsafe {
            let ptr = (self.gpio_rx_regs.as_mut_ptr() as *mut u8).add(offset_bytes) as *mut u32;
            write_volatile(ptr, value);
        }
    }

    /// Safe wrapper to read a 32-bit value from a specific offset in the GPIO RX registers.
    #[inline(always)]
    pub fn read_gpio_rx(&self, offset_bytes: usize) -> u32 {
        unsafe {
            let ptr = (self.gpio_rx_regs.as_ptr() as *const u8).add(offset_bytes) as *const u32;
            read_volatile(ptr)
        }
    }

    /// Safe wrapper to write a 32-bit value to a specific offset in the GPIO TX registers.
    /// Alignment is guaranteed because the mapped physical registers are page-aligned (4KB bounds),
    /// and the register offsets used are all 32-bit aligned.
    #[inline(always)]
    pub fn write_gpio_tx(&mut self, offset_bytes: usize, value: u32) {
        unsafe {
            let ptr = (self.gpio_tx_regs.as_mut_ptr() as *mut u8).add(offset_bytes) as *mut u32;
            write_volatile(ptr, value);
        }
    }

    /// Safe wrapper to read a 32-bit value from a specific offset in the GPIO TX registers.
    #[inline(always)]
    pub fn read_gpio_tx(&self, offset_bytes: usize) -> u32 {
        unsafe {
            let ptr = (self.gpio_tx_regs.as_ptr() as *const u8).add(offset_bytes) as *const u32;
            read_volatile(ptr)
        }
    }

    /// Safe wrapper to write a 32-bit value to a specific offset in the AD9361 registers.
    /// Alignment is guaranteed because the mapped physical registers are page-aligned (4KB bounds),
    /// and the register offsets used are all 32-bit aligned.
    #[inline(always)]
    pub fn write_ad9361(&mut self, offset_bytes: usize, value: u32) {
        unsafe {
            let ptr = (self.ad9361_regs.as_mut_ptr() as *mut u8).add(offset_bytes) as *mut u32;
            write_volatile(ptr, value);
        }
    }

    /// Safe wrapper to write a 32-bit value to the AXI DMA registers.
    /// Alignment is guaranteed because the mapped physical registers are page-aligned (4KB bounds),
    /// and the register offsets used are all 32-bit aligned.
    #[inline(always)]
    pub fn write_dma(&mut self, offset_bytes: usize, value: u32) {
        unsafe {
            let ptr = (self.dma_regs.as_mut_ptr() as *mut u8).add(offset_bytes) as *mut u32;
            write_volatile(ptr, value);
        }
    }

    /// Safe wrapper to read a 32-bit value from the AXI DMA registers.
    #[inline(always)]
    pub fn read_dma(&self, offset_bytes: usize) -> u32 {
        unsafe {
            let ptr = (self.dma_regs.as_ptr() as *const u8).add(offset_bytes) as *const u32;
            read_volatile(ptr)
        }
    }
}

/// Safely bulk-copies `count` packed 32-bit words from the raw mmapped DMA buffer pointer
/// `ram_ptr` into a caller-provided destination vector outside the mutex lock.
pub fn copy_dma_words(ram_ptr: *const u32, count: usize, dest: &mut Vec<u32>) {
    if ram_ptr.is_null() || count == 0 {
        dest.clear();
        return;
    }
    dest.clear();
    dest.reserve(count);
    // SAFETY: ram_ptr points at `count` 32-bit words in the mmapped DDR buffer.
    // `dest` has reserved capacity for at least `count` elements.
    unsafe {
        std::ptr::copy_nonoverlapping(ram_ptr, dest.as_mut_ptr(), count);
        dest.set_len(count);
    }
}

/// Unpacks DMA words ({Q,I} packed 16+16, I in the low half, matching `iq_packer.v`) into separate I/Q vectors.
pub fn unpack_iq_words(words: &[u32], i_buf: &mut Vec<i16>, q_buf: &mut Vec<i16>) {
    i_buf.reserve(words.len());
    q_buf.reserve(words.len());
    for &packed in words {
        i_buf.push((packed & 0xFFFF) as i16);
        q_buf.push(((packed >> 16) & 0xFFFF) as i16);
    }
}

/// Blocks on the UIO file until the audio-DMA completion interrupt fires or `timeout_ms` elapses.
/// Returns `Ok(Some(interrupt_count))`, `Ok(None)` on a timeout or
/// EINTR (caller should just retry), and `Err` on a real poll/read failure.
pub fn wait_for_uio_interrupt(
    uio: &mut std::fs::File,
    timeout_ms: i32,
) -> std::io::Result<Option<u32>> {
    let mut fds = [libc::pollfd {
        fd: uio.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    }];
    // SAFETY: fds points to a valid stack-allocated array of size 1.
    let poll_ret = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
    if poll_ret < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            return Ok(None);
        }
        return Err(err);
    }
    if poll_ret == 0 {
        return Ok(None);
    }
    let mut int_info = [0u8; 4];
    uio.read_exact(&mut int_info)?;
    Ok(Some(u32::from_ne_bytes(int_info)))
}

fn init_mem_system() -> Result<PlutoSystem, Box<dyn std::error::Error>> {
    let mem_file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_SYNC)
        .open(DEV_MEM)?;
    let uio_file = OpenOptions::new().read(true).write(true).open(DEV_UIO)?;

    let dma_regs = unsafe { MmapOptions::new().len(0x1000).map_mut(&uio_file)? };
    let ram_buf = unsafe {
        MmapOptions::new()
            .offset(RAM_PHYS_ADDR)
            .len(0x400000) // 4MB mapped
            .map_mut(&mem_file)?
    };
    let gpio_rx_regs = unsafe {
        MmapOptions::new()
            .offset(GPIO_PHYS_ADDR)
            .len(0x10000)
            .map_mut(&mem_file)?
    };
    let gpio_tx_regs = unsafe {
        MmapOptions::new()
            .offset(GPIO_TX_PHYS_ADDR)
            .len(0x10000)
            .map_mut(&mem_file)?
    };
    let ad9361_regs = unsafe {
        MmapOptions::new()
            .offset(AD9361_PHYS_ADDR)
            .len(0x10000)
            .map_mut(&mem_file)?
    };

    Ok(PlutoSystem {
        dma_regs,
        ram_buf,
        gpio_rx_regs,
        gpio_tx_regs,
        ad9361_regs,
        uio_file,
        rx_antenna: 0,
        rx_cic_decimation: 0,
        rx_burst_gate: true,
        tx_antenna: 0,
        tx_cic_interpolation: 4,
        tx_phase_inc: 0,
        tx_dds_offset_hz: crate::DEFAULT_TX_OFFSET_HZ as f64,
        tx_dsp_enabled: true,
        is_configuring: false,
        dma_running: false,
        ping_pong: false,
    })
}

/// AD9361 TX interface clock (fabric `l_clk`) frequency for a baseband sample rate `fs`.
pub fn tx_interface_clock_hz(fs: f64) -> f64 {
    2.0 * fs
}

/// 16-bit phase increment for `tx_sample_enable` so its clock-enable strobe lands at exactly `fs`:
/// `strobe = l_clk * phase_inc / 65536`  =>  `phase_inc = round(65536 * fs / l_clk)`.
/// With `l_clk = 2 * fs` this is `round(65536 * 0.5) = 32768`. `fs/l_clk = 0.5`, so it fits in 16 bits.
pub fn tx_strobe_phase_inc(fs: f64) -> u32 {
    let l_clk = tx_interface_clock_hz(fs);
    ((65536.0 * fs / l_clk).round() as i64).clamp(0, 0xFFFF) as u32
}

// FPGA CIC decimation for an RX baseband rate, targeting ~960 kHz into the AD9361 FIR: clamp to
/// [4, 64] and round up to a power of two (iq_packer.v's bit-shift audio scaling requires exact
/// powers of two). The bounds are the CIC cores' configured Minimum_Rate/Maximum_Rate; the upper
/// one is not reachable today in code, since MAX_SPAN (30.72 MHz) yields exactly 32.
/// This is the RX counterpart to the TX rate math in tx_apply_dsp_config.
pub fn rx_cic_decimation_for_rate(rx_fs: i64) -> u32 {
    ((rx_fs / 960_000).clamp(4, 64) as u32).next_power_of_two()
}

/// Rounds a requested TX baseband rate to a clean multiple of 192 kHz (48 kHz x 4x FIR interpolation).
pub fn tx_rounded_fs(tx_fs: f64) -> f64 {
    (tx_fs / 192_000.0).round() * 192_000.0
}

/// FPGA CIC interpolation factor for a (already 192-kHz-rounded) TX rate. The hardware FIR compiler
/// handles the 4x interpolation, so the CIC only covers 1/4 of the total factor.
pub fn tx_cic_interpolation(rounded_tx_fs: f64) -> u32 {
    let total_interpolation = (rounded_tx_fs / crate::AUDIO_SAMPLE_RATE as f64)
        .round()
        .max(16.0)
        .min(256.0) as u32;
    total_interpolation / 4
}
