#!/usr/bin/env python3
"""Round 419b: joint strict+content LFE position scan.

For each dumped substream (subNNN.bin, 16-bit audio_size header), sweep
LFE-body start positions x (section-width, m-bits) variants; a position
counts only if the body parses SPEC-STRICT (ac4scan primitives, no
saturation) AND its dequantized band-energy profile correlates >0.55
with the 5.1 E-AC-3 reference LFE channel's forward-MDCT bands at the
project's fixed lag (96). Oracle validated on frame-0's r410-proven
body (0.546).

Finding on the Kraftwerk test track: 118/183 frames carry a
content-validated LFE at FRAME-VARIABLE positions 75-199 — i.e. a
variable-length element (~80-180 bits, ~0 on frame 0) sits between the
6-bit P-head and the LFE. Size class matches acpl_data_1ch x4.

Usage: lfe_content_scan.py <subs_dir> <ref51.wav> [lag]
"""
import sys, re, glob, wave
import numpy as np

sys.path.insert(0, __file__.rsplit('/', 1)[0])
import ac4scan as A

subs_dir, ref_path = sys.argv[1], sys.argv[2]
LAG = int(sys.argv[3]) if len(sys.argv) > 3 else 96
w = wave.open(ref_path, 'rb')
ref = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)
ref = ref.astype(np.float64).reshape(-1, w.getnchannels())[:, 3]
N = 2048
n_ = np.arange(2 * N); k_ = np.arange(N)
basis = np.cos(np.pi / N * (n_[:, None] + 0.5 + N / 2) * (k_[None, :] + 0.5))
win = np.sin(np.pi * (n_ + 0.5) / (2 * N))
nb = 28

def refbands(fr):
    st = fr * N + LAG
    if st < 0 or st + 2 * N > len(ref):
        return None
    X = ((ref[st:st + 2 * N] * win)[:, None] * basis).sum(0)
    return np.abs(X[:nb])

for f in sorted(glob.glob(subs_dir + '/sub*.bin')):
    fr = int(re.search(r'sub(\d+)\.bin', f).group(1))
    data = open(f, 'rb').read()
    a = refbands(fr)
    if a is None or a.max() < 50:
        continue
    hits = []
    for sb in range(16, 220):
        for width in (3, 5):
            for mbits in (3, 4):
                try:
                    bits = A.Bits(data, sb)
                    m = bits.u(mbits)
                    if m == 0 or m > 7:
                        continue
                    sects = A.parse_sections(bits, width, m, True)
                    q, mqi = A.parse_spectra(bits, sects, A.SFB_2048, m)
                    A.parse_scalefac(bits, sects, mqi, m, True)
                    A.parse_snf(bits, sects, mqi, m)
                except Exception:
                    continue
                qp = np.abs(np.array(q, dtype=np.float64))
                if (qp != 0).sum() < 2:
                    continue
                b = np.zeros(nb)
                b[:min(nb, len(qp))] = qp[:nb] ** (4 / 3.)
                c = float(np.corrcoef(np.log(a + 1e-9), np.log(b + 1e-9))[0, 1])
                if c > 0.55:
                    hits.append((sb, width, mbits, round(c, 2), bits.p))
    if hits:
        print(f'f{fr}:', hits[:6])
