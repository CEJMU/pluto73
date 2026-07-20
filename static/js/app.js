import { formatFrequency, formatHzToMhz, formatHzToMhzPrecise, formatHzToMsps, formatHzShort } from './format.js';
import { playAudioChunk, initAudioUI } from './audio.js';
import { initTx, syncTxToRx, applyConnectionState } from './tx.js';

export { sendCommand, sendBinary, updateStatusBar };

// --- DOM Element Declarations ---
const runStatusToggle = document.getElementById('run-status-toggle');
const setFreqButton = document.getElementById('set-frequency');
const frequencyInput = document.getElementById('frequency');
const statusLabel = document.getElementById('status');
const canvas = document.getElementById('waterfallCanvas');
const ctx = canvas.getContext('2d');
const centerFreqInput = document.getElementById('center-freq');
const setCenterFreqButton = document.getElementById('set-center-freq');
const modeSelect = document.getElementById('mode-select');
const filterBwInput = document.getElementById('filter-bw');
const setFilterBwButton = document.getElementById('set-filter-bw');
const waterfallSpeedSelect = document.getElementById('waterfall-speed');
const waterfallFftSizeSelect = document.getElementById('waterfall-fft-size');
const antennaSelect = document.getElementById('antenna-select');
const muteCheckbox = document.getElementById('mute-checkbox');
const hoverTooltip = document.getElementById('hover-tooltip');

// TX Mode/Bandwidth selectors (read by the status bar; controls live in tx.js)
const txModeSelect = document.getElementById('tx-mode-select');
const txFilterBwInput = document.getElementById('tx-filter-bw');
const txOffsetInput = document.getElementById('tx-offset');

const rxGainModeSelect = document.getElementById('rx-gain-mode');
const rxGainSlider = document.getElementById('rx-gain');
const rxGainVal = document.getElementById('rx-gain-val');
const txGainSlider = document.getElementById('tx-gain');
const txGainVal = document.getElementById('tx-gain-val');
const telemetryTemp = document.getElementById('telemetry-temp');
const telemetryVccint = document.getElementById('telemetry-vccint');
const telemetryVccoddr = document.getElementById('telemetry-vccoddr');
const telemetryVccintSpan = document.getElementById('telemetry-vccint-span');
const telemetryVccoddrSpan = document.getElementById('telemetry-vccoddr-span');

const rfBandwidthInput = document.getElementById('rf-bandwidth');
const setRfBandwidthButton = document.getElementById('set-rf-bandwidth');
const syncRfBwCheckbox = document.getElementById('sync-rf-bw');
const wfMinDbSlider = document.getElementById('wf-min-db');
const wfMinDbVal = document.getElementById('wf-min-db-val');
const wfMaxDbSlider = document.getElementById('wf-max-db');
const wfMaxDbVal = document.getElementById('wf-max-db-val');
const wfResetButton = document.getElementById('wf-reset');

const zoomStatusEl = document.getElementById('zoom-status');
const visualSpanEl = document.getElementById('visual-span-val');
const hardwareSpanEl = document.getElementById('hardware-span-val');
const loFreqEl = document.getElementById('lo-freq-val');
const sampleRateEl = document.getElementById('sample-rate-val');
const resolutionEl = document.getElementById('resolution-val');

const txStatusModeVal = document.getElementById('tx-status-mode-val');
const txStatusBwVal = document.getElementById('tx-status-bw-val');
const txStatusRateVal = document.getElementById('tx-status-rate-val');
const txStatusLoVal = document.getElementById('tx-status-lo-val');
const txStatusOffsetVal = document.getElementById('tx-status-offset-val');
const txStatusGainVal = document.getElementById('tx-status-gain-val');

// --- Global Application State ---
let isRunning = true; // Matches the backend's default running state on connect
let cachedRowData = null;
const rowHistory = [];
let ws = null;
let wsReconnectTimer = null;
let frequencyTimeout = null;
let currentCenterHz = 99300000;         // Current center of visual window (e.g. 99.3 MHz)
let currentBandwidthHz = 3840000;        // Current visible bandwidth (can change via zoom)
let hardwareLoHz = 99300000;            // Target hardware LO frequency
let sdrHardwareLoHz = hardwareLoHz;      // Current streaming hardware LO frequency
let sdrBandwidthHz = currentBandwidthHz;  // Current streaming sample rate/bandwidth
let minHardwareSpanHz = 3840000;         // Minimum supported hardware span for streaming audio
let isConfigInitialized = false;
const axisHeight = 30;                   // Height of the frequency scale axis (CSS pixels)
// Scales the canvas backing buffer to the display's true resolution (capped at 2x) to avoid blur.
let dpr = Math.min(window.devicePixelRatio || 1, 2);
function axisHeightPx() { return Math.round(axisHeight * dpr); }  // axisHeight in backing-buffer pixels, for the waterfall region math.
function rowStepPx() { return Math.max(1, Math.round(dpr)); }     // Backing-buffer pixels per waterfall row

let isWaitingForHardware = false;        // Blocks rendering during tuning to avoid buffer corruption
let awaitingFirstRow = false;            // After Config: still dropping settling frames until real signal
let settleFallbackTimer = null;          // Safety valve: force-resume if no valid row ever arrives

// A settling burst reads as an all-near-zero row (solid blue band); real spectrum sits above 0.
function isEmptyRow(row) {
  for (let i = 0; i < row.length; i++) {
    if (row[i] > 2) return false;
  }
  return true;
}

// Force rendering to resume after `ms`, so a failed/silent reconfig can't freeze the waterfall.
function armSettleFallback(ms) {
  if (settleFallbackTimer) clearTimeout(settleFallbackTimer);
  settleFallbackTimer = setTimeout(() => {
    isWaitingForHardware = false;
    awaitingFirstRow = false;
    settleFallbackTimer = null;
  }, ms);
}

// Demodulator Settings
let currentMode = 'FM';
let currentFilterBw = 15000;

// TX DDS offset: the TX LO sits this far below the listening frequency; the FPGA DDS shifts
// the signal back up. Keeps TX LO leakage (carrier spike) away from the transmitted signal.
let txOffsetHz = 50000;

// Largest usable TX offset at the current rate: the TX sample rate follows the RX hardware
// span and the analog TX bandwidth equals that rate, so the shifted signal must stay within
// +-fs/2 (20 kHz margin covers the widest TX filter). Mirrors max_tx_offset() in the backend.
function maxTxOffsetHz() {
  return sdrBandwidthHz / 2 - 20000;
}

function clampTxOffset(hz) {
  const limit = maxTxOffsetHz();
  return Math.max(-limit, Math.min(limit, hz));
}

// Dragging Interactions
let isDragging = false;
let dragMoved = false;
let dragStartX = 0;
let dragStartCenterHz = 0;
let isDraggingBar = false;               // Track dragging: 'carrier', 'left', 'right', 'lo', 'txlo', or false
let dragBarMoved = false;
let filterBwTimeout = null;

// Zoom & Keyboard Panning
let keyboardPanTimeout = null;
let zoomTimeout = null;
// Serialize span reconfigs (one in flight): rapid zooming would otherwise queue many slow AD9361
// retunes. Later zooms just flag a resend; the latest view is sent when the current one is acked.
let spanReconfigInFlight = false;
let spanReconfigQueued = false;
let spanReconfigTimer = null;
// Track the span/LO we last requested so Config acks from unrelated commands
// (demod changes, antenna, etc.) don't falsely release the in-flight lock.
let spanReconfigExpectedSpanHz = null;
let spanReconfigExpectedLoHz = null;
const ZOOM_STEPS = [
  12500, 25000, 50000, 100000, 250000, 500000, 720000, 960000,
  1200000, 1440000, 1680000, 1920000, 2160000, 2400000, 3000000,
  3600000, 4800000, 6000000, 8000000, 10000000, 15000000, 20000000, 30000000
];

// --- Helper & Formatting Functions ---

