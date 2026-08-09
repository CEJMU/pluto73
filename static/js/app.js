import { formatFrequency, formatHzToMhz, formatHzToMhzPrecise, formatHzShort } from './format.js';
import { playAudioChunk, initAudioUI, setAudioSampleRate, applyServerAudioState } from './audio.js';
import { initTx, syncTxToRx, applyConnectionState, reassertTxState, setTxAudioSampleRate } from './tx.js';
import { FRAME_TYPE, HEADER_BYTES } from './framing.js';

export { sendCommand, sendBinary, updateStatusBar, clampFilterBw, clampTxFilterBw };

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

const txOffsetInput = document.getElementById('tx-offset');
const setTxOffsetButton = document.getElementById('set-tx-offset');

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
const resolutionEl = document.getElementById('resolution-val');

const txStatusLoVal = document.getElementById('tx-status-lo-val');

// --- Global Application State & Constants ---
const WF_SCALE_DEFAULTS = { min_db: -100.0, max_db: -40.0 };

let isRunning = true; // Matches the backend's default running state on connect
let cachedRowData = null;
let cachedFullImageData = null;
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
let manualRfBwHz = null;
let isConfigInitialized = false;
const axisHeight = 30;                   // Height of the frequency scale axis (CSS pixels)
let dpr = Math.min(window.devicePixelRatio || 1, 2);
function axisHeightPx() { return Math.round(axisHeight * dpr); }
function rowStepPx() { return Math.max(1, Math.round(dpr)); }

let isWaitingForHardware = false;        // Blocks rendering during tuning to avoid buffer corruption
let awaitingFirstRow = false;            // After Config: still dropping settling frames until real signal
let settleFallbackTimer = null;          // Safety valve: force-resume if no valid row ever arrives

// TX DDS offset: the TX LO sits this far below the listening frequency; the FPGA DDS shifts
// the signal back up. Keeps TX LO leakage (carrier spike) away from the transmitted signal.
let txOffsetHz = 1000000;

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
let spanReconfigInFlight = false;
let spanReconfigQueued = false;
let spanReconfigTimer = null;
let spanRequestIdCounter = 0;
let spanReconfigExpectedRequestId = null;
const ZOOM_STEPS = [
  12500, 25000, 50000, 100000, 250000, 500000, 720000, 960000,
  1200000, 1440000, 1680000, 1920000, 2160000, 2400000, 3000000,
  3600000, 4800000, 6000000, 8000000, 10000000, 15000000, 20000000, 30000000
];

// Re-render coalescing using requestAnimationFrame
let redrawPending = false;
function requestRedraw() {
  if (redrawPending) return;
  redrawPending = true;
  requestAnimationFrame(() => {
    redrawPending = false;
    redrawWaterfallFromHistory();
  });
}

const MIN_LO_HZ = 70000000;
const MAX_LO_HZ = 6000000000;

function isValidLoHz(hz) {
  return typeof hz === 'number' && !Number.isNaN(hz) && hz >= MIN_LO_HZ && hz <= MAX_LO_HZ;
}

function isEmptyRow(row) {
  for (let i = 0; i < row.length; i++) {
    if (row[i] > 2) return false;
  }
  return true;
}

function armSettleFallback(ms = 500) {
  if (settleFallbackTimer) clearTimeout(settleFallbackTimer);
  settleFallbackTimer = setTimeout(() => {
    isWaitingForHardware = false;
    awaitingFirstRow = false;
    settleFallbackTimer = null;
  }, ms);
}

function clampListeningHz(hz, loHz = sdrHardwareLoHz, spanHz = sdrBandwidthHz) {
  const margin = 1000;
  const minFreq = loHz - spanHz / 2 + margin;
  const maxFreq = loHz + spanHz / 2 - margin;
  return Math.max(minFreq, Math.min(maxFreq, hz));
}

function desiredHardwareLo(currentCenterHz, currentBandwidthHz, sdrBandwidthHz, listeningHz) {
  const halfVis = currentBandwidthHz / 2;
  const halfSdr = sdrBandwidthHz / 2;
  const margin = 1000;
  let minLo = currentCenterHz + halfVis + margin - halfSdr;
  let maxLo = currentCenterHz - halfVis - margin + halfSdr;

  if (!Number.isNaN(listeningHz)) {
    minLo = Math.max(minLo, listeningHz + margin - halfSdr);
    maxLo = Math.min(maxLo, listeningHz - margin + halfSdr);
  }

  if (minLo > maxLo) {
    return Math.round((minLo + maxLo) / 2);
  }
  return Math.round(Math.max(minLo, Math.min(maxLo, sdrHardwareLoHz)));
}

