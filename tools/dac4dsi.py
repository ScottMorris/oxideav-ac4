#!/usr/bin/env python3
"""Parse the AC-4 DSI (`dac4` box) out of an MP4 per ETSI TS 103 190-2
Annex E (E.6 ac4_dsi_v1, E.10 ac4_presentation_v1_dsi, E.11
ac4_substream_group_dsi).

This is the container-level ground truth for what an AC-4 file carries —
presentations, versions, channel modes, substream groups — fully
independent of the frame-TOC parser in the decoder. Built during the
round-403 investigation, where it proved that Tidal "Atmos" AC-4
downloads are channel-coded STEREO IMS (presentation_version 2,
b_pre_virtualized = 1) rather than the 7.1 the TOC parse claimed.

Usage: dac4dsi.py <file.mp4>
"""
import struct
import sys


def main(path):
    data = open(path, 'rb').read()
    i = data.find(b'dac4')
    if i < 0:
        sys.exit("no dac4 box found")
    size = struct.unpack('>I', data[i - 4:i])[0]
    box = data[i + 4:i - 4 + size]
    bits = ''.join(f'{b:08b}' for b in box)
    p = 0

    def u(n):
        nonlocal p
        v = int(bits[p:p + n], 2)
        p += n
        return v

    ver = u(3)
    bsver = u(7)
    print(f"ac4_dsi_version={ver} bitstream_version={bsver}")
    print(f"fs_index={u(1)} frame_rate_index={u(4)}")
    npres = u(9)
    print(f"n_presentations={npres}")
    if bsver > 1:
        if u(1):  # b_program_id
            u(16)
            if u(1):
                p += 16 * 8
    u(2)
    u(32)
    u(32)  # ac4_bitrate_dsi
    if p % 8:
        p += 8 - (p % 8)

    def group():
        out = {'sub_present': u(1), 'hsf': u(1)}
        cc = u(1)
        out['ch_coded'] = cc
        n_sub = u(8)
        subs = []
        for _ in range(n_sub):
            s = {'sf_mult': u(2)}
            if u(1):
                s['br_ind'] = u(5)
            if cc:
                u(6)
                s['ch_groups'] = format(u(18), '018b')
            else:
                s['b_ajoc'] = u(1)
                if s['b_ajoc']:
                    s['static_dmx'] = u(1)
                    if not s['static_dmx']:
                        s['n_dmx-1'] = u(4)
                    s['n_umx-1'] = u(6)
                s['bed'], s['dyn'], s['isf'] = u(1), u(1), u(1)
                u(1)
            subs.append(s)
        out['subs'] = subs
        if u(1):  # b_content_type
            out['classifier'] = u(3)
            if u(1):
                nl = u(6)
                for _ in range(nl):
                    u(8)
        return out

    CHMODES = {0: 'mono', 1: 'stereo', 2: '3.0', 3: '5.0', 4: '5.1',
               5: '7.0 3/4/0', 6: '7.1 3/4/0.1', 7: '7.0 5/2/0',
               8: '7.1 5/2/0.1', 9: '7.0 3/2/2', 10: '7.1 3/2/2.1',
               11: '7.0.4', 12: '7.1.4', 13: '9.0.4', 14: '9.1.4', 15: '22.2'}

    for pres in range(npres):
        pv = u(8)
        plen = u(8)
        q0 = p
        print(f"presentation {pres}: version={pv} ({plen} bytes)")
        pc = u(5)
        print(f"  presentation_config_v1={pc}" + (" (single group)" if pc == 31 else ""))
        u(3)  # md_compat
        if u(1):
            print(f"  presentation_id={u(5)}")
        u(2)
        u(2)
        u(5)
        u(10)
        if u(1):  # channel coded
            chm = u(5)
            print(f"  channel-coded, dsi_presentation_ch_mode={chm} ({CHMODES.get(chm, '?')})")
            if chm in (11, 12, 13, 14):
                print(f"  4back={u(1)} top_pairs={u(2)}")
            u(6)
            print(f"  pres ch_groups mask={format(u(18), '018b')}")
        if u(1):  # core differs
            if u(1):
                print(f"  core ch mode={u(2)}")
        if u(1):  # filter
            u(1)
            nf = u(8)
            for _ in range(nf):
                u(8)
        if pc == 31:
            print(f"  group: {group()}")
        else:
            u(1)  # b_multi_pid
            ng = {0: 2, 1: 2, 2: 2, 3: 3, 4: 3}.get(pc, 0)
            if pc == 5:
                ng = u(3) + 2
            for _ in range(ng):
                print(f"  group: {group()}")
        print(f"  b_pre_virtualized={u(1)}  <- 1 means AC-4 IMS (binaural stereo)")
        if u(1):
            na = u(7)
            print(f"  n_add_emdf_substreams={na}")
        p = q0 + plen * 8


if __name__ == "__main__":
    main(sys.argv[1])
