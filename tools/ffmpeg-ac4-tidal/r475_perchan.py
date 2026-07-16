#!/usr/bin/env python3
"""R475: PER-CHANNEL ANCHOR HARVEST -> within-frame pad sequences.

Independent full-position scan per frame (no chaining). Every valid
v2 parse candidate (nz>=6) is correlated free-lag against EIGHT
oracles: the 6 discrete E-AC-3 reference channels plus M=L+R and
S=L-R. Hits with |c|>=CMIN are deduped by body end, pruned to a
non-overlapping set (weighted interval scheduling by sum|c|), sorted
by position -> pad sequence per frame.

Goal: test the monotone-pad law (war f33/f60: 9/9 monotone steps)
at scale, and get (position-in-frame, pad, channel) triples.

Usage: r475_perchan.py <track> <fr> [fr ...]
Log rows: fN: SEQ [...json...]   (durable across kills)
"""
import sys, json, wave, os
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T = A.T; SFB = A.SFB_2048
N = 2048
n_ = np.arange(2 * N); k_ = np.arange(N)
BASIS = np.cos(np.pi / N * (n_[:, None] + 0.5 + N / 2) * (k_[None, :] + 0.5)).astype(np.float32)
from numpy import i0
xg = np.arange(N + 1) / N
kern = i0(np.pi * 3.0 * np.sqrt(np.clip(1 - (2 * xg - 1) ** 2, 0, 1)))
cs = np.cumsum(kern[:N]); KBDh = np.sqrt(cs / cs[-1])
WIN = np.concatenate([KBDh, KBDh[::-1]]).astype(np.float32)
CMIN = 0.50

TRACK = sys.argv[1]
REFWAV = {'kw': 'kw-ref51.wav', 'spk': 'spk-ref51.wav'}[TRACK]
DUMPDIR = {'kw': 'kw4', 'spk': 'spk4'}[TRACK]
w = wave.open(REFWAV, 'rb')
_R = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
# 8 oracles: 6 discrete + M + S
REF = np.zeros((_R.shape[0], 8))
REF[:, :6] = _R
REF[:, 6] = _R[:, 0] + _R[:, 1]
REF[:, 7] = _R[:, 0] - _R[:, 1]
ORNAMES = ['L', 'R', 'C', 'LFE', 'Ls', 'Rs', 'M', 'S']
LAGS = list(range(992, 1073, 8))

def v2_parse(d, P, width):
    bits = A.Bits(d, P)
    m = bits.u(5)
    if m > 56 or m < 1: raise ValueError
    esc = (1 << width) - 1; sects = []; k = 0
    while k < m:
        cb = bits.u(4); ln = 1; li = bits.u(width)
        while li == esc: ln += esc; li = bits.u(width)
        ln += li
        if k + ln > 127: raise ValueError
        sects.append((k, k + ln, cb)); k += ln
    mqi = [0] * m; q = np.zeros(SFB[m])
    for (s, e, cb) in sects:
        if cb == 0 or cb > 11 or e > m: continue
        q2, mq2 = A.parse_spectra(bits, [(s, e, cb)], SFB, m)
        for xx in range(s, e): mqi[xx] = max(mqi[xx], mq2[xx])
        for i2 in range(SFB[s], SFB[e]): q[i2] = q2[i2]
    ref_sf = bits.u(8)
    sfb_cb = [0] * m
    for (s, e, cb) in sects:
        for xx in range(s, min(e, m)): sfb_cb[xx] = cb
    sf = ref_sf; first = False; sf_per = [None] * m
    lens, cws = T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']
    for sfb in range(m):
        if sfb_cb[sfb] == 0 or mqi[sfb] == 0: continue
        if first:
            sf += A.huff(bits, lens, cws) - 60
            if not (0 <= sf <= 255): raise ValueError
        else: first = True
        sf_per[sfb] = sf
    if bits.u(1):
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for sfb in range(m):
            if sfb_cb[sfb] == 0 or mqi[sfb] == 0: A.huff(bits, l2, c2)
    sp = np.zeros(N, dtype=np.float32)
    for sfb in range(m):
        if sf_per[sfb] is None: continue
        g2 = 2.0 ** (0.25 * (sf_per[sfb] - ref_sf))
        for kk in range(SFB[sfb], SFB[sfb + 1]):
            sp[kk] = np.sign(q[kk]) * abs(q[kk]) ** (4 / 3.) * g2
    return sp, bits.p, m

