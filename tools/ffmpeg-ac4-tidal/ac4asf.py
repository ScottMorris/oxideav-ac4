#!/usr/bin/env python3
"""Faithful port of the ffmpeg-ac4-tidal ASF channel parse (short +
multi-window-group). Split into parse_sf_info (transform+psy) and
parse_sf_data (section/spectral/scalefac/snf) so interleaved element
layouts (info,info,data,data) can be reproduced. Knobs baked in:
MSFB5=on, OVERSHOOT_SKIP=on, CB15=permissive. frame_len_base=2048.
"""
import sys
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T = A.T
huff = A.huff
CB_DIM, CB_OFF, CB_MOD, UNSIGNED = A.CB_DIM, A.CB_OFF, A.CB_MOD, A.UNSIGNED

FLB = 2048
TRANSF_2048 = [128, 256, 512, 1024, 2048]
N_GRP_A = [[15, 10, 8, 7], [10, 7, 4, 3], [8, 4, 3, 1], [7, 3, 1, 1]]
SFB_OFF = {
 2048: [0,4,8,12,16,20,24,28,32,36,40,44,52,60,68,76,84,92,100,108,116,124,136,148,160,172,188,204,220,240,260,284,308,336,364,396,432,468,508,552,600,652,704,768,832,896,960,1024,1088,1152,1216,1280,1344,1408,1472,1536,1600,1664,1728,1792,1856,1920,1984,2048,2176,2304,2432,2560,2688,2816,2944,3072,3200,3328,3456,3584,3712,3840,3968,4096,4224,4352,4480,4608,4736,4864,4992,5120,5248,5376,5504,5632,5760,5888,6016,6144,6272,6400,6528,6656,6784,6912,7040,7168,7296,7424,7552,7680,7808,7936,8064,8192],
 1024: [0,4,8,12,16,20,24,28,32,36,40,48,56,64,72,80,88,96,108,120,132,144,160,176,196,216,240,264,292,320,352,384,416,448,480,512,544,576,608,640,672,704,736,768,800,832,864,896,928,1024,1152,1280,1408,1536,1664,1792,1920,2048,2176,2304,2432,2560,2688,2816,2944,3072,3200,3328,3456,3584,3712,3840,3968,4096],
 512: [0,4,8,12,16,20,24,28,32,36,40,44,48,52,56,60,68,76,84,92,100,112,124,136,148,164,184,208,236,268,300,332,364,396,428,460,512,576,640,704,768,832,896,960,1024,1088,1152,1216,1280,1344,1408,1472,1536,1600,1664,1728,1792,1856,1920,1984,2048],
 256: [0,4,8,12,16,20,24,28,36,44,52,64,76,92,108,128,148,172,196,224,256,288,320,352,384,416,448,480,512,576,640,704,768,832,896,960,1024],
 128: [0,4,8,12,16,20,28,36,44,56,68,80,96,112,128,144,160,176,192,208,224,240,256,288,320,352,384,416,448,480,512],
}
NUM_SFB_48 = {2048: 63, 1024: 49, 512: 36, 256: 20, 128: 14}


def num_sfb_48(tl):
    return NUM_SFB_48[tl]


def get_msfb_bits(tl):
    if tl >= 384:
        return 5           # MSFB5
    if 192 <= tl <= 256:
        return 5
    return 4


def get_msfbl_bits(flb):
    return 3 if 1536 <= flb <= 2048 else 2


def get_side_bits(tl):
    if 480 <= tl <= 2048:
        return 5
    if 240 <= tl <= 384:
        return 4
    return 3


class Cfg:
    pass


