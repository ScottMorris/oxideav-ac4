#!/usr/bin/env python3
"""Short-transform body synthesis (FULL-CORE campaign).

Parses short/multigroup bodies with ac4asf (parse_sf_info long_frame=0
+ parse_sf_data), deinterleaves the grouped spectral lines (AAC-style
[group][sfb][window][bin] layout implied by sect_sfb_offset =
group_offset + sfb_offset*nwg), IMDCTs each window at its transform
length (KBD alpha per ac4dec_data: 128:6, 256:5, 512:4.5, 1024:4),
and OLAs the window sequence into a 4096-sample block whose placement
offset is chosen by best correlation vs the M oracle (documented
reference assist).

Supports uniform window splits (idx0 == idx1). different_framing
bodies are skipped.
"""
import sys
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf
from numpy import i0

KBD_ALPHA = {128: 6.0, 256: 5.0, 512: 4.5, 1024: 4.0, 2048: 3.0}
_cache = {}


def kbd_win(Nw):
    if Nw in _cache:
        return _cache[Nw]
    a = KBD_ALPHA[Nw]
    xg = np.arange(Nw + 1) / Nw
    kern = i0(np.pi * a * np.sqrt(np.clip(1 - (2 * xg - 1) ** 2, 0, 1)))
    cs = np.cumsum(kern[:Nw])
    h = np.sqrt(cs / cs[-1])
    w = np.concatenate([h, h[::-1]])
    _cache[Nw] = w
    return w


_bcache = {}


def basis(Nw):
    if Nw in _bcache:
        return _bcache[Nw]
    n = np.arange(2 * Nw); k = np.arange(Nw)
    B = np.cos(np.pi / Nw * (n[:, None] + 0.5 + Nw / 2) * (k[None, :] + 0.5))
    _bcache[Nw] = B
    return B


def parse_short(d, P):
    """parse a short body; return (cfg, data) or None."""
    bits = A.Bits(d, P)
    cfg = ac4asf.parse_sf_info(bits)
    if cfg.long_frame != 0:
        return None
    if cfg.different_framing:
        return None
    data = ac4asf.parse_sf_data(bits, cfg)
    return cfg, data


def synth_short(cfg, data):
    """returns time signal of the window sequence (num_windows*Nw + Nw
    samples) via per-window IMDCT + OLA at hop Nw."""
    Nw = cfg.tl0
    W = cfg.num_windows
    B = basis(Nw); KW = kbd_win(Nw)
    total = W * Nw + Nw
    out = np.zeros(total)
    win_global = 0
    for g in range(cfg.num_window_groups):
        nwg = cfg.num_win_in_group[g]
        row = cfg.sect_sfb_offset[g]
        msfb = len(row) - 1
        # per-window spectra for this group
        sps = np.zeros((nwg, Nw))
        sfbo = ac4asf.SFB_OFF[Nw]
        for sfb in range(msfb):
            gain = data.sf_gain[g][sfb]
            if gain == 0:
                continue
            a, b = row[sfb], row[sfb + 1]
            bw = sfbo[sfb + 1] - sfbo[sfb]
            for wnd in range(nwg):
                seg = data.quant[a + wnd * bw: a + (wnd + 1) * bw]
                for j, q in enumerate(seg):
                    if q and sfbo[sfb] + j < Nw:
                        sps[wnd, sfbo[sfb] + j] = np.sign(q) * abs(q) ** (4.0 / 3.0) * gain
        for wnd in range(nwg):
            pcm = (B @ sps[wnd]) * KW
            st = (win_global + wnd) * Nw
            out[st:st + 2 * Nw] += pcm
        win_global += nwg
    return out


def short_block(cfg, data, offset):
    """place the window-sequence signal into a 4096 block at offset."""
    sig = synth_short(cfg, data)
    blk = np.zeros(4096)
    a = max(0, offset)
    b = min(4096, offset + len(sig))
    if b > a:
        blk[a:b] = sig[a - offset: b - offset]
    return blk
