#!/usr/bin/env python3
"""R498c: aspx_config under the CORRECT 7_X_channel_element reading.

sub*.bin = ac4_substream: [audio_size 15][b_more 1][7_X_channel_element...]
7_X: codec_mode u(2) at bit 16; iframe: aspx_config (15b) at bit 18.
Check codec_mode constancy across ALL 1410 frames + config across iframes.
"""
import sys, glob, math
from collections import Counter
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A

LOWRES = [10,11,12,13,14,15,16,17,18,19,20,22,24,26,28,30,32,35,38,42,46]
HIGHRES = [18,19,20,21,22,23,24,26,28,30,32,34,36,38,40,42,44,47,50,53,56,59,62]

frames = sorted(int(f[-8:-4]) for f in glob.glob('kw4/sub*.bin'))
modes = Counter(); cfgs = Counter(); sample = None
for fr in frames:
    d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
    b = A.Bits(d, 16)
    cm = b.u(2)
    modes[cm] += 1
    if fr % 24 == 0 and cm != 0:
        c = dict(quant_mode_env=b.u(1), start_freq=b.u(3), stop_freq=b.u(2),
                 master_scale=b.u(1), interpolation=b.u(1), preflat=b.u(1),
                 limiter=b.u(1), noise_sbg=b.u(2),
                 num_env_bits_fixfix=b.u(1), freq_res_mode=b.u(2))
        key = tuple(c.items())
        cfgs[key] += 1
        sample = c

print('codec_mode across ALL frames:', dict(modes))
print('iframe configs:', len(cfgs), 'distinct')
for key, n in cfgs.most_common(3):
    print(f'  x{n}:', dict(key))
c = sample
tmpl = HIGHRES if c['master_scale'] else LOWRES
n_tmpl = 22 if c['master_scale'] else 20
n_master = n_tmpl - 2*c['start_freq'] - 2*c['stop_freq']
master = [tmpl[2*c['start_freq'] + g] for g in range(n_master + 1)]
print(f'\nn_sbg_master={n_master} master={master}')
print(f'A-SPX range: QMF {master[0]}-{master[-1]} = '
      f'{master[0]*375}-{master[-1]*375} Hz')
for xo in range(min(4, n_master)):
    n_hi = n_master - xo
    hi = master[xo:]
    n_lo = n_hi - n_hi // 2
    sbx, sbz = hi[0], hi[-1]
    n_noise = max(1, math.floor(c['noise_sbg'] * math.log2(sbz / sbx) + 0.5))
    print(f'  xover={xo}: n_hi={n_hi} n_lo={n_lo} n_noise={n_noise} '
          f'sbx={sbx}({sbx*375}Hz)')
