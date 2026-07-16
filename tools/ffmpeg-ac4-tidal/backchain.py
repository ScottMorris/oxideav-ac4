#!/usr/bin/env python3
"""R474: within-frame pad-sequence harvest by backward chaining.

From each anchored body0 (sb), find predecessor bodies: candidate
starts whose v2-parse END lands in [sb - MAXPAD, sb], validated by
correlation vs ANY reference channel (free lag). Chain backwards up
to 5 steps. Output per-frame pad sequences (war f33 ground truth:
pads 9,11,18,23 walking back).
Usage: backchain.py <track> [fr fr ...]
"""
import sys, json, wave
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
MAXPAD = 40
CMIN = 0.38

TRACK = sys.argv[1]
REFWAV = {'kw': 'kw-ref51.wav', 'spk': 'spk-ref51.wav'}[TRACK]
w = wave.open(REFWAV, 'rb')
REF = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)

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

def best_ref_corr(fr, sp):
    pcm = (BASIS @ sp) * WIN
    if pcm.std() < 1e-9: return 0.0, -1
    best = (0.0, -1)
    for ch in range(6):
        for lag in range(992, 1073, 16):
            st = fr * N + lag
            if st + 2 * N > len(REF): continue
            r = REF[st:st + 2 * N, ch]
            if r.std() < 10: continue
            c = float(np.corrcoef(pcm, r)[0, 1])
            if abs(c) > abs(best[0]): best = (c, ch)
    return best

def back_dag(d, fr, anchor_sb, max_depth=5, beam=48):
    """Enumerate backward chains ending at anchor_sb via beam DFS.
    Returns best chain: list of (s, e, w, m, c, ch) back-to-front."""
    from functools import lru_cache
    pmemo = {}
    def parse_at(s, w2):
        k = (s, w2)
        if k in pmemo: return pmemo[k]
        try:
            sp, e, m = v2_parse(d, s, w2)
            nz = int((sp != 0).sum())
            r = (e, m, nz, sp)
        except Exception:
            r = None
        pmemo[k] = r
        return r
    cmemo = {}
    def corr_at(s, w2):
        k = (s, w2)
        if k in cmemo: return cmemo[k]
        r = parse_at(s, w2)
        c, ch = best_ref_corr(fr, r[3]) if r else (0.0, -1)
        cmemo[k] = (c, ch)
        return cmemo[k]
    def preds(target):
        """bodies ending in [target-MAXPAD, target]."""
        out = []
        for w2 in (3, 5):
            for s in range(max(16, target - 4200), target - 13):
                r = parse_at(s, w2)
                if r is None: continue
                e, m, nz, sp = r
                if not (target - MAXPAD <= e <= target): continue
                out.append((s, e, w2, m, nz))
        return out
    # beam of partial chains: (score, nval, chain list back-to-front)
    partial = [(0.0, 0, [(anchor_sb, None, None, None, None, 'anchor')])]
    finals = []
    for depth in range(max_depth):
        nxt = []
        for score, nval, chain in partial:
            target = chain[-1][0]
            for (s, e, w2, m, nz) in preds(target):
                c, ch = corr_at(s, w2) if nz >= 4 else (0.0, -1)
                v = abs(c) >= CMIN
                sc = score + (abs(c) if v else 0.0) - 0.002 * (target - e)
                nc = chain + [(s, e, w2, m, round(c, 3), ch)]
                nxt.append((sc, nval + v, nc))
                # full chain: 4 predecessors AND lands in the header region
                if len(nc) == 5 and 100 <= s <= 470:
                    finals.append((sc, nval + v, nc))
        if not nxt: break
        nxt.sort(key=lambda x: -x[0])
        partial = nxt[:beam]
    if not finals: return None
    finals.sort(key=lambda x: (-x[1], -x[0]))   # most validated, then score
    return finals[0]

if __name__ == '__main__':
    rows = {r['fr']: r for r in json.load(open('spk_gaps_big.json'))}
    import re
    for srcf in ('jointbest.out', 'jointfull.out'):
        for ln in open(srcf):
            mm = re.match(r'f(\d+): JOINT sb=(\d+) e0=(\d+) gap=(\d+)', ln)
            if mm and int(mm.group(1)) not in rows:
                rows[int(mm.group(1))] = {'fr': int(mm.group(1)), 'sb': int(mm.group(2)),
                                          'e0': int(mm.group(3)), 'gap': int(mm.group(4))}
    frames = [int(x) for x in sys.argv[2:]] if len(sys.argv) > 2 else sorted(rows)
    out = []
    for fr in frames:
        r = rows.get(fr)
        if not r: continue
        d = open(f'{TRACK}4/sub{fr:04d}.bin', 'rb').read()
        bf = back_dag(d, fr, r['sb'])
        if bf is None:
            print(f'f{fr}: no chain', flush=True); continue
        score, nval, chain = bf
        pads = []
        for i in range(len(chain) - 1):
            tgt = chain[i][0]
            pads.append(tgt - chain[i + 1][1])
        rec = {'fr': fr, 'pads_backward': pads, 'nval': nval,
               'chain': [(c[0], c[1]) for c in chain[1:]],
               'cs': [c[4] for c in chain[1:]], 'chs': [c[5] for c in chain[1:]]}
        out.append(rec)
        print(f"f{fr}: nval={nval} pads(back)={pads} starts={[c[0] for c in chain[1:]]} cs={[c[4] for c in chain[1:]]}", flush=True)
        json.dump(out, open(f'backchain_{TRACK}.json', 'w'))
    print('DONE', len(out))
