# Pluto SDR Test Suite

This document describes the diagnostic and measurement test commands built into the Pluto SDR Rust application. These tests verify the DSP chain, DMA timing, RF loopback performance, and signal quality.

---

## Running Tests on the Device

Since the application links against `libiio` and interacts with hardware registers, these tests must be executed directly on the PlutoSDR device. They are compiled into a separate `diagnostics` binary (`src/bin/diagnostics.rs`), not the main `pluto` app.

1. **Deploy and run a test in one step** (cross-compiles, copies the binary to the device, and executes it):
   ```bash
   make run-diagnostics ARGS="--test-rf-raw-loopback"
   ```
2. **Or deploy once and run flags manually over SSH**:
   ```bash
   make deploy-diagnostics
   ssh root@192.168.2.1 "/root/diagnostics --test-rf-raw-loopback"
   ```

_Note: All WAV file inputs and outputs resolve on the device's filesystem. Copy input files to `/root/` on the device first, and retrieve output files using `scp`. Input WAV files must have a sample rate of exactly 48000 Hz._

---

## Code & File Structure

The test files are organized under the `src/bin/test/` directory and grouped logically by domain:

- **`rf_loopback.rs`** - Contains raw RF loopbacks, real-world audio loopbacks, and diagnostic tone loopbacks.
- **`dma_diagnostics.rs`** - Implements raw audio DMA probes and phase-continuity tearing tests.
- **`narrowband.rs`** - Exercises sub-2.083 MSPS AD9361 internal FIR configurations.
- **`spectral_analysis.rs`** - Sweeps tone frequencies and drives to analyze occupied bandwidth, ACPR, and distortion.
- **`timing_pacing.rs`** - Verifies DMA delay, backpressure, and pacing behaviors.
- **`software_dsp.rs`** - Simulates the modulation/demodulation pipeline offline in software.
- **`fm_broadcast.rs`** - Evaluates live over-the-air FM signal qualities.

---

## Test Reference Catalog

### 1. RF & Hardware Loopback Tests (`rf_loopback.rs`)

_Requires connecting a physical loopback cable (TX1 -> 20 dB attenuator -> RX1)._

- **`--test-rf-raw-loopback`**: Basic RF loopback verification.
  - _Method_: Transmits a raw 1 kHz complex tone at 48 kHz and reads from the raw wideband ADC buffer via waterfall bursts.
  - _Analysis_: Computes a forward FFT of the captured buffer and measures relative amplitudes at DC (0 Hz), DDS carrier (+50 kHz), DUC target (+51 kHz), and out-of-band noise (+100 kHz) to verify basic hardware RF routing.
- **`--test-rf-audio-loopback <input.wav> <output.wav> [fs_hz] [rx_gain_db] [lo_hz] [usb|lsb]`**: End-to-end hardware loopback using a real audio file (RX gain defaults to 40 dB manual, LO defaults to 900 MHz. Pass a different `lo_hz` for interference A/B checks; sideband defaults to `usb`).
  - _Requirements_: The input WAV must be exactly 48000 Hz mono.
  - _Method_: SSB modulates the audio file in real-time, streams it to the TX DMA, routes it through the physical loopback path, captures the downconverted signal via the RX audio DMA, demodulates it in software, and writes the output WAV. The trailing `usb|lsb` argument sets **both** the TX modulator sideband and the RX demod sideband (kept matched)
  - _Analysis_: Validates audio intelligibility, resampler pacing, and software demodulator behavior on real signals.
- **`--test-rf-tone-loopback <freq_hz> <duration_s> <output.wav> [chunk_size] [fs_hz]`**: Continuous tone loopback.
  - _Method_: Transmits a continuous single-frequency tone (e.g. 1000 Hz) modulated to SSB USB for the specified duration.
  - _Analysis_: Used to isolate content-independent buffering seams or pacing dropouts from demodulator-induced artifacts. Writes loopback output to a WAV file.

### 2. Audio DMA & Continuity Probes (`dma_diagnostics.rs`)

- **`--test-dma-probe`**: Captures raw IQ data from the FPGA audio DMA path and performs spectral FFT analysis. Note that the RX audio path is permanently wired to the TX post-DDS output in this FPGA design. It executes three stages sequentially:
  1. _Probe 1 (Baseline)_: Captures RX DMA with TX inactive to measure baseline noise/carrier leakage.
  2. _Probe 2 (Raw Tone)_: Transmits a raw 1 kHz complex tone (bypassing software modulation) to check baseband DDS placement.
  3. _Probe 3 (Modulated Tone)_: Transmits a 1 kHz audio tone modulated to SSB USB through the production `TxModulator` software block and TX FPGA path. Expected peak lands at +1 kHz.
