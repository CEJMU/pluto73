"""
TX 4x Interpolating FIR - CIC Alias Suppression Filter
----------------------------------------------------------
Chain:   48 kHz  ->  [FIR x4]  ->  192 kHz  ->  [CIC xR]  ->  tx_fs

Method: Parks-McClellan equiripple (remez) for optimal alias suppression.
Bands:
    Band 0      [0, 20_000 Hz]          passband
    Transition  [20_000, 28_000 Hz]     8 kHz transition band
    Band 1      [28_000, 96_000 Hz]     stopband
"""

import numpy as np
from scipy.signal import remez

# -- Chain parameters ----------------------------------------------------------
F_IN    = 48_000
F_OUT   = 192_000
NYQUIST = F_OUT // 2

# -- remez design parameters ---------------------------------------------------
N_TAPS  = 55
F_PASS  = 20_000         # passband edge [Hz]
F_STOP  = 28_000         # stopband start [Hz] (first alias starts at F_IN - F_PASS)
W_PASS  = 1.0            # passband weight
W_STOP  = 1870.0         # optimized stopband weight

# -- Design and scale to 16-bit integers (max amplitude = 32767) ---------------
h = remez(N_TAPS, [0, F_PASS, F_STOP, NYQUIST], [1.0, 0.0], weight=[W_PASS, W_STOP], fs=F_OUT)
h_int = np.round(h / np.max(np.abs(h)) * 32767).astype(int)

print(f"Symmetric {N_TAPS}-tap CoefficientVector for Vivado FIR Compiler:")
print(",".join(map(str, h_int.tolist())))