def work(fr):
    try:
        d = open(f'{DUMPDIR}/sub{fr:04d}.bin', 'rb').read()
    except FileNotFoundError:
        return None
    wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
    if fr * N + 1104 + 2 * N > len(REF): return None
    # precompute lagged ref windows: (nlags, 2N, 8) normalized
    RW = np.zeros((len(LAGS), 2 * N, 8), dtype=np.float32)
    RSD = np.zeros((len(LAGS), 8))
    for li, lag in enumerate(LAGS):
        st = fr * N + lag
        seg = REF[st:st + 2 * N]
        mu = seg.mean(0); sd = seg.std(0)
        RSD[li] = sd
        RW[li] = ((seg - mu) / np.where(sd < 1e-9, 1, sd)).astype(np.float32)
    hits = []
    for w2 in (3, 5):
        for sb in range(16, max(17, wall - 150)):
            try:
                sp, e, m = v2_parse(d, sb, w2)
            except Exception:
                continue
            if (sp != 0).sum() < 6: continue
            mx = np.abs(sp).max()
            if mx <= 0: continue
            pcm = (BASIS @ (sp / mx)) * WIN
            psd = pcm.std()
            if psd < 1e-9: continue
            pz = (pcm - pcm.mean()) / psd
            # corr vs all 8 oracles x all lags in one matmul
            C = np.einsum('t,lto->lo', pz, RW) / (2 * N)
            C[RSD < 10] = 0.0
            li2, ch = np.unravel_index(np.argmax(np.abs(C)), C.shape)
            c = float(C[li2, ch])
            if abs(c) < CMIN: continue
            hits.append(dict(s=sb, e=e, w=w2, m=m, c=round(c, 3),
                             ch=ORNAMES[ch], lag=LAGS[li2]))
    if not hits:
        return {'fr': fr, 'n': 0, 'seq': []}
    # dedupe by end: keep best |c|
    bye = {}
    for h in hits:
        if h['e'] not in bye or abs(h['c']) > abs(bye[h['e']]['c']):
            bye[h['e']] = h
    hs = sorted(bye.values(), key=lambda h: h['e'])
    # weighted interval scheduling: max sum|c| non-overlapping set
    import bisect
    ends = [h['e'] for h in hs]
    best = [0.0] * (len(hs) + 1)
    take = [False] * len(hs)
    prev = [0] * len(hs)
    for i, h in enumerate(hs):
        prev[i] = bisect.bisect_right(ends, h['s'], 0, i)
    for i, h in enumerate(hs):
        w_ = abs(h['c'])
        if best[prev[i]] + w_ > best[i]:
            best[i + 1] = best[prev[i]] + w_; take[i] = True
        else:
            best[i + 1] = best[i]
    sel = []; i = len(hs)
    while i > 0:
        if take[i - 1]:
            sel.append(hs[i - 1]); i = prev[i - 1]
        else: i -= 1
    sel.reverse()
    pads = [sel[j + 1]['s'] - sel[j]['e'] for j in range(len(sel) - 1)]
    return {'fr': fr, 'n': len(sel), 'seq': sel, 'pads': pads, 'wall': wall}

if __name__ == '__main__':
    frames = [int(x) for x in sys.argv[2:]]
    for fr in frames:
        r = work(fr)
        if r is None:
            print(f'f{fr}: skip', flush=True); continue
        print(f'f{fr}: SEQ ' + json.dumps(r), flush=True)
    print('DONE', flush=True)
