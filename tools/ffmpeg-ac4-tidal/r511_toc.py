#!/usr/bin/env python3
"""R511: hand-parse the v2 ac4_toc from the mdat (spec part2 6.2.1.x).

Frames are back-to-back in mdat starting at the first TOC. Self-check:
TOC's substream_index_table sizes must chain to the next frame's TOC.
Settles channel_mode / b_channel_coded / b_ajoc for real.
"""
import sys

MP4 = ('/home/scott/Music/riptide/Kraftwerk - Radio-Activity (2009 Remaster)'
       ' (1975)/Atmos-AC4/02 - Radioactivity (2009 Remaster).mp4')
START = 67701          # first TOC byte (verified: sub0000 at 67730 - 29)

CH_MODES = [
    ('0', 'mono'), ('10', 'stereo'), ('1100', '3.0'), ('1101', '5.0'),
    ('1110', '5.1'), ('1111000', '7.0:3/4/0'), ('1111001', '7.1:3/4/0.1'),
    ('1111010', '7.0:5/2/0'), ('1111011', '7.1:5/2/0.1'),
    ('1111100', '7.0:3/2/2'), ('1111101', '7.1:3/2/2.1'),
    ('11111100', '7.0.4'), ('11111101', '7.1.4'),
    ('111111100', '9.0.4'), ('111111101', '9.1.4'),
    ('111111110', '22.2'),
]


class B:
    def __init__(self, d, p=0):
        self.d = d; self.p = p
    def u(self, n):
        v = 0
        for _ in range(n):
            v = (v << 1) | ((self.d[self.p >> 3] >> (7 - (self.p & 7))) & 1)
            self.p += 1
        return v
    def vb(self, n):
        v = 0
        while True:
            v += self.u(n)
            if not self.u(1):
                return v
            v <<= n
            v += 1 << n


def read_channel_mode(b):
    s = ''
    for _ in range(9):
        s += str(b.u(1))
        for pat, name in CH_MODES:
            if s == pat:
                return name, s
    return f'reserved({s})', s


