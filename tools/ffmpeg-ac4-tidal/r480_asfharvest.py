#!/usr/bin/env python3
"""R480: does ac4asf (correct short/multigroup + absolute scalefac)
capture better long-body content than the old long-only v2_parse?

Scan dump positions with parse_sf_info+parse_sf_data. For LONG bodies
build the 2048 MDCT spectrum and correlate vs the 6 refs (free lag,
sign). Report best correlation + max_sfb captured per frame, compared
to the old harvester's anchor for the same frame.
"""
import sys, json, re, wave
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf
N = 2048
n_ = np.arange(2 * N); k_ = np.arange(N)
BASIS = np.cos(np.pi / N * (n_[:, None] + 0.5 + N / 2) * (k_[None, :] + 0.5))
from numpy import i0
xg = np.arange(N + 1) / N
kern = i0(np.pi * 3.0 * np.sqrt(np.clip(1 - (2 * xg - 1) ** 2, 0, 1)))
cs = np.cumsum(kern[:N]); KBDh = np.sqrt(cs / cs[-1])
WIN = np.concatenate([KBDh, KBDh[::-1]])
w = wave.open('kw-ref51.wav', 'rb')
REF = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
ORN = ['L', 'R', 'C', 'LFE', 'Ls', 'Rs']


def bestcorr(fr, sp):
    pcm = (BASIS @ sp) * WIN
    if pcm.std() < 1e-9:
        return 0.0, -1, 0
    best = (0.0, -1, 0)
    for ch in range(6):
        for lag in range(960, 1105, 8):
            st = fr * N + lag
            if st < 0 or st + 2 * N > len(REF):
                continue
            r = REF[st:st + 2 * N, ch]
            if r.std() < 10:
                continue
            c = float(np.corrcoef(pcm, r)[0, 1])
            if abs(c) > abs(best[0]):
                best = (c, ch, lag)
    return best


def scan_frame(d, fr, wall):
    """scan all starts, parse_sf_info+data, keep best-correlating LONG
    bodies per (channel,end)."""
    hits = []
    for sb in range(16, max(17, wall - 200)):
        try:
            bits = A.Bits(d, sb)
            cfg = ac4asf.parse_sf_info(bits)
        except Exception:
            continue
        if cfg.long_frame != 1:
            continue
        if not (1 <= cfg.get_max_sfb(0) <= 63):
            continue
        try:
            data = ac4asf.parse_sf_data(bits, cfg)
        except Exception:
            continue
        sp = ac4asf.core_spectrum(cfg, data)
        nz = int((sp != 0).sum())
        if nz < 12:
            continue
        c, ch, lag = bestcorr(fr, sp)
        if abs(c) < 0.45:
            continue
        hits.append((abs(c), sb, data.end, cfg.get_max_sfb(0), ch, round(c, 3), nz))
    hits.sort(reverse=True)
    # dedupe by end
    seen = set(); out = []
    for h in hits:
        if h[2] in seen:
            continue
        seen.add(h[2]); out.append(h)
    return out


if __name__ == '__main__':
    # load old anchors for comparison
    old = {}
    for src in ('kwjoint4.out', 'kwmel.out', 'kwfull.out'):
        try:
            for ln in open(src):
                mm = re.match(r'f(\d+): JOINT sb=(\d+) e0=(\d+) gap=(\d+) w1=(\d+)', ln)
                if mm and int(mm.group(1)) not in old:
                    old[int(mm.group(1))] = dict(sb=int(mm.group(2)), m0=None)
        except FileNotFoundError:
            pass
    frames = [int(x) for x in sys.argv[1:]] if len(sys.argv) > 1 else [24, 48, 72, 120, 240, 360, 480]
    for fr in frames:
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            print(f'f{fr}: nodump'); continue
        wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
        if fr * N + 1104 + 2 * N > len(REF):
            print(f'f{fr}: noref'); continue
        hits = scan_frame(d, fr, wall)
        top = hits[:6]
        msfbs = [h[3] for h in hits[:10]]
        maxc = hits[0][0] if hits else 0
        print(f'f{fr}: {len(hits)} long-hits |c|>=.45  bestc={maxc:.2f}  '
              f'max_sfb seen(top10)={sorted(set(msfbs), reverse=True)}  '
              + ' '.join(f'[{ORN[h[4]]}{h[5]:+.2f}@{h[1]} m={h[3]} nz={h[6]}]' for h in top), flush=True)