def _build_elements(cfg):
    """asf_psy_elements: windows, groups, sect_sfb_offset, offset2sfb."""
    long_frame = cfg.long_frame
    idx0, idx1 = cfg.idx0, cfg.idx1
    n_grp_bits = cfg.n_grp_bits
    sfg = cfg.sfg
    different_framing = cfg.different_framing
    num_windows = 1; num_window_groups = 1
    window_to_group = [0] * 64
    if long_frame == 0:
        num_windows = n_grp_bits + 1
        if different_framing:
            num_windows_0 = 1 << (3 - idx0)
            for i in range(n_grp_bits, num_windows_0 - 1, -1):
                sfg[i] = sfg[i - 1]
            sfg[num_windows_0 - 1] = 0
            num_windows += 1
        for i in range(num_windows - 1):
            if sfg[i] == 0:
                num_window_groups += 1
            window_to_group[i + 1] = num_window_groups - 1
    cfg.num_windows = num_windows
    cfg.num_window_groups = num_window_groups
    cfg.window_to_group = window_to_group

    def get_transf_length(g):
        if long_frame:
            return FLB, 4
        num_windows_0 = 1 << (3 - idx0)
        if g < window_to_group[num_windows_0]:
            return cfg.tl0, idx0
        return cfg.tl1, idx1
    cfg.get_transf_length = get_transf_length

    def get_max_sfb(g, side_ch=None):
        idx = 0
        if different_framing:
            num_windows_0 = 1 << (3 - idx0)
            if g >= window_to_group[num_windows_0]:
                idx = 1
        sc = cfg.side_channel if side_ch is None else side_ch
        if cfg.side_limited or (cfg.dual_maxsfb and sc):
            return cfg.max_sfb_side[idx]
        return cfg.max_sfb[idx]
    cfg.get_max_sfb = get_max_sfb

    num_win_in_group = [0] * num_window_groups
    sect_sfb_offset = [None] * num_window_groups
    offset2sfb = {}
    group_offset = 0
    for g in range(num_window_groups):
        tlg, gidx = get_transf_length(g)
        sfbo = SFB_OFF[tlg]
        nwg = sum(1 for w in range(num_windows) if window_to_group[w] == g)
        num_win_in_group[g] = nwg
        msfb = get_max_sfb(g)
        row = [0] * (msfb + 1)
        for sfb in range(msfb):
            row[sfb] = group_offset + sfbo[sfb] * nwg
        group_offset += sfbo[msfb] * nwg
        row[msfb] = group_offset
        sect_sfb_offset[g] = row
        for sfb in range(msfb):
            for j in range(row[sfb], row[sfb + 1]):
                offset2sfb[j] = sfb
    cfg.num_win_in_group = num_win_in_group
    cfg.sect_sfb_offset = sect_sfb_offset
    cfg.offset2sfb = offset2sfb
    cfg.total_lines = group_offset
    return cfg


def parse_sf_info(bits, side_limited=0, dual_maxsfb=0, side_channel=0):
    cfg = Cfg()
    cfg.side_limited = side_limited
    cfg.dual_maxsfb = dual_maxsfb
    cfg.side_channel = side_channel
    long_frame = bits.u(1)
    if long_frame == 0:
        idx0 = bits.u(2); idx1 = bits.u(2)
        tl0 = TRANSF_2048[idx0]; tl1 = TRANSF_2048[idx1]
    else:
        idx0 = idx1 = 4; tl0 = FLB; tl1 = 0
    cfg.long_frame = long_frame; cfg.idx0 = idx0; cfg.idx1 = idx1
    cfg.tl0 = tl0; cfg.tl1 = tl1
    cfg.different_framing = 1 if (long_frame == 0 and idx0 != idx1) else 0

    n_msfb0 = get_msfb_bits(tl0); n_side0 = get_side_bits(tl0)
    max_sfb = [0, 0]; max_sfb_side = [0, 0]
    if side_limited:
        max_sfb_side[0] = bits.u(n_side0)
    else:
        max_sfb[0] = bits.u(n_msfb0)
        if dual_maxsfb:
            max_sfb_side[0] = bits.u(n_msfb0)
    cfg.msfb_pos = bits.p            # matches walk 'POS msfb@'
    if cfg.different_framing:
        n_msfb1 = get_msfb_bits(tl1); n_side1 = get_side_bits(tl1)
        if side_limited:
            max_sfb_side[1] = bits.u(n_side1)
        else:
            max_sfb[1] = bits.u(n_msfb1)
            if dual_maxsfb:
                max_sfb_side[1] = bits.u(n_msfb1)
    cfg.max_sfb = max_sfb; cfg.max_sfb_side = max_sfb_side
    n_grp_bits = 0 if long_frame == 1 else N_GRP_A[idx0][idx1]
    sfg = [0] * 64
    for i in range(n_grp_bits):
        sfg[i] = bits.u(1)
    cfg.n_grp_bits = n_grp_bits; cfg.sfg = sfg
    return _build_elements(cfg)


