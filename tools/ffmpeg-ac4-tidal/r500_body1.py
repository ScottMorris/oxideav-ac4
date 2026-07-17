#!/usr/bin/env python3
"""R500b: nail body-1. Entropy map says payload starts right after the
LFE. Scan h=0..12 after each frame's LFE end for a v2 body that passes
r489's SEMANTIC validation (band-envelope corr vs ref oracles).
Controls: h=24..36 (should validate less if h-small is real).
"""
import sys
import numpy as np
import r489_multibody as R
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf

from collections import Counter


def lfe_end_general(d, P):
    b = A.Bits(d, P)
    cfg = ac4asf.parse_sf_info_lfe(b)
    ac4asf.parse_sf_data(b, cfg)
    return b.p


def main():
    hits = Counter()
    nulls = Counter()
    n = 0
    frames = [fr for fr in range(1, 1300) if fr % 24][::4][:300]
    for fr in frames:
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        try:
            E = lfe_end_general(d, 18)
        except Exception:
            continue
        RW, RSD = R.make_refwin(fr)
        if RW is None:
            continue
        BP = R.band_profiles(fr)
        n += 1
        for h in range(0, 13):
            b = R.body_at(d, E + h, RW, RSD, BP)
            if b and (abs(b['c']) >= 0.35 or b['bc'] >= 0.5):
                hits[h] += 1
        for h in range(24, 37):
            b = R.body_at(d, E + h, RW, RSD, BP)
            if b and (abs(b['c']) >= 0.35 or b['bc'] >= 0.5):
                nulls[h - 24] += 1
    print(f'frames tested {n}')
    print('validated body-1 at LFE_end+h:')
    for h in range(13):
        print(f'  h={h:2d}: {hits[h]:4d} ({hits[h]/max(n,1):.2f})')
    print('null window (h=24..36):')
    for h in range(13):
        print(f'  h={h+24:2d}: {nulls[h]:4d} ({nulls[h]/max(n,1):.2f})')


if __name__ == '__main__':
    main()
