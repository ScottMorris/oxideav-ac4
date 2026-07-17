#!/usr/bin/env python3
"""EXACT A-SPX parser for the kw4 stream (R498).

Locked config (read deterministically from all 59 iframes at bit 18):
  quant_mode_env=1 (3dB), start_freq=7, stop_freq=1, master_scale=1
  -> sbg_master=[40,42,44,47,50,53,56], n_sbg_master=6 (15-21 kHz)
  interpolation=1, preflat=1, limiter=1, noise_sbg=3,
  num_env_bits_fixfix=0 (num_env in {1,2}), freq_res_mode=2.
Element tail (7_X codec_mode=ASPX): aspx_data_2ch, 2ch, 1ch, 2ch.

Implements Tables 51-58 exactly, incl. mode-2 freq_res (Pseudocode 77),
FIXFIX tab_border, per-channel FIXFIX/1-env qmode override, and
inter-frame previous_stop for VARFIX/VARVAR left borders.
"""
import sys, math
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import numpy as _np

T = A.T
TIMESLOTS = 16
FIXFIX, FIXVAR, VARVAR, VARFIX = 0, 1, 2, 3

MASTER = [40, 42, 44, 47, 50, 53, 56]
N_MASTER = 6
QUANT_MODE = 1
ENV_BITS_FIXFIX = 0
NOISE_SBG = 3
TAB_BORDER = {1: [0, 16], 2: [0, 8, 16]}

_OFF = {}


def bands_for_xover(xo):
    """(nsb_hi, nsb_lo, nsb_noise, hi_table, lo_table) per Pseudocode 68-70."""
    if not (0 <= xo < N_MASTER):
        raise ValueError('xover')
    hi = MASTER[xo:]
    n_hi = len(hi) - 1
    n_lo = n_hi - n_hi // 2
    lo = [hi[0]]
    if n_hi % 2 == 0:
        lo += [hi[2 * g] for g in range(1, n_lo + 1)]
    else:
        lo += [hi[2 * g - 1] for g in range(1, n_lo + 1)]
    sbx, sbz = hi[0], hi[-1]
    n_noise = max(1, math.floor(NOISE_SBG * math.log2(sbz / sbx) + 0.5))
    return n_hi, n_lo, n_noise, hi, lo


def _huff(bits, name):
    idx = A.huff(bits, T[name + '_LEN'], T[name + '_CW'])
    if name not in _OFF:
        _OFF[name] = int(_np.argmin(T[name + '_LEN']))
    return idx - _OFF[name]


def read_int_class(bits):
    if bits.u(1) == 0:
        return FIXFIX
    if bits.u(1) == 0:
        return FIXVAR
    return VARVAR if bits.u(1) == 0 else VARFIX


def _mode2_freq_res(borders, atsg, tsg_ptr):
    if (atsg < tsg_ptr and TIMESLOTS > 8) or \
       (borders[atsg + 1] - borders[atsg]) > (TIMESLOTS / 6.0 + 3.25):
        return 1
    return 0


