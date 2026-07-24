// Transmit path: captures microphone or file audio and streams it to the backend as binary WS
// frames, plus the TX mode/bandwidth/sync UI. Self-contained - reaches into the core only for
// `sendBinary` (to push PCM), `sendCommand` (to key/configure TX), and `updateStatusBar`.

import { sendCommand, sendBinary, updateStatusBar, clampTxFilterBw } from './app.js';
import { FRAME_TYPE, encodeFrame } from './framing.js';

const txStatusLabel = document.getElementById('tx-status');
const txMicButton = document.getElementById('tx-mic');
const txFileInput = document.getElementById('tx-file');
const txModeSelect = document.getElementById('tx-mode-select');
const txFilterBwInput = document.getElementById('tx-filter-bw');
const txFileConfirmButton = document.getElementById('tx-file-confirm');
const syncRxTxCheckbox = document.getElementById('sync-rx-tx');
const txGainSlider = document.getElementById('tx-gain');
const modeSelect = document.getElementById('mode-select');
const filterBwInput = document.getElementById('filter-bw');

let isTransmitting = false;
let txStarting = false;
let txChunkCount = 0;
let selectedTxFile = null;
let txStream = null;
let txProcessor = null;
let txAudioCtx = null;
let txFileSource = null;

let workletLoadPromise = null;

// TX capture rate. 48000 is only the pre-Config default; the backend delivers the authoritative
// rate in every Config message (applied via setTxAudioSampleRate before any TX can start).
let txAudioSampleRate = 48000;

export function setTxAudioSampleRate(hz) {
  txAudioSampleRate = hz;
}

function initTxAudio() {
  if (!txAudioCtx) {
    txAudioCtx = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: txAudioSampleRate });
  }
  if (!workletLoadPromise) {
    console.log("[TX Client] Loading AudioWorklet module...");
    workletLoadPromise = txAudioCtx.audioWorklet.addModule('js/tx-processor.js')
      .then(() => {
        console.log("[TX Client] AudioWorklet module loaded successfully.");
      })
      .catch((err) => {
        console.error("[TX Client] Failed to load AudioWorklet module:", err);
        workletLoadPromise = null;
        throw err;
      });
  }
  return workletLoadPromise;
}

function sendTxAudioChunk(pcmArray) {
  if (!sendBinary(encodeFrame(FRAME_TYPE.TX_AUDIO, pcmArray))) return;

  txChunkCount++;
  if (txChunkCount === 1) {
    console.log(`[TX Client] Sent FIRST audio chunk to WebSocket (samples: ${pcmArray.length})`);
  }
  if (txChunkCount % 100 === 0) {
    console.log(`[TX Audio Debug] Sent ${txChunkCount} chunks (samples per chunk: ${pcmArray.length})`);
  }
}

function sendTxState(active) {
  sendCommand({
    type: 'SetTxState',
    payload: {
      active,
      tx_gain_db: parseFloat(txGainSlider.value) || 0
    }
  });
}

function sendTxModulation(mode, bwHz) {
  const cleanMode = (mode === 'USB' || mode === 'LSB') ? mode : 'USB';
  const cleanBw = clampTxFilterBw(bwHz || 2800);
  txModeSelect.value = cleanMode;
  txFilterBwInput.value = cleanBw;
  sendCommand({
    type: 'SetTxModulation',
    payload: {
      mode: cleanMode,
      filter_bw_hz: cleanBw
    }
  });
  updateStatusBar();
}

function stopTx() {
  if (txStream) {
    txStream.getTracks().forEach(track => track.stop());
    txStream = null;
  }
  if (txFileSource) {
    txFileSource.onended = null;
    try { txFileSource.stop(); } catch (_) {}
    try { txFileSource.disconnect(); } catch (_) {}
    txFileSource = null;
  }
  if (txProcessor) {
    try { txProcessor.disconnect(); } catch (_) {}
    txProcessor = null;
  }
  isTransmitting = false;

  if (txStatusLabel) {
    txStatusLabel.textContent = "TX Ready";
    txStatusLabel.style.color = "";
  }
  if (txMicButton) {
    txMicButton.textContent = "Mic TX";
    txMicButton.style.backgroundColor = "";
    txMicButton.disabled = !window.isConnected;
  }
  if (txFileInput) {
    txFileInput.value = "";
    txFileInput.disabled = !window.isConnected;
  }
  selectedTxFile = null;
  if (txFileConfirmButton) {
    txFileConfirmButton.textContent = "Send File";
    txFileConfirmButton.style.backgroundColor = "";
    txFileConfirmButton.style.display = 'none';
  }

  sendTxState(false);
}

async function startMicTx() {
  if (txStarting || isTransmitting) return;
  txStarting = true;

  if (window.location.protocol === 'http:' && window.location.hostname !== 'localhost' && window.location.hostname !== '127.0.0.1') {
    const secureUrl = `https://${window.location.host}${window.location.pathname}`;
    if (confirm("Microphone access requires HTTPS on non-localhost connections.\nWould you like to redirect to the secure version?")) {
      window.location.href = secureUrl;
      txStarting = false;
      return;
    }
  }

  txChunkCount = 0;
  try {
    await initTxAudio();
    if (txAudioCtx.state === 'suspended') {
      await txAudioCtx.resume();
    }

    console.log("[TX Client] Requesting microphone access...");
    txStream = await navigator.mediaDevices.getUserMedia({ audio: true });
    console.log("[TX Client] Microphone access granted. Connecting audio nodes.");
    const source = txAudioCtx.createMediaStreamSource(txStream);

    txProcessor = new AudioWorkletNode(txAudioCtx, 'tx-processor');
    txProcessor.port.onmessage = (event) => {
      if (!isTransmitting) return;
      sendTxAudioChunk(new Float32Array(event.data));
    };

    source.connect(txProcessor);
    txProcessor.connect(txAudioCtx.destination);

    isTransmitting = true;
    txStatusLabel.textContent = "TX: MIC (Live)";
    txStatusLabel.style.color = "#FF5555";
    txMicButton.textContent = "Stop Mic TX";
    txMicButton.style.backgroundColor = "#aa0000";
    if (txFileInput) txFileInput.disabled = true;

    sendTxState(true);
  } catch (err) {
    console.error("Error accessing microphone or initializing TX audio:", err);
    if (txStream) {
      txStream.getTracks().forEach(track => track.stop());
      txStream = null;
    }
    alert("Could not access microphone.");
    stopTx();
  } finally {
    txStarting = false;
  }
}

