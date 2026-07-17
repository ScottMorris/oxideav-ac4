#!/usr/bin/env python3
"""R498b: deterministic top-down parse from bit 16 (after audio_size header).

Sequential over frames, carrying the (5,6) pair cfgs across frames for
b_use_sap_add_ch chparam reads. Match metric: fraction of parsed body
start positions that appear exactly in the validated multibody inventory.
"""
import sys, json, re, glob, statistics
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4imms

inv = {}
for ln in open('multibody_kw.log'):
    mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
    if mm:
        r = json.loads(mm.group(2)); inv[r['fr']] = r

frames = sorted(int(f[-8:-4]) for f in glob.glob('kw4/sub*.bin'))
ok = 0
fails = {}
hit_fracs = []
per_frame = []
prev56 = None
for fr in frames:
    d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
    iframe = 1 if fr % 24 == 0 else 0
    try:
        out, end, mode, grouping = ac4imms.parse_immersive_core(
            d, 16, iframe, b_lfe=1, prev_cfg56=prev56)
        ok += 1
        if 5 in out and 6 in out:
            prev56 = (out[5][0], out[6][0])
        r = inv.get(fr)
        if r and r['bodies']:
            inv_pos = set(b['s'] for b in r['bodies'])
            got = [data.start for ch, (cfg, data) in out.items()
                   if getattr(data, 'start', None) is not None]
            hits = sum(1 for p in got if p in inv_pos)
            if got:
                hit_fracs.append(hits / len(got))
                per_frame.append((fr, mode, grouping, len(got), hits, end,
                                  r['wall']))
    except Exception as e:
        key = str(e)[:50]
        fails[key] = fails.get(key, 0) + 1
        prev56 = prev56  # keep stale

print(f'parse ok {ok} / {len(frames)}')
for k, n in sorted(fails.items(), key=lambda kv: -kv[1])[:8]:
    print(f'  FAIL x{n}: {k}')
if hit_fracs:
    print(f'\nframes compared {len(hit_fracs)}')
    print(f'mean position-hit fraction {statistics.mean(hit_fracs):.3f}  '
          f'median {statistics.median(hit_fracs):.3f}')
    full = sum(1 for h in hit_fracs if h == 1.0)
    zero = sum(1 for h in hit_fracs if h == 0.0)
    print(f'all-hit frames {full}, zero-hit frames {zero}')
    print('\nsample rows (fr mode grp nbodies hits end wall):')
    for row in per_frame[:12]:
        print('  f%-4d m%d g%d n%d h%d end%-5d wall%d' % row)