function filterBandEdges(listeningHz) {
  const mode = modeSelect ? modeSelect.value : 'FM';
  const filterBw = (filterBwInput ? parseInt(filterBwInput.value, 10) : 15000) || 15000;
  if (mode === 'FM') {
    return [listeningHz - filterBw / 2, listeningHz + filterBw / 2];
  } else if (mode === 'USB') {
    return [listeningHz, listeningHz + filterBw];
  } else if (mode === 'LSB') {
    return [listeningHz - filterBw, listeningHz];
  }
  return null;
}

function clampTxOffset(hz) {
  const limit = sdrBandwidthHz / 2 - 20000;
  return Math.max(-limit, Math.min(limit, hz));
}

function clampFilterBw(bw) {
  const mode = modeSelect ? modeSelect.value : 'FM';
  const maxBw = (mode === 'USB' || mode === 'LSB') ? 20000 : 110000;
  return Math.max(1000, Math.min(maxBw, bw));
}

function clampTxFilterBw(bw) {
  return Math.max(1000, Math.min(20000, bw));
}

function hzToX(hz, startHz, bwHz, width) {
  return Math.round(((hz - startHz) / bwHz) * width);
}

function computeOverlayColumns(startHz, bwHz, width, listeningHz) {
  let listenX = -1;
  let boxStartX = -1;
  let boxEndX = -1;
  if (!Number.isNaN(listeningHz) && listeningHz >= startHz && listeningHz <= startHz + bwHz) {
    listenX = hzToX(listeningHz, startHz, bwHz, width);
    const edges = filterBandEdges(listeningHz);
    if (edges) {
      boxStartX = hzToX(edges[0], startHz, bwHz, width);
      boxEndX = hzToX(edges[1], startHz, bwHz, width);
    }
  }
  return { listenX, boxStartX, boxEndX };
}

function hitTestBar(x, y, rect, listeningHz, startHz, bwHz) {
  const mode = modeSelect ? modeSelect.value : 'FM';
  const isInAxis = y >= (rect.height - axisHeight);

  if (!Number.isNaN(listeningHz)) {
    const listenClientX = ((listeningHz - startHz) / bwHz) * rect.width;
    if (Math.abs(x - listenClientX) <= 6) return 'carrier';

    const edges = filterBandEdges(listeningHz);
    if (edges) {
      const [boxStartHz, boxEndHz] = edges;
      const startClientX = ((boxStartHz - startHz) / bwHz) * rect.width;
      const endClientX = ((boxEndHz - startHz) / bwHz) * rect.width;

      if ((mode === 'FM' || mode === 'LSB') && Math.abs(x - startClientX) <= 6) return 'left';
      if ((mode === 'FM' || mode === 'USB') && Math.abs(x - endClientX) <= 6) return 'right';
    }

    const txLoHz = listeningHz - txOffsetHz;
    const txLoX = ((txLoHz - startHz) / bwHz) * rect.width;
    if (isInAxis && Math.abs(x - txLoX) <= 6) return 'txlo';
  }

  const sdrCenterX = ((hardwareLoHz - startHz) / bwHz) * rect.width;
  if (isInAxis && Math.abs(x - sdrCenterX) <= 6) return 'lo';

  return null;
}

// --- Helper & Formatting Functions ---
function updateStatus(text) {
  statusLabel.textContent = text;
  statusLabel.style.color = text === 'connected' ? '#00FF00' : 'red';
}

function updateRunStatusBadge() {
  if (!runStatusToggle) return;
  runStatusToggle.textContent = isRunning ? 'Started' : 'Stopped';
  runStatusToggle.classList.toggle('is-running', isRunning);
  runStatusToggle.classList.toggle('is-stopped', !isRunning);
}

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

  if (resolutionEl) {
    const fftSize = (waterfallFftSizeSelect && parseInt(waterfallFftSizeSelect.value, 10)) ||
      ((rowHistory.length > 0 && rowHistory[0].row) ? rowHistory[0].row.length : 8192);
    const fftRes = sdrBandwidthHz / fftSize;
    const zoomFactor = 30000000 / currentBandwidthHz;

    let detailRating = 'Ultra-Fine';
    if (fftRes > 800) detailRating = 'Coarse';
    else if (fftRes > 350) detailRating = 'Medium';
    else if (fftRes > 180) detailRating = 'Fine';

    resolutionEl.textContent = `${zoomFactor.toFixed(1)}x (${detailRating}, ${formatHzShort(fftRes)})`;
  }

  const listeningHz = parseInt(frequencyInput.value, 10);
  if (txStatusLoVal && !Number.isNaN(listeningHz)) {
    txStatusLoVal.textContent = formatHzToMhzPrecise(listeningHz - txOffsetHz, 6);
  }
}

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

