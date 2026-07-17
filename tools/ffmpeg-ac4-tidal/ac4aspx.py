#!/usr/bin/env python3
"""A-SPX bitstream parser (Tables 51-58) with parametrized band counts.

Unknowns (from unlocated aspx_config / master band tables) are free
parameters: nsb_hi (num_sbg_sig_highres), nsb_noise, env_bits_fixfix,
freq_res_mode. nsb_lo = ceil(nsb_hi/2). num_aspx_timeslots = 16.
parse_aspx_2ch/1ch return end position; raise on invalid.
"""
import sys
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T = A.T
TIMESLOTS = 16
FIXFIX, FIXVAR, VARVAR, VARFIX = 0, 1, 2, 3


import numpy as _np
_OFF = {}


def _huff(bits, name):
    idx = A.huff(bits, T[name + '_LEN'], T[name + '_CW'])
    if name not in _OFF:
        _OFF[name] = int(_np.argmin(T[name + '_LEN']))  # mode = zero point
    return idx - _OFF[name]


def read_int_class(bits):
    # 1..3 bit code: FIXFIX=0b0? Per spec aspx_int_class is 1..3 bits.
    # Standard coding: 0 -> FIXFIX; 10 -> FIXVAR; 110 -> VARVAR; 111 -> VARFIX
    if bits.u(1) == 0:
        return FIXFIX
    if bits.u(1) == 0:
        return FIXVAR
    return VARVAR if bits.u(1) == 0 else VARFIX


def aspx_framing(bits, cfg, iframe):
    num_rel_l = num_rel_r = 0
    cls = read_int_class(bits)
    freq_res = []
    if cls == FIXFIX:
        envbits = cfg['env_bits_fixfix'] + 1
        tmp = bits.u(envbits)
        num_env = 1 << tmp
        if num_env > 8: raise ValueError('num_env')
        if cfg['freq_res_mode'] == 0:
            freq_res = [bits.u(1)] * num_env
    else:
        if cls == FIXVAR:
            bits.u(2)
            num_rel_r = bits.u(2)
            for _ in range(num_rel_r): bits.u(2)
        elif cls == VARVAR:
            if iframe: bits.u(2)
            num_rel_l = bits.u(2)
            for _ in range(num_rel_l): bits.u(2)
            bits.u(2)
            num_rel_r = bits.u(2)
            for _ in range(num_rel_r): bits.u(2)
        elif cls == VARFIX:
            if iframe: bits.u(2)
            num_rel_l = bits.u(2)
            for _ in range(num_rel_l): bits.u(2)
        num_env = num_rel_l + num_rel_r + 1
        if num_env > 8: raise ValueError('num_env')
        import math
        ptr_bits = math.ceil(math.log2(num_env + 2))
        bits.u(ptr_bits)
        if cfg['freq_res_mode'] == 0:
            freq_res = [bits.u(1) for _ in range(num_env)]
    if cfg['freq_res_mode'] == 1:
        freq_res = [0] * num_env
    elif cfg['freq_res_mode'] >= 2:
        freq_res = [1] * num_env    # approx for mode2; mode3 exact
    num_noise = 2 if num_env > 1 else 1
    return num_env, num_noise, freq_res


def aspx_delta_dir(bits, num_env, num_noise):
    sig = [bits.u(1) for _ in range(num_env)]
    noi = [bits.u(1) for _ in range(num_noise)]
    return sig, noi


def hfgen_iwc_2ch(bits, cfg, balance):
    nn = cfg['nsb_noise']; nh = cfg['nsb_hi']
    for _ in range(nn): bits.u(2)
    if balance == 0:
        for _ in range(nn): bits.u(2)
    if bits.u(1):
        for _ in range(nh): bits.u(1)
    if bits.u(1):
        for _ in range(nh): bits.u(1)
    if bits.u(1):   # fic_present
        if bits.u(1):
            for _ in range(nh): bits.u(1)
        if bits.u(1):
            for _ in range(nh): bits.u(1)
    if bits.u(1):   # tic_present
        copy = bits.u(1)
        tl = tr = 0
        if copy == 0:
            tl = bits.u(1); tr = bits.u(1)
        if copy or tl:
            for _ in range(TIMESLOTS): bits.u(1)
        if tr:
            for _ in range(TIMESLOTS): bits.u(1)


