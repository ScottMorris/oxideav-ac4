#!/usr/bin/env python3
"""R473: parse-shape feature extractor for the true-body classifier.

v2 parse instrumented to emit structural statistics. Usage modes:
  python3 bodyfeat.py extract <track> <n_frames>   # scan anchored frames,
      dump features for every valid candidate + truth labels
"""
import sys, json
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T = A.T; SFB = A.SFB_2048

def v2_features(d, P, width):
    """Parse + feature vector. Raises on invalid."""
    bits = A.Bits(d, P)
    m = bits.u(5)
    if m > 56 or m < 1: raise ValueError
    esc = (1 << width) - 1; sects = []; k = 0
    n_esc = 0
    while k < m:
        cb = bits.u(4); ln = 1; li = bits.u(width)
        while li == esc:
            ln += esc; li = bits.u(width); n_esc += 1
        ln += li
        if k + ln > 127: raise ValueError
        sects.append((k, k + ln, cb)); k += ln
    sect_bits = bits.p - P - 5
    mqi = [0] * m; q = np.zeros(SFB[m])
    spec_start = bits.p
    for (s, e, cb) in sects:
        if cb == 0 or cb > 11 or e > m: continue
        q2, mq2 = A.parse_spectra(bits, [(s, e, cb)], SFB, m)
        for xx in range(s, e): mqi[xx] = max(mqi[xx], mq2[xx])
        for i2 in range(SFB[s], SFB[e]): q[i2] = q2[i2]
    spec_bits = bits.p - spec_start
    ref_sf = bits.u(8)
    sfb_cb = [0] * m
    for (s, e, cb) in sects:
        for xx in range(s, min(e, m)): sfb_cb[xx] = cb
    sf = ref_sf; first = False; sf_per = [None] * m
    lens, cws = T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']
    deltas = []
    scf_start = bits.p
    for sfb in range(m):
        if sfb_cb[sfb] == 0 or mqi[sfb] == 0: continue
        if first:
            dd = A.huff(bits, lens, cws) - 60
            sf += dd; deltas.append(dd)
            if not (0 <= sf <= 255): raise ValueError
        else: first = True
        sf_per[sfb] = sf
    scf_bits = bits.p - scf_start
    snf_gate = bits.u(1)
    n_snf = 0
    if snf_gate:
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for sfb in range(m):
            if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
                A.huff(bits, l2, c2); n_snf += 1
    end = bits.p
    body_bits = end - P
    nz = int((q != 0).sum())
    # band energies of |q|^(4/3) (unit gains) for smoothness/slope stats
    aq = np.abs(q) ** (4 / 3.)
    be = np.array([np.sum(aq[SFB[i]:SFB[i + 1]] ** 2) for i in range(m)])
    lbe = np.log(be + 1e-9)
    active = be > 1e-9
    n_active = int(active.sum())
    smooth = float(np.mean(np.abs(np.diff(lbe[active])))) if n_active > 2 else 20.0
    slope = float(lbe[active][-1] - lbe[active][0]) if n_active > 2 else 0.0
    ncb0 = sum(1 for (s, e, cb) in sects if cb == 0)
    ncbx = sum(1 for (s, e, cb) in sects if cb >= 12)
    novr = sum(1 for (s, e, cb) in sects if e > m)
    slens = [e - s for (s, e, cb) in sects]
    dmean = float(np.mean(np.abs(deltas))) if deltas else 0.0
    dmax = float(np.max(np.abs(deltas))) if deltas else 0.0
    feats = [
        m, width == 3, len(sects), float(np.mean(slens)), max(slens),
        ncb0 / len(sects), ncbx / len(sects), novr, n_esc,
        nz, nz / max(SFB[m], 1), body_bits, body_bits / max(nz, 1),
        sect_bits / max(m, 1), spec_bits / max(nz, 1) if nz else 0.0,
        len(deltas), dmean, dmax, scf_bits / max(len(deltas), 1),
        snf_gate, n_snf, ref_sf,
        n_active / m, smooth, slope,
    ]
    return np.array(feats, dtype=float), end

FEATNAMES = ['m', 'w3', 'nsect', 'slen_mean', 'slen_max', 'fcb0', 'fcbx',
             'novr', 'nesc', 'nz', 'nzdens', 'bits', 'bits_per_nz',
             'sectbits_per_m', 'specbits_per_nz', 'nscf', 'dmean', 'dmax',
             'scfbits_per', 'snfgate', 'nsnf', 'refsf', 'factive',
             'smooth', 'slope']

if __name__ == '__main__':
    mode = sys.argv[1]
    if mode == 'extract':
        track = sys.argv[2]; nfr = int(sys.argv[3])
        src = 'spk_gaps_big.json' if track == 'spk' else None
        rows = json.load(open(src))
        # spread over corpus
        rows = rows[:: max(1, len(rows) // nfr)][:nfr]
        X = []; y = []; meta = []
        for r in rows:
            d = open(f"{track}4/sub{r['fr']:04d}.bin", 'rb').read()
            wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
            truth = {r['sb'], r['e0'] + r['gap']}
            for w in (3, 5):
                for sb in range(16, wall - 180):
                    try:
                        f, end = v2_features(d, sb, w)
                    except Exception:
                        continue
                    if f[9] < 6: continue   # nz >= 6
                    lab = 1 if sb in truth else 0
                    # keep all positives; subsample negatives 1-in-3
                    if lab == 0 and (sb % 3): continue
                    X.append(f); y.append(lab); meta.append((r['fr'], sb, w))
            print(f"f{r['fr']}: cum {len(X)} rows ({sum(y)} pos)", flush=True)
            np.savez('bodyfeat_' + track + '.npz', X=np.array(X), y=np.array(y),
                     meta=np.array(meta))
        print('SAVED', len(X), 'rows,', sum(y), 'positives')