// Updates connection state labels
function updateStatus(text) {
  statusLabel.textContent = text;
  statusLabel.style.color = text === 'connected' ? '#00FF00' : 'red';
}

// Reflects isRunning in the toggleable Started/Stopped badge
function updateRunStatusBadge() {
  if (!runStatusToggle) return;
  runStatusToggle.textContent = isRunning ? 'Started' : 'Stopped';
  runStatusToggle.classList.toggle('is-running', isRunning);
  runStatusToggle.classList.toggle('is-stopped', !isRunning);
}

// Redraws the status details bar with active settings and zoom state
function updateStatusBar() {
  if (!zoomStatusEl) return;

  if (currentBandwidthHz < minHardwareSpanHz) {
    zoomStatusEl.textContent = 'Software Zoom';
    zoomStatusEl.style.color = '#ff9800';
  } else {
    zoomStatusEl.textContent = 'Hardware Adjusted';
    zoomStatusEl.style.color = '#4caf50';
  }

  visualSpanEl.textContent = formatHzToMhz(currentBandwidthHz);
  hardwareSpanEl.textContent = formatHzToMhz(sdrBandwidthHz);
  loFreqEl.textContent = formatHzToMhzPrecise(sdrHardwareLoHz, 6);
  sampleRateEl.textContent = formatHzToMsps(sdrBandwidthHz);

  if (syncRfBwCheckbox && syncRfBwCheckbox.checked && rfBandwidthInput) {
    rfBandwidthInput.value = sdrBandwidthHz;
  }

  if (resolutionEl) {
    const fftSize = (rowHistory.length > 0 && rowHistory[0].row) ? rowHistory[0].row.length : 8192;
    const fftRes = sdrBandwidthHz / fftSize;
    const zoomFactor = 30000000 / currentBandwidthHz;

    let detailRating = 'Ultra-Fine';
    if (fftRes > 800) detailRating = 'Coarse';
    else if (fftRes > 350) detailRating = 'Medium';
    else if (fftRes > 180) detailRating = 'Fine';

    resolutionEl.textContent = `${zoomFactor.toFixed(1)}x (${detailRating}, ${formatHzShort(fftRes)})`;
  }

  // Update TX Status Bar elements
  const listeningHz = parseInt(frequencyInput.value, 10);
  if (txStatusModeVal && txModeSelect) {
    txStatusModeVal.textContent = txModeSelect.value;
  }
  if (txStatusBwVal && txFilterBwInput) {
    txStatusBwVal.textContent = txFilterBwInput.value + ' Hz';
  }
  if (txStatusRateVal) {
    txStatusRateVal.textContent = '48.00 kHz';
  }
  if (txStatusLoVal && !Number.isNaN(listeningHz)) {
    // TX LO hardware sits txOffsetHz below playback_hz; the FPGA TX DDS brings the signal
    // back up to playback_hz.
    const txLoMhz = (listeningHz - txOffsetHz) / 1000000;
    txStatusLoVal.textContent = txLoMhz.toFixed(6) + ' MHz';
  }
  if (txStatusOffsetVal) {
    const sign = txOffsetHz >= 0 ? '+' : '';
    txStatusOffsetVal.textContent = `${sign}${formatHzShort(txOffsetHz)} (FPGA DDS)`;
  }
  if (txStatusGainVal && txGainSlider) {
    txStatusGainVal.textContent = parseFloat(txGainSlider.value).toFixed(1) + ' dB';
  }
}

// Enables/disables controls based on connection state
function updatePlaybackAbility() {
  runStatusToggle.disabled = !window.isConnected;
  setFreqButton.disabled = !window.isConnected;

  if (muteCheckbox) muteCheckbox.disabled = !window.isConnected;
  
  if (rxGainModeSelect) rxGainModeSelect.disabled = !window.isConnected;
  if (rxGainSlider) {
    rxGainSlider.disabled = !window.isConnected || (rxGainModeSelect && rxGainModeSelect.value !== 'manual');
  }
  if (txGainSlider) txGainSlider.disabled = !window.isConnected;
  if (syncRfBwCheckbox) syncRfBwCheckbox.disabled = !window.isConnected;
  if (rfBandwidthInput) {
    rfBandwidthInput.disabled = !window.isConnected || (syncRfBwCheckbox && syncRfBwCheckbox.checked);
  }
  if (setRfBandwidthButton) {
    setRfBandwidthButton.disabled = !window.isConnected || (syncRfBwCheckbox && syncRfBwCheckbox.checked);
  }

  applyConnectionState(window.isConnected);
}

// Caches connection state and updates status bar
function setConnected(isConnected) {
  window.isConnected = isConnected;
  if (isConnected) {
    isRunning = true; // Backend starts running by default on connect
    updateRunStatusBadge();
  }
  updatePlaybackAbility();
  updateStatus(isConnected ? 'connected' : 'disconnected');
}

// Sends current waterfall DB scaling configuration to the backend
function updateWaterfallScale() {
  if (!wfMinDbSlider || !wfMaxDbSlider) return;
  const min_db = parseFloat(wfMinDbSlider.value);
  const max_db = parseFloat(wfMaxDbSlider.value);
  sendCommand({
    type: 'SetRxWaterfallScale',
    payload: { min_db, max_db }
  });
}

// --- WebSocket Connection & Communication ---

// Sends a JSON command to the backend
function sendCommand(command) {
  if (!ws || ws.readyState !== WebSocket.OPEN) {
    console.warn('WebSocket not open');
    return;
  }
  console.log('[WS Control Command]', command);
  ws.send(JSON.stringify(command));
}

// Sends raw binary (used by TX)
function sendBinary(buffer) {
  if (!ws || ws.readyState !== WebSocket.OPEN) return false;
  ws.send(buffer);
  return true;
}

// Handles raw binary data from WebSocket (waterfall lines and audio PCM)
function handleBinaryMessage(arrayBuffer) {
  const view = new DataView(arrayBuffer);
  const header = view.getUint8(0);
  
  if (header === 0) {
    // Waterfall row
    const values = new Uint8Array(arrayBuffer, 4);
    appendWaterfallRow(values);
  } else if (header === 1) {
    // Audio PCM chunk
    if (arrayBuffer.byteLength < 8) {
      console.warn("Received malformed audio chunk with byte length:", arrayBuffer.byteLength);
      return;
    }
    const pcm = new Float32Array(arrayBuffer, 4);
    playAudioChunk(pcm);
  } else {
    console.warn("Unknown binary message header:", header);
  }
}

// Handles structured text configuration/status/telemetry from WebSocket
function handleTextMessage(data) {
  if (data.type === 'Status') {
    updateStatus(data.payload.state);
  } else if (data.type === 'Config') {
    handleConfigUpdate(data.payload);
  } else if (data.type === 'Settings') {
    handleSettingsUpdate(data.payload);
  } else if (data.type === 'Telemetry') {
    handleTelemetryUpdate(data.payload);
  }
}

