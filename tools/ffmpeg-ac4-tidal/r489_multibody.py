#!/usr/bin/env python3
"""R489 (FULL-CORE campaign): multi-body harvest per frame.

From each frame's anchor (body0), chain FORWARD and BACKWARD:
next body start is scanned in gap 0..MAXGAP after previous end
(back: body end must land in [start-MAXGAP, start]). Bodies are
validated WITHOUT reference by: v2 parse success + min-bits width
self-consistency (R487, 95%) + minimum content (nz), and scored
WITH reference by best |corr| against 8 oracles (6 discrete + M + S),
free lag. Keep bodies with corr >= CKEEP or (parse-valid AND
min-bits-consistent AND nz >= NZMIN and gap small).

Output per frame: ordered body list [(s, e, w, m, nz, c, ch)] and
gaps. This is both the v8 render inventory AND the cartography corpus.

Usage: r489_multibody.py <fr...>        (kw only)
Log rows: fN: BODIES [...json...]
"""
import sys, json, re, wave, os
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
w = wave.open('kw-ref51.wav', 'rb')
_R = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
REF = np.zeros((_R.shape[0], 8))
REF[:, :6] = _R
REF[:, 6] = _R[:, 0] + _R[:, 1]
REF[:, 7] = _R[:, 0] - _R[:, 1]
ORN = ['L', 'R', 'C', 'LFE', 'Ls', 'Rs', 'M', 'S']
LAGS = list(range(984, 1081, 8))
MAXGAP = 90
CKEEP = 0.35
NZMIN = 6


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
    return sp, bits.p, m, sects


def sect_cost(sects, width):
    esc = (1 << width) - 1; c = 0
    for (s, e, cb) in sects:
        ln = e - s; c += 4; ln -= 1
        while ln >= esc: c += width; ln -= esc
        c += width
    return c


def minbits_ok(sects, width):
    c3, c5 = sect_cost(sects, 3), sect_cost(sects, 5)
    return (width == 3 and c3 <= c5) or (width == 5 and c5 <= c3)


def make_refwin(fr):
    RW = np.zeros((len(LAGS), 2 * N, 8), dtype=np.float32)
    RSD = np.zeros((len(LAGS), 8))
    for li, lag in enumerate(LAGS):
        st = fr * N + lag
        if st < 0 or st + 2 * N > len(REF): return None, None
        seg = REF[st:st + 2 * N]
        mu = seg.mean(0); sd = seg.std(0); RSD[li] = sd
        RW[li] = ((seg - mu) / np.where(sd < 1e-9, 1, sd)).astype(np.float32)
    return RW, RSD


def band_profiles(fr):
    """log band-energy profiles of the 8 oracles at lag 1024."""
    out = np.zeros((8, 48))
    for ch in range(8):
        seg = REF[fr * N + 1024: fr * N + 1024 + 2 * N, ch]
        X = (seg * WIN.astype(np.float64)) @ BASIS.astype(np.float64)
        out[ch] = [np.log(np.sum(X[SFB[i]:SFB[i + 1]] ** 2) + 1e-6) for i in range(48)]
    return out


def body_at(d, P, RW, RSD, BP, widths=(3, 5)):
    """best (parse+minbits) body at exact start P; returns dict or None.
    Validation: time corr (free lag, 8 oracles) AND band-envelope corr
    vs the 8 oracle profiles (bcorr)."""
    best = None
    for w2 in widths:
        try:
            sp, e, m, sects = v2_parse(d, P, w2)
        except Exception:
            continue
        nz = int((sp != 0).sum())
        if nz < NZMIN: continue
        if not minbits_ok(sects, w2): continue
        c = 0.0; ch = -1
        mx = float(np.abs(sp).max())
        if mx <= 0: continue
        pcm = (BASIS @ (sp / mx)) * WIN
        psd = pcm.std()
        if psd > 1e-9:
            pz = (pcm - pcm.mean()) / psd
            C = np.einsum('t,lto->lo', pz, RW) / (2 * N)
            C[RSD < 10] = 0.0
            li2, ch = np.unravel_index(np.argmax(np.abs(C)), C.shape)
            c = float(C[li2, ch])
        # band-envelope validation
        nb = min(m, 48)
        eb = np.array([np.sum(sp[SFB[i]:SFB[i + 1]] ** 2) for i in range(nb)], dtype=np.float64)
        lb = np.log(eb + 1e-9)
        bc = 0.0
        if nb >= 6 and lb.std() > 1e-9:
            for ch2 in range(8):
                prof = BP[ch2][:nb]
                if prof.std() > 1e-9:
                    v = float(np.corrcoef(lb, prof)[0, 1])
                    if v > bc: bc = v
        cand = dict(s=P, e=e, w=w2, m=m, nz=nz, c=round(c, 3),
                    bc=round(bc, 2), ch=ORN[ch] if ch >= 0 else '?')
        if best is None or abs(cand['c']) > abs(best['c']):
            best = cand
    return best


