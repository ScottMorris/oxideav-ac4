#!/usr/bin/env python3
"""R497: extract per-frame A-SPX signal envelope (12-band highband shape).

For each frame with a fittable tail, locate the A-SPX chain
([2ch][2ch][1ch]) at the tail start (small position + config search),
decode the SIGNAL envelopes, average across time-envelopes and the
7 channels, and log a 12-value highband shape (dequantized, 3dB steps).
These feed v11's highband (real data, no reference envelope).
Log: fN: AENV {"fr":..,"shape":[12 gains],"P":..,"cons":..}
"""
import sys, json, re, os
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4aspx

NSB = 12
CFGS = [dict(nsb_hi=12, nsb_lo=6, nsb_noise=nn, env_bits_fixfix=ebf,
             freq_res_mode=3, quant_mode=0)
        for nn in (2, 3, 4, 5) for ebf in (0, 1)]


def fit_extract(d, P, cfg, iframe, wall):
    try:
        b = A.Bits(d, P); envs = []
        ac4aspx.parse_aspx_2ch(b, cfg, iframe, envs)
        ac4aspx.parse_aspx_2ch(b, cfg, iframe, envs)
        # 1ch: capture its signal env too
        e1 = _parse_1ch_env(b, cfg, iframe, envs)
        if b.p <= wall:
            return b.p, envs
    except Exception:
        pass
    return None, None


def _parse_1ch_env(bits, cfg, iframe, envs):
    if iframe:
        bits.u(3)
    ne, nn, fr = ac4aspx.aspx_framing(bits, cfg, iframe)
    sd, nd = ac4aspx.aspx_delta_dir(bits, ne, nn)
    ac4aspx.hfgen_iwc_1ch(bits, cfg)
    s = []
    ac4aspx.ec_data(bits, cfg, 'SIGNAL', ne, fr, cfg['quant_mode'], 'LVL', sd, s)
    ac4aspx.ec_data(bits, cfg, 'NOISE', nn, None, 1, 'LVL', nd)
    envs.append(('1ch', fr, s, None))
    return bits.p


def shape_from_envs(envs):
    """average all SIGNAL envelope vectors (that are full highres = 12)
    into one 12-band level vector, then dequantize (3dB=level*0.5 in
    log2 energy -> gain 2**(0.25*level))."""
    acc = np.zeros(NSB); cnt = 0
    for tag, fr, s0, s1 in envs:
        for sset in (s0, s1):
            if not sset:
                continue
            for arr in sset:
                if len(arr) == NSB:
                    acc += np.array(arr, float); cnt += 1
    if cnt == 0:
        return None
    lv = acc / cnt
    lv = lv - lv.mean()            # relative shape (absolute anchored at render)
    return (2.0 ** (0.25 * lv)).tolist()


if __name__ == '__main__':
    done = set()
    if os.path.exists('aspxenv_kw.log'):
        for ln in open('aspxenv_kw.log'):
            mm = re.match(r'f(\d+): (AENV|none)', ln)
            if mm: done.add(int(mm.group(1)))
    inv = {}
    for ln in open('multibody_kw.log'):
        mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
        if mm:
            r = json.loads(mm.group(2)); inv[r['fr']] = r
    out = open('aspxenv_kw.log', 'a')
    for fr in sorted(inv):
        if fr in done: continue
        r = inv[fr]
        if not r['bodies']:
            out.write(f'f{fr}: none\n'); continue
        tail_s = r['bodies'][-1]['e']; wall = r['wall']
        if not (300 <= wall - tail_s <= 3000):
            out.write(f'f{fr}: none\n'); continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        iframe = 1 if fr % 24 == 0 else 0
        best = None
        for P in range(tail_s, min(tail_s + 32, wall - 100)):
            for cfg in CFGS:
                e, envs = fit_extract(d, P, cfg, iframe, wall)
                if e is not None:
                    cons = (e - P) / (wall - P)
                    if cons >= 0.88 and (best is None or cons > best[0]):
                        best = (cons, P, envs)
        if best:
            shape = shape_from_envs(best[2])
            if shape:
                out.write(f'f{fr}: AENV ' + json.dumps({'fr': fr,
                    'shape': [round(x, 3) for x in shape],
                    'P': best[1], 'cons': round(best[0], 2)}) + '\n')
            else:
                out.write(f'f{fr}: none\n')
        else:
            out.write(f'f{fr}: none\n')
        out.flush()
    print('AENV DONE')
