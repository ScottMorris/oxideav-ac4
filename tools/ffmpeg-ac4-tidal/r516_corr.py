#!/usr/bin/env python3
"""R516: synthesize dumped core spectra (float, no clipping) and
cross-correlate every coded channel vs every reference channel.
Validates whether the MSFB5 core decode is semantically correct
independent of the fork's 7.1 channel routing."""
import numpy as np, wave
N = 2048
sp = np.fromfile('/tmp/spec.bin', dtype=np.float32).astype(np.float64)
nf = sp.size // (8 * N)
sp = sp[:nf * 8 * N].reshape(nf, 8, N)
n_ = np.arange(2 * N); k_ = np.arange(N)
BASIS = np.cos(np.pi / N * (n_[:, None] + 0.5 + N / 2) * (k_[None, :] + 0.5))
from numpy import i0
xg = np.arange(N + 1) / N
kern = i0(np.pi * 5.0 * np.sqrt(np.clip(1 - (2 * xg - 1) ** 2, 0, 1)))
cs = np.cumsum(kern[:N]); KBDh = np.sqrt(cs / cs[-1])
WIN = np.concatenate([KBDh, KBDh[::-1]])


def synth(ch):
    out = np.zeros(nf * N + N)
    for f in range(nf):
        s = sp[f, ch]
        mx = np.abs(s).max()
        if mx > 1e6:
            s = s / mx * 1e4      # per-frame magnitude clamp, keep shape
        out[f * N:f * N + 2 * N] += (BASIS @ s) * WIN
    return out[:nf * N]


w = wave.open('kw-ref51.wav', 'rb'); rc = w.getnchannels()
ref = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16
                    ).astype(np.float64).reshape(-1, rc)


def corr(a, b):
    a = a - a.mean(); b = b - b.mean(); d = a.std() * b.std()
    return float((a * b).mean() / d) if d > 1e-9 else 0.0


for cc in range(8):
    y = synth(cc)
    if np.abs(y).max() < 1:
        print(f'coded ch{cc}: silent'); continue
    best = (0, 0, 0.0); m = min(len(y), len(ref))
    for rc_ in range(rc):
        r = ref[:, rc_]
        for lag in range(-2048, 2049, 64):
            if lag >= 0:
                c = corr(y[lag:m], r[:m - lag])
            else:
                c = corr(y[:m + lag], r[-lag:m])
            if abs(c) > abs(best[2]):
                best = (rc_, lag, c)
    print(f'coded ch{cc}: best vs ref ch{best[0]} lag{best[1]} '
          f'corr {best[2]:+.3f}')