function setConnected(isConnected) {
  window.isConnected = isConnected;
  if (isConnected) {
    isRunning = true;
    updateRunStatusBadge();
  }
  updatePlaybackAbility();
  updateStatus(isConnected ? 'connected' : 'disconnected');
}

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
function sendCommand(command) {
  if (!ws || ws.readyState !== WebSocket.OPEN) {
    console.warn('WebSocket not open');
    return;
  }
  console.log('[WS Control Command]', command);
  ws.send(JSON.stringify(command));
}

function sendBinary(buffer) {
  if (!ws || ws.readyState !== WebSocket.OPEN) return false;
  ws.send(buffer);
  return true;
}

const BINARY_HANDLERS = {
  [FRAME_TYPE.WATERFALL]: (frame) => {
    appendWaterfallRow(new Uint8Array(frame, HEADER_BYTES));
  },
  [FRAME_TYPE.AUDIO]: (frame) => {
    if (frame.byteLength < HEADER_BYTES + 4) {
      console.warn("Received malformed audio chunk with byte length:", frame.byteLength);
      return;
    }
    playAudioChunk(new Float32Array(frame, HEADER_BYTES));
  },
  [FRAME_TYPE.IQ]: () => {},
};

function handleBinaryMessage(arrayBuffer) {
  if (arrayBuffer.byteLength < HEADER_BYTES) {
    console.warn("Received truncated binary frame of", arrayBuffer.byteLength, "bytes");
    return;
  }
  const type = new DataView(arrayBuffer).getUint8(0);
  const handler = BINARY_HANDLERS[type];
  if (!handler) {
    console.warn("Unknown binary frame type:", type);
    return;
  }
  handler(arrayBuffer);
}

function handleTextMessage(data) {
  if (data.type === 'Status') {
    updateStatus(data.payload.state);
  } else if (data.type === 'Config') {
    handleConfigUpdate(data.payload);
  } else if (data.type === 'Settings') {
    handleSettingsUpdate(data.payload);
  } else if (data.type === 'Telemetry') {
    handleTelemetryUpdate(data.payload);
  } else if (data.type === 'RxGain') {
    handleRxGainUpdate(data.payload);
  }
}

function handleConfigUpdate(payload) {
  const prevSampleRateHz = sdrBandwidthHz;
  sdrHardwareLoHz = payload.lo_hz;
  hardwareLoHz = sdrHardwareLoHz;
  sdrBandwidthHz = payload.sample_rate_hz;

  if (payload.min_span_hz) {
    minHardwareSpanHz = payload.min_span_hz;
  }

  if (payload.audio_sample_rate_hz) {
    setAudioSampleRate(payload.audio_sample_rate_hz);
    setTxAudioSampleRate(payload.audio_sample_rate_hz);
  }

  const clampedTxOffset = clampTxOffset(txOffsetHz);
  if (clampedTxOffset !== txOffsetHz) {
    txOffsetHz = clampedTxOffset;
    if (txOffsetInput) txOffsetInput.value = txOffsetHz;
  }

  if (currentBandwidthHz > sdrBandwidthHz) {
    currentBandwidthHz = sdrBandwidthHz;
  }

  if (rfBandwidthInput && payload.rf_bandwidth_hz !== undefined &&
      document.activeElement !== rfBandwidthInput) {
    rfBandwidthInput.value = payload.rf_bandwidth_hz;
  }

  // A span change re-slaves the filter to the new span.
  if (sdrBandwidthHz !== prevSampleRateHz) {
    manualRfBwHz = null;
    if (syncRfBwCheckbox) syncRfBwCheckbox.checked = true;
  }

  if (!isConfigInitialized) {
    isConfigInitialized = true;
    currentCenterHz = sdrHardwareLoHz;
    centerFreqInput.value = Math.round(currentCenterHz);

    if (payload.rf_bandwidth_hz !== undefined && payload.rf_bandwidth_hz !== sdrBandwidthHz) {
      manualRfBwHz = payload.rf_bandwidth_hz;
      if (syncRfBwCheckbox) syncRfBwCheckbox.checked = false;
    }
  }

  requestRedraw();

  // Span reconfig acknowledged: release the lock and, if the user kept zooming, send one more SetRxSpan for the latest view
  if (spanReconfigInFlight &&
      spanReconfigExpectedRequestId !== null &&
      payload.request_id >= spanReconfigExpectedRequestId) {
    spanReconfigInFlight = false;
    spanReconfigExpectedRequestId = null;
    if (spanReconfigTimer) {
      clearTimeout(spanReconfigTimer);
      spanReconfigTimer = null;
    }
    if (spanReconfigQueued) {
      spanReconfigQueued = false;
      sendSpanUpdate();
    }
  }

  if (isWaitingForHardware) {
    awaitingFirstRow = true;
    armSettleFallback(500);
  }

  updatePlaybackAbility();
}

