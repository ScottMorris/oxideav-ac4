#!/usr/bin/env python3
"""R469: twin-peak gap harvest on IFRAME rows (fr%24==0) of both tracks.
v2 grammar + KBD windows. Tests the P-frame-only gap hypothesis:
if iframes are uniformly gap=0, the gap field is inter-frame
prediction state absent on iframes."""
import sys, json, wave
import numpy as np
from multiprocessing import Pool
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

TRACK = sys.argv[1]  # 'kw' or 'spk'
REFWAV = {'kw': 'kw-ref51.wav', 'spk': 'spk-ref51.wav'}[TRACK]
DUMPDIR = {'kw': 'kw4', 'spk': 'spk4'}[TRACK]
w = wave.open(REFWAV, 'rb')
_R = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
REF = np.zeros((_R.shape[0], 2))
REF[:, 0] = _R[:, 0] + _R[:, 1]  # M oracle
REF[:, 1] = _R[:, 0] - _R[:, 1]  # S oracle
NFR = len(REF) // N

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
    sp = np.zeros(N)
    for sfb in range(m):
        if sf_per[sfb] is None: continue
        g2 = 2.0 ** (0.25 * (sf_per[sfb] - ref_sf))
        for kk in range(SFB[sfb], SFB[sfb + 1]):
            sp[kk] = np.sign(q[kk]) * abs(q[kk]) ** (4 / 3.) * g2
    return sp, bits.p, m

def refbands(fr, ch, msfb=48):
    X = ((REF[fr * N + 1024: fr * N + 1024 + 2 * N, ch] * WIN)[:, None] * BASIS).sum(0)
    e = np.array([np.sum(X[SFB[i]:SFB[i + 1]] ** 2) for i in range(msfb)])
    return np.log(e + 1e-6)

def pcm_corr(fr, sp, ch, fixed=False):
    pcm = (BASIS @ sp) * WIN
    if pcm.std() < 1e-9: return 0.0, 0
    best = (0.0, 0)
    rng = range(1024, 1105, 16) if fixed else range(992, 1073, 8)
    for lag in rng:
        st = fr * N + lag
        if st < 0 or st + 2 * N > len(REF): continue
        r = REF[st:st + 2 * N, ch]
        if r.std() < 1e-9: continue
        c = float(np.corrcoef(pcm, r)[0, 1])
        if abs(c) > abs(best[0]): best = (c, lag)
    return best

def bitstr(d, a, b):
    s = ''.join(f'{x:08b}' for x in d)
    return s[a:b]

def work(fr):
    try:
        d = open(f'{DUMPDIR}/sub{fr:04d}.bin', 'rb').read()
    except FileNotFoundError:
        return fr, None, 'nodump'
    wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
    if fr * N + 1024 + 2 * N > len(REF): return fr, None, 'noref'
    refM = REF[fr * N + 1024: fr * N + 1024 + 2 * N, 0]
    if refM.std() < 10: return fr, None, 'silent'
    a0 = refbands(fr, 0)
    # stage 1: find body0 (M) — full position scan, both widths
    cands = []
    for w0 in (3, 5):
        for sb in range(16, max(17, wall - 180)):
            try:
                s0, e0, m0 = v2_parse(d, sb, w0)
            except Exception:
                continue
            if (s0 != 0).sum() < 20: continue
            e = np.array([np.sum(np.abs(s0)[SFB[i]:SFB[i + 1]] ** 2) for i in range(min(m0, 48))])
            cb_ = float(np.corrcoef(a0[:len(e)], np.log(e + 1e-9))[0, 1])
            if cb_ < 0.30: continue
            c0, l0 = pcm_corr(fr, s0, 0, fixed=True)
            if abs(c0) < 0.32: continue
            cands.append((abs(c0), sb, e0, w0, m0, c0))
    if not cands: return fr, None, 'nobody0'
    cands.sort(reverse=True)
    # dedupe: many start bits converge on the same body end — keep best per e0
    seen_e0 = set(); dedup = []
    for c in cands:
        if c[2] in seen_e0: continue
        seen_e0.add(c[2]); dedup.append(c)
    cands = dedup
    best = None
    for _, sb, e0, w0, m0, c0 in cands[:24]:
        # stage 2: twin-peak S scan after body0
        for g in range(0, 35):
            for w1 in (5, 3):
                try:
                    s1, e1, m1 = v2_parse(d, e0 + g, w1)
                except Exception:
                    continue
                if (s1 != 0).sum() < 6: continue
                c1, l1 = pcm_corr(fr, s1, 1)
                c1m, _ = pcm_corr(fr, s1, 0)  # twin-peak: S body echoes M on near-mono content
                cc = max(abs(c1), abs(c1m))
                if cc < 0.40: continue
                score = abs(c0) + cc
                if best is None or score > best[0]:
                    best = (score, sb, e0, g, w1, e1, m0, m1, w0, c0, c1, c1m)
    if best is None:
        # report body0-only (no S found) with top body0
        _, sb, e0, w0, m0, c0 = cands[0]
        return fr, {'fr': fr, 'sb': sb, 'e0': e0, 'w0': w0, 'm0': m0, 'c0': round(c0, 3), 'gap': None}, 'noS'
    score, sb, e0, g, w1, e1, m0, m1, w0, c0, c1, c1m = best
    return fr, {'fr': fr, 'sb': sb, 'e0': e0, 'w0': w0, 'm0': m0, 'c0': round(c0, 3),
                'gap': g, 'w1': w1, 'm1': m1, 'c1': round(c1, 3), 'c1m': round(c1m, 3),
                'bits': bitstr(d, e0, e0 + g)}, 'ok'

if __name__ == '__main__':
    frames = [f for f in range(0, NFR, 24)]
    if len(sys.argv) > 2: frames = [int(x) for x in sys.argv[2:]]
    out = []
    with Pool(3) as p:
        for fr, row, status in p.imap(work, frames):
            if row:
                print(f'f{fr}: {status} ' + json.dumps(row), flush=True)
                out.append(row)
            else:
                print(f'f{fr}: {status}', flush=True)
    json.dump(out, open(f'iframe_gaps_{TRACK}.json', 'w'))
    print(f'DONE {len(out)} rows -> iframe_gaps_{TRACK}.json', flush=True)
