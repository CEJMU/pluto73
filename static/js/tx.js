// Transmit path: captures microphone or file audio and streams it to the backend as binary WS
// frames, plus the TX mode/bandwidth/sync UI. Self-contained - reaches into the core only for
// `sendBinary` (to push PCM), `sendCommand` (to key/configure TX), and `updateStatusBar`.

import { sendCommand, sendBinary, updateStatusBar } from './app.js';

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
let txChunkCount = 0;
let selectedTxFile = null;
let txStream = null;
let txProcessor = null;
let txAudioCtx = null;
let txFileSource = null;

let isWorkletLoaded = false;
let workletLoadPromise = null;

function initTxAudio() {
  if (!txAudioCtx) {
    txAudioCtx = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: 48000 });
  }
  if (!isWorkletLoaded && !workletLoadPromise) {
    console.log("[TX Client] Loading AudioWorklet module...");
    workletLoadPromise = txAudioCtx.audioWorklet.addModule('js/tx-processor.js')
      .then(() => {
        isWorkletLoaded = true;
        console.log("[TX Client] AudioWorklet module loaded successfully.");
      })
      .catch((err) => {
        console.error("[TX Client] Failed to load AudioWorklet module:", err);
        workletLoadPromise = null;
      });
  }
  return workletLoadPromise || Promise.resolve();
}

function sendTxAudioChunk(pcmArray) {
  // 4-byte header for alignment. Header = 2 indicates TX audio.
  const buffer = new ArrayBuffer(4 + pcmArray.length * 4);
  const view = new DataView(buffer);
  view.setUint32(0, 2, true); // Little endian 2

  const floats = new Float32Array(buffer, 4);
  floats.set(pcmArray);

  if (!sendBinary(buffer)) return;

  txChunkCount++;
  if (txChunkCount === 1) {
    console.log(`[TX Client] Sent FIRST audio chunk to WebSocket (samples: ${pcmArray.length})`);
  }
  if (txChunkCount % 100 === 0) {
    console.log(`[TX Audio Debug] Sent ${txChunkCount} chunks (samples per chunk: ${pcmArray.length})`);
  }
}

async function startMicTx() {
  if (window.location.protocol === 'http:' && window.location.hostname !== 'localhost' && window.location.hostname !== '127.0.0.1') {
    const secureUrl = `https://${window.location.host}${window.location.pathname}`;
    if (confirm("Microphone access requires HTTPS on non-localhost connections.\nWould you like to redirect to the secure version?")) {
      window.location.href = secureUrl;
      return;
    }
  }

  txChunkCount = 0;
  await initTxAudio();
  if (txAudioCtx.state === 'suspended') {
    await txAudioCtx.resume();
  }

  console.log("[TX Client] Requesting microphone access...");
  try {
    txStream = await navigator.mediaDevices.getUserMedia({ audio: true });
    console.log("[TX Client] Microphone access granted. Connecting audio nodes.");
    const source = txAudioCtx.createMediaStreamSource(txStream);

    txProcessor = new AudioWorkletNode(txAudioCtx, 'tx-processor');
    txProcessor.port.onmessage = (event) => {
      if (!isTransmitting) return;
      sendTxAudioChunk(event.data);
    };

    source.connect(txProcessor);
    txProcessor.connect(txAudioCtx.destination);

    isTransmitting = true;
    txStatusLabel.textContent = "TX: MIC (Live)";
    txStatusLabel.style.color = "#FF5555";
    txMicButton.textContent = "Stop Mic TX";
    txMicButton.style.backgroundColor = "#aa0000";
    txFileInput.disabled = true;

    sendCommand({
      type: 'SetTxState',
      payload: {
        active: true,
        tx_gain_db: parseFloat(txGainSlider.value)
      }
    });
  } catch (err) {
    console.error("Error accessing microphone:", err);
    alert("Could not access microphone.");
  }
}

function stopMicTx() {
  if (txStream) {
    txStream.getTracks().forEach(track => track.stop());
    txStream = null;
  }
  if (txProcessor) {
    txProcessor.disconnect();
    txProcessor = null;
  }
  isTransmitting = false;
  txStatusLabel.textContent = "TX Ready";
  txStatusLabel.style.color = "";
  txMicButton.textContent = "Mic TX";
  txMicButton.style.backgroundColor = "";
  txFileInput.disabled = !window.isConnected;

  sendCommand({
    type: 'SetTxState',
    payload: {
      active: false,
      tx_gain_db: parseFloat(txGainSlider.value)
    }
  });
}

