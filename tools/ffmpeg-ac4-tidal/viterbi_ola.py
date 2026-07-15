#!/usr/bin/env python3
"""R472: aggregated OLA self-consistency chain scorer (Viterbi pilot).

Reference-free: per frame, candidates = all v2-valid parse positions
(nz >= 20, deduped by end). Transition score between consecutive
frames = |corr(imdct_f[N:], imdct_f1[:N])| (shared overlap audio).
Validate: does the Viterbi-best path pass through known anchors?
"""
import sys, json
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

def candidates(d, wall, cap=2600):
    out = []
    seen = set()
    for w in (3, 5):
        for sb in range(16, wall - 180):
            try:
                sp, e0, m = v2_parse(d, sb, w)
            except Exception:
                continue
            nz = int((sp != 0).sum())
            if nz < 15: continue
            if (e0, w) in seen: continue
            seen.add((e0, w))
            out.append((nz, sb, w, e0, m, sp))
    out.sort(key=lambda x: -x[0])
    return out[:cap]

def run_viterbi(track, frames, anchors):
    dumps = {f: open(f'{track}4/sub{f:04d}.bin', 'rb').read() for f in frames}
    cands = {}
    for f in frames:
        d = dumps[f]
        wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
        cands[f] = candidates(d, wall)
        print(f'f{f}: {len(cands[f])} candidates', flush=True)
    # imdct halves, normalized
    heads = {}; tails = {}
    B32 = BASIS.astype(np.float32)
    for f in frames:
        SP = np.array([sp / (np.abs(sp).max() + 1e-30) for (nz, sb, w, e0, m, sp) in cands[f]], dtype=np.float32)
        Y = (SP @ B32.T) * WIN.astype(np.float32)[None, :]
        H = Y[:, :N]; Tl = Y[:, N:]
        H = H / (np.linalg.norm(H, axis=1, keepdims=True) + 1e-12)
        Tl = Tl / (np.linalg.norm(Tl, axis=1, keepdims=True) + 1e-12)
        heads[f] = H; tails[f] = Tl
    # viterbi (abs corr transitions)
    f0 = frames[0]
    score = np.zeros(len(cands[f0]))
    back = {}
    for i in range(1, len(frames)):
        fp, fc = frames[i - 1], frames[i]
        M = np.abs(tails[fp] @ heads[fc].T)   # |cos sim| ~ |corr| (zero-mean-ish audio)
        tot = score[:, None] + M
        back[fc] = np.argmax(tot, axis=0)
        score = np.max(tot, axis=0)
    # backtrack
    path = [int(np.argmax(score))]
    for i in range(len(frames) - 1, 0, -1):
        path.append(int(back[frames[i]][path[-1]]))
    path.reverse()
    hits0 = hits_any = 0
    for f, ci in zip(frames, path):
        nz, sb, w, e0, m, sp = cands[f][ci]
        a = anchors.get(f)
        tag = ''
        if a:
            body0, body1 = a
            if sb == body0: hits0 += 1; tag = 'BODY0'
            elif sb == body1: tag = 'BODY1'
            if sb in (body0, body1): hits_any += 1
            # rank of true body0 among candidates
            rank = next((j for j, c in enumerate(cands[f]) if c[1] == body0), None)
            tag += f' (true b0 rank={rank})'
        print(f'f{f}: picked sb={sb} w={w} nz={nz} {tag}', flush=True)
    print(f'RUN {frames[0]}..{frames[-1]}: body0 hits {hits0}/{len(frames)}, any-body hits {hits_any}/{len(frames)}')
    return hits0, hits_any

if __name__ == '__main__':
    rows = {r['fr']: r for r in json.load(open('spk_gaps_big.json'))}
    runs = [list(range(1251, 1262)), list(range(936, 944))]
    if len(sys.argv) > 1:
        s, e = int(sys.argv[1]), int(sys.argv[2]); runs = [list(range(s, e))]
    for run in runs:
        anchors = {f: (rows[f]['sb'], rows[f]['e0'] + rows[f]['gap']) for f in run if f in rows}
        run_viterbi('spk', run, anchors)