function handleSettingsUpdate(payload) {
  console.log('[WS Settings Update] Received active settings from server:', payload);
  if (frequencyInput && payload.playback_hz !== undefined) {
    frequencyInput.value = payload.playback_hz;
  }

  if (modeSelect && payload.demod_mode) {
    modeSelect.value = payload.demod_mode;
  }

  if (filterBwInput && payload.filter_bw_hz !== undefined) {
    filterBwInput.value = payload.filter_bw_hz;
  }
  syncTxToRx();

  if (payload.audio_enabled !== undefined) {
    applyServerAudioState(payload.audio_enabled);
  }

  if (waterfallSpeedSelect && payload.waterfall_interval_ms !== undefined) {
    waterfallSpeedSelect.value = payload.waterfall_interval_ms;
  }

  if (waterfallFftSizeSelect && payload.waterfall_fft_size) {
    waterfallFftSizeSelect.value = payload.waterfall_fft_size;
  }

  if (antennaSelect && payload.antenna !== undefined) {
    antennaSelect.value = payload.antenna.toString();
  }

  if (rxGainModeSelect && payload.rx_gain_mode) {
    rxGainModeSelect.value = payload.rx_gain_mode;
  }

  if (rxGainSlider && payload.rx_gain_db !== undefined) {
    rxGainSlider.value = payload.rx_gain_db;
    if (rxGainVal) {
      rxGainVal.textContent = payload.rx_gain_db.toFixed(1) + ' dB';
    }
    rxGainSlider.disabled = !window.isConnected || payload.rx_gain_mode !== 'manual';
  }

  if (txOffsetInput && payload.tx_offset_hz !== undefined) {
    txOffsetHz = payload.tx_offset_hz;
    txOffsetInput.value = txOffsetHz;
  }

  const minDb = payload.waterfall_min_db !== undefined ? payload.waterfall_min_db : WF_SCALE_DEFAULTS.min_db;
  const maxDb = payload.waterfall_max_db !== undefined ? payload.waterfall_max_db : WF_SCALE_DEFAULTS.max_db;
  if (wfMinDbSlider) {
    wfMinDbSlider.value = minDb;
    if (wfMinDbVal) wfMinDbVal.textContent = minDb.toFixed(0) + ' dB';
  }
  if (wfMaxDbSlider) {
    wfMaxDbSlider.value = maxDb;
    if (wfMaxDbVal) wfMaxDbVal.textContent = maxDb.toFixed(0) + ' dB';
  }

  requestRedraw();
  updatePlaybackAbility();
}

function handleRxGainUpdate(payload) {
  if (!rxGainSlider || payload.gain_db === undefined) return;
  // Only reflect AGC's own readback while AGC is actually selected locally, so this doesn't
  // fight a user who just switched to manual (or a stale message racing the mode switch).
  if (rxGainModeSelect && rxGainModeSelect.value === 'manual') return;

  rxGainSlider.value = payload.gain_db;
  if (rxGainVal) {
    rxGainVal.textContent = payload.gain_db.toFixed(1) + ' dB';
  }
}

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
    reassertTxState();
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
    isConfigInitialized = false;
    if (telemetryTemp) telemetryTemp.textContent = '-- °C';
    if (telemetryVccint) telemetryVccint.textContent = '-- V';
    if (telemetryVccoddr) telemetryVccoddr.textContent = '-- V';

    console.warn('WebSocket closed; scheduling reconnect in 2s');
    setTimeout(connect, 2000);
  });

  ws.addEventListener('error', (err) => {
    console.error('WebSocket error:', err);
    ws.close();
  });
}

function syncCenterFreqInput() {
  centerFreqInput.value = Math.round(hardwareLoHz);
}