// Mirrors the RX mode/bandwidth onto TX when the "sync RX<->TX" box is checked.
export function syncTxToRx() {
  if (!syncRxTxCheckbox || !syncRxTxCheckbox.checked) return;
  sendTxModulation(modeSelect.value, parseFloat(filterBwInput.value) || 2800);
}

// A reconnect must converge the radio to the UI state: if this page is not transmitting,
// re-assert TX off (a connection drop mid-transmission may have left the backend keyed).
export function reassertTxState() {
  if (isTransmitting) return;
  sendTxState(false);
}

// Enables/cleans up the TX controls when the connection state changes. Called by the core's
// updatePlaybackAbility so it doesn't need to reach into this module's private state.
export function applyConnectionState(isConnected) {
  if (txMicButton && !isTransmitting) txMicButton.disabled = !isConnected;
  if (txFileInput && !isTransmitting) txFileInput.disabled = !isConnected;

  if (!isConnected) {
    if (isTransmitting) {
      stopTx();
    } else {
      selectedTxFile = null;
      if (txFileInput) txFileInput.value = "";
      if (txFileConfirmButton) txFileConfirmButton.style.display = 'none';
    }
  }
}

// Wires all TX-related controls. Called once from the core after load.
export function initTx() {
  txModeSelect.addEventListener('change', (e) => {
    sendTxModulation(e.target.value, parseFloat(txFilterBwInput.value) || 2800);
  });

  txFilterBwInput.addEventListener('change', () => {
    sendTxModulation(txModeSelect.value, parseFloat(txFilterBwInput.value) || 0);
  });

  txMicButton.addEventListener('click', () => {
    if (isTransmitting) {
      stopTx();
    } else {
      startMicTx();
    }
  });

  txFileInput.addEventListener('change', (e) => {
    const file = e.target.files[0];
    if (!file) {
      selectedTxFile = null;
      txFileConfirmButton.style.display = 'none';
      return;
    }
    selectedTxFile = file;
    txFileConfirmButton.textContent = "Send File";
    txFileConfirmButton.style.backgroundColor = "";
    txFileConfirmButton.style.display = 'inline-block';
  });

  txFileConfirmButton.addEventListener('click', async () => {
    if (txStarting || isTransmitting) {
      stopTx();
      return;
    }

    if (!selectedTxFile) return;
    txStarting = true;

    try {
      await initTxAudio();
      if (txAudioCtx.state === 'suspended') {
        await txAudioCtx.resume();
      }

      txChunkCount = 0;
      console.log("[TX Client] Reading selected audio file: " + selectedTxFile.name + " (" + selectedTxFile.size + " bytes)...");
      const arrayBuffer = await selectedTxFile.arrayBuffer();

      console.log("[TX Client] Decoding audio data...");
      const audioBuffer = await txAudioCtx.decodeAudioData(arrayBuffer);
      console.log("[TX Client] Audio file decoded successfully. Duration: " + audioBuffer.duration.toFixed(2) + "s, Sample Rate: " + audioBuffer.sampleRate + "Hz.");

      const source = txAudioCtx.createBufferSource();
      source.buffer = audioBuffer;

      txProcessor = new AudioWorkletNode(txAudioCtx, 'tx-processor');
      txProcessor.port.onmessage = (ev) => {
        if (!isTransmitting) return;
        sendTxAudioChunk(new Float32Array(ev.data));
      };

      source.connect(txProcessor);
      txProcessor.connect(txAudioCtx.destination);

      source.onended = () => {
        stopTx();
      };

      txFileSource = source;
      source.start(0);

      isTransmitting = true;
      txStatusLabel.textContent = "TX: FILE (Playing)";
      txStatusLabel.style.color = "#FF5555";
      txMicButton.disabled = true;
      txFileInput.disabled = true;

      txFileConfirmButton.textContent = "Stop File";
      txFileConfirmButton.style.backgroundColor = "#aa0000";

      sendTxState(true);
    } catch (err) {
      console.error("Error decoding audio file:", err);
      alert("Could not decode the selected audio file.");
      stopTx();
    } finally {
      txStarting = false;
    }
  });

  if (syncRxTxCheckbox) {
    const syncedInit = syncRxTxCheckbox.checked;
    txModeSelect.disabled = syncedInit;
    txFilterBwInput.disabled = syncedInit;
    if (syncedInit) {
      syncTxToRx();
    }

    syncRxTxCheckbox.addEventListener('change', () => {
      const synced = syncRxTxCheckbox.checked;
      txModeSelect.disabled = synced;
      txFilterBwInput.disabled = synced;
      if (synced) {
        syncTxToRx();
      }
      updateStatusBar();
    });
  }
}