// Sub-handler for Config updates
function handleConfigUpdate(payload) {
  sdrHardwareLoHz = payload.lo_hz;
  hardwareLoHz = sdrHardwareLoHz;
  sdrBandwidthHz = payload.sample_rate_hz;
  
  if (payload.min_span_hz) {
    minHardwareSpanHz = payload.min_span_hz;
  }

  // Mirror the backend: a shrunken hardware rate re-clamps the TX offset.
  const clampedTxOffset = clampTxOffset(txOffsetHz);
  if (clampedTxOffset !== txOffsetHz) {
    txOffsetHz = clampedTxOffset;
    if (txOffsetInput) txOffsetInput.value = txOffsetHz;
  }

  if (currentBandwidthHz > sdrBandwidthHz) {
    currentBandwidthHz = sdrBandwidthHz;
  }

  if (!isConfigInitialized) {
    isConfigInitialized = true;
    currentCenterHz = sdrHardwareLoHz;
    centerFreqInput.value = Math.round(currentCenterHz);
    if (rfBandwidthInput) {
      rfBandwidthInput.value = sdrBandwidthHz;
    }
  } else if (syncRfBwCheckbox && syncRfBwCheckbox.checked) {
    if (rfBandwidthInput) {
      rfBandwidthInput.value = sdrBandwidthHz;
    }
  }

  redrawWaterfallFromHistory();

  // Span reconfig acknowledged: release the lock and, if the user kept zooming, send one more
  // SetRxSpan for the latest view (coalescing into a single final retune).
  // Only release if this Config actually matches the span we requested. Otherwise it's an
  // unrelated Config (demod change, antenna, etc.) and the span retune is still pending.
  if (spanReconfigInFlight &&
      spanReconfigExpectedSpanHz !== null &&
      sdrBandwidthHz === spanReconfigExpectedSpanHz &&
      Math.abs(sdrHardwareLoHz - spanReconfigExpectedLoHz) < 100) {
    spanReconfigInFlight = false;
    spanReconfigExpectedSpanHz = null;
    spanReconfigExpectedLoHz = null;
    if (spanReconfigTimer) {
      clearTimeout(spanReconfigTimer);
      spanReconfigTimer = null;
    }
    if (spanReconfigQueued) {
      spanReconfigQueued = false;
      sendSpanUpdate();
    }
  }

  // Retune done, but the first bursts are still settling. Stay gated; appendWaterfallRow
  // resumes on the first row with real signal.
  if (isWaitingForHardware) {
    awaitingFirstRow = true;
    armSettleFallback(3000);
  }

  updatePlaybackAbility();
}

// Sub-handler for Settings updates
function handleSettingsUpdate(payload) {
  console.log('[WS Settings Update] Received active settings from server:', payload);
  const newFreq = payload.playback_hz;
  if (frequencyInput) {
    frequencyInput.value = newFreq;
  }

  const newMode = payload.demod_mode;
  currentMode = newMode;
  if (modeSelect) {
    modeSelect.value = newMode;
  }

  const newFilterBw = payload.filter_bw_hz;
  currentFilterBw = newFilterBw;
  if (filterBwInput) {
    filterBwInput.value = newFilterBw;
  }

  const audioEnabled = payload.audio_enabled;
  if (muteCheckbox) {
    muteCheckbox.checked = !audioEnabled;
  }

  const intervalMs = payload.waterfall_interval_ms;
  if (waterfallSpeedSelect) {
    waterfallSpeedSelect.value = intervalMs;
  }

  const fftSize = payload.waterfall_fft_size;
  if (waterfallFftSizeSelect && fftSize) {
    waterfallFftSizeSelect.value = fftSize;
  }

  const gainMode = payload.rx_gain_mode;
  if (rxGainModeSelect) {
    rxGainModeSelect.value = gainMode;
  }

  const gainDb = payload.rx_gain_db;
  if (rxGainSlider) {
    rxGainSlider.value = gainDb;
    if (rxGainVal) {
      rxGainVal.textContent = gainDb.toFixed(1) + ' dB';
    }
    rxGainSlider.disabled = !window.isConnected || gainMode !== 'manual';
  }

  if (payload.tx_offset_hz !== undefined) {
    txOffsetHz = payload.tx_offset_hz;
    if (txOffsetInput) {
      txOffsetInput.value = txOffsetHz;
    }
  }

  const rfBw = payload.rf_bandwidth_hz;
  if (rfBandwidthInput) {
    if (rfBw === 0) {
      rfBandwidthInput.value = sdrBandwidthHz;
      if (syncRfBwCheckbox) syncRfBwCheckbox.checked = true;
    } else {
      rfBandwidthInput.value = rfBw;
      if (syncRfBwCheckbox) syncRfBwCheckbox.checked = false;
    }
  }

  const minDb = payload.waterfall_min_db;
  const maxDb = payload.waterfall_max_db;
  if (wfMinDbSlider) {
    wfMinDbSlider.value = minDb;
    if (wfMinDbVal) wfMinDbVal.textContent = minDb.toFixed(0) + ' dB';
  }
  if (wfMaxDbSlider) {
    wfMaxDbSlider.value = maxDb;
    if (wfMaxDbVal) wfMaxDbVal.textContent = maxDb.toFixed(0) + ' dB';
  }

  drawAxis(canvas.width / dpr, canvas.height / dpr);
  redrawWaterfallFromHistory();
  updatePlaybackAbility();
}

// Sub-handler for Telemetry updates
function handleTelemetryUpdate(payload) {
  const temp = payload.temp_c;
  const vccint = payload.vccint_v;
  const vccoddr = payload.vccoddr_v;

  if (telemetryTemp) {
    telemetryTemp.textContent = temp.toFixed(1) + ' °C';
  }
  
  if (vccint > 0.0) {
    if (telemetryVccint) telemetryVccint.textContent = vccint.toFixed(3) + ' V';
    if (telemetryVccintSpan) telemetryVccintSpan.style.display = 'inline';
  } else {
    if (telemetryVccintSpan) telemetryVccintSpan.style.display = 'none';
  }

  if (vccoddr > 0.0) {
    if (telemetryVccoddr) telemetryVccoddr.textContent = vccoddr.toFixed(3) + ' V';
    if (telemetryVccoddrSpan) telemetryVccoddrSpan.style.display = 'inline';
  } else {
    if (telemetryVccoddrSpan) telemetryVccoddrSpan.style.display = 'none';
  }
}

// Connects/Reconnects WebSocket connection
function connect() {
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
    return;
  }

  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const address = `${protocol}//${window.location.host}/ws`;
  updateStatus('connecting');
  ws = new WebSocket(address);
  ws.binaryType = 'arraybuffer';

  if (wsReconnectTimer) {
    clearTimeout(wsReconnectTimer);
  }
  wsReconnectTimer = setTimeout(() => {
    if (ws && ws.readyState === WebSocket.CONNECTING) {
      console.warn('WebSocket still connecting after 3s; retrying');
      ws.close();
    }
  }, 3000);

  ws.addEventListener('open', () => {
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
    setConnected(true);
  });

  ws.addEventListener('message', (event) => {
    if (event.data instanceof ArrayBuffer) {
      handleBinaryMessage(event.data);
    } else {
      try {
        const data = JSON.parse(event.data);
        handleTextMessage(data);
      } catch (err) {
        console.warn('Failed to parse message', err);
      }
    }
  });

  ws.addEventListener('close', () => {
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
    setConnected(false);
    if (telemetryTemp) telemetryTemp.textContent = '-- °C';
    if (telemetryVccint) telemetryVccint.textContent = '-- V';
    if (telemetryVccoddr) telemetryVccoddr.textContent = '-- V';
    setTimeout(connect, 2000);
  });

  ws.addEventListener('error', (err) => {
    console.error("WebSocket error:", err);
    if (ws && ws.readyState !== WebSocket.CLOSED && ws.readyState !== WebSocket.CLOSING) {
      ws.close();
    }
  });
}

// --- DSP & Tuning Math Helpers ---

// Returns [lowHz, highHz] limits of the demodulator passband around listeningHz
function filterBandEdges(listeningHz) {
  if (Number.isNaN(listeningHz)) return null;
  if (currentMode === 'FM') return [listeningHz - currentFilterBw / 2, listeningHz + currentFilterBw / 2];
  if (currentMode === 'USB') return [listeningHz, listeningHz + currentFilterBw];
  if (currentMode === 'LSB') return [listeningHz - currentFilterBw, listeningHz];
  return null;
}

// Clamps listening frequency to the current physical SDR bandwidth limits
function clampListeningHz(hz) {
  const halfSpan = sdrBandwidthHz / 2;
  return Math.max(sdrHardwareLoHz - halfSpan, Math.min(sdrHardwareLoHz + halfSpan, hz));
}

