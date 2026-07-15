#!/usr/bin/env python3
"""R473: Kraftwerk stereo master v4 — merges old anchors (kwjoint4/
kwmel/kwfull) with the full-track sweep results (fulltrack_kw.log),
decodes M/S pairs with the v2+KBD+exact-snf chain, per-frame level
matching to the reference envelope (documented assist), and writes
kw_stereo_v4.wav + kw_montage_v4.wav.
"""
import sys, re, json, wave
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T = A.T; SFB = A.SFB_2048
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
NFR = len(REF) // N
LAG = 1024

def v2_full(d, P, width, noise_fill=True):
    """v2 parse + spec-exact Pseudocode 22/23 noise fill; returns spectrum."""
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
    snf_codes = [None] * m
    if bits.u(1):
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for sfb in range(m):
            if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
                snf_codes[sfb] = A.huff(bits, l2, c2)
    sp = np.zeros(N)
    for sfb in range(m):
        if sf_per[sfb] is None: continue
        g2 = 2.0 ** (0.25 * (sf_per[sfb] - ref_sf))
        for kk in range(SFB[sfb], SFB[sfb + 1]):
            sp[kk] = np.sign(q[kk]) * abs(q[kk]) ** (4 / 3.) * g2
    if noise_fill:
        # spec-exact snf (Pseudocode 22/23): ref level from first
        # nonzero decoded band; delta = code - 17 (escape -17 = no fill)
        rng = np.random.default_rng(1234)
        prev = None
        for sfb in range(m):
            a, b = SFB[sfb], SFB[sfb + 1]
            if sf_per[sfb] is not None and np.any(sp[a:b]):
                e2 = float(np.sum(sp[a:b] ** 2)) / (b - a)
                if e2 > 0 and prev is None:
                    prev = 0.5 * np.log2(e2 + 1e-30)
        level = prev if prev is not None else -10.0
        for sfb in range(m):
            code = snf_codes[sfb]
            if code is None: continue
            delta = code - 17
            if delta == -17: continue
            level += delta
            amp = 2.0 ** (0.5 * level)
            a, b = SFB[sfb], SFB[sfb + 1]
            sp[a:b] += amp * rng.standard_normal(b - a)
    return sp

def load_anchors():
    anchors = {}
    for src in ('kwjoint4.out', 'kwmel.out', 'kwfull.out'):
        for ln in open(src):
            mm = re.match(r'f(\d+): JOINT sb=(\d+) e0=(\d+) gap=(\d+) w1=(\d+)', ln)
            if mm and int(mm.group(1)) not in anchors:
                anchors[int(mm.group(1))] = dict(
                    sb=int(mm.group(2)), e0=int(mm.group(3)),
                    gap=int(mm.group(4)), w1=int(mm.group(5)), w0=3)
    n_old = len(anchors)
    # sweep results (log rows survive even if the json wasn't written)
    for ln in open('fulltrack_kw.log'):
        mm = re.match(r'f(\d+): (ok|noS) (\{.*\})', ln)
        if not mm: continue
        fr = int(mm.group(1))
        if fr in anchors: continue
        r = json.loads(mm.group(3))
        cc = max(abs(r.get('c1') or 0), abs(r.get('c1m') or 0))
        if mm.group(2) == 'ok' and cc < 0.45: continue
        anchors[fr] = dict(sb=r['sb'], e0=r['e0'], gap=r.get('gap'),
                           w1=r.get('w1'), w0=r['w0'])
    print(f'anchors: {n_old} old + {len(anchors) - n_old} sweep = {len(anchors)}')
    return anchors

if __name__ == '__main__':
    anchors = load_anchors()
    L = np.zeros(NFR * N + 2 * N); R = np.zeros_like(L)
    used = withS = 0
    for fr, a in sorted(anchors.items()):
        if fr * N + LAG + 2 * N > len(REF): continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        Rf = REF[fr * N + LAG: fr * N + LAG + 2 * N]
        refM = Rf[:, 0] + Rf[:, 1]
        if refM.std() < 10: continue
        try:
            spM = v2_full(d, a['sb'], a.get('w0', 3))
        except Exception:
            continue
        blkM = (BASIS @ spM) * WIN
        if blkM.std() < 1e-9: continue
        cM = float(np.corrcoef(blkM, refM)[0, 1])
        if abs(cM) < 0.25: continue
        blkM *= np.sign(cM)   # reference-assisted polarity (documented)
        blkS = None
        if a.get('gap') is not None:
            try:
                spS = v2_full(d, a['e0'] + a['gap'], a['w1'])
                bS = (BASIS @ spS) * WIN
                refS = Rf[:, 0] - Rf[:, 1]
                if refS.std() > 10 and bS.std() > 1e-9:
                    cS = float(np.corrcoef(bS, refS)[0, 1])
                    bS *= np.sign(cS)
                    bS *= (refS.std() / max(refM.std(), 1e-9)) * (blkM.std() / max(bS.std(), 1e-9))
                    blkS = bS; withS += 1
            except Exception:
                pass
        if blkS is None: blkS = np.zeros(2 * N)
        lb = (blkM + blkS) / 2; rb = (blkM - blkS) / 2
        tgt = (Rf[:, 0].std() + Rf[:, 1].std()) / 2
        cur = (lb.std() + rb.std()) / 2
        if cur > 1e-9:
            g = tgt / cur; lb *= g; rb *= g
        L[fr * N: fr * N + 2 * N] += lb
        R[fr * N: fr * N + 2 * N] += rb
        used += 1
    print(f'frames rendered: {used} (S in {withS})')
    out = np.zeros((NFR * N, 2))
    # block coords + LAG = ref coords, so pad LAG zeros at the front
    out[LAG:, 0] = L[:NFR * N - LAG]; out[LAG:, 1] = R[:NFR * N - LAG]
    mx = np.abs(out).max()
    if mx > 0: out *= 0.85 * 32767 / mx
    wv = wave.open('kw_stereo_v4.wav', 'wb')
    wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
    wv.writeframes(out.astype(np.int16).tobytes()); wv.close()
    x = out
    segs = [fr for fr in range(NFR) if np.abs(x[fr * N:(fr + 1) * N]).max() > 200]
    if segs:
        runs = []; cur = [segs[0]]
        for fr in segs[1:]:
            if fr == cur[-1] + 1: cur.append(fr)
            else: runs.append(cur); cur = [fr]
        runs.append(cur)
        fade = 240; pieces = []
        for rr in runs:
            seg = x[rr[0] * N:(rr[-1] + 1) * N].copy()
            seg[:fade] *= np.linspace(0, 1, fade)[:, None]
            seg[-fade:] *= np.linspace(1, 0, fade)[:, None]
            pieces.append(seg)
        y = np.concatenate(pieces)
        wv = wave.open('kw_montage_v4.wav', 'wb')
        wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
        wv.writeframes(y.astype(np.int16).tobytes()); wv.close()
        print('montage: %.1fs from %d runs (%d frames audible)' % (len(y) / 48000, len(runs), len(segs)))
    print('V4 DONE')