def hfgen_iwc_1ch(bits, cfg):
    nn = cfg['nsb_noise']; nh = cfg['nsb_hi']
    for _ in range(nn): bits.u(2)
    if bits.u(1):
        for _ in range(nh): bits.u(1)
    if bits.u(1):
        for _ in range(nh): bits.u(1)
    if bits.u(1):
        for _ in range(TIMESLOTS): bits.u(1)


def _hcb(data_type, qm, sm, kind):
    lvl = 'BALANCE' if sm == 'BAL' else 'LEVEL'
    res = '30' if qm else '15'
    if data_type == 'NOISE':
        return f'ASPX_HCB_NOISE_{lvl}_{kind}'
    return f'ASPX_HCB_ENV_{lvl}_{res}_{kind}'


def ec_data(bits, cfg, data_type, num_env, freq_res, qm, sm, dirs, sink=None):
    """decode; if sink is a list, append per-env raw envelope arrays."""
    prev = None
    for env in range(num_env):
        if data_type == 'SIGNAL':
            nsb = cfg['nsb_hi'] if (freq_res and freq_res[env]) else cfg['nsb_lo']
        else:
            nsb = cfg['nsb_noise']
        d = dirs[env] if env < len(dirs) else 0
        vals = []
        if d == 0:   # FREQ: F0 absolute then DF cumulative across bands
            v0 = _huff(bits, _hcb(data_type, qm, sm, 'F0'))
            vals.append(v0)
            acc = v0
            for _ in range(1, nsb):
                acc += _huff(bits, _hcb(data_type, qm, sm, 'DF'))
                vals.append(acc)
        else:        # TIME: DT delta vs previous env, per band
            base = prev if prev is not None else [0] * nsb
            for i in range(nsb):
                b = base[i] if i < len(base) else 0
                vals.append(b + _huff(bits, _hcb(data_type, qm, sm, 'DT')))
        prev = vals
        if sink is not None:
            sink.append(vals)


def parse_aspx_2ch(bits, cfg, iframe, envs=None):
    if iframe:
        bits.u(3)   # xover_subband_offset
    ne0, nn0, fr0 = aspx_framing(bits, cfg, iframe)
    qm0 = cfg['quant_mode']
    balance = bits.u(1)
    ne1, nn1, fr1 = ne0, nn0, fr0
    if balance == 0:
        ne1, nn1, fr1 = aspx_framing(bits, cfg, iframe)
    sd0, nd0 = aspx_delta_dir(bits, ne0, nn0)
    sd1, nd1 = aspx_delta_dir(bits, ne1, nn1)
    hfgen_iwc_2ch(bits, cfg, balance)
    s0 = [] if envs is not None else None
    s1 = [] if envs is not None else None
    ec_data(bits, cfg, 'SIGNAL', ne0, fr0, qm0, 'LVL', sd0, s0)
    ec_data(bits, cfg, 'SIGNAL', ne1, fr1, qm0, 'BAL' if balance else 'LVL', sd1, s1)
    ec_data(bits, cfg, 'NOISE', nn0, None, 1, 'LVL', nd0)
    ec_data(bits, cfg, 'NOISE', nn1, None, 1, 'BAL' if balance else 'LVL', nd1)
    if envs is not None:
        envs.append(('2ch', fr0, s0, s1))
    return bits.p


def parse_aspx_1ch(bits, cfg, iframe):
    if iframe:
        bits.u(3)
    ne, nn, fr = aspx_framing(bits, cfg, iframe)
    sd, nd = aspx_delta_dir(bits, ne, nn)
    hfgen_iwc_1ch(bits, cfg)
    ec_data(bits, cfg, 'SIGNAL', ne, fr, cfg['quant_mode'], 'LVL', sd)
    ec_data(bits, cfg, 'NOISE', nn, None, 1, 'LVL', nd)
    return bits.p
