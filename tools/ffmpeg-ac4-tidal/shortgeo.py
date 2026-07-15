#!/usr/bin/env python3
"""R470: grouped/short-geometry sf_data parser + gap-absorption test.

Hypothesis: corpus body0s that carry a nonzero "gap" are actually
short-transform (grouped) sf_datas; our v2 long parse ends early and
the gap is the unread tail. Test: a spec-exact grouped parse from the
same start bit must end exactly at body1's start (e0_long + gap).

Variant A: [5b max_sfb][per-group sections]... grouping brute-forced.
Variant B: [5b max_sfb][W-1 grouping bits][per-group sections]...
"""
import sys, json, re
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T = A.T

SFB_SHORT = {
    1024: [0,4,8,12,16,20,24,28,32,36,40,48,56,64,72,80,88,96,108,120,132,144,
           160,176,196,216,240,264,292,320,352,384,416,448,480,512,544,576,608,
           640,672,704,736,768,800,832,864,896,928,1024],
    512: [0,4,8,12,16,20,24,28,32,36,40,44,48,52,56,60,68,76,84,92,100,112,124,
          136,148,164,184,208,236,268,300,332,364,396,428,460,512],
    256: [0,4,8,12,16,20,24,28,36,44,52,64,76,92,108,128,148,172,196,224,256],
}
NUM_SFB = {1024: 49, 512: 36, 256: 20, 2048: 63}
# clip tables to num_sfb entries (offset arrays above already sized to num_sfb+? keep as-is)
for tl in (1024, 512, 256):
    SFB_SHORT[tl] = SFB_SHORT[tl][:NUM_SFB[tl] + 1]

def parse_grouped(d, P, tl, wins_per_group, self_grouping=False):
    """Parse one grouped sf_data starting at bit P.
    tl: transform length (1024/512/256); wins_per_group: list of ints
    summing to 2048//tl (ignored if self_grouping: read W-1 bits).
    Returns (end_bit, msfb, total_nonzero_lines)."""
    bits = A.Bits(d, P)
    m = bits.u(5)
    if m < 1 or m > 31: raise ValueError('msfb')
    W = 2048 // tl
    if self_grouping:
        wpg = [1]
        for i in range(W - 1):
            if bits.u(1): wpg[-1] += 1     # grouping bit 1 = same group
            else: wpg.append(1)
        wins_per_group = wpg
    G = len(wins_per_group)
    nsfb = NUM_SFB[tl]
    offs = SFB_SHORT[tl]
    width = 3 if tl <= 1024 else 5
    esc = (1 << width) - 1
    # per-group sections
    sects = []
    for g in range(G):
        k = 0; gs = []
        while k < m:
            cb = bits.u(4)
            ln = 1; li = bits.u(width)
            while li == esc: ln += esc; li = bits.u(width)
            ln += li
            if k + ln > 127: raise ValueError('sect overrun')
            gs.append((k, k + ln, cb)); k += ln
        sects.append(gs)
    # per-group spectra (clip at num_sfb; overshoot beyond carries no lines)
    mqi = [[0] * m for _ in range(G)]
    nz = 0
    for g in range(G):
        nw = wins_per_group[g]
        goffs = [o * nw for o in offs]  # scaled group offsets
        for (s, e, cb) in sects[g]:
            if cb == 0 or cb > 11: continue
            if s >= nsfb: continue
            e2 = min(e, nsfb, m)
            if e2 <= s: continue
            q2, mq2 = A.parse_spectra(bits, [(s, e2, cb)], goffs, min(m, nsfb))
            for x in range(s, e2): mqi[g][x] = max(mqi[g][x], mq2[x])
            nz += sum(1 for i2 in range(goffs[s], goffs[e2]) if q2[i2] != 0)
    # scalefactors
    ref_sf = bits.u(8)
    sfb_cb = [[0] * m for _ in range(G)]
    for g in range(G):
        for (s, e, cb) in sects[g]:
            for x in range(s, min(e, m)): sfb_cb[g][x] = cb
    lens, cws = T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']
    sf = ref_sf; first = False
    for g in range(G):
        mx = min(m, nsfb)
        for sfb in range(mx):
            if sfb_cb[g][sfb] == 0 or mqi[g][sfb] == 0: continue
            if first:
                sf += A.huff(bits, lens, cws) - 60
                if not (0 <= sf <= 255): raise ValueError('sf range')
            else: first = True
    # snf
    if bits.u(1):
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for g in range(G):
            mx = min(m, nsfb)
            for sfb in range(mx):
                if sfb_cb[g][sfb] == 0 or mqi[g][sfb] == 0:
                    A.huff(bits, l2, c2)
    return bits.p, m, nz

