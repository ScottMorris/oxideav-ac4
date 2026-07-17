#!/usr/bin/env python3
"""R498: read the REAL aspx_config (Table 50, 15 bits) from iframe fronts.

immersive_channel_element = [mode_code 1|3b][aspx_config 15b if mode!=SCPL]...
The config sits BEFORE the bed-region desync, so it should be readable
deterministically. Fields (in order):
  quant_mode_env 1 | start_freq 3 | stop_freq 2 | master_scale 1 |
  interpolation 1 | preflat 1 | limiter 1 | noise_sbg 2 |
  num_env_bits_fixfix 1 | freq_res_mode 2
Then derive the band tables per Pseudocode 67-70.
"""
import sys, glob, math
from collections import Counter
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A

LOWRES = [10,11,12,13,14,15,16,17,18,19,20,22,24,26,28,30,32,35,38,42,46]
HIGHRES = [18,19,20,21,22,23,24,26,28,30,32,34,36,38,40,42,44,47,50,53,56,59,62]


def read_cfg(d):
    b = A.Bits(d, 16)   # skip [audio_size_value 15][b_more_bits 1]
    if b.u(1):
        mode = 4
    else:
        mode = b.u(2)
    if mode == 0:
        return None
    c = dict(mode=mode,
             quant_mode_env=b.u(1), start_freq=b.u(3), stop_freq=b.u(2),
             master_scale=b.u(1), interpolation=b.u(1), preflat=b.u(1),
             limiter=b.u(1), noise_sbg=b.u(2),
             num_env_bits_fixfix=b.u(1), freq_res_mode=b.u(2))
    return c


def derive_bands(c, xover):
    tmpl = HIGHRES if c['master_scale'] else LOWRES
    n_tmpl = 22 if c['master_scale'] else 20
    n_master = n_tmpl - 2*c['start_freq'] - 2*c['stop_freq']
    if n_master < 1:
        return None
    master = [tmpl[2*c['start_freq'] + g] for g in range(n_master + 1)]
    n_hi = n_master - xover
    if n_hi < 1:
        return None
    hi = master[xover:]
    n_lo = n_hi - n_hi // 2
    lo = [hi[0]]
    if n_hi % 2 == 0:
        lo += [hi[2*g] for g in range(1, n_lo + 1)]
    else:
        lo += [hi[2*g - 1] for g in range(1, n_lo + 1)]
    sbx, sbz = hi[0], hi[-1]
    n_noise = max(1, math.floor(c['noise_sbg'] * math.log2(sbz / sbx) + 0.5))
    return dict(n_master=n_master, master=master, n_hi=n_hi, hi=hi,
                n_lo=n_lo, lo=lo, sbx=sbx, sbz=sbz, n_noise=n_noise)


if __name__ == '__main__':
    frames = sorted(int(f[-8:-4]) for f in glob.glob('kw4/sub*.bin'))
    iframes = [f for f in frames if f % 24 == 0]
    cnt = Counter()
    samples = {}
    for fr in iframes:
        d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        c = read_cfg(d)
        if c is None:
            cnt['SCPL'] += 1
            continue
        key = tuple(sorted(c.items()))
        cnt[key] += 1
        samples[key] = c
    print(f'{len(iframes)} iframes')
    for key, n in cnt.most_common(5):
        print(f'\n== {n} iframes ==')
        if key == 'SCPL':
            print('  mode SCPL (no aspx)')
            continue
        c = samples[key]
        print(' ', c)
        for xo in range(4):
            b = derive_bands(c, xo)
            if b:
                print(f'  xover={xo}: n_hi={b["n_hi"]} n_lo={b["n_lo"]} '
                      f'n_noise={b["n_noise"]} sbx={b["sbx"]} sbz={b["sbz"]} '
                      f'hi={b["hi"]}')
