#!/usr/bin/env python3
"""R499: header-window tiling test (task #12 decisive experiment).

Hypothesis H-B: after the (silent) LFE, v2 continues as a flat chain of
self-contained bodies [k-bit hdr][5b msfb][sections][spectra][ref_sf]
[scf][snf], with small inter-body headers. Prediction: chaining v2
bodies forward from LFE_end reaches the first VALIDATED harvest body
at its exact position.

Control: same chain started from LFE_end + jitter j (j=13,29) must land
less often (grammar permissiveness null).

Uses r489's v2_parse via import; allows cb 12-15 no-payload sections
(already tolerated: cb>11 skips spectra) and overshoot (k+len<=127).
"""
import sys, json, re
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf

T = A.T


def v2_parse_end(d, P, width):
    bits = A.Bits(d, P)
    m = bits.u(5)
    if m > 56 or m < 1:
        raise ValueError
    esc = (1 << width) - 1
    sects = []
    k = 0
    while k < m:
        cb = bits.u(4)
        ln = 1
        li = bits.u(width)
        while li == esc:
            ln += esc
            li = bits.u(width)
        ln += li
        if k + ln > 127:
            raise ValueError
        sects.append((k, k + ln, cb))
        k += ln
    SFB = A.SFB_2048
    mqi = [0] * m
    nz = 0
    for (s, e, cb) in sects:
        if cb == 0 or cb > 11 or e > m:
            continue
        q2, mq2 = A.parse_spectra(bits, [(s, e, cb)], SFB, m)
        for xx in range(s, e):
            mqi[xx] = max(mqi[xx], mq2[xx])
        nz += int(np.count_nonzero(q2[SFB[s]:SFB[e]]))
    ref_sf = bits.u(8)
    sfb_cb = [0] * m
    for (s, e, cb) in sects:
        for xx in range(s, min(e, m)):
            sfb_cb[xx] = cb
    lens, cws = T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']
    sf = ref_sf
    first = False
    for sfb in range(m):
        if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
            continue
        if first:
            sf += A.huff(bits, lens, cws) - 60
            if not (0 <= sf <= 255):
                raise ValueError
        else:
            first = True
    if bits.u(1):
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for sfb in range(m):
            if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
                A.huff(bits, l2, c2)
    return bits.p, m, nz


def chain_from(d, start, target, wall, maxgap=8, maxbodies=12):
    """chain v2 bodies from `start`; return True if any body start lands
    exactly on `target` (searching both widths, greedy DFS bounded)."""
    seen = set()
    stack = [(start, 0)]
    while stack:
        P, depth = stack.pop()
        if P == target:
            return True
        if P > target or depth >= maxbodies or P in seen:
            continue
        seen.add(P)
        for g in range(maxgap + 1):
            for w in (3, 5):
                try:
                    e, m, nz = v2_parse_end(d, P + g, w)
                except Exception:
                    continue
                if e <= wall:
                    if P + g == target:
                        return True
                    stack.append((e, depth + 1))
    return False


def lfe_end(d, fr):
    P = 33 if fr % 24 == 0 else 18
    b = A.Bits(d, P)
    cfg = ac4asf.parse_sf_info_lfe(b)
    ac4asf.parse_sf_data(b, cfg)
    return b.p


if __name__ == '__main__':
    firsts = {}
    for ln in open('backext_kw.log'):
        mm = re.match(r'f(\d+): PRE (\{.*\})', ln)
        if mm:
            r = json.loads(mm.group(2))
            firsts[r['fr']] = r['first']
    walls = {}
    for ln in open('multibody_kw.log'):
        mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
        if mm:
            r = json.loads(mm.group(2))
            walls[r['fr']] = r['wall']
    hits = {0: 0, 13: 0, 29: 0}
    n = 0
    for fr, first in sorted(firsts.items()):
        if fr % 24 == 0 or first > 400:
            continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        try:
            e0 = lfe_end(d, fr)
        except Exception:
            continue
        wall = walls.get(fr, len(d) * 8)
        n += 1
        for j in hits:
            if chain_from(d, e0 + j, first, wall):
                hits[j] += 1
        if n % 100 == 0:
            print(f'..{n}: real {hits[0]}, null13 {hits[13]}, '
                  f'null29 {hits[29]}', flush=True)
    print(f'\nframes tested {n} (first body <400 bits, P-frames)')
    print(f'chain lands EXACTLY on first validated body:')
    print(f'  from LFE_end+0 : {hits[0]}  ({hits[0]/max(n,1):.2f})')
    print(f'  null +13 bits  : {hits[13]}  ({hits[13]/max(n,1):.2f})')
    print(f'  null +29 bits  : {hits[29]}  ({hits[29]/max(n,1):.2f})')