// Computes the desired hardware LO to center visualBw at centerHz.
// Offsets the LO to push any potential DC leakage spike outside the visible window.
function desiredHardwareLo(centerHz, visualBw, hwSpan, listeningHz) {
  const maxShift = hwSpan * 0.5 - visualBw * 0.5 - hwSpan * 0.02;
  if (maxShift <= 0) return Math.round(centerHz);
  const shift = Math.min(visualBw * 0.6, maxShift);
  const listenerBelowCenter = !Number.isNaN(listeningHz) && listeningHz < centerHz;
  return Math.round(listenerBelowCenter ? centerHz + shift : centerHz - shift);
}

// Triggers center frequency tuning request if hardware LO needs to be adjusted
function updateHardwareLo() {
  if (!window.isConnected) return;

  // When zoomed in, panning is often purely visual: if the visible window still fits inside
  // the current hardware capture window, keep the LO where it is so the AD9361 doesn't need
  // to retune and the movement stays instantaneous.
  const margin = sdrBandwidthHz * 0.02;
  const viewStartHz = currentCenterHz - currentBandwidthHz / 2;
  const viewEndHz = currentCenterHz + currentBandwidthHz / 2;
  const hwStartHz = sdrHardwareLoHz - sdrBandwidthHz / 2;
  const hwEndHz = sdrHardwareLoHz + sdrBandwidthHz / 2;
  if (viewStartHz >= hwStartHz + margin && viewEndHz <= hwEndHz - margin) {
    hardwareLoHz = sdrHardwareLoHz;
    return;
  }

  const listeningHz = parseInt(frequencyInput.value, 10);
  const targetLo = desiredHardwareLo(currentCenterHz, currentBandwidthHz, sdrBandwidthHz, listeningHz);
  hardwareLoHz = targetLo;
  if (Math.round(targetLo) !== Math.round(sdrHardwareLoHz)) {
    isWaitingForHardware = true;
    awaitingFirstRow = false;
    armSettleFallback(3000);
    sendCommand({ type: 'SetRxCenterFrequency', payload: { hz: targetLo } });
  }
}

// Helper to determine the next bandwidth zoom step from ZOOM_STEPS
function getNextZoomStep(currentBw, zoomOut) {
  let idx = ZOOM_STEPS.indexOf(currentBw);
  if (idx === -1) {
    let minDiff = Infinity;
    for (let i = 0; i < ZOOM_STEPS.length; i++) {
      const diff = Math.abs(ZOOM_STEPS[i] - currentBw);
      if (diff < minDiff) {
        minDiff = diff;
        idx = i;
      }
    }
  }
  idx = zoomOut ? Math.min(ZOOM_STEPS.length - 1, idx + 1) : Math.max(0, idx - 1);
  return ZOOM_STEPS[idx];
}

// Debounces physical SDR span updates to the backend
function scheduleRxSpanUpdate() {
  // Freeze rendering right away: rows still arriving were captured at the old span/LO and would
  // render misaligned against the new view. Resumes on the first valid row after Config.
  isWaitingForHardware = true;
  awaitingFirstRow = false;
  armSettleFallback(3000);
  if (zoomTimeout) clearTimeout(zoomTimeout);
  zoomTimeout = setTimeout(() => {
    zoomTimeout = null;
    sendSpanUpdate();
  }, 250);
}

// Sends one SetRxSpan, honouring the single-in-flight rule: if a reconfig is already pending the
// request is deferred and re-issued on the next Config. The timer guards against a dropped Config.
function sendSpanUpdate() {
  if (spanReconfigInFlight) {
    spanReconfigQueued = true;
    return;
  }
  spanReconfigInFlight = true;
  spanReconfigQueued = false;
  if (spanReconfigTimer) clearTimeout(spanReconfigTimer);
  spanReconfigTimer = setTimeout(() => {
    spanReconfigInFlight = false;
    spanReconfigExpectedSpanHz = null;
    spanReconfigExpectedLoHz = null;
    spanReconfigTimer = null;
    if (spanReconfigQueued) sendSpanUpdate();
  }, 3000);
  const requestedHardwareSpan = Math.max(minHardwareSpanHz, currentBandwidthHz);
  spanReconfigExpectedSpanHz = requestedHardwareSpan;
  spanReconfigExpectedLoHz = Math.round(hardwareLoHz);
  sendCommand({
    type: 'SetRxSpan',
    payload: { center_hz: Math.round(hardwareLoHz), span_hz: requestedHardwareSpan }
  });
}

// --- Waterfall Drawing & Rendering ---

// Draws the frequency ticks, passband box, and DC/TX LO markers on the bottom axis. width/height
// are CSS pixels; setTransform scales to the backing buffer so text/ticks stay crisp on HiDPI.
function drawAxis(width, height) {
  ctx.save();
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const axisY = height - axisHeight;
  ctx.fillStyle = '#111'; // Dark background
  ctx.fillRect(0, axisY, width, axisHeight);

  ctx.fillStyle = '#fff'; // White text
  ctx.font = '16px monospace';
  ctx.textBaseline = 'middle';

  const startHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const endHz = currentCenterHz + (currentBandwidthHz * 0.5);

  const minTickSpacingPixels = 40;
  const minTickHz = (minTickSpacingPixels / width) * currentBandwidthHz;
  const magnitude = Math.pow(10, Math.floor(Math.log10(minTickHz)));
  const normalized = minTickHz / magnitude;

  let interval;
  if (normalized <= 1.0) interval = 1;
  else if (normalized <= 2.0) interval = 2;
  else if (normalized <= 5.0) interval = 5;
  else interval = 10;
  
  const tickIntervalHz = interval * magnitude;
  const subTickIntervalHz = tickIntervalHz / 10;

  const firstTickHz = Math.ceil(startHz / subTickIntervalHz) * subTickIntervalHz;
  let lastTextRight = -100;
  ctx.textAlign = 'center';

  for (let freqHz = firstTickHz; freqHz <= endHz; freqHz += subTickIntervalHz) {
    const x = ((freqHz - startHz) / currentBandwidthHz) * width;
    const tickIndex = Math.abs(Math.round(freqHz / subTickIntervalHz)) % 10;

    if (tickIndex === 0) {
      ctx.fillRect(x - 1, axisY, 2, 8); // Major ticks

      const freqStr = formatFrequency(freqHz, tickIntervalHz);
      const textWidth = ctx.measureText(freqStr).width;
      const textLeft = x - textWidth / 2;
      const textRight = x + textWidth / 2;

      if (textLeft > 0 && textRight < width && textLeft > lastTextRight + 5) {
        ctx.fillText(freqStr, x, axisY + axisHeight / 2 + 1);
        lastTextRight = textRight;
      }
    } else if (tickIndex === 5) {
      ctx.fillRect(x, axisY, 1, 6); // Medium ticks
    } else {
      ctx.fillRect(x, axisY, 1, 4); // Minor ticks
    }
  }

  // Draw demodulation passband box & carrier line
  const listeningHz = parseInt(frequencyInput.value, 10);
  if (!Number.isNaN(listeningHz) && listeningHz >= startHz && listeningHz <= endHz) {
    const edges = filterBandEdges(listeningHz);
    if (edges) {
      const [boxStartHz, boxEndHz] = edges;
      const boxStartX = Math.round(((boxStartHz - startHz) / currentBandwidthHz) * width);
      const boxEndX = Math.round(((boxEndHz - startHz) / currentBandwidthHz) * width);

      ctx.fillStyle = 'rgba(0, 255, 0, 0.3)';
      ctx.fillRect(boxStartX, axisY, boxEndX - boxStartX, axisHeight);
    }

    const listenX = Math.round(((listeningHz - startHz) / currentBandwidthHz) * width);
    ctx.fillStyle = '#00FF00';
    ctx.fillRect(listenX, axisY, 1, axisHeight);
  }

  // Draw SDR Center / DC Spike Indicator (Orange Vertical Bar on the Axis)
  const sdrCenterX = Math.round(((hardwareLoHz - startHz) / currentBandwidthHz) * width);
  if (sdrCenterX >= 0 && sdrCenterX <= width) {
    ctx.fillStyle = '#ff9800'; // Amber/Orange
    ctx.fillRect(sdrCenterX - 1, axisY, 2, axisHeight);
  }

  // Draw TX LO Indicator (Magenta/Pink bar on axis)
  if (!Number.isNaN(listeningHz)) {
    const txLoHz = listeningHz - txOffsetHz;
    const txLoX = Math.round(((txLoHz - startHz) / currentBandwidthHz) * width);
    if (txLoX >= 0 && txLoX <= width) {
      ctx.fillStyle = '#ff5599'; // Bright Magenta/Pink
      ctx.fillRect(txLoX - 1, axisY, 2, axisHeight);
    }
  }

  ctx.restore();
}