def compositions(W):
    """All ways to split W windows into consecutive groups."""
    if W == 1: return [[1]]
    out = []
    for mask in range(1 << (W - 1)):
        wpg = [1]
        for i in range(W - 1):
            if mask & (1 << i): wpg[-1] += 1
            else: wpg.append(1)
        out.append(wpg)
    return out

def load_corpus():
    rows = []
    anch = {}
    for src in ('jointbest.out',):
        for ln in open(src):
            mm = re.match(r'f(\d+): JOINT sb=(\d+) e0=(\d+)', ln)
            if mm: anch[int(mm.group(1))] = (int(mm.group(2)), int(mm.group(3)))
    for src in ('kwjoint4.out', 'kwmel.out', 'kwfull.out'):
        for ln in open(src):
            mm = re.match(r'f(\d+): JOINT sb=(\d+) e0=(\d+)', ln)
            if mm and ('kw', int(mm.group(1))) not in anch:
                anch[('kw', int(mm.group(1)))] = (int(mm.group(2)), int(mm.group(3)))
    for r in json.load(open('gap_corpus.json')):
        key = r['fr'] if r['t'] == 'spk' else ('kw', r['fr'])
        if key in anch:
            sb, e0 = anch[key]
            rows.append({'t': r['t'], 'fr': r['fr'], 'sb': sb, 'e0': e0,
                         'gap': r['gap'], 'm0': r['m0']})
    for r in json.load(open('iframe_gaps_spk.json')):
        if r.get('gap') is None: continue
        cc = max(abs(r.get('c1', 0)), abs(r.get('c1m', 0)))
        if cc < 0.45: continue
        rows.append({'t': 'spk', 'fr': r['fr'], 'sb': r['sb'], 'e0': r['e0'],
                     'gap': r['gap'], 'm0': r['m0'], 'ifr': True})
    return rows

if __name__ == '__main__':
    rows = load_corpus()
    print(f'{len(rows)} corpus samples with anchors')
    GEOS = [(tl, wpg) for tl in (1024, 512, 256) for wpg in compositions(2048 // tl)]
    print(f'{len(GEOS)} geometries (variant A)')
    hitsA = hitsB = 0
    for r in rows:
        dump = ('spk4' if r['t'] == 'spk' else 'kw4') + f"/sub{r['fr']:04d}.bin"
        d = open(dump, 'rb').read()
        target = r['e0'] + r['gap']
        bestA = None
        for tl, wpg in GEOS:
            try:
                end, m, nz = parse_grouped(d, r['sb'], tl, wpg)
            except Exception:
                continue
            diff = end - target
            if bestA is None or abs(diff) < abs(bestA[0]):
                bestA = (diff, tl, len(wpg), m, nz)
        bestB = None
        for tl in (1024, 512, 256):
            try:
                end, m, nz = parse_grouped(d, r['sb'], tl, None, self_grouping=True)
            except Exception:
                continue
            diff = end - target
            if bestB is None or abs(diff) < abs(bestB[0]):
                bestB = (diff, tl, m, nz)
        tag = f"{r['t']}{r['fr']}{'(I)' if r.get('ifr') else ''} gap={r['gap']}"
        pa = f"A: d={bestA[0]:+d} tl={bestA[1]} G={bestA[2]} m={bestA[3]} nz={bestA[4]}" if bestA else 'A: none'
        pb = f"B: d={bestB[0]:+d} tl={bestB[1]} m={bestB[2]} nz={bestB[3]}" if bestB else 'B: none'
        if bestA and bestA[0] == 0: hitsA += 1; pa = '**' + pa
        if bestB and bestB[0] == 0: hitsB += 1; pb = '**' + pb
        print(f'{tag:22s} {pa:44s} {pb}')
    print(f'EXACT absorption: variant A {hitsA}/{len(rows)}, variant B {hitsB}/{len(rows)}')