def accept(b, g):
    """validation gate: real time-corr, OR small gap + envelope match."""
    if abs(b['c']) >= CKEEP:
        return True
    return g <= 40 and b['nz'] >= 12 and b.get('bc', 0) >= 0.45


def chain(d, fr, seed_s, seed_e, wall, RW, RSD, BP):
    """bidirectional chaining from a seed body. Returns ordered bodies."""
    bodies = {}
    b0 = body_at(d, seed_s, RW, RSD, BP)
    if b0 is not None and b0['e'] > wall: b0 = None
    if b0 is None: return []
    bodies[b0['s']] = b0
    # forward
    cur_end = b0['e']
    while True:
        found = None
        for g in range(0, MAXGAP + 1):
            P = cur_end + g
            if P + 60 > wall: break
            b = body_at(d, P, RW, RSD, BP)
            if b is None or b['e'] > wall: continue
            if not accept(b, g): continue
            found = b; break
        if found is None: break
        if found['s'] in bodies: break
        bodies[found['s']] = found
        cur_end = found['e']
        if len(bodies) > 14: break
    # backward: cheap end-only parses to find bodies ending near cur_start
    def parse_end_only(P, w2):
        try:
            sp, e, m, sects = v2_parse(d, P, w2)
        except Exception:
            return None
        return e, sects, int((sp != 0).sum())
    cur_start = b0['s']
    steps = 0
    while steps < 12:
        steps += 1
        found = None
        lo = max(16, cur_start - 3600)
        for P in range(cur_start - 60, lo, -1):
            for w2 in (3, 5):
                r = parse_end_only(P, w2)
                if r is None: continue
                e, sects, nz = r
                if not (0 <= cur_start - e <= MAXGAP): continue
                if nz < NZMIN or not minbits_ok(sects, w2): continue
                b = body_at(d, P, RW, RSD, BP, widths=(w2,))
                if b is None or b['e'] != e: continue
                if accept(b, cur_start - e):
                    found = b
                    break
            if found: break
        if found is None: break
        if found['s'] in bodies: break
        bodies[found['s']] = found
        cur_start = found['s']
    # drop overlaps: keep max-|c| non-overlapping set in order
    ordered = [bodies[s] for s in sorted(bodies)]
    clean = []
    for b in ordered:
        if clean and b['s'] < clean[-1]['e']:
            if abs(b['c']) > abs(clean[-1]['c']):
                clean[-1] = b
            continue
        clean.append(b)
    return clean


if __name__ == '__main__':
    anch = {}
    for ln in open('fulltrack_kw.log'):
        mm = re.match(r'f(\d+): (ok|noS) (\{.*\})', ln)
        if not mm: continue
        fr = int(mm.group(1))
        if fr in anch: continue
        anch[fr] = json.loads(mm.group(3))
    frames = [int(x) for x in sys.argv[1:]]
    for fr in frames:
        a = anch.get(fr)
        if not a:
            print(f'f{fr}: noanchor', flush=True); continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            print(f'f{fr}: nodump', flush=True); continue
        wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
        RW, RSD = make_refwin(fr)
        if RW is None:
            print(f'f{fr}: noref', flush=True); continue
        BP = band_profiles(fr)
        bodies = chain(d, fr, a['sb'], a['e0'], wall, RW, RSD, BP)
        gaps = [bodies[i + 1]['s'] - bodies[i]['e'] for i in range(len(bodies) - 1)]
        print(f'f{fr}: BODIES ' + json.dumps({'fr': fr, 'n': len(bodies),
              'gaps': gaps, 'bodies': bodies, 'wall': wall}), flush=True)
