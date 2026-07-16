#!/usr/bin/env python3
"""R491: short-body hole-fill pass. For frames in multibody_kw.log,
scan inventory holes for short/multigroup bodies (ac4short), validate
by corr vs M/S oracles at 6 placement offsets, log fN: SHORT rows.
Resumable: skips frames already in shortfill_kw.log.
"""
import sys, json, re, wave, os
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf, ac4short
N = 2048
w = wave.open('kw-ref51.wav', 'rb')
REF = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
MOR = REF[:, 0] + REF[:, 1]
SOR = REF[:, 0] - REF[:, 1]
OFFS = (768, 832, 896, 960, 1024, 1088)
CMIN = 0.30

done = set()
if os.path.exists('shortfill_kw.log'):
    for ln in open('shortfill_kw.log'):
        mm = re.match(r'f(\d+): (SHORT|none)', ln)
        if mm: done.add(int(mm.group(1)))

inv = {}
for ln in open('multibody_kw.log'):
    mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
    if mm:
        r = json.loads(mm.group(2)); inv[r['fr']] = r

out = open('shortfill_kw.log', 'a')
for fr in sorted(inv):
    if fr in done: continue
    r = inv[fr]; bs = r['bodies']; wall = r['wall']
    try:
        d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
    except FileNotFoundError:
        continue
    if fr * N + 1024 + 4096 > len(MOR):
        continue
    refM = MOR[fr * N + 1024: fr * N + 1024 + 4096]
    refS = SOR[fr * N + 1024: fr * N + 1024 + 4096]
    holes = []
    if bs:
        holes.append((max(16, bs[0]['s'] - 3000), bs[0]['s']))
        for i in range(len(bs) - 1):
            g = bs[i + 1]['s'] - bs[i]['e']
            if g > 40: holes.append((bs[i]['e'], bs[i + 1]['s']))
        if wall - bs[-1]['e'] > 200:
            holes.append((bs[-1]['e'], wall - 100))
    found = []
    seen_e = set()
    for (a, b) in holes:
        for P in range(a, b):
            try:
                pr = ac4short.parse_short(d, P)
            except Exception:
                continue
            if pr is None: continue
            cfg, data = pr
            if data.end > b + 60 or data.end in seen_e: continue
            nz = int((data.quant != 0).sum())
            if nz < 25: continue
            best = (0.0, 0, 'M')
            for off in OFFS:
                blk = ac4short.short_block(cfg, data, off)
                sd = blk.std()
                if sd < 1e-9: continue
                if refM.std() > 10:
                    c = float(np.corrcoef(blk, refM)[0, 1])
                    if abs(c) > abs(best[0]): best = (c, off, 'M')
                if refS.std() > 10:
                    c = float(np.corrcoef(blk, refS)[0, 1])
                    if abs(c) > abs(best[0]): best = (c, off, 'S')
            if abs(best[0]) >= CMIN:
                seen_e.add(data.end)
                found.append(dict(s=P, e=data.end, tl=cfg.tl0,
                                  W=cfg.num_windows, g=cfg.num_window_groups,
                                  nz=nz, c=round(best[0], 3), off=best[1],
                                  o=best[2]))
    if found:
        found.sort(key=lambda x: -abs(x['c']))
        out.write(f'f{fr}: SHORT ' + json.dumps({'fr': fr, 'bodies': found[:6]}) + '\n')
    else:
        out.write(f'f{fr}: none\n')
    out.flush()
print('SHORTFILL DONE')
