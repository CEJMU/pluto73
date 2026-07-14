// Pure frequency-formatting helpers. No DOM or shared-state dependencies, so this module is
// trivially reusable/testable in isolation.

// Formats a frequency in Hz as a human string (GHz/MHz/kHz/Hz), choosing decimal places from the
// optional `precisionHz` (the tick/bin spacing) so the label shows just enough resolution.
export function formatFrequency(freqHz, precisionHz) {
  const absFreq = Math.abs(freqHz);
  if (absFreq >= 1000000000) {
    const decimals = precisionHz ? Math.max(0, Math.ceil(-Math.log10(precisionHz / 1000000000) - 1e-6)) : 4;
    return (freqHz / 1000000000).toFixed(decimals) + ' GHz';
  } else if (absFreq >= 1000000) {
    const decimals = precisionHz ? Math.max(0, Math.ceil(-Math.log10(precisionHz / 1000000) - 1e-6)) : 4;
    return (freqHz / 1000000).toFixed(decimals) + ' MHz';
  } else if (absFreq >= 1000) {
    const decimals = precisionHz ? Math.max(0, Math.ceil(-Math.log10(precisionHz / 1000) - 1e-6)) : 3;
    return (freqHz / 1000).toFixed(decimals) + ' kHz';
  } else {
    const decimals = precisionHz ? Math.max(0, Math.ceil(-Math.log10(precisionHz) - 1e-6)) : 0;
    return freqHz.toFixed(decimals) + ' Hz';
  }
}
export function formatHzToMhz(hz) {
  return (hz / 1000000).toFixed(3) + ' MHz';
}

export function formatHzToMhzPrecise(hz, decimals) {
  return (hz / 1000000).toFixed(decimals) + ' MHz';
}

export function formatHzToMsps(hz) {
  return (hz / 1000000).toFixed(3) + ' MSPS';
}

export function formatHzShort(hz) {
  if (hz >= 1000) {
    return (hz / 1000).toFixed(1) + ' kHz';
  } else {
    return hz.toFixed(0) + ' Hz';
  }
}