def parse_sf_info_lfe(bits):
    cfg = Cfg()
    cfg.side_limited = 0; cfg.dual_maxsfb = 0; cfg.side_channel = 0
    cfg.long_frame = 1; cfg.idx0 = 4; cfg.idx1 = 4; cfg.tl0 = FLB; cfg.tl1 = 0
    cfg.different_framing = 0
    n = get_msfbl_bits(FLB)
    cfg.max_sfb = [bits.u(n), 0]; cfg.max_sfb_side = [0, 0]
    cfg.msfb_pos = bits.p
    cfg.n_grp_bits = 0; cfg.sfg = [0] * 64
    return _build_elements(cfg)


def A_ext(bits):
    n = 0
    while bits.u(1):
        n += 1
        if n > 21:
            return 0
    return (1 << (n + 4)) + bits.u(n + 4)


def parse_sf_data(bits, cfg):
    ng = cfg.num_window_groups
    sect_cb = [[0] * 130 for _ in range(ng)]
    sfb_cb = [[0] * 130 for _ in range(ng)]
    sect_start = [[0] * 130 for _ in range(ng)]
    sect_end = [[0] * 130 for _ in range(ng)]
    num_sec = [0] * ng; num_sec_lsf = [0] * ng
    for g in range(ng):
        tlg, gidx = cfg.get_transf_length(g)
        if gidx <= 2:
            esc = 7; nb = 3
        else:
            esc = 31; nb = 5
        k = 0; i = 0
        msfb = cfg.get_max_sfb(g)
        nsfb48 = num_sfb_48(tlg)
        while k < msfb:
            cb = bits.u(4)
            ln = 1; inc = bits.u(nb)
            while inc == esc:
                ln += esc; inc = bits.u(nb)
            ln += inc
            if k + ln > 127 or i > 125:
                raise ValueError('sect overflow')
            sect_start[g][i] = k; sect_end[g][i] = k + ln; sect_cb[g][i] = cb
            if sect_start[g][i] < nsfb48 <= sect_end[g][i]:
                num_sec_lsf[g] = i + 1
                if sect_end[g][i] > nsfb48:
                    sect_end[g][i] = nsfb48
                    i += 1
                    sect_start[g][i] = nsfb48
                    sect_end[g][i] = k + ln
                    sect_cb[g][i] = sect_cb[g][i - 1]
            for sfb in range(k, min(k + ln, 130)):
                sfb_cb[g][sfb] = cb
            k += ln; i += 1
        num_sec[g] = i
        if num_sec_lsf[g] == 0:
            num_sec_lsf[g] = num_sec[g]

    pos_spec = bits.p
    quant = np.zeros(max(cfg.total_lines, 1), dtype=np.int32)
    max_qi = [[0] * 130 for _ in range(ng)]
    o2s = cfg.offset2sfb
    for g in range(ng):
        for i in range(num_sec_lsf[g]):
            cb = sect_cb[g][i]
            if cb == 0 or cb > 11:
                continue
            if sect_end[g][i] > cfg.get_max_sfb(g):    # OVERSHOOT_SKIP
                continue
            row = cfg.sect_sfb_offset[g]
            s_line = row[sect_start[g][i]]; e_line = row[sect_end[g][i]]
            lens = T[f'ASF_HCB_{cb}_LEN']; cws = T[f'ASF_HCB_{cb}_CW']
            dim = CB_DIM[cb]; off = CB_OFF[cb]; mod = CB_MOD[cb]; uns = UNSIGNED[cb]
            k = s_line
            while k < e_line:
                idx = huff(bits, lens, cws)
                if dim == 4:
                    rem = idx
                    v0 = rem // (mod ** 3) - off; rem -= (v0 + off) * mod ** 3
                    v1 = rem // (mod ** 2) - off; rem -= (v1 + off) * mod ** 2
                    v2 = rem // mod - off; rem -= (v2 + off) * mod
                    v3 = rem - off
                    vals = [v0, v1, v2, v3]
                    if uns:
                        for j in range(4):
                            if vals[j] and bits.u(1):
                                vals[j] = -vals[j]
                    for j in range(4):
                        quant[k + j] = vals[j]
                        x = o2s.get(k + j)
                        if x is not None:
                            av = abs(vals[j])
                            if av > max_qi[g][x]: max_qi[g][x] = av
                    k += 4
                else:
                    v0 = idx // mod - off
                    v1 = idx - (v0 + off) * mod - off
                    s0 = bits.u(1) if (uns and v0) else 0
                    s1 = bits.u(1) if (uns and v1) else 0
                    if cb == 11:
                        if v0 == 16: v0 = A_ext(bits)
                        if v1 == 16: v1 = A_ext(bits)
                    if s0: v0 = -v0
                    if s1: v1 = -v1
                    quant[k] = v0; quant[k + 1] = v1
                    x = o2s.get(k)
                    if x is not None and abs(v0) > max_qi[g][x]: max_qi[g][x] = abs(v0)
                    x = o2s.get(k + 1)
                    if x is not None and abs(v1) > max_qi[g][x]: max_qi[g][x] = abs(v1)
                    k += 2

    pos_scf = bits.p
    scale_factor = bits.u(8)
    sf_gain = [[0.0] * 130 for _ in range(ng)]
    first = False
    for g in range(ng):
        msfb = min(cfg.get_max_sfb(g), num_sfb_48(cfg.get_transf_length(g)[0]))
        for sfb in range(msfb):
            if sfb_cb[g][sfb] != 0 and max_qi[g][sfb] > 0:
                if first:
                    scale_factor += huff(bits, T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']) - 60
                else:
                    first = True
                sf_gain[g][sfb] = 2.0 ** (0.25 * (scale_factor - 100))

    pos_snf = bits.p
    snf_exists = bits.u(1)
    dpcm_snf = [[None] * 130 for _ in range(ng)]
    if snf_exists:
        for g in range(ng):
            msfb = min(cfg.get_max_sfb(g), num_sfb_48(cfg.get_transf_length(g)[0]))
            for sfb in range(msfb):
                if sfb_cb[g][sfb] == 0 or max_qi[g][sfb] == 0:
                    dpcm_snf[g][sfb] = huff(bits, T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW'])

    res = Cfg()
    res.quant = quant; res.max_qi = max_qi; res.sf_gain = sf_gain
    res.sect_cb = sect_cb; res.sfb_cb = sfb_cb
    res.num_sec = num_sec; res.num_sec_lsf = num_sec_lsf
    res.snf_exists = snf_exists; res.dpcm_snf = dpcm_snf
    res.end = bits.p; res.cfg = cfg
    res.pos_spec = pos_spec; res.pos_scf = pos_scf; res.pos_snf = pos_snf
    return res


def core_spectrum(cfg, data):
    """Long-frame -> natural 2048 MDCT bins. Short/multigroup ->
    deinterleaved: group windows concatenated in win_offset order (the
    ffmpeg layout). Returns 2048-length array of dequantized coeffs."""
    sp = np.zeros(2048)
    for g in range(cfg.num_window_groups):
        row = cfg.sect_sfb_offset[g]
        msfb = len(row) - 1
        for sfb in range(msfb):
            gain = data.sf_gain[g][sfb]
            if gain == 0:
                continue
            a, b = row[sfb], row[sfb + 1]
            for k in range(a, min(b, len(data.quant))):
                q = data.quant[k]
                if q and k < 2048:
                    sp[k] = np.sign(q) * abs(q) ** (4.0 / 3.0) * gain
    return sp
