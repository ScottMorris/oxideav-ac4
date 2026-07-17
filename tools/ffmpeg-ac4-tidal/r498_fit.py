#!/usr/bin/env python3
"""R498d: fit the exact [2ch][2ch][1ch][2ch] A-SPX tail on iframes.

Search P near the last inventory body end; require all 4 blocks to parse
with xover in range and end <= wall. Report candidate multiplicity,
xover agreement across blocks, class/num_env stats, wall-end leftover.
"""
import sys, json, re, glob
from collections import Counter
import ac4aspx2 as X
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')

inv = {}
for ln in open('multibody_kw.log'):
    mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
    if mm:
        r = json.loads(mm.group(2)); inv[r['fr']] = r

iframes = sorted(fr for fr in inv if fr % 24 == 0)
fit1 = fit_many = fit0 = 0
xover_c = Counter(); cls_c = Counter(); left_c = []
xover_agree = Counter()
examples = []
for fr in iframes:
    r = inv[fr]
    if not r['bodies']:
        fit0 += 1
        continue
    tail_s = r['bodies'][-1]['e']; wall = r['wall']
    try:
        d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
    except FileNotFoundError:
        continue
    cands = []
    for P in range(max(20, tail_s - 8), min(tail_s + 64, wall - 50)):
        states = [X.ChState() for _ in range(4)]
        try:
            blocks, end = X.parse_tail(d, P, 1, states)
        except Exception:
            continue
        if end > wall:
            continue
        xos = [b['xover'] for b in blocks]
        cands.append((P, end, xos, blocks))
    if not cands:
        fit0 += 1
        continue
    if len(cands) == 1:
        fit1 += 1
    else:
        fit_many += 1
    # prefer candidate whose end is closest to wall (least leftover)
    P, end, xos, blocks = max(cands, key=lambda c: c[1])
    xover_c.update(xos)
    xover_agree[len(set(xos))] += 1
    for b in blocks:
        for fm in b['fm']:
            cls_c[(fm['cls'], fm['num_env'])] += 1
    left_c.append(wall - end)
    if len(examples) < 6:
        examples.append((fr, P, tail_s, end, wall, xos, len(cands)))

print(f'iframes {len(iframes)}: unique-fit {fit1}, multi-fit {fit_many}, '
      f'no-fit {fit0}')
print('xover values:', dict(xover_c))
print('distinct xovers per frame:', dict(xover_agree))
print('(cls,num_env):', dict(cls_c))
if left_c:
    import statistics
    print(f'leftover wall-end: min {min(left_c)} med '
          f'{statistics.median(left_c)} max {max(left_c)}')
print('examples (fr P tail_s end wall xovers ncand):')
for e in examples:
    print('  ', e)