function updateHardwareLo() {
  if (!window.isConnected) return;

  // When zoomed in, panning is often purely visual
  const margin = sdrBandwidthHz * 0.02;
  const viewStartHz = currentCenterHz - currentBandwidthHz / 2;
  const viewEndHz = currentCenterHz + currentBandwidthHz / 2;
  const hwStartHz = sdrHardwareLoHz - sdrBandwidthHz / 2;
  const hwEndHz = sdrHardwareLoHz + sdrBandwidthHz / 2;
  if (viewStartHz >= hwStartHz + margin && viewEndHz <= hwEndHz - margin) {
    hardwareLoHz = sdrHardwareLoHz;
    syncCenterFreqInput();
    return;
  }

  const listeningHz = parseInt(frequencyInput.value, 10);
  const targetLo = desiredHardwareLo(currentCenterHz, currentBandwidthHz, sdrBandwidthHz, listeningHz);
  hardwareLoHz = targetLo;
  syncCenterFreqInput();

  const hwSpan = Math.max(minHardwareSpanHz, currentBandwidthHz);
  const clampedListening = clampListeningHz(listeningHz, targetLo, hwSpan);
  if (clampedListening !== listeningHz) {
    if (frequencyInput) frequencyInput.value = clampedListening;
    sendCommand({ type: 'SetRxFrequency', payload: { hz: clampedListening } });
  }

  if (Math.round(targetLo) !== Math.round(sdrHardwareLoHz)) {
    isWaitingForHardware = true;
    awaitingFirstRow = false;
    armSettleFallback(500);
    sendCommand({ type: 'SetRxCenterFrequency', payload: { hz: targetLo } });
  }
}

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
  isWaitingForHardware = true;
  awaitingFirstRow = false;
  armSettleFallback(500);
  if (zoomTimeout) clearTimeout(zoomTimeout);
  zoomTimeout = setTimeout(() => {
    zoomTimeout = null;
    sendSpanUpdate();
  }, 250);
}

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
    spanReconfigExpectedRequestId = null;
    spanReconfigTimer = null;
    if (spanReconfigQueued) sendSpanUpdate();
  }, 3000);
  const requestedHardwareSpan = Math.max(minHardwareSpanHz, currentBandwidthHz);
  spanRequestIdCounter++;
  spanReconfigExpectedRequestId = spanRequestIdCounter;
  sendCommand({
    type: 'SetRxSpan',
    payload: {
      center_hz: Math.round(hardwareLoHz),
      span_hz: requestedHardwareSpan,
      request_id: spanRequestIdCounter
    }
  });
}

// --- Waterfall Drawing & Rendering ---

// Draws the frequency ticks, passband box, and DC/TX LO markers on the bottom axis. width/height
// are CSS pixels; setTransform scales to the backing buffer so text/ticks stay crisp on HiDPI.
function drawAxis(width, height) {
  ctx.save();
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const axisY = height - axisHeight;
  ctx.fillStyle = '#111';
  ctx.fillRect(0, axisY, width, axisHeight);

  ctx.fillStyle = '#fff';
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
      ctx.fillRect(x - 1, axisY, 2, 8);

      const freqStr = formatFrequency(freqHz, tickIntervalHz);
      const textWidth = ctx.measureText(freqStr).width;
      const textLeft = x - textWidth / 2;
      const textRight = x + textWidth / 2;

      if (textLeft > 0 && textRight < width && textLeft > lastTextRight + 5) {
        ctx.fillText(freqStr, x, axisY + axisHeight / 2 + 1);
        lastTextRight = textRight;
      }
    } else if (tickIndex === 5) {
      ctx.fillRect(x, axisY, 1, 6);
    } else {
      ctx.fillRect(x, axisY, 1, 4);
    }
  }

  const listeningHz = parseInt(frequencyInput.value, 10);
  const { listenX, boxStartX, boxEndX } = computeOverlayColumns(startHz, currentBandwidthHz, width, listeningHz);
  if (listenX !== -1) {
    if (boxStartX !== -1 && boxEndX !== -1) {
      ctx.fillStyle = 'rgba(0, 255, 0, 0.3)';
      ctx.fillRect(boxStartX, axisY, boxEndX - boxStartX, axisHeight);
    }
    ctx.fillStyle = '#00FF00';
    ctx.fillRect(listenX, axisY, 1, axisHeight);
  }

  const sdrCenterX = Math.round(((hardwareLoHz - startHz) / currentBandwidthHz) * width);
  if (sdrCenterX >= 0 && sdrCenterX <= width) {
    ctx.fillStyle = '#ff9800';
    ctx.fillRect(sdrCenterX - 1, axisY, 2, axisHeight);
  }

  if (!Number.isNaN(listeningHz)) {
    const txLoHz = listeningHz - txOffsetHz;
    const txLoX = Math.round(((txLoHz - startHz) / currentBandwidthHz) * width);
    if (txLoX >= 0 && txLoX <= width) {
      ctx.fillStyle = '#ff5599';
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

function redrawWaterfallFromHistory() {
  const width = canvas.width;
  const wfHeight = canvas.height - axisHeightPx();
  ctx.fillStyle = '#000';
  ctx.fillRect(0, 0, width, wfHeight);

  const currentStartHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const listeningHz = parseInt(frequencyInput.value, 10);
  const { listenX, boxStartX, boxEndX } = computeOverlayColumns(currentStartHz, currentBandwidthHz, width, listeningHz);

  if (!cachedFullImageData || cachedFullImageData.width !== width || cachedFullImageData.height !== wfHeight) {
    cachedFullImageData = ctx.createImageData(width, wfHeight);
  }
  const imgData = cachedFullImageData;
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
    for (let i = 1; i < rowStep; i++) {
      data.copyWithin(base + i * lineBytes, base, base + lineBytes);
    }
  }
  ctx.putImageData(imgData, 0, 0);
  drawAxis(canvas.width / dpr, canvas.height / dpr);
  updateStatusBar();
}

function appendWaterfallRow(row) {
  if (isWaitingForHardware) {
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
    row,
    hardwareLoHz: sdrHardwareLoHz,
    bandwidthHz: sdrBandwidthHz
  });
  if (rowHistory.length > Math.floor(wfHeight / rowStep)) rowHistory.pop();

  ctx.drawImage(canvas, 0, rowStep, width, wfHeight - rowStep, 0, 0, width, wfHeight - rowStep);

  if (!cachedRowData || cachedRowData.width !== width || cachedRowData.height !== rowStep) {
    cachedRowData = ctx.createImageData(width, rowStep);
  }
  const rowData = cachedRowData;
  const pixels = rowData.data;

  const startHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const sdrStartHz = sdrHardwareLoHz - (sdrBandwidthHz * 0.5);
  const listeningHz = parseInt(frequencyInput.value, 10);
  const { listenX, boxStartX, boxEndX } = computeOverlayColumns(startHz, currentBandwidthHz, width, listeningHz);

  paintWaterfallRow(pixels, 0, row, sdrStartHz, sdrBandwidthHz,
    startHz, currentBandwidthHz, width, listenX, boxStartX, boxEndX);

  const lineBytes = width * 4;
  for (let i = 1; i < rowStep; i++) {
    pixels.copyWithin(i * lineBytes, 0, lineBytes);
  }
  ctx.putImageData(rowData, 0, wfHeight - rowStep);
}