function stopFileTx() {
  if (txFileSource) {
    txFileSource.onended = null;
    txFileSource.stop();
    txFileSource.disconnect();
    txFileSource = null;
  }
  if (txProcessor) {
    txProcessor.disconnect();
    txProcessor = null;
  }
  isTransmitting = false;
  txStatusLabel.textContent = "TX Ready";
  txStatusLabel.style.color = "";
  txFileInput.value = "";
  txFileInput.disabled = !window.isConnected;
  txMicButton.disabled = !window.isConnected;

  selectedTxFile = null;
  txFileConfirmButton.textContent = "Send File";
  txFileConfirmButton.style.backgroundColor = "";
  txFileConfirmButton.style.display = 'none';

  sendCommand({
    type: 'SetTxState',
    payload: {
      active: false,
      tx_gain_db: parseFloat(txGainSlider.value)
    }
  });
}

// Mirrors the RX mode/bandwidth onto TX when the "sync RX<->TX" box is checked.
export function syncTxToRx() {
  if (!syncRxTxCheckbox || !syncRxTxCheckbox.checked) return;
  txModeSelect.value = modeSelect.value;
  txFilterBwInput.value = filterBwInput.value;

  sendCommand({
    type: 'SetTxModulation',
    payload: {
      mode: txModeSelect.value,
      filter_bw_hz: parseFloat(txFilterBwInput.value) || 2800
    }
  });
  updateStatusBar();
}

// Enables/cleans up the TX controls when the connection state changes. Called by the core's
// updatePlaybackAbility so it doesn't need to reach into this module's private state.
export function applyConnectionState(isConnected) {
  if (txMicButton && !isTransmitting) txMicButton.disabled = !isConnected;
  if (txFileInput && !isTransmitting) txFileInput.disabled = !isConnected;

  if (!isConnected && txFileConfirmButton) {
    if (isTransmitting) {
      stopFileTx();
    } else {
      selectedTxFile = null;
      if (txFileInput) txFileInput.value = "";
      txFileConfirmButton.style.display = 'none';
    }
  }
}

// Wires all TX-related controls. Called once from the core after load.
export function initTx() {
  txModeSelect.addEventListener('change', (e) => {
    const mode = e.target.value;
    // TX only supports SSB (USB/LSB); default to a 2.8 kHz voice bandwidth.
    const defaultBw = 2800;
    txFilterBwInput.value = defaultBw;
    sendCommand({
      type: 'SetTxModulation',
      payload: {
        mode: mode,
        filter_bw_hz: defaultBw
      }
    });
    updateStatusBar();
  });

  txFilterBwInput.addEventListener('change', () => {
    let val = parseFloat(txFilterBwInput.value) || 0;
    val = Math.max(1000, Math.min(20000, val));
    txFilterBwInput.value = val;
    sendCommand({
      type: 'SetTxModulation',
      payload: {
        mode: txModeSelect.value,
        filter_bw_hz: val
      }
    });
    updateStatusBar();
  });

  txMicButton.addEventListener('click', () => {
    if (isTransmitting) {
      stopMicTx();
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
    if (isTransmitting) {
      stopFileTx();
      return;
    }

    if (!selectedTxFile) return;

    await initTxAudio();
    if (txAudioCtx.state === 'suspended') {
      await txAudioCtx.resume();
    }

    txChunkCount = 0;
    console.log("[TX Client] Reading selected audio file: " + selectedTxFile.name + " (" + selectedTxFile.size + " bytes)...");
    const arrayBuffer = await selectedTxFile.arrayBuffer();
    try {
      console.log("[TX Client] Decoding audio data...");
      const audioBuffer = await txAudioCtx.decodeAudioData(arrayBuffer);
      console.log("[TX Client] Audio file decoded successfully. Duration: " + audioBuffer.duration.toFixed(2) + "s, Sample Rate: " + audioBuffer.sampleRate + "Hz.");

      const source = txAudioCtx.createBufferSource();
      source.buffer = audioBuffer;

      txProcessor = new AudioWorkletNode(txAudioCtx, 'tx-processor');
      txProcessor.port.onmessage = (ev) => {
        if (!isTransmitting) return;
        sendTxAudioChunk(ev.data);
      };

      source.connect(txProcessor);
      txProcessor.connect(txAudioCtx.destination);

      source.onended = () => {
        stopFileTx();
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

      sendCommand({
        type: 'SetTxState',
        payload: {
          active: true,
          tx_gain_db: parseFloat(txGainSlider.value)
        }
      });
    } catch (err) {
      console.error("Error decoding audio file:", err);
      alert("Could not decode the selected audio file.");
      txFileInput.value = "";
      selectedTxFile = null;
      txFileConfirmButton.style.display = 'none';
    }
  });

  if (syncRxTxCheckbox) {
    // Sync is checked by default: disable TX selectors and run initial sync
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