// Paints a single waterfall row into an RGBA pixel buffer
function paintWaterfallRow(pixels, rowOffset, row, histStartHz, histBandwidthHz, viewStartHz, viewBandwidthHz, width, listenX, boxStartX, boxEndX) {
  const binWidthInPixels = (histBandwidthHz / row.length) / (viewBandwidthHz / width);
  for (let x = 0; x < width; x++) {
    let intensity = 0;
    let outOfBounds = false;

    if (binWidthInPixels > 1.0) {
      const freq = viewStartHz + ((x + 0.5) / width) * viewBandwidthHz;
      if (freq < histStartHz || freq > histStartHz + histBandwidthHz) {
        outOfBounds = true;
      } else {
        const binIdx = ((freq - histStartHz) / histBandwidthHz) * row.length;
        const clampedIdx = Math.max(0, Math.min(row.length - 1, binIdx));
        const idxL = Math.floor(clampedIdx);
        const idxR = Math.min(row.length - 1, idxL + 1);
        const frac = clampedIdx - idxL;
        intensity = row[idxL] * (1 - frac) + row[idxR] * frac;
      }
    } else {
      const startFreq = viewStartHz + (x / width) * viewBandwidthHz;
      const endFreq = viewStartHz + ((x + 1) / width) * viewBandwidthHz;

      if (endFreq < histStartHz || startFreq > histStartHz + histBandwidthHz) {
        outOfBounds = true;
      } else {
        let startBin = Math.floor(((startFreq - histStartHz) / histBandwidthHz) * row.length);
        let endBin = Math.floor(((endFreq - histStartHz) / histBandwidthHz) * row.length);

        if (startBin > endBin) {
          const tmp = startBin; startBin = endBin; endBin = tmp;
        }

        startBin = Math.max(0, startBin);
        endBin = Math.min(row.length, endBin);
        if (endBin === startBin && startBin < row.length) endBin = startBin + 1;

        let maxVal = 0;
        if (startBin < row.length && endBin > 0) {
          for (let i = startBin; i < endBin; i++) {
            if (row[i] > maxVal) maxVal = row[i];
          }
        }
        intensity = maxVal;
      }
    }

    const offset = rowOffset + x * 4;

    if (listenX !== -1 && x === listenX) {
      pixels[offset] = 0;
      pixels[offset + 1] = 255;
      pixels[offset + 2] = 0;
      pixels[offset + 3] = 255;
    } else if (outOfBounds) {
      pixels[offset] = 0;
      pixels[offset + 1] = 0;
      pixels[offset + 2] = 0;
      pixels[offset + 3] = 255;
    } else if (listenX !== -1 && x >= boxStartX && x <= boxEndX) {
      pixels[offset] = Math.max(0, intensity - 30);
      pixels[offset + 1] = Math.min(255, Math.round(intensity / 2) + 50);
      pixels[offset + 2] = Math.max(0, 255 - intensity - 30);
      pixels[offset + 3] = 255;
    } else {
      pixels[offset] = intensity;
      pixels[offset + 1] = Math.round(intensity / 2);
      pixels[offset + 2] = 255 - intensity;
      pixels[offset + 3] = 255;
    }
  }
}

// Redraws the waterfall display from history buffer
function redrawWaterfallFromHistory() {
  const width = canvas.width;
  const wfHeight = canvas.height - axisHeightPx();
  ctx.fillStyle = '#000';
  ctx.fillRect(0, 0, width, wfHeight);

  const currentStartHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const listeningHz = parseInt(frequencyInput.value, 10);
  const listenX = (!Number.isNaN(listeningHz) && listeningHz >= currentStartHz && listeningHz <= currentStartHz + currentBandwidthHz)
    ? Math.round(((listeningHz - currentStartHz) / currentBandwidthHz) * width)
    : -1;

  let boxStartX = -1, boxEndX = -1;
  const edges = listenX !== -1 ? filterBandEdges(listeningHz) : null;
  if (edges) {
    boxStartX = Math.round(((edges[0] - currentStartHz) / currentBandwidthHz) * width);
    boxEndX = Math.round(((edges[1] - currentStartHz) / currentBandwidthHz) * width);
  }

  const imgData = ctx.createImageData(width, wfHeight);
  const data = imgData.data;

  const rowStep = rowStepPx();
  const lineBytes = width * 4;
  for (let y = 0; y < rowHistory.length; y++) {
    const hist = rowHistory[y];
    const histStartHz = hist.hardwareLoHz - (hist.bandwidthHz * 0.5);
    const topLine = wfHeight - (y + 1) * rowStep;
    if (topLine < 0) break;
    const base = topLine * lineBytes;
    paintWaterfallRow(data, base, hist.row, histStartHz, hist.bandwidthHz,
      currentStartHz, currentBandwidthHz, width, listenX, boxStartX, boxEndX);
    // Replicate the painted line to fill the row's full height (cheap byte copy, no re-interp).
    for (let i = 1; i < rowStep; i++) {
      data.copyWithin(base + i * lineBytes, base, base + lineBytes);
    }
  }
  ctx.putImageData(imgData, 0, 0);
  drawAxis(canvas.width / dpr, canvas.height / dpr);
  updateStatusBar();
}

// Appends a new incoming DSP row to the history and shifts display pixels upward
function appendWaterfallRow(row) {
  if (isWaitingForHardware) {
    // Reconfiguring: drop rows until Config lands, then drop settling frames and resume on the
    // first row with real signal.
    if (!awaitingFirstRow || isEmptyRow(row)) return;
    isWaitingForHardware = false;
    awaitingFirstRow = false;
    if (settleFallbackTimer) {
      clearTimeout(settleFallbackTimer);
      settleFallbackTimer = null;
    }
  }

  const width = canvas.width;
  const height = canvas.height;
  const wfHeight = height - axisHeightPx();
  const rowStep = rowStepPx();

  rowHistory.unshift({
    row: new Uint8Array(row),
    hardwareLoHz: sdrHardwareLoHz,
    bandwidthHz: sdrBandwidthHz
  });
  if (rowHistory.length > Math.floor(wfHeight / rowStep)) rowHistory.pop();

  ctx.drawImage(canvas, 0, rowStep, width, wfHeight - rowStep, 0, 0, width, wfHeight - rowStep);

  if (!cachedRowData || cachedRowData.width !== width) {
    cachedRowData = ctx.createImageData(width, 1);
  }
  const rowData = cachedRowData;
  const pixels = rowData.data;

  const startHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const endHz = currentCenterHz + (currentBandwidthHz * 0.5);
  const sdrStartHz = sdrHardwareLoHz - (sdrBandwidthHz * 0.5);

  const listeningHz = parseInt(frequencyInput.value, 10);
  const listenX = (!Number.isNaN(listeningHz) && listeningHz >= startHz && listeningHz <= endHz)
    ? Math.round(((listeningHz - startHz) / currentBandwidthHz) * width)
    : -1;

  let boxStartX = -1, boxEndX = -1;
  const edges = listenX !== -1 ? filterBandEdges(listeningHz) : null;
  if (edges) {
    boxStartX = Math.round(((edges[0] - startHz) / currentBandwidthHz) * width);
    boxEndX = Math.round(((edges[1] - startHz) / currentBandwidthHz) * width);
  }

  paintWaterfallRow(pixels, 0, row, sdrStartHz, sdrBandwidthHz,
    startHz, currentBandwidthHz, width, listenX, boxStartX, boxEndX);

  for (let i = 0; i < rowStep; i++) {
    ctx.putImageData(rowData, 0, wfHeight - rowStep + i);
  }
  drawAxis(width / dpr, height / dpr);
}

