#!/usr/bin/env python3
"""R477b: does the ffmpeg walk TILE the frame with no mystery pad?

For each frame, list channel-element boundaries and body ends from
the walk. Measure:
  (a) intra-element gap: end[i] -> sect/msfb[i+1] within same element
  (b) inter-element gap: last body end -> next element's first msfb
If (a) ~ 0 everywhere and (b) = small consistent header, the pad
field was never real -- it was my long-only parser's under-read.
"""
import sys, json, re
import numpy as np
from collections import Counter
TRACK = sys.argv[1] if len(sys.argv) > 1 else 'kw'
WALK = {'kw': 'kw_mp4_walk.json', 'spk': 'spk_walk.json'}[TRACK]
walk = json.load(open(WALK))

ELEM = re.compile(r'POS (mono\(lfe=\d\)|2ch|1ch|3ch|4ch|5ch)@(\d+)')

def parse_rec(rec):
    """return ordered event list of ('elem',pos,name)|('msfb',pos)|
    ('audit',sect,m,g,long)|('end',pos)|('aspx',pos,name)."""
    seq = []
    for e in rec['ev']:
        m = ELEM.match(e)
        if m: seq.append(('elem', int(m.group(2)), m.group(1))); continue
        m = re.match(r'POS msfb=(\d+)@(\d+)', e)
        if m: seq.append(('msfb', int(m.group(2)), int(m.group(1)))); continue
        m = re.match(r'AUDIT m=(\d+) g=(\d+) long=(\d+) sect@(\d+)', e)
        if m: seq.append(('audit', int(m.group(4)), int(m.group(1)), int(m.group(2)), int(m.group(3)))); continue
        m = re.match(r'POS end@(\d+)', e)
        if m: seq.append(('end', int(m.group(1)))); continue
        m = re.match(r'POS (aspx_\w+)@(\d+)', e)
        if m: seq.append(('aspx', int(m.group(2)), m.group(1))); continue
    return seq

intra = Counter()      # end -> next sect within an element run
elem_hdr = Counter()   # last-end-before-elem -> first sect-after-elem
big_short = 0; big_long = 0
gmulti = Counter()
n_frames = 0
for rec in walk[:400]:
    if 'ev' not in rec: continue
    seq = parse_rec(rec)
    n_frames += 1
    last_end = None
    after_elem = False
    for i, ev in enumerate(seq):
        if ev[0] == 'audit':
            sect = ev[1]; m = ev[2]; g = ev[3]; lng = ev[4]
            gmulti[(g, lng)] += 1
            if m >= 12:
                if lng == 0: big_short += 1
                else: big_long += 1
            if last_end is not None:
                gap = sect - last_end
                if after_elem: elem_hdr[gap] += 1
                else: intra[gap] += 1
            after_elem = False
        elif ev[0] == 'end':
            last_end = ev[1]
        elif ev[0] == 'elem':
            after_elem = True
        elif ev[0] == 'aspx':
            after_elem = True   # bodies after aspx belong to next element
            last_end = None

def top(c, k=12):
    return sorted(c.items())[:k]

print(f'frames analyzed: {n_frames}')
print(f'\nINTRA-element gap (end[i] -> next body sect, same element):')
tot = sum(intra.values())
for g, n in sorted(intra.items())[:15]:
    print(f'  gap={g:4d}: {n:5d} ({100*n/tot:.0f}%)')
print(f'  total intra transitions: {tot}, gap==0: {intra.get(0,0)} ({100*intra.get(0,0)/max(tot,1):.0f}%)')
print(f'\nINTER-element/header gap (last end before new elem -> first body sect):')
tot2 = sum(elem_hdr.values())
for g, n in sorted(elem_hdr.items())[:20]:
    print(f'  hdr={g:4d}: {n:5d} ({100*n/tot2:.0f}%)')
print(f'\nbig-body (m>=12): short(long=0)={big_short}  long(long=1)={big_long}')
print(f'(g, long) distribution:', dict(sorted(gmulti.items(), key=lambda x:-x[1])[:10]))
