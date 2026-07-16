#!/usr/bin/env python3
"""R476: anchored full-enumeration chain search -> pad sequences.

The R448 method that cracked f33, scaled: per anchored spk frame,
memoize v2-parse ends over the bed region, build the gap-digraph
(0 <= pad <= MAXPAD), enumerate ALL chains through the anchor that
start in the post-LFE region [LO, HI]. m=0 empty bodies allowed
(14-bit [5b m][8b ref][1b snf]). Frames with a unique (or few)
complete pad sequence become ground truth.

Usage: r476_dagseq.py <fr> [fr ...]      (spk only)
Log:   fN: CHAINS k=<n_distinct_pad_seqs> [...]
"""
import sys, json
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T = A.T; SFB = A.SFB_2048
MAXPAD = 30
LO, HI = 40, 420          # allowed chain-start region (post-LFE)
MINBODY, MAXBODY = 4, 8   # bed bodies incl. empties

def parse_end(d, P, width):
    """v2 parse, end position only (no spectrum build). m=0 allowed."""
    bits = A.Bits(d, P)
    m = bits.u(5)
    if m > 56: raise ValueError
    if m == 0:
        bits.u(8)
        if bits.u(1): raise ValueError   # empty body: snf gate must be 0
        return bits.p, 0, 0
    esc = (1 << width) - 1; sects = []; k = 0
    while k < m:
        cb = bits.u(4); ln = 1; li = bits.u(width)
        while li == esc: ln += esc; li = bits.u(width)
        ln += li
        if k + ln > 127: raise ValueError
        sects.append((k, k + ln, cb)); k += ln
    mqi = [0] * m
    nz = 0
    import numpy as np
    for (s, e, cb) in sects:
        if cb == 0 or cb > 11 or e > m: continue
        q2, mq2 = A.parse_spectra(bits, [(s, e, cb)], SFB, m)
        for xx in range(s, e): mqi[xx] = max(mqi[xx], mq2[xx])
        nz += int((np.asarray(q2)[SFB[s]:SFB[e]] != 0).sum())
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

def frame_chains(d, sb, e0, wall):
    roof = min(wall - 60, e0 + 1500)
    ends = {}          # (s,w) -> (e, m, nz)
    for w in (3, 5):
        for s in range(LO, roof):
            try:
                e, m, nz = parse_end(d, s, w)
                if e <= roof + 200: ends[(s, w)] = (e, m, nz)
            except Exception:
                pass
    # nodes keyed by (s, e); merge widths (same span = same tiling)
    nodes = sorted({(s, e) for (s, w), (e, m, nz) in ends.items()})
    nodeset = set(nodes)
    anchor = (sb, e0)
    if anchor not in nodeset: return None, len(nodes)
    starts_at = {}
    for (s, e) in nodes: starts_at.setdefault(s, []).append((s, e))
    ends_at = {}
    for (s, e) in nodes: ends_at.setdefault(e, []).append((s, e))
    def succs(node):
        out = []
        for s2 in range(node[1], node[1] + MAXPAD + 1):
            out.extend(starts_at.get(s2, []))
        return out
    def preds(node):
        out = []
        for e2 in range(node[0] - MAXPAD, node[0] + 1):
            out.extend(ends_at.get(e2, []))
        return out
    # counting DP over path length (no enumeration)
    # nb[node][L] = #paths of L bodies ending at node, starting in [LO,HI]
    order = nodes  # sorted by s; edges go strictly forward in s (e > s)
    nb = {n: [0] * (MAXBODY + 1) for n in nodes}
    for n in order:
        if LO <= n[0] <= HI: nb[n][1] += 1
        for p in preds(n):
            pb = nb[p]
            for L in range(1, MAXBODY):
                if pb[L]: nb[n][L + 1] += pb[L]
    nf = {n: [0] * (MAXBODY + 1) for n in nodes}
    for n in reversed(order):
        term = not succs(n)
        if term: nf[n][1] = 1
        for s2 in succs(n):
            fb = nf[s2]
            for L in range(1, MAXBODY):
                if fb[L]: nf[n][L + 1] += fb[L]
    # total chains through anchor with body count in [MINBODY, MAXBODY]
    total = 0
    for Lb in range(1, MAXBODY + 1):
        if not nb[anchor][Lb]: continue
        for Lf in range(1, MAXBODY + 1 - Lb + 1):
            if Lb + Lf - 1 < MINBODY or Lb + Lf - 1 > MAXBODY: continue
            total += nb[anchor][Lb] * nf[anchor][Lf]
    if total == 0 or total > 400:
        return {'count': total}, len(nodes)
    # reconstruct via bounded DFS
    def back_all(node, depth):
        res = []
        if LO <= node[0] <= HI: res.append((node,))
        if depth < MAXBODY:
            for p in preds(node):
                for pp in back_all(p, depth + 1):
                    res.append(pp + (node,))
        return res
    def fwd_all(node, depth):
        res = []
        ss = succs(node)
        if not ss: res.append((node,))
        if depth < MAXBODY:
            for s2 in ss:
                for fp in fwd_all(s2, depth + 1):
                    res.append((node,) + fp)
        return res
    seqs = {}
    for bp in back_all(anchor, 1):
        for fp in fwd_all(anchor, 1):
            ch = bp[:-1] + fp
            if MINBODY <= len(ch) <= MAXBODY:
                pads = tuple(ch[i + 1][0] - ch[i][1] for i in range(len(ch) - 1))
                seqs.setdefault(pads, []).append(ch)
    return seqs, len(nodes)

if __name__ == '__main__':
    anch = {}
    for r in json.load(open('spk_gaps_big.json')):
        anch[r['fr']] = r
    for r in json.load(open('fulltrack_gaps_spk.json')):
        if r['fr'] not in anch and abs(r.get('c0') or 0) >= 0.5:
            anch[r['fr']] = r
    import re as _re
    for srcf in ('jointbest.out', 'jointfull.out'):
        try:
            for ln in open(srcf):
                mm = _re.match(r'f(\d+): JOINT sb=(\d+) e0=(\d+)', ln)
                if mm and int(mm.group(1)) not in anch:
                    anch[int(mm.group(1))] = dict(fr=int(mm.group(1)),
                        sb=int(mm.group(2)), e0=int(mm.group(3)))
        except FileNotFoundError:
            pass
    frames = [int(x) for x in sys.argv[1:]]
    for fr in frames:
        a = anch.get(fr)
        if not a:
            print(f'f{fr}: noanchor', flush=True); continue
        try:
            d = open(f'spk4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            print(f'f{fr}: nodump', flush=True); continue
        wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
        if a['sb'] > 2600:
            print(f'f{fr}: anchor-too-deep sb={a["sb"]}', flush=True); continue
        seqs, nn = frame_chains(d, a['sb'], a['e0'], wall)
        if seqs is None:
            print(f'f{fr}: anchor-not-node (nodes={nn})', flush=True); continue
        if 'count' in seqs and isinstance(seqs['count'], int):
            print(f'f{fr}: COUNT {seqs["count"]} nodes={nn}', flush=True); continue
        rows = []
        for pads, chs in sorted(seqs.items(), key=lambda kv: len(kv[0])):
            ch = chs[0]
            rows.append({'pads': list(pads), 'starts': [c[0] for c in ch],
                         'ends': [c[1] for c in ch]})
        print(f'f{fr}: CHAINS k={len(seqs)} nodes={nn} ' + json.dumps(rows[:6]), flush=True)
    print('DONE', flush=True)