// --- Interactive Canvas Mouse & Gesture Listeners ---

canvas.addEventListener('mousedown', (e) => {
  dragBarMoved = false;
  dragMoved = false;
  const rect = canvas.getBoundingClientRect();
  const x = e.clientX - rect.left;
  const y = e.clientY - rect.top;

  const startHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const listeningHz = parseInt(frequencyInput.value, 10);

  if (!Number.isNaN(listeningHz)) {
    const edges = filterBandEdges(listeningHz);
    if (edges) {
      const [boxStartHz, boxEndHz] = edges;
      const listenClientX = ((listeningHz - startHz) / currentBandwidthHz) * rect.width;
      const startClientX = ((boxStartHz - startHz) / currentBandwidthHz) * rect.width;
      const endClientX = ((boxEndHz - startHz) / currentBandwidthHz) * rect.width;

      if (Math.abs(x - listenClientX) <= 5) {
        isDraggingBar = 'carrier';
        dragBarMoved = false;
        return;
      } else if ((currentMode === 'FM' || currentMode === 'LSB') && Math.abs(x - startClientX) <= 5) {
        isDraggingBar = 'left';
        dragBarMoved = false;
        return;
      } else if ((currentMode === 'FM' || currentMode === 'USB') && Math.abs(x - endClientX) <= 5) {
        isDraggingBar = 'right';
        dragBarMoved = false;
        return;
      }
    }
  }

  // Check if dragging the orange LO center bar on the axis
  const sdrCenterX = ((hardwareLoHz - startHz) / currentBandwidthHz) * rect.width;
  const isInAxis = y >= (rect.height - axisHeight);
  if (isInAxis && Math.abs(x - sdrCenterX) <= 6) {
    isDraggingBar = 'lo';
    dragBarMoved = false;
    return;
  }

  // Check if dragging the pink TX LO bar on the axis (adjusts the TX DDS offset)
  if (!Number.isNaN(listeningHz)) {
    const txLoX = ((listeningHz - txOffsetHz - startHz) / currentBandwidthHz) * rect.width;
    if (isInAxis && Math.abs(x - txLoX) <= 6) {
      isDraggingBar = 'txlo';
      dragBarMoved = false;
      return;
    }
  }

  isDragging = true;
  dragMoved = false;
  dragStartX = e.clientX;
  dragStartCenterHz = currentCenterHz;
});

canvas.addEventListener('mousemove', (e) => {
  const rect = canvas.getBoundingClientRect();
  const x = e.clientX - rect.left;
  const y = e.clientY - rect.top;
  const freqHz = currentCenterHz - (currentBandwidthHz * 0.5) + (x / rect.width) * currentBandwidthHz;

  const binHz = currentBandwidthHz / rect.width;
  const startHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const sdrCenterX = ((hardwareLoHz - startHz) / currentBandwidthHz) * rect.width;

  const listeningHz = parseInt(frequencyInput.value, 10);
  const txLoHz = listeningHz - txOffsetHz;
  const txLoX = ((txLoHz - startHz) / currentBandwidthHz) * rect.width;

  const isNearSdrCenter = Math.abs(x - sdrCenterX) < 10;
  const isNearTxLo = !Number.isNaN(listeningHz) && Math.abs(x - txLoX) < 10;
  const isInAxis = y >= (rect.height - axisHeight);
  const isBigTooltip = (isNearSdrCenter || isNearTxLo) && isInAxis;

  // Set up DC spike warning / LO indicator tooltip
  if (isNearSdrCenter && isInAxis) {
    hoverTooltip.innerHTML =
      `<span style="color: #ff9800; font-weight: bold;">⚠️ SDR Center (Potential DC Spike)</span><br/>` +
      `<span style="color: #fff;">Freq: ${formatFrequency(hardwareLoHz, binHz)}</span><br/>` +
      `<span style="color: #aaa; font-size: 11px; font-family: sans-serif; line-height: 1.2;">Drag to retune hardware LO. LO leakage can cause a DC offset spike here.</span>`;
  } else if (isNearTxLo && isInAxis) {
    hoverTooltip.innerHTML =
      `<span style="color: #ff5599; font-weight: bold;">⚠️ TX LO (DDS Offset: ${formatHzShort(txOffsetHz)})</span><br/>` +
      `<span style="color: #fff;">Freq: ${formatFrequency(txLoHz, binHz)}</span><br/>` +
      `<span style="color: #aaa; font-size: 11px; font-family: sans-serif; line-height: 1.2;">The TX LO sits this offset below the listening frequency; the FPGA DDS shifts the signal back up. TX LO leakage can cause a carrier spike here — drag to change the offset.</span>`;
  } else {
    hoverTooltip.textContent = formatFrequency(freqHz, binHz);
  }

  let leftOffset = x + 15;
  let topOffset = y + 15;
  const tooltipApproxHeight = isBigTooltip ? 75 : 30;
  const tooltipApproxWidth = isBigTooltip ? 250 : 120;

  if (y + tooltipApproxHeight + 25 > rect.height) {
    topOffset = y - tooltipApproxHeight - 15;
  }
  if (x + tooltipApproxWidth + 25 > rect.width) {
    leftOffset = x - tooltipApproxWidth - 15;
  }

  hoverTooltip.style.left = leftOffset + 'px';
  hoverTooltip.style.top = topOffset + 'px';
  hoverTooltip.style.display = 'block';

  if (isDraggingBar) {
    dragBarMoved = true;
    canvas.style.cursor = 'ew-resize';
    
    if (isDraggingBar === 'carrier') {
      const carrierHz = Math.round(clampListeningHz(freqHz));
      frequencyInput.value = carrierHz;
      if (frequencyTimeout) clearTimeout(frequencyTimeout);
      frequencyTimeout = setTimeout(() => {
        sendCommand({ type: 'SetRxFrequency', payload: { hz: carrierHz } });
      }, 50);
    } else if (isDraggingBar === 'lo') {
      hardwareLoHz = Math.round(freqHz);
    } else if (isDraggingBar === 'txlo') {
      if (!Number.isNaN(listeningHz)) {
        txOffsetHz = clampTxOffset(Math.round(listeningHz - freqHz));
        if (txOffsetInput) txOffsetInput.value = txOffsetHz;
      }
    } else if (isDraggingBar === 'left') {
      const maxBw = (currentMode === 'USB' || currentMode === 'LSB') ? 20000 : 110000;
      if (currentMode === 'FM') currentFilterBw = Math.max(1000, Math.min(maxBw, (listeningHz - freqHz) * 2));
      else if (currentMode === 'LSB') currentFilterBw = Math.max(1000, Math.min(maxBw, listeningHz - freqHz));
      filterBwInput.value = Math.round(currentFilterBw);
      if (filterBwTimeout) clearTimeout(filterBwTimeout);
      filterBwTimeout = setTimeout(() => {
        sendCommand({ type: 'SetRxDemodulation', payload: { mode: currentMode, filter_bw_hz: currentFilterBw } });
      }, 50);
    } else if (isDraggingBar === 'right') {
      const maxBw = (currentMode === 'USB' || currentMode === 'LSB') ? 20000 : 110000;
      if (currentMode === 'FM') currentFilterBw = Math.max(1000, Math.min(maxBw, (freqHz - listeningHz) * 2));
      else if (currentMode === 'USB') currentFilterBw = Math.max(1000, Math.min(maxBw, freqHz - listeningHz));
      filterBwInput.value = Math.round(currentFilterBw);
      if (filterBwTimeout) clearTimeout(filterBwTimeout);
      filterBwTimeout = setTimeout(() => {
        sendCommand({ type: 'SetRxDemodulation', payload: { mode: currentMode, filter_bw_hz: currentFilterBw } });
      }, 50);
    }

    redrawWaterfallFromHistory();
    return;
  }

  if (isDragging) {
    const deltaX = e.clientX - dragStartX;
    if (Math.abs(deltaX) > 5) {
      dragMoved = true;
      canvas.style.cursor = 'grabbing';
    }
    // The hardware LO (and its orange marker) stays put while panning; updateHardwareLo
    // decides on mouseup whether a retune is actually needed.
    const deltaHz = (deltaX / rect.width) * currentBandwidthHz;
    currentCenterHz = dragStartCenterHz - deltaHz;
    centerFreqInput.value = Math.round(currentCenterHz);

    if (dragMoved) {
      redrawWaterfallFromHistory();
    }
  } else {
    let hoverEdge = false;
    if (!Number.isNaN(listeningHz)) {
      const edges = filterBandEdges(listeningHz);
      if (edges) {
        const [boxStartHz, boxEndHz] = edges;
        const listenClientX = ((listeningHz - startHz) / currentBandwidthHz) * rect.width;
        const startClientX = ((boxStartHz - startHz) / currentBandwidthHz) * rect.width;
        const endClientX = ((boxEndHz - startHz) / currentBandwidthHz) * rect.width;

        if (Math.abs(x - listenClientX) <= 5) hoverEdge = true;
        if ((currentMode === 'FM' || currentMode === 'LSB') && Math.abs(x - startClientX) <= 5) hoverEdge = true;
        if ((currentMode === 'FM' || currentMode === 'USB') && Math.abs(x - endClientX) <= 5) hoverEdge = true;

        const sdrCenterX_hover = ((hardwareLoHz - startHz) / currentBandwidthHz) * rect.width;
        const isInAxis_hover = y >= (rect.height - axisHeight);
        if (isInAxis_hover && Math.abs(x - sdrCenterX_hover) <= 6) hoverEdge = true;
      }
      if (isInAxis && Math.abs(x - txLoX) <= 6) hoverEdge = true;
    }

    canvas.style.cursor = hoverEdge ? 'ew-resize' : 'crosshair';
  }
});