def aspx_framing(bits, iframe, prev_stop):
    """returns dict(num_env, num_noise, freq_res[], borders, cls, new_stop)"""
    cls = read_int_class(bits)
    num_rel_l = num_rel_r = 0
    rel_l, rel_r = [], []
    var_l = var_r = 0
    if cls == FIXFIX:
        tmp = bits.u(ENV_BITS_FIXFIX + 1)
        num_env = 1 << tmp
        borders = list(TAB_BORDER[num_env])
        tsg_ptr = 0
        fr0 = _mode2_freq_res(borders, 0, tsg_ptr)
        freq_res = [fr0] * num_env
    else:
        if cls == FIXVAR:
            var_r = bits.u(2)
            num_rel_r = bits.u(2)
            rel_r = [2 * bits.u(2) + 2 for _ in range(num_rel_r)]
        elif cls == VARVAR:
            if iframe:
                var_l = bits.u(2)
            num_rel_l = bits.u(2)
            rel_l = [2 * bits.u(2) + 2 for _ in range(num_rel_l)]
            var_r = bits.u(2)
            num_rel_r = bits.u(2)
            rel_r = [2 * bits.u(2) + 2 for _ in range(num_rel_r)]
        elif cls == VARFIX:
            if iframe:
                var_l = bits.u(2)
            num_rel_l = bits.u(2)
            rel_l = [2 * bits.u(2) + 2 for _ in range(num_rel_l)]
        num_env = num_rel_l + num_rel_r + 1
        if num_env > 8:
            raise ValueError('num_env')
        ptr_bits = math.ceil(math.log2(num_env + 2))
        tsg_ptr = bits.u(ptr_bits) - 1
        # borders per Pseudocode 76
        borders = [0] * (num_env + 1)
        if cls == FIXVAR:
            borders[0] = 0
            borders[num_env] = var_r + TIMESLOTS
            for t in range(num_rel_r):
                borders[num_env - t - 1] = borders[num_env - t] - rel_r[t]
        elif cls == VARFIX:
            borders[0] = var_l if iframe else prev_stop - TIMESLOTS
            borders[num_env] = TIMESLOTS
            for t in range(num_rel_l):
                borders[t + 1] = borders[t] + rel_l[t]
        else:  # VARVAR
            borders[0] = var_l if iframe else prev_stop - TIMESLOTS
            borders[num_env] = var_r + TIMESLOTS
            for t in range(num_rel_l):
                borders[t + 1] = borders[t] + rel_l[t]
            for t in range(num_rel_r):
                borders[num_env - t - 1] = borders[num_env - t] - rel_r[t]
        if any(borders[i + 1] <= borders[i] for i in range(num_env)):
            raise ValueError('borders')
        freq_res = [_mode2_freq_res(borders, e, tsg_ptr)
                    for e in range(num_env)]
    num_noise = 2 if num_env > 1 else 1
    return dict(cls=cls, num_env=num_env, num_noise=num_noise,
                freq_res=freq_res, borders=borders,
                new_stop=borders[num_env])


def aspx_delta_dir(bits, fm):
    sig = [bits.u(1) for _ in range(fm['num_env'])]
    noi = [bits.u(1) for _ in range(fm['num_noise'])]
    return sig, noi


def hfgen_iwc_2ch(bits, nh, nn, balance):
    for _ in range(nn):
        bits.u(2)
    if balance == 0:
        for _ in range(nn):
            bits.u(2)
    if bits.u(1):
        for _ in range(nh):
            bits.u(1)
    if bits.u(1):
        for _ in range(nh):
            bits.u(1)
    if bits.u(1):   # fic_present
        if bits.u(1):
            for _ in range(nh):
                bits.u(1)
        if bits.u(1):
            for _ in range(nh):
                bits.u(1)
    if bits.u(1):   # tic_present
        copy = bits.u(1)
        tl = tr = 0
        if copy == 0:
            tl = bits.u(1)
            tr = bits.u(1)
        if copy or tl:
            for _ in range(TIMESLOTS):
                bits.u(1)
        if tr:
            for _ in range(TIMESLOTS):
                bits.u(1)


def hfgen_iwc_1ch(bits, nh, nn):
    for _ in range(nn):
        bits.u(2)
    if bits.u(1):
        for _ in range(nh):
            bits.u(1)
    if bits.u(1):
        for _ in range(nh):
            bits.u(1)
    if bits.u(1):
        for _ in range(TIMESLOTS):
            bits.u(1)


def _hcb(data_type, qm, sm, kind):
    lvl = 'BALANCE' if sm == 'BAL' else 'LEVEL'
    res = '30' if qm else '15'
    if data_type == 'NOISE':
        return f'ASPX_HCB_NOISE_{lvl}_{kind}'
    return f'ASPX_HCB_ENV_{lvl}_{res}_{kind}'


def ec_data(bits, data_type, fm, qm, sm, dirs, n_hi, n_lo, n_noise,
            prev_env=None):
    """returns list of per-envelope value arrays; TIME deltas chain to
    prev_env (last envelope of previous frame's same channel)."""
    out = []
    prev = prev_env
    for env in range(fm['num_env'] if data_type == 'SIGNAL'
                     else fm['num_noise']):
        if data_type == 'SIGNAL':
            nsb = n_hi if fm['freq_res'][env] else n_lo
        else:
            nsb = n_noise
        d = dirs[env] if env < len(dirs) else 0
        vals = []
        if d == 0:
            v0 = _huff(bits, _hcb(data_type, qm, sm, 'F0'))
            vals.append(v0)
            acc = v0
            for _ in range(1, nsb):
                acc += _huff(bits, _hcb(data_type, qm, sm, 'DF'))
                vals.append(acc)
        else:
            base = prev if prev is not None else [0] * nsb
            for i in range(nsb):
                b = base[i] if i < len(base) else (base[-1] if base else 0)
                vals.append(b + _huff(bits, _hcb(data_type, qm, sm, 'DT')))
        prev = vals
        out.append(vals)
    return out