def parse_toc(d, verbose=False):
    b = B(d)
    out = {}
    ver = b.u(2)
    if ver == 3:
        ver += b.vb(2)
    out['version'] = ver
    out['seq'] = b.u(10)
    if b.u(1):
        wf = b.u(3)
        if wf > 0:
            b.u(2)
        out['wait_frames'] = wf
    out['fs_index'] = b.u(1)
    fri = b.u(4)
    out['frame_rate_index'] = fri
    out['b_iframe_global'] = b.u(1)
    single = b.u(1)
    if single:
        n_pres = 1
    else:
        n_pres = b.vb(2) + 2 if b.u(1) else 0
    out['n_presentations'] = n_pres
    if b.u(1):                       # b_payload_base
        pb = b.u(5) + 1
        if pb == 0x20:
            pb += b.vb(3)
        out['payload_base'] = pb
    groups_seen = set()
    assert ver >= 2
    if b.u(1):                       # b_program_id
        b.u(16)
        if b.u(1):
            for _ in range(16):
                b.u(8)
    frf = 1                          # frame_rate_factor (multiplier default)
    for i in range(n_pres):
        p = {}
        ssg = b.u(1)                 # b_single_substream_group
        if ssg != 1:
            pc = b.u(3)
            if pc == 7:
                pc += b.vb(2)
        else:
            pc = -1
        p['config'] = pc
        v = 0
        while b.u(1):                # presentation_version unary
            v += 1
        p['pver'] = v
        if not (ssg != 1 and pc == 6):
            p['md_compat'] = b.u(3)
            if b.u(1):
                p['pres_id'] = b.vb(2)
            # frame_rate_multiply_info
            if fri in (2, 3, 4):
                if b.u(1):
                    b.u(1)
            elif fri in (0, 1, 7, 8, 9):
                b.u(1)
            # frame_rate_fractions_info
            if fri in (5, 6, 7, 8, 9):
                pass                 # needs frame_rate_factor==1: read 1
            if fri in (10, 11, 12):
                if b.u(1):
                    b.u(1)
            # emdf_info
            ev = b.u(2)
            if ev == 3:
                ev += b.vb(2)
            k = b.u(3)
            if k == 7:
                k += b.vb(3)
            if b.u(1):               # b_emdf_payloads_substream_info
                si = b.u(2)
                if si == 3:
                    si += b.vb(2)
            if b.u(1):               # b_presentation_filter
                b.u(1)
            if ssg == 1:
                gi = b.u(3)
                if gi == 7:
                    gi += b.vb(2)
                p['groups'] = [gi]
                groups_seen.add(gi)
            else:
                b.u(1)               # b_multi_pid
                ngr = {0: 2, 1: 2, 2: 2, 3: 3, 4: 3}.get(pc)
                gis = []
                if pc == 5:
                    ng = b.u(2) + 2
                    if ng == 5:
                        ng += b.vb(2)
                    ngr = ng
                if ngr is None:      # config >= 6: ext info
                    nsk = b.u(5)
                    if b.u(1):
                        nsk += b.vb(2) << 5
                    for _ in range(nsk):
                        b.u(8)
                else:
                    for _ in range(ngr):
                        gi = b.u(3)
                        if gi == 7:
                            gi += b.vb(2)
                        gis.append(gi)
                        groups_seen.add(gi)
                p['groups'] = gis
            p['b_pre_virtualized'] = b.u(1)
            add_emdf = b.u(1)
            # ac4_presentation_substream_info
            p['b_alternative'] = b.u(1)
            p['b_pres_ndot'] = b.u(1)
            si = b.u(2)
            if si == 3:
                si += b.vb(2)
            p['pres_sus_idx'] = si
        else:
            add_emdf = 1
        if add_emdf:
            na = b.u(2)
            if na == 0:
                na = b.vb(2) + 4
            for _ in range(na):
                ev = b.u(2)
                if ev == 3:
                    ev += b.vb(2)
                k = b.u(3)
                if k == 7:
                    k += b.vb(3)
                if b.u(1):
                    si = b.u(2)
                    if si == 3:
                        si += b.vb(2)
        out.setdefault('pres', []).append(p)
    # substream groups
    n_groups = len(groups_seen)
    out['n_groups'] = n_groups
    max_sus = -1
    for g in range(n_groups):
        gr = {}
        sp = b.u(1)                  # b_substreams_present
        hsf = b.u(1)
        sss = b.u(1)                 # b_single_substream
        if sss:
            nsub = 1
        else:
            nsub = b.u(2) + 2
            if nsub == 5:
                nsub += b.vb(2)
        gr['n_lf_substreams'] = nsub
        cc = b.u(1)                  # b_channel_coded
        gr['b_channel_coded'] = cc
        if cc:
            for _ in range(nsub):
                nm, pat = read_channel_mode(b)
                gr.setdefault('channel_modes', []).append(nm)
                if pat in ('11111100', '11111101', '111111100', '111111101'):
                    b.u(1); b.u(1); b.u(2)
                if out['fs_index'] == 1:
                    if b.u(1):
                        b.u(1)
                if b.u(1):           # b_bitrate_info
                    v0 = b.u(3)
                    if v0 in (3, 7):
                        b.u(2)
                if pat in ('1111010', '1111011', '1111100', '1111101'):
                    b.u(1)
                for _ in range(frf):
                    gr.setdefault('ndot', []).append(b.u(1))
                if sp == 1:
                    si = b.u(2)
                    if si == 3:
                        si += b.vb(2)
                    gr.setdefault('sus_idx', []).append(si)
                    max_sus = max(max_sus, si)
                if hsf:
                    if sp == 1:
                        si = b.u(2)
                        if si == 3:
                            si += b.vb(2)
        else:
            gr['b_oamd'] = b.u(1)
            if gr['b_oamd']:
                b.u(1)
                if sp == 1:
                    si = b.u(2)
                    if si == 3:
                        si += b.vb(2)
                    max_sus = max(max_sus, si)
            for _ in range(nsub):
                aj = b.u(1)
                gr.setdefault('b_ajoc', []).append(aj)
                if aj:
                    gr['b_lfe'] = b.u(1)
                    st = b.u(1)      # b_static_dmx
                    gr['b_static_dmx'] = st
                    if not st:
                        nd = b.u(4) + 1
                        gr['n_dmx'] = nd
                        # bed_dyn_obj_assignment(nd) -- complex; bail
                        raise NotImplementedError('dyn dmx')
                    if b.u(1):       # oamd_common_data_present
                        raise NotImplementedError('oamd common')
                    nu = b.u(4) + 1
                    if nu == 16:
                        nu += b.vb(3)
                    gr['n_umx'] = nu
                    # bed_dyn_obj_assignment(nu):
                    if b.u(1):       # b_dyn_objects_only
                        pass
                    else:
                        raise NotImplementedError('bed assignment')
                    if out['fs_index'] == 1:
                        if b.u(1):
                            b.u(1)
                    if b.u(1):
                        v0 = b.u(3)
                        if v0 in (3, 7):
                            b.u(2)
                    for _ in range(frf):
                        b.u(1)
                    if sp == 1:
                        si = b.u(2)
                        if si == 3:
                            si += b.vb(2)
                        gr.setdefault('sus_idx', []).append(si)
                        max_sus = max(max_sus, si)
                else:
                    raise NotImplementedError('objs substream')
        if b.u(1):                   # b_content_type
            cl = b.u(4)
            if b.u(1):               # b_language_indicator etc (approx)
                pass
        out.setdefault('groups', []).append(gr)
    # substream_index_table
    n_sus = max_sus + 1 if max_sus >= 0 else 0
    # per Table 14: n_substreams from TOC context; if 0 -> 2 bits + vb
    if n_sus == 0:
        n_sus = b.u(2)
        if n_sus == 0:
            n_sus = b.vb(2) + 4
    sizes = []
    if n_sus == 1:
        size_present = b.u(1)
    else:
        size_present = 1
    if size_present:
        for _ in range(n_sus):
            more = b.u(1)
            sz = b.u(10)
            if more:
                sz += b.vb(2) << 10
            sizes.append(sz)
    out['sus_sizes'] = sizes
    out['toc_end_bits'] = b.p
    out['toc_bytes'] = (b.p + 7) // 8
    return out


if __name__ == '__main__':
    d = open(MP4, 'rb').read()
    pos = START
    ok = 0; fail = 0
    from collections import Counter
    stats = Counter()
    for i in range(int(sys.argv[1]) if len(sys.argv) > 1 else 8):
        try:
            t = parse_toc(d[pos:pos + 4096])
        except Exception as e:
            print(f'frame {i} at {pos}: PARSE FAIL {e!r}')
            break
        total = t['toc_bytes'] + sum(t['sus_sizes'])
        if i < 3:
            print(f'frame {i} @ {pos}: {t}')
        # verify chain: next TOC should parse with version==2
        stats[('ver', t['version'])] += 1
        for g in t.get('groups', []):
            stats[('cc', g.get('b_channel_coded'))] += 1
            for cm in g.get('channel_modes', []):
                stats[('chmode', cm)] += 1
            for aj in g.get('b_ajoc', []) or []:
                stats[('ajoc', aj)] += 1
        pos += total
        ok += 1
    print(f'\nchained {ok} frames; stats: {dict(stats)}')