- **`--test-dma-continuity`**: Checks phase continuity of the RX audio DMA.
  - _Method_: Transmits a seamlessly looping cyclic 3 kHz tone (continuous, bypassing TX DMA pushing seams).
  - _Analysis_: Captures the loopback audio DMA and measures the per-sample phase delta. A jump in the middle of a buffer indicates **buffer tearing** (software reading while hardware overwrites); a jump at a buffer boundary indicates **dropped buffers**.
- **`--test-dma-carrier-offset [--loopback]`**: High-resolution TX carrier / opposite-sideband suppression probe.
  - _Method_: Programs the RX DDS to -45 kHz instead of -50 kHz, so the TX carrier (LO +50 kHz) lands at +5 kHz in the audio DMA passband, **outside the fabric DC blocker**, which otherwise silently removes the carrier and flatters every audio-DMA measurement. The wanted 1 kHz USB tone lands at +6 kHz, the opposite-sideband image at +4 kHz, and the RX chain's own LO leakage at -45 kHz, all cleanly separated. Captures with the transmitter keyed but silent (static DAC bias only) and with a modulated tone.
  - _Analysis_: A single full-capture FFT over 3-4s gives sub-Hz resolution bandwidth (noise floor approx. −68 dBc), three decades sharper than the wideband burst test's 234 Hz bins. Reports carrier, opposite sideband, blocked-DC residual and noise, each in dBc relative to the wanted tone, plus the static-vs-modulated carrier split.

### 3. Narrowband & Low Visual Span Tests (`narrowband.rs`)

- **`--test-narrowband-rx [rate_hz] [duration_s]`**: Validates sub-2.083 MSPS AD9361 internal FIR configurations.
  - _Method_: Captures wideband waterfall samples and audio DMA samples at low rates (default: 768 kHz).
  - _Analysis_: Computes average power spectra using FFT to ensure the decimation filters are stable and no folding/spurs appear.
- **`--test-narrowband-loopback [rate_hz] [duration_s]`**: SSB loopback at custom sample rates.
  - _Method_: Modulates and loopbacks a 1 kHz tone at sub-2.083 MSPS rates.
  - _Analysis_: Verifies that both the TX and RX FIR filters are properly designed, loaded, and paced.

### 4. Live Broadcast & Quality Metrics (`fm_broadcast.rs`)

- **`--test-fm-broadcast-quality <station_hz> <duration_s> [out_prefix]`**: Reference-free quality audit of a live, over-the-air FM broadcast station.
  - _Method_: Records the composite FM multiplex (MPX) at a 240 kHz sample rate.
  - _Analysis_: Performs a Hamming-windowed FFT on the composite signal to measure pilot carrier SNR (at 19 kHz) and ultrasonic noise floor (in the empty 60-100 kHz band) to report signal quality metrics independent of current program audio. Demodulates mono audio to a WAV file.

### 5. Pure Software Verification (`software_dsp.rs`)

- **`--test-soft-ssb-loopback <output.wav>`**: Pure software simulation of the SSB modulation/demodulation chain.
  - _Method_: Generates a dual-tone input (500 Hz + 1500 Hz), modulates it to SSB USB, simulates FPGA decimation, runs the production SSB demodulator, and writes output and reference WAVs.
  - _Analysis_: Runs entirely offline on the host without hardware. Uses FFT to verify that opposite-sideband images and out-of-band spurs are fully suppressed (expected attenuation $<-57$ dB).

### 6. Spectral Purity & Sweeps (`spectral_analysis.rs`)

- **`--test-spec-audio-sweep [tone_hz] [duration_s] [--save]`**: Sweeps center frequencies, spans, and offsets in a grid to verify demodulator correctness (matching tone recovery and spur suppression) across the operating space.
- **`--test-spec-tx-shape [--loopback]`**: Measures the spectral roll-off and out-of-band shape of the transmit signal.
- **`--test-spec-tx-wideband [--loopback]`**: Captures raw ADC wideband buffers to audit occupied bandwidth, sideband conventions (USB vs. LSB), and the crucial DC carrier leakage for QO-100 operations.

  _`--loopback` (supported by `--test-spec-tx-shape`, `--test-spec-tx-wideband`, and `--test-dma-carrier-offset`): routes the capture through the AD9361 BIST (Built-In Self-Test) digital loopback, bypassing DAC/RF/LO/ADC. A spur/impairment that persists in loopback is digital/FPGA; one that disappears is analog/RF. Falls back to the normal RF path (with a printed notice) if the loopback mode is unavailable in the running firmware._

### 7. Timing & Pacing Tests (`timing_pacing.rs`)

- **`--test-pacing-dma-delay [rate_hz]`**: Measures blocking duration of TX `push()`. Verifies that backpressure from the FPGA pacing strobe correctly throttles DMA writes to match real-time.
