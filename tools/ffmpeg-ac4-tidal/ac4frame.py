#!/usr/bin/env python3
"""Top-down CORE parser for the ffmpeg-ac4-tidal 7x channel element.

Key fact: in channel_element_7x ALL channel cores (LFE + up to 7) are
parsed BEFORE any aspx_data. So we can recover every core body at its
TRUE position deterministically, no correlation, no aspx parsing.
Returns per-channel (start, end, cfg, data) for IMDCT.

Handles codec_mode SIMPLE/ASPX (not the ACPL variants' extra add-ch
sf_data path, which we detect and bail on). frame_len_base=2048.
"""
import sys
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf
T = A.T

CM_SIMPLE, CM_ASPX, CM_ASPX_ACPL_1, CM_ASPX_ACPL_2 = 0, 1, 2, 3


def _huff_scf(bits):
    return A.huff(bits, T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW'])


def chparam_info(bits, cfg):
    sap_mode = bits.u(2)
    if sap_mode == 1:
        for g in range(cfg.num_window_groups):
            for sfb in range(cfg.get_max_sfb(g)):
                bits.u(1)
    elif sap_mode == 3:
        sap_data(bits, cfg)
    return sap_mode


def sap_data(bits, cfg):
    used = [[0] * 130 for _ in range(cfg.num_window_groups)]
    if not bits.u(1):
        for g in range(cfg.num_window_groups):
            msfb = cfg.get_max_sfb(g)
            for sfb in range(0, msfb, 2):
                u = bits.u(1); used[g][sfb] = u
                if sfb + 1 < msfb:
                    used[g][sfb + 1] = u
    else:
        for g in range(cfg.num_window_groups):
            for sfb in range(cfg.get_max_sfb(g)):
                used[g][sfb] = 1
    if cfg.num_window_groups != 1:
        bits.u(1)   # delta_code_time
    for g in range(cfg.num_window_groups):
        for sfb in range(0, cfg.get_max_sfb(g), 2):
            if used[g][sfb]:
                _huff_scf(bits)


def companding_control(bits, num_chan):
    sync = bits.u(1) if num_chan > 1 else 0
    nc = 1 if sync else num_chan
    need_avg = 0
    for i in range(nc):
        if not bits.u(1):
            need_avg = 1
    if need_avg:
        bits.u(1)


def aspx_config(bits):
    bits.u(1); bits.u(3); bits.u(2); bits.u(1); bits.u(1)
    bits.u(1); bits.u(1); bits.u(2); bits.u(1); bits.u(2)


def acpl_config_1ch(bits, mode):
    bits.u(2); bits.u(1)
    if mode == 'partial':
        bits.u(3)


def _sfd(bits, cfg):
    st = bits.p
    data = ac4asf.parse_sf_data(bits, cfg)
    data.start = st
    return data


def _one(bits, side_limited=0, dual=0, side_ch=0):
    cfg = ac4asf.parse_sf_info(bits, side_limited, dual, side_ch)
    data = _sfd(bits, cfg)
    return cfg, data


def two_channel_data(bits, out, idxs):
    proc = bits.u(1)
    if proc:
        cfg = ac4asf.parse_sf_info(bits, 0, 0, 0)
        chparam_info(bits, cfg)
        d0 = _sfd(bits, cfg)
        d1 = _sfd(bits, cfg)
        out[idxs[0]] = (cfg, d0); out[idxs[1]] = (cfg, d1)
    else:
        c0 = ac4asf.parse_sf_info(bits, 0, 0, 0)
        c1 = ac4asf.parse_sf_info(bits, 0, 0, 0)
        d0 = _sfd(bits, c0)
        d1 = _sfd(bits, c1)
        out[idxs[0]] = (c0, d0); out[idxs[1]] = (c1, d1)


def five_channel_data(bits, out, idxs):
    cfg = ac4asf.parse_sf_info(bits, 0, 0, 0)   # shared config
    bits.u(4)                                    # chel_matsel
    for _ in range(5):
        chparam_info(bits, cfg)
    for j in range(5):
        d = _sfd(bits, cfg)
        out[idxs[j]] = (cfg, d)


def three_channel_data(bits, out, idxs):
    cfg = ac4asf.parse_sf_info(bits, 0, 0, 0)
    bits.u(4)                                    # chel_matsel
    chparam_info(bits, cfg)
    chparam_info(bits, cfg)
    for j in range(3):
        d = _sfd(bits, cfg)
        out[idxs[j]] = (cfg, d)


def four_channel_data(bits, out, idxs):
    cfg = ac4asf.parse_sf_info(bits, 0, 0, 0)
    for _ in range(4):
        chparam_info(bits, cfg)
    for j in range(4):
        d = _sfd(bits, cfg)
        out[idxs[j]] = (cfg, d)


def mono_data(bits, lfe):
    if lfe:
        cfg = ac4asf.parse_sf_info_lfe(bits)
    else:
        bits.u(1)   # spec_frontend
        cfg = ac4asf.parse_sf_info(bits, 0, 0, 0)
    data = ac4asf.parse_sf_data(bits, cfg)
    return cfg, data


def parse_core_7x(d, P, iframe):
    """Parse all cores starting at bit P (audio_data start). Returns
    (out dict ch->(cfg,data), end_pos) or raises."""
    bits = A.Bits(d, P)
    out = {}
    codec_mode = bits.u(2)
    if iframe and codec_mode != CM_SIMPLE:
        aspx_config(bits)
        if codec_mode == CM_ASPX_ACPL_1:
            acpl_config_1ch(bits, 'partial')
        elif codec_mode == CM_ASPX_ACPL_2:
            acpl_config_1ch(bits, 'full')
    # channel_mode 6: LFE first
    out[7] = mono_data(bits, 1)
    if codec_mode in (CM_ASPX_ACPL_1, CM_ASPX_ACPL_2):
        companding_control(bits, 5)
    coding_config = bits.u(2)
    if coding_config == 0:
        bits.u(1)   # mode_2ch
        two_channel_data(bits, out, (0, 1))
        two_channel_data(bits, out, (2, 3))
    elif coding_config == 1:
        three_channel_data(bits, out, (0, 1, 2))
        two_channel_data(bits, out, (3, 4))
    elif coding_config == 2:
        four_channel_data(bits, out, (0, 1, 2, 3))
    elif coding_config == 3:
        five_channel_data(bits, out, (0, 1, 2, 3, 4))
    # add channels (5,6) for SIMPLE/ASPX
    if codec_mode in (CM_SIMPLE, CM_ASPX):
        if bits.u(1):   # b_use_sap_add_ch
            # chparam for ch5,ch6 -- need their cfg; they come from the
            # add pair's sf_info, not yet parsed. In ffmpeg these use
            # ssch[5/6].scp which is stale here; skip via two_channel_data
            # (the pair below reparses). But chparam consumes bits using
            # a PRIOR cfg -> we lack it. Bail if this bit is set.
            raise ValueError('b_use_sap_add_ch set (needs prior cfg)')
        two_channel_data(bits, out, (5, 6))
    elif codec_mode == CM_ASPX_ACPL_1:
        bits.u(5)   # max_sfb_master
        # chparam_info(ch5), chparam_info(ch6) using stale cfg -> bail
        raise ValueError('ACPL_1 add-ch path')
    if coding_config in (0, 2):
        out[4] = mono_data(bits, 0)
    return out, bits.p, codec_mode, coding_config
