#!/usr/bin/env python3
"""R471: reference-free body-chain walker.

Hypothesis: v2-validity chains of depth>=4 (bodies separated by
0..MAXGAP pad bits) are nearly unique per frame, so positions can be
derived without any reference audio. Validate: for frames with known
(sb, e0) anchors, does the best validity-chain pass through sb?

Method: memoized parse over all bit positions x widths, then DFS
chain enumeration with pruning; score chains by total decoded lines;
compare best chain's node set against the known anchor.
"""
import sys, json, re
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import numpy as np
T = A.T; SFB = A.SFB_2048
MAXGAP = 60
MINBODY_NZ = 6

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
    mqi = [0] * m; nz = 0
    for (s, e, cb) in sects:
        if cb == 0 or cb > 11 or e > m: continue
        q2, mq2 = A.parse_spectra(bits, [(s, e, cb)], SFB, m)
        for xx in range(s, e): mqi[xx] = max(mqi[xx], mq2[xx])
        nz += int((np.asarray(q2[SFB[s]:SFB[e]]) != 0).sum())
    ref_sf = bits.u(8)
    sfb_cb = [0] * m
    for (s, e, cb) in sects:
        for xx in range(s, min(e, m)): sfb_cb[xx] = cb
    sf = ref_sf; first = False
    lens, cws = T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']
    for sfb in range(m):
        if sfb_cb[sfb] == 0 or mqi[sfb] == 0: continue
        if first:
            sf += A.huff(bits, lens, cws) - 60
            if not (0 <= sf <= 255): raise ValueError
        else: first = True
    if bits.u(1):
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for sfb in range(m):
            if sfb_cb[sfb] == 0 or mqi[sfb] == 0: A.huff(bits, l2, c2)
    return bits.p, m, nz

def frame_chains(d, lo, hi, min_depth=4):
    """Memoized parse at every (pos,w); DFS chains; return list of
    (score, [(pos,w,end,m,nz), ...]) sorted best-first."""
    memo = {}
    def parse(p, w):
        key = (p, w)
        if key in memo: return memo[key]
        try:
            end, m, nz = v2_parse(d, p, w)
            r = (end, m, nz) if nz >= MINBODY_NZ and end <= hi else None
        except Exception:
            r = None
        memo[key] = r
        return r
    from functools import lru_cache
    sys.setrecursionlimit(100000)
    best_from = {}
    def chain_from(p, depth=0):
        """Longest/best chain starting at position p (choose best w)."""
        if p in best_from: return best_from[p]
        best = (0, [])   # (score, nodes)
        if depth < 40:
            for w in (3, 5):
                r = parse(p, w)
                if r is None: continue
                end, m, nz = r
                # follow: next body within MAXGAP
                sub_best = (0, [])
                for g2 in range(0, MAXGAP + 1):
                    np_ = end + g2
                    if np_ > hi - 20: break
                    s2 = chain_from(np_, depth + 1)
                    if s2[0] > sub_best[0]:
                        sub_best = s2
                cand = (nz + sub_best[0], [(p, w, end, m, nz)] + sub_best[1])
                if cand[0] > best[0]: best = cand
        best_from[p] = best
        return best
    # chains can start anywhere in [lo, lo+2200]
    starts = []
    for p in range(lo, min(lo + 2200, hi - 100)):
        s = chain_from(p)
        if len(s[1]) >= min_depth:
            starts.append((s[0], p, s[1]))
    starts.sort(reverse=True)
    return starts

if __name__ == '__main__':
    rows = json.load(open('spk_gaps_big.json'))
    subset = rows[:: max(1, len(rows) // int(sys.argv[1] if len(sys.argv) > 1 else 30))]
    hit = miss = 0
    for r in subset:
        d = open(f"spk4/sub{r['fr']:04d}.bin", 'rb').read()
        wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
        ch = frame_chains(d, 16, wall)
        if not ch:
            print(f"f{r['fr']}: no chains (anchor sb={r['sb']})", flush=True)
            miss += 1; continue
        score, start, nodes = ch[0]
        poss = [n[0] for n in nodes]
        ok = r['sb'] in poss
        nxt = r['e0'] + r['gap']
        ok1 = nxt in poss
        hit += ok; miss += (not ok)
        print(f"f{r['fr']}: best chain depth={len(nodes)} score={score} "
              f"start={start} anchors: body0@{r['sb']} {'HIT' if ok else 'miss'}, "
              f"body1@{nxt} {'HIT' if ok1 else 'miss'} | chain={[(p, e) for p, w, e, m, nz in nodes][:9]}", flush=True)
    print(f'BODY0 HIT RATE: {hit}/{hit + miss}')