class ChState:
    """per-aspx-block persistent state (across frames)."""
    def __init__(self):
        self.xover = None
        self.prev_stop = TIMESLOTS
        self.prev_env = [None, None]     # per channel in block
        self.prev_noise = [None, None]


def parse_aspx_2ch(bits, iframe, st):
    if iframe:
        st.xover = bits.u(3)
    if st.xover is None:
        raise ValueError('no xover')
    n_hi, n_lo, n_noise, hi, lo = bands_for_xover(st.xover)
    fm0 = aspx_framing(bits, iframe, st.prev_stop)
    qm0 = 0 if (fm0['cls'] == FIXFIX and fm0['num_env'] == 1) else QUANT_MODE
    balance = bits.u(1)
    fm1, qm1 = fm0, qm0
    if balance == 0:
        fm1 = aspx_framing(bits, iframe, st.prev_stop)
        qm1 = 0 if (fm1['cls'] == FIXFIX and fm1['num_env'] == 1) \
            else QUANT_MODE
    sd0, nd0 = aspx_delta_dir(bits, fm0)
    sd1, nd1 = aspx_delta_dir(bits, fm1)
    hfgen_iwc_2ch(bits, n_hi, n_noise, balance)
    s0 = ec_data(bits, 'SIGNAL', fm0, qm0, 'LVL', sd0, n_hi, n_lo, n_noise,
                 st.prev_env[0])
    s1 = ec_data(bits, 'SIGNAL', fm1, qm1, 'BAL' if balance else 'LVL',
                 sd1, n_hi, n_lo, n_noise, st.prev_env[1])
    n0 = ec_data(bits, 'NOISE', fm0, 1, 'LVL', nd0, n_hi, n_lo, n_noise,
                 st.prev_noise[0])
    n1 = ec_data(bits, 'NOISE', fm1, 1, 'BAL' if balance else 'LVL',
                 nd1, n_hi, n_lo, n_noise, st.prev_noise[1])
    st.prev_stop = fm0['new_stop']
    st.prev_env = [s0[-1], s1[-1]]
    st.prev_noise = [n0[-1], n1[-1]]
    return dict(balance=balance, fm=(fm0, fm1), sig=(s0, s1),
                noise=(n0, n1), xover=st.xover, end=bits.p)


def parse_aspx_1ch(bits, iframe, st):
    if iframe:
        st.xover = bits.u(3)
    if st.xover is None:
        raise ValueError('no xover')
    n_hi, n_lo, n_noise, hi, lo = bands_for_xover(st.xover)
    fm = aspx_framing(bits, iframe, st.prev_stop)
    qm = 0 if (fm['cls'] == FIXFIX and fm['num_env'] == 1) else QUANT_MODE
    sd, nd = aspx_delta_dir(bits, fm)
    hfgen_iwc_1ch(bits, n_hi, n_noise)
    s = ec_data(bits, 'SIGNAL', fm, qm, 'LVL', sd, n_hi, n_lo, n_noise,
                st.prev_env[0])
    n = ec_data(bits, 'NOISE', fm, 1, 'LVL', nd, n_hi, n_lo, n_noise,
                st.prev_noise[0])
    st.prev_stop = fm['new_stop']
    st.prev_env = [s[-1], None]
    st.prev_noise = [n[-1], None]
    return dict(balance=None, fm=(fm,), sig=(s,), noise=(n,),
                xover=st.xover, end=bits.p)


def parse_tail(d, P, iframe, states):
    """parse the full 7_X ASPX tail [2ch][2ch][1ch][2ch] at bit P.
    states: list of 4 ChState. Returns (blocks, end)."""
    bits = A.Bits(d, P)
    blocks = [parse_aspx_2ch(bits, iframe, states[0]),
              parse_aspx_2ch(bits, iframe, states[1]),
              parse_aspx_1ch(bits, iframe, states[2]),
              parse_aspx_2ch(bits, iframe, states[3])]
    return blocks, bits.p