canvas.addEventListener('click', (e) => {
  if (dragMoved || dragBarMoved) {
    dragBarMoved = false;
    dragMoved = false;
    return;
  }
  const rect = canvas.getBoundingClientRect();
  const x = e.clientX - rect.left;
  const freqHz = currentCenterHz - (currentBandwidthHz * 0.5) + (x / rect.width) * currentBandwidthHz;
  const listeningHz = Math.round(clampListeningHz(freqHz));
  frequencyInput.value = listeningHz;
  sendCommand({ type: 'SetRxFrequency', payload: { hz: listeningHz } });
  updateHardwareLo();
  redrawWaterfallFromHistory();
});

canvas.addEventListener('mouseleave', () => {
  hoverTooltip.style.display = 'none';
});

canvas.addEventListener('wheel', (e) => {
  e.preventDefault();
  if (!window.isConnected) return;

  const rect = canvas.getBoundingClientRect();
  const x = e.clientX - rect.left;
  const pointerFreqHz = currentCenterHz - (currentBandwidthHz * 0.5) + (x / rect.width) * currentBandwidthHz;

  const newBandwidthHz = getNextZoomStep(currentBandwidthHz, e.deltaY > 0);
  if (newBandwidthHz === currentBandwidthHz) return;

  const newCenterHz = Math.round(pointerFreqHz + newBandwidthHz * (0.5 - x / rect.width));

  currentBandwidthHz = newBandwidthHz;
  currentCenterHz = newCenterHz;
  const hwSpan = Math.max(minHardwareSpanHz, currentBandwidthHz);
  hardwareLoHz = desiredHardwareLo(currentCenterHz, currentBandwidthHz, hwSpan, parseInt(frequencyInput.value, 10));

  redrawWaterfallFromHistory();

  centerFreqInput.value = currentCenterHz;
  updatePlaybackAbility();

  scheduleRxSpanUpdate();
}, { passive: false });

// --- Window-Level Event Listeners (Keys & Global Mouse release) ---

window.addEventListener('mouseup', () => {
  if (isDraggingBar) {
    const wasCarrierDrag = isDraggingBar === 'carrier' && dragBarMoved;
    const wasLoDrag = isDraggingBar === 'lo' && dragBarMoved;
    const wasTxLoDrag = isDraggingBar === 'txlo' && dragBarMoved;
    isDraggingBar = false;
    canvas.style.cursor = 'crosshair';
    if (wasCarrierDrag) {
      const newHz = parseInt(frequencyInput.value, 10);
      if (!Number.isNaN(newHz)) {
        sendCommand({ type: 'SetRxFrequency', payload: { hz: newHz } });
        redrawWaterfallFromHistory();
      }
    } else if (wasLoDrag) {
      centerFreqInput.value = Math.round(hardwareLoHz);
      sendCommand({ type: 'SetRxCenterFrequency', payload: { hz: Math.round(hardwareLoHz) } });
      redrawWaterfallFromHistory();
    } else if (wasTxLoDrag) {
      sendCommand({ type: 'SetTxOffset', payload: { hz: txOffsetHz } });
      redrawWaterfallFromHistory();
    }
    return;
  }
  if (isDragging) {
    isDragging = false;
    canvas.style.cursor = 'crosshair';
    if (dragMoved) {
      updateHardwareLo();
    }
  }
});

window.addEventListener('keydown', (e) => {
  if (!window.isConnected) return;
  if (!e.ctrlKey) return;

  const activeEl = document.activeElement;
  if (activeEl && (activeEl.tagName === 'INPUT' || activeEl.tagName === 'TEXTAREA' || activeEl.isContentEditable)) {
    return;
  }

  if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
    e.preventDefault();

    const newBandwidthHz = getNextZoomStep(currentBandwidthHz, e.key === 'ArrowDown');
    if (newBandwidthHz === currentBandwidthHz) return;

    const listeningHz = parseInt(frequencyInput.value, 10);
    if (!Number.isNaN(listeningHz)) {
      // Keep the listening frequency at the same relative position in the view,
      // so zooming anchors on it instead of the waterfall center.
      const relPos = (listeningHz - (currentCenterHz - currentBandwidthHz * 0.5)) / currentBandwidthHz;
      currentCenterHz = Math.round(listeningHz + newBandwidthHz * (0.5 - relPos));
    }

    currentBandwidthHz = newBandwidthHz;
    const hwSpan = Math.max(minHardwareSpanHz, currentBandwidthHz);
    hardwareLoHz = desiredHardwareLo(currentCenterHz, currentBandwidthHz, hwSpan, listeningHz);

    redrawWaterfallFromHistory();
    centerFreqInput.value = currentCenterHz;
    updatePlaybackAbility();

    scheduleRxSpanUpdate();

  } else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
    e.preventDefault();
    
    const panStepHz = Math.round(currentBandwidthHz * 0.1);
    if (e.key === 'ArrowLeft') {
      currentCenterHz -= panStepHz;
    } else {
      currentCenterHz += panStepHz;
    }

    redrawWaterfallFromHistory();
    centerFreqInput.value = Math.round(currentCenterHz);

    if (keyboardPanTimeout) {
      clearTimeout(keyboardPanTimeout);
    }
    keyboardPanTimeout = setTimeout(() => {
      updateHardwareLo();
      keyboardPanTimeout = null;
    }, 250);
  }
});

// --- DOM Control Event Listeners ---