// --- Interactive Canvas Mouse & Gesture Listeners ---
let lastTooltipContent = '';
function updateTooltipContent(content, isHtml) {
  if (content !== lastTooltipContent) {
    lastTooltipContent = content;
    if (isHtml) {
      hoverTooltip.innerHTML = content;
    } else {
      hoverTooltip.textContent = content;
    }
  }
}

canvas.addEventListener('mousedown', (e) => {
  dragBarMoved = false;
  dragMoved = false;
  const rect = canvas.getBoundingClientRect();
  const x = e.clientX - rect.left;
  const y = e.clientY - rect.top;

  const startHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const listeningHz = parseInt(frequencyInput.value, 10);

  const bar = hitTestBar(x, y, rect, listeningHz, startHz, currentBandwidthHz);
  if (bar) {
    isDraggingBar = bar;
    return;
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
  const startHz = currentCenterHz - (currentBandwidthHz * 0.5);
  const freqHz = startHz + (x / rect.width) * currentBandwidthHz;
  const binHz = currentBandwidthHz / rect.width;

  const sdrCenterX = ((hardwareLoHz - startHz) / currentBandwidthHz) * rect.width;
  const listeningHz = parseInt(frequencyInput.value, 10);
  const txLoHz = listeningHz - txOffsetHz;
  const txLoX = ((txLoHz - startHz) / currentBandwidthHz) * rect.width;

  const isNearSdrCenter = Math.abs(x - sdrCenterX) < 10;
  const isNearTxLo = !Number.isNaN(listeningHz) && Math.abs(x - txLoX) < 10;
  const isInAxis = y >= (rect.height - axisHeight);
  const isBigTooltip = (isNearSdrCenter || isNearTxLo) && isInAxis;

  if (isNearSdrCenter && isInAxis) {
    updateTooltipContent(
      `<span style="color: #ff9800; font-weight: bold;">⚠️ SDR Center (Potential DC Spike)</span><br/>` +
      `<span style="color: #fff;">Freq: ${formatFrequency(hardwareLoHz, binHz)}</span><br/>` +
      `<span style="color: #aaa; font-size: 11px; font-family: sans-serif; line-height: 1.2;">RX LO leakage can cause a DC offset spike here.</span>`,
      true
    );
  } else if (isNearTxLo && isInAxis) {
    updateTooltipContent(
      `<span style="color: #ff5599; font-weight: bold;">⚠️ TX LO (DDS Offset: ${formatHzShort(txOffsetHz)})</span><br/>` +
      `<span style="color: #fff;">Freq: ${formatFrequency(txLoHz, binHz)}</span><br/>` +
      `<span style="color: #aaa; font-size: 11px; font-family: sans-serif; line-height: 1.2;">TX LO leakage can cause a carrier spike here.</span>`,
      true
    );
  } else {
    updateTooltipContent(formatFrequency(freqHz, binHz), false);
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
      hardwareLoHz = Math.max(MIN_LO_HZ, Math.min(MAX_LO_HZ, Math.round(freqHz)));
    } else if (isDraggingBar === 'txlo') {
      if (!Number.isNaN(listeningHz)) {
        txOffsetHz = clampTxOffset(Math.round(listeningHz - freqHz));
        if (txOffsetInput) txOffsetInput.value = txOffsetHz;
      }
    } else if (isDraggingBar === 'left' || isDraggingBar === 'right') {
      const diff = isDraggingBar === 'left' ? listeningHz - freqHz : freqHz - listeningHz;
      const mode = modeSelect ? modeSelect.value : 'FM';
      const rawBw = mode === 'FM' ? diff * 2 : diff;
      const newBw = clampFilterBw(rawBw);
      filterBwInput.value = Math.round(newBw);
      if (filterBwTimeout) clearTimeout(filterBwTimeout);
      filterBwTimeout = setTimeout(() => {
        sendCommand({ type: 'SetRxDemodulation', payload: { mode, filter_bw_hz: newBw } });
        syncTxToRx();
      }, 50);
    }

    requestRedraw();
    return;
  }

  if (isDragging) {
    const deltaX = e.clientX - dragStartX;
    if (Math.abs(deltaX) > 5) {
      dragMoved = true;
      canvas.style.cursor = 'grabbing';
    }
    const deltaHz = (deltaX / rect.width) * currentBandwidthHz;
    currentCenterHz = dragStartCenterHz - deltaHz;

    if (dragMoved) {
      requestRedraw();
    }
  } else {
    const bar = hitTestBar(x, y, rect, listeningHz, startHz, currentBandwidthHz);
    canvas.style.cursor = bar ? 'ew-resize' : 'crosshair';
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
  requestRedraw();
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

  const listeningHz = parseInt(frequencyInput.value, 10);
  const clampedListening = clampListeningHz(listeningHz, hardwareLoHz, hwSpan);
  if (clampedListening !== listeningHz) {
    if (frequencyInput) frequencyInput.value = clampedListening;
    sendCommand({ type: 'SetRxFrequency', payload: { hz: clampedListening } });
  }

  requestRedraw();

  syncCenterFreqInput();
  updatePlaybackAbility();

  scheduleRxSpanUpdate();
}, { passive: false });

// --- Window-Level Event Listeners (Keys & Global Mouse release) ---

window.addEventListener('mouseup', () => {
  if (isDraggingBar) {
    const wasCarrierDrag = isDraggingBar === 'carrier' && dragBarMoved;
    const wasLoDrag = isDraggingBar === 'lo' && dragBarMoved;
    const wasTxLoDrag = isDraggingBar === 'txlo' && dragBarMoved;
    const wasFilterEdgeDrag = (isDraggingBar === 'left' || isDraggingBar === 'right') && dragBarMoved;
    isDraggingBar = false;
    canvas.style.cursor = 'crosshair';
    if (wasCarrierDrag) {
      const newHz = parseInt(frequencyInput.value, 10);
      if (!Number.isNaN(newHz)) {
        sendCommand({ type: 'SetRxFrequency', payload: { hz: newHz } });
        requestRedraw();
      }
    } else if (wasLoDrag) {
      syncCenterFreqInput();
      isWaitingForHardware = true;
      awaitingFirstRow = false;
      armSettleFallback(500);
      sendCommand({ type: 'SetRxCenterFrequency', payload: { hz: Math.round(hardwareLoHz) } });
      requestRedraw();
    } else if (wasTxLoDrag) {
      sendCommand({ type: 'SetTxOffset', payload: { hz: txOffsetHz } });
      requestRedraw();
    } else if (wasFilterEdgeDrag) {
      syncTxToRx();
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

    requestRedraw();
    syncCenterFreqInput();
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
    currentCenterHz = Math.max(MIN_LO_HZ, Math.min(MAX_LO_HZ, currentCenterHz));

    requestRedraw();

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
    if (!isValidLoHz(hz)) {
      alert(`The Pluto+ (AD9361) tuning range is 70 MHz to 6.0 GHz.\nPlease enter a value between ${MIN_LO_HZ} and ${MAX_LO_HZ}.`);
      return;
    }
    sendCommand({ type: 'SetRxFrequency', payload: { hz } });
    currentCenterHz = hz;
    updateHardwareLo();
    requestRedraw();
  }
});

setFilterBwButton.addEventListener('click', () => {
  const bw = parseInt(filterBwInput.value, 10);
  if (!Number.isNaN(bw) && bw > 0) {
    const clamped = clampFilterBw(bw);
    filterBwInput.value = clamped;
    sendCommand({
      type: 'SetRxDemodulation',
      payload: { mode: modeSelect.value, filter_bw_hz: clamped }
    });
    requestRedraw();
    syncTxToRx();
  }
});

modeSelect.addEventListener('change', (e) => {
  const mode = e.target.value;
  const defaultBw = mode === 'FM' ? 15000 : 3000;
  filterBwInput.value = defaultBw;

  sendCommand({
    type: 'SetRxDemodulation',
    payload: { mode, filter_bw_hz: defaultBw }
  });
  requestRedraw();
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
    updateStatusBar();
  }
});

antennaSelect.addEventListener('change', (e) => {
  const antenna = parseInt(e.target.value, 10);
  if (!Number.isNaN(antenna)) {
    sendCommand({ type: 'SetRxAntenna', payload: { antenna } });
    isWaitingForHardware = true;
    awaitingFirstRow = false;
    armSettleFallback(500);
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
  });
  txGainSlider.addEventListener('change', (e) => {
    const val = parseFloat(e.target.value);
    sendCommand({ type: 'SetTxGain', payload: { db: val } });
  });
}

if (setTxOffsetButton && txOffsetInput) {
  setTxOffsetButton.addEventListener('click', () => {
    let hz = parseInt(txOffsetInput.value, 10);
    if (Number.isNaN(hz)) {
      txOffsetInput.value = txOffsetHz;
      return;
    }
    hz = clampTxOffset(hz);
    txOffsetInput.value = hz;
    txOffsetHz = hz;
    sendCommand({ type: 'SetTxOffset', payload: { hz } });
    requestRedraw();
  });
}

if (setRfBandwidthButton && rfBandwidthInput) {
  setRfBandwidthButton.addEventListener('click', () => {
    const bw = parseInt(rfBandwidthInput.value, 10);
    if (!Number.isNaN(bw) && bw >= 200000 && bw <= 40000000) {
      manualRfBwHz = bw;
      sendCommand({ type: 'SetRxRfBandwidth', payload: { bw_hz: bw } });
    }
  });
}

if (syncRfBwCheckbox) {
  syncRfBwCheckbox.addEventListener('change', (e) => {
    if (e.target.checked) {
      manualRfBwHz = null;
      sendCommand({ type: 'SetRxRfBandwidth', payload: { bw_hz: 0 } });
    } else {
      const bw = rfBandwidthInput ? parseInt(rfBandwidthInput.value, 10) : NaN;
      manualRfBwHz = Number.isNaN(bw) ? sdrBandwidthHz : bw;
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
      wfMinDbSlider.value = WF_SCALE_DEFAULTS.min_db;
      if (wfMinDbVal) wfMinDbVal.textContent = `${WF_SCALE_DEFAULTS.min_db} dB`;
    }
    if (wfMaxDbSlider) {
      wfMaxDbSlider.value = WF_SCALE_DEFAULTS.max_db;
      if (wfMaxDbVal) wfMaxDbVal.textContent = `${WF_SCALE_DEFAULTS.max_db} dB`;
    }
    sendCommand({
      type: 'SetRxWaterfallScale',
      payload: { min_db: WF_SCALE_DEFAULTS.min_db, max_db: WF_SCALE_DEFAULTS.max_db }
    });
  });
}

setCenterFreqButton.addEventListener('click', () => {
  const hz = parseInt(centerFreqInput.value, 10);
  if (isValidLoHz(hz)) {
    hardwareLoHz = hz;
    requestRedraw();
    isWaitingForHardware = true;
    awaitingFirstRow = false;
    armSettleFallback(500);
    sendCommand({ type: 'SetRxCenterFrequency', payload: { hz } });
  } else {
    alert(`The Pluto+ (AD9361) tuning range is 70 MHz to 6.0 GHz.\nPlease enter a value between ${MIN_LO_HZ} and ${MAX_LO_HZ}.`);
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
    requestRedraw();
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