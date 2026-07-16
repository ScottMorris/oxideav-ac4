#!/usr/bin/env python3
"""Top-down parser for TS 103 190-2 immersive_channel_element (the TRUE
grammar of Tidal v2 immersive content), pre-ASPX portion:

  immersive_codec_mode_code (1|3 bits)
  iframe: immers_cfg = [aspx_config 15b if mode!=SCPL]
                       [acpl_config partial 6b if ACPL_1 / full 3b if ACPL_2]
  mono_data(LFE)
  AJCC: companding_control(5)
  core_5ch_grouping (2b):
    0: [1b 2ch_mode] two_channel_data x2, mono_data(0)   (1+2+2)
    1: three_channel_data, two_channel_data              (3+2)
    2: four_channel_data, mono_data(0)                   (1+4)
    3: five_channel_data                                 (5)
  7CH_STATIC: [1b b_use_sap_add_ch (+2 chparam if set)] two_channel_data

Returns all core bodies (channels 0..6 + LFE=7) with positions.
"""
import sys
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf
import ac4frame

MODES = ['SCPL', 'ASPX_SCPL', 'ASPX_ACPL_1', 'ASPX_ACPL_2', 'ASPX_AJCC']


def parse_immersive_core(d, P, iframe, b_lfe=1):
    bits = A.Bits(d, P)
    out = {}
    if bits.u(1):
        mode = 4  # ASPX_AJCC
    else:
        mode = bits.u(2)
    if iframe:
        if mode != 0:
            ac4frame.aspx_config(bits)
        if mode == 2:
            ac4frame.acpl_config_1ch(bits, 'partial')
        elif mode == 3:
            ac4frame.acpl_config_1ch(bits, 'full')
    if b_lfe:
        out[7] = ac4frame.mono_data(bits, 1)
    if mode == 4:
        ac4frame.companding_control(bits, 5)
    grouping = bits.u(2)
    if grouping == 0:
        bits.u(1)  # 2ch_mode
        ac4frame.two_channel_data(bits, out, (0, 1))
        ac4frame.two_channel_data(bits, out, (2, 3))
        out[4] = ac4frame.mono_data(bits, 0)
    elif grouping == 1:
        ac4frame.three_channel_data(bits, out, (0, 1, 2))
        ac4frame.two_channel_data(bits, out, (3, 4))
    elif grouping == 2:
        ac4frame.four_channel_data(bits, out, (0, 1, 2, 3))
        out[4] = ac4frame.mono_data(bits, 0)
    else:
        ac4frame.five_channel_data(bits, out, (0, 1, 2, 3, 4))
    add_used = None
    if mode != 4:  # 7CH_STATIC
        add_used = bits.u(1)
        if add_used:
            raise ValueError('b_use_sap_add_ch set')
        ac4frame.two_channel_data(bits, out, (5, 6))
    return out, bits.p, mode, grouping
