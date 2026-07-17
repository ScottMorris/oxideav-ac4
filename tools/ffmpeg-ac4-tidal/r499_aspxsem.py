#!/usr/bin/env python3
"""R499b: A-SPX semantic test with the EXACT R498 parser.

Per frame: window-search the [2ch][2ch][1ch][2ch] tail near the last
inventory body end. P-frames: sweep one shared xover 0..5 (standalone
fit; TIME-coded envelopes are skipped so only absolute FREQ-coded
values enter). Level metric per frame = mean signal-envelope value
(3dB units, LEVEL-coded channels only, balance ch skipped).

Validation: correlation of the per-frame level series vs the ref's
15-21kHz log-energy series, against shift nulls (+/-149 frames).
Log: fN: SEM {...} to aspxsem_kw.log
"""
import sys, json, re, os
import numpy as np
import ac4aspx2 as X
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A

inv = {}
for ln in open('multibody_kw.log'):
    mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
    if mm:
        r = json.loads(mm.group(2)); inv[r['fr']] = r


def levels_from_blocks(blocks):
    vals = []
    for b in blocks:
        fms = b['fm']
        for chi, envs in enumerate(b['sig']):
            if b['balance'] and chi == 1:
                continue
            fm = fms[chi if chi < len(fms) else 0]
            for ei, arr in enumerate(envs):
                # only absolute FREQ-coded env values (delta dir handled
                # inside parser; TIME-coded chains depend on prev frame)
                vals.extend(arr)
    return vals


def fit_frame(d, fr, tail_s, wall):
    iframe = 1 if fr % 24 == 0 else 0
    best = None
    xovers = [None] if iframe else list(range(6))
    for xo in xovers:
        for P in range(max(20, tail_s - 8), min(tail_s + 64, wall - 40)):
            states = [X.ChState() for _ in range(4)]
            if xo is not None:
                for st in states:
                    st.xover = xo
            try:
                blocks, end = X.parse_tail(d, P, iframe, states)
            except Exception:
                continue
            if end > wall:
                continue
            if best is None or end > best[1]:
                best = (P, end, blocks, xo)
    return best


if __name__ == '__main__':
    done = set()
    if os.path.exists('aspxsem_kw.log'):
        for ln in open('aspxsem_kw.log'):
            mm = re.match(r'f(\d+):', ln)
            if mm:
                done.add(int(mm.group(1)))
    out = open('aspxsem_kw.log', 'a')
    for fr in sorted(inv):
        if fr in done:
            continue
        r = inv[fr]
        if not r['bodies']:
            out.write(f'f{fr}: none\n'); out.flush(); continue
        tail_s = r['bodies'][-1]['e']; wall = r['wall']
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        best = fit_frame(d, fr, tail_s, wall)
        if best is None:
            out.write(f'f{fr}: none\n'); out.flush(); continue
        P, end, blocks, xo = best
        vals = levels_from_blocks(blocks)
        if not vals:
            out.write(f'f{fr}: none\n'); out.flush(); continue
        out.write(f'f{fr}: SEM ' + json.dumps({
            'fr': fr, 'P': P, 'end': end, 'wall': wall,
            'xo': xo if xo is not None else -1,
            'xos': [b['xover'] for b in blocks],
            'lvl': round(float(np.mean(vals)), 3),
            'n': len(vals)}) + '\n')
        out.flush()
    print('SEM DONE')