runStatusToggle.addEventListener('click', () => {
  isRunning = !isRunning;
  sendCommand({ type: isRunning ? 'Start' : 'Stop' });
  updateRunStatusBadge();
});

setFreqButton.addEventListener('click', () => {
  const hz = parseInt(frequencyInput.value, 10);
  if (!Number.isNaN(hz)) {
    if (hz < 70000000 || hz > 6000000000) {
      alert(`The Pluto+ (AD9361) tuning range is 70 MHz to 6.0 GHz.\nPlease enter a value between 70000000 and 6000000000.`);
      return;
    }
    sendCommand({ type: 'SetRxFrequency', payload: { hz } });
    currentCenterHz = hz;
    updateHardwareLo();
    redrawWaterfallFromHistory();
  }
});

setFilterBwButton.addEventListener('click', () => {
  const bw = parseInt(filterBwInput.value, 10);
  if (!Number.isNaN(bw) && bw > 0) {
    const maxBw = (currentMode === 'USB' || currentMode === 'LSB') ? 20000 : 110000;
    currentFilterBw = Math.max(1000, Math.min(maxBw, bw));
    filterBwInput.value = currentFilterBw;
    sendCommand({
      type: 'SetRxDemodulation',
      payload: { mode: currentMode, filter_bw_hz: currentFilterBw }
    });
    drawAxis(canvas.width / dpr, canvas.height / dpr);
    redrawWaterfallFromHistory();
    syncTxToRx();
  }
});

modeSelect.addEventListener('change', (e) => {
  currentMode = e.target.value;
  currentFilterBw = currentMode === 'FM' ? 15000 : 3000;
  filterBwInput.value = currentFilterBw;

  sendCommand({
    type: 'SetRxDemodulation',
    payload: { mode: currentMode, filter_bw_hz: currentFilterBw }
  });
  drawAxis(canvas.width / dpr, canvas.height / dpr);
  redrawWaterfallFromHistory();
  syncTxToRx();
});

waterfallSpeedSelect.addEventListener('change', (e) => {
  const ms = parseInt(e.target.value, 10);
  if (!Number.isNaN(ms)) {
    sendCommand({ type: 'SetRxWaterfallInterval', payload: { ms } });
  }
});

waterfallFftSizeSelect.addEventListener('change', (e) => {
  const size = parseInt(e.target.value, 10);
  if (!Number.isNaN(size)) {
    sendCommand({ type: 'SetRxWaterfallFftSize', payload: { size } });
  }
});

antennaSelect.addEventListener('change', (e) => {
  const antenna = parseInt(e.target.value, 10);
  if (!Number.isNaN(antenna)) {
    sendCommand({ type: 'SetRxAntenna', payload: { antenna } });
    isWaitingForHardware = true;
    awaitingFirstRow = false;
    armSettleFallback(3000);
  }
});

if (rxGainModeSelect) {
  rxGainModeSelect.addEventListener('change', (e) => {
    const mode = e.target.value;
    sendCommand({ type: 'SetRxGainMode', payload: { mode } });
    updatePlaybackAbility();
  });
}

if (rxGainSlider) {
  rxGainSlider.addEventListener('input', (e) => {
    const val = parseFloat(e.target.value);
    if (rxGainVal) {
      rxGainVal.textContent = val.toFixed(1) + ' dB';
    }
  });
  rxGainSlider.addEventListener('change', (e) => {
    const val = parseFloat(e.target.value);
    sendCommand({ type: 'SetRxGain', payload: { db: val } });
  });
}

if (txGainSlider) {
  txGainSlider.addEventListener('input', (e) => {
    const val = parseFloat(e.target.value);
    if (txGainVal) {
      txGainVal.textContent = val.toFixed(2) + ' dB';
    }
    updateStatusBar();
  });
  txGainSlider.addEventListener('change', (e) => {
    const val = parseFloat(e.target.value);
    sendCommand({ type: 'SetTxGain', payload: { db: val } });
    updateStatusBar();
  });
}

if (txOffsetInput) {
  txOffsetInput.addEventListener('change', () => {
    let hz = parseInt(txOffsetInput.value, 10);
    if (Number.isNaN(hz)) {
      txOffsetInput.value = txOffsetHz;
      return;
    }
    hz = clampTxOffset(hz);
    txOffsetInput.value = hz;
    txOffsetHz = hz;
    sendCommand({ type: 'SetTxOffset', payload: { hz } });
    redrawWaterfallFromHistory();
  });
}

if (setRfBandwidthButton && rfBandwidthInput) {
  setRfBandwidthButton.addEventListener('click', () => {
    const bw = parseInt(rfBandwidthInput.value, 10);
    if (!Number.isNaN(bw) && bw >= 200000 && bw <= 40000000) {
      sendCommand({ type: 'SetRxRfBandwidth', payload: { bw_hz: bw } });
    }
  });
}

if (syncRfBwCheckbox) {
  syncRfBwCheckbox.addEventListener('change', (e) => {
    const checked = e.target.checked;
    if (checked) {
      sendCommand({ type: 'SetRxRfBandwidth', payload: { bw_hz: 0 } });
      if (rfBandwidthInput) {
        rfBandwidthInput.value = sdrBandwidthHz;
      }
    }
    updatePlaybackAbility();
  });
}

if (wfMinDbSlider) {
  wfMinDbSlider.addEventListener('input', (e) => {
    const val = parseFloat(e.target.value);
    if (wfMinDbVal) {
      wfMinDbVal.textContent = val.toFixed(0) + ' dB';
    }
  });
  wfMinDbSlider.addEventListener('change', updateWaterfallScale);
}

if (wfMaxDbSlider) {
  wfMaxDbSlider.addEventListener('input', (e) => {
    const val = parseFloat(e.target.value);
    if (wfMaxDbVal) {
      wfMaxDbVal.textContent = val.toFixed(0) + ' dB';
    }
  });
  wfMaxDbSlider.addEventListener('change', updateWaterfallScale);
}

if (wfResetButton) {
  wfResetButton.addEventListener('click', () => {
    if (wfMinDbSlider) {
      wfMinDbSlider.value = -100;
      if (wfMinDbVal) wfMinDbVal.textContent = '-100 dB';
    }
    if (wfMaxDbSlider) {
      wfMaxDbSlider.value = -40;
      if (wfMaxDbVal) wfMaxDbVal.textContent = '-40 dB';
    }
    sendCommand({
      type: 'SetRxWaterfallScale',
      payload: { min_db: -100.0, max_db: -40.0 }
    });
  });
}

setCenterFreqButton.addEventListener('click', () => {
  const hz = parseInt(centerFreqInput.value, 10);
  if (!Number.isNaN(hz)) {
    hardwareLoHz = hz;
    redrawWaterfallFromHistory();
    isWaitingForHardware = true;
    awaitingFirstRow = false;
    armSettleFallback(3000);
    sendCommand({ type: 'SetRxCenterFrequency', payload: { hz } });
  }
});

// --- Application Initialization ---
frequencyInput.value = currentCenterHz;
centerFreqInput.value = currentCenterHz;
updateRunStatusBadge();

function resizeCanvas() {
  const rect = canvas.getBoundingClientRect();
  dpr = Math.min(window.devicePixelRatio || 1, 2);
  const targetWidth = Math.round(rect.width * dpr) || 1024;
  const targetHeight = Math.round(rect.height * dpr) || 400;

  if (canvas.width !== targetWidth || canvas.height !== targetHeight) {
    canvas.width = targetWidth;
    canvas.height = targetHeight;
    redrawWaterfallFromHistory();
  }
}

window.addEventListener('resize', resizeCanvas);

// Re-check DPR on display changes (e.g. moving to another monitor). The media query fires only for
// the current DPR, so re-register after each change.
function watchDprChange() {
  window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`)
    .addEventListener('change', () => {
      resizeCanvas();
      watchDprChange();
    }, { once: true });
}
watchDprChange();

connect();
initAudioUI();
initTx();
resizeCanvas();