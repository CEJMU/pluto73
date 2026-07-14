// RX audio playback: decodes the f32 PCM the backend streams over the WebSocket and schedules it
// through a small jitter buffer, plus the volume/mute UI. Self-contained - it only reaches into the
// core for `sendCommand` (to toggle backend audio) and shares no mutable state with it.

import { sendCommand } from './app.js';

const muteCheckbox = document.getElementById('mute-checkbox');
const volumeSlider = document.getElementById('vol');

let audioCtx = null;
let gainNode = null;
let compressorNode = null;
let nextAudioTime = 0;
let debugAudioReceivedCount = 0;
let isAudioEnabled = false;
let currentVolume = 0.0;

function initAudio() {
  if (!audioCtx) {
    console.log("Initializing AudioContext at 48000 Hz...");
    try {
      // Default to 48kHz which is common for SDR audio pipelines
      audioCtx = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: 48000 });

      // Add a limiter to compress loud sounds automatically without harsh clipping
      compressorNode = audioCtx.createDynamicsCompressor();
      compressorNode.threshold.value = -3.0; // Start compressing near the maximum level
      compressorNode.knee.value = 5.0;       // Smooth transition into compression
      compressorNode.ratio.value = 20.0;     // Aggressive limiting ratio
      compressorNode.attack.value = 0.002;   // Very fast attack to catch spikes
      compressorNode.release.value = 0.100;  // Fast release

      // Use a GainNode for volume instead of manually multiplying samples
      gainNode = audioCtx.createGain();
      gainNode.gain.value = currentVolume;

      gainNode.connect(compressorNode);
      compressorNode.connect(audioCtx.destination);
    } catch (err) {
      console.error("Failed to initialize AudioContext:", err);
      alert("Failed to initialize audio. Your browser might be blocking it.");
    }
  }
}

// Schedules one received PCM chunk (Float32Array, mono, 48 kHz) for playback, nudging playbackRate
// to keep the jitter buffer near ~200 ms without audible pitch shift.
export function playAudioChunk(pcmArray) {
  if (!isAudioEnabled) return;

  if (!audioCtx) {
    console.warn("Audio chunk received, but AudioContext is not initialized yet.");
    return;
  }
  if (audioCtx.state === 'suspended') {
    console.log("AudioContext is suspended. Attempting to resume...");
    audioCtx.resume();
  }

  debugAudioReceivedCount++;

  const buffer = audioCtx.createBuffer(1, pcmArray.length, 48000);
  const channelData = buffer.getChannelData(0);
  let maxAmp = 0;
  for (let i = 0; i < pcmArray.length; i++) {
    let val = pcmArray[i];
    if (!Number.isFinite(val)) val = 0.0;
    if (val > 1.0) val = 1.0;
    else if (val < -1.0) val = -1.0;
    channelData[i] = val;
    if (Math.abs(val) > maxAmp) maxAmp = Math.abs(val);
  }

  const source = audioCtx.createBufferSource();
  source.buffer = buffer;
  source.connect(gainNode);

  const currentTime = audioCtx.currentTime;
  let queueDepth = nextAudioTime - currentTime;

  if (queueDepth < 0.0) {
    if (debugAudioReceivedCount > 1) {
      console.warn(`Audio buffer underflow! Missed by ${(-queueDepth).toFixed(3)}s`);
    }
    nextAudioTime = currentTime + 0.20;
    queueDepth = 0.20;
  } else if (queueDepth > 0.5) {
    console.warn("Audio buffer overflow! Dropping latency to catch up.");
    nextAudioTime = currentTime + 0.20;
    queueDepth = 0.20;
  }

  if (queueDepth > 0.35) source.playbackRate.value = 1.015;
  else if (queueDepth > 0.25) source.playbackRate.value = 1.010;
  else if (queueDepth < 0.05) source.playbackRate.value = 0.985;
  else if (queueDepth < 0.10) source.playbackRate.value = 0.990;
  else source.playbackRate.value = 1.0;

  source.onended = () => {
    source.disconnect();
  };
  source.start(nextAudioTime);
  nextAudioTime += buffer.duration / source.playbackRate.value;
}

async function setMutedState(muted) {
  muteCheckbox.checked = muted;
  if (!muted) {
    console.log("Unmuted. Requesting audio initialization...");
    initAudio();
    if (audioCtx && audioCtx.state === 'suspended') {
      await audioCtx.resume();
      console.log("AudioContext forcefully resumed. State:", audioCtx.state);
    }

    // Play a short silent burst to permanently unlock the browser's audio engine
    const osc = audioCtx.createOscillator();
    const gain = audioCtx.createGain();
    gain.gain.value = 0;
    osc.connect(gain);
    gain.connect(audioCtx.destination);
    osc.start(0);
    osc.stop(audioCtx.currentTime + 0.1);

    isAudioEnabled = true;
    nextAudioTime = 0; // Reset scheduling to avoid massive underflow warnings after being paused
    debugAudioReceivedCount = 0; // Reset debug counter
    sendCommand({ type: 'SetRxAudioEnabled', payload: { enabled: true } });
  } else {
    isAudioEnabled = false;
    sendCommand({ type: 'SetRxAudioEnabled', payload: { enabled: false } });
  }
}

// Wires the volume slider and mute checkbox. Called once from the core after load.
export function initAudioUI() {
  volumeSlider.addEventListener('input', async (e) => {
    const val = parseInt(e.target.value, 10);
    currentVolume = val / 100.0;

    if (gainNode && audioCtx) {
      // Smoothly set volume to avoid zipper noise clicks when dragging the slider
      gainNode.gain.setTargetAtTime(currentVolume, audioCtx.currentTime, 0.05);
    }

    if (val === 0) {
      if (!muteCheckbox.checked) {
        await setMutedState(true);
      }
    } else {
      if (muteCheckbox.checked) {
        await setMutedState(false);
      }
    }
  });

  muteCheckbox.addEventListener('change', async (e) => {
    const muted = e.target.checked;
    if (!muted && parseInt(volumeSlider.value, 10) === 0) {
      // If user manually unmutes, slide volume to a default comfortable level (100)
      volumeSlider.value = 100;
      currentVolume = 1.0;
      if (gainNode && audioCtx) {
        gainNode.gain.value = currentVolume;
      }
    }
    await setMutedState(muted);
  });
}
