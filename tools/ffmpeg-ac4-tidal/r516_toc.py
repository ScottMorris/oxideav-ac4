#!/usr/bin/env python3
"""R516: v2 ac4_toc parser, corrected from r511_toc.py.

Fix vs r511: emdf_info() ends with emdf_reserved() = [2b primary]
[2b secondary][8*n_skip bytes] (TS 103 190-1 table 80) — r511 omitted
it inside presentations (the 40-bit field R511's entropy map found on
Tidal). Validation targets (from the fork, single-frame packets):
  ims f32: TOC ends <=23B, payload_base=55, sus sizes [766, 22],
           presentation substream index 1.
Self-check on Tidal mdat chaining must still pass (regression).
"""
import sys

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


def emdf_info(b, out=None):
    ev = b.u(2)
    if ev == 3:
        ev += b.vb(2)
    k = b.u(3)
    if k == 7:
        k += b.vb(3)
    if b.u(1):                       # b_emdf_payloads_substream_info
        si = b.u(2)
        if si == 3:
            si += b.vb(2)
    # emdf_reserved (aka emdf_protection): THE r511 gap
    p1 = b.u(2); p2 = b.u(2)
    nskip = 0
    if p1 > 0:
        nskip += 1 << (2 * (p1 - 1))
    if p2 > 0:
        nskip += 1 << (2 * (p2 - 1))
    b.u(8 * nskip)
    if out is not None:
        out['emdf'] = (ev, k, p1, p2, nskip)


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
    groups_seen = []
    assert ver >= 2
    if b.u(1):                       # b_program_id
        b.u(16)
        if b.u(1):
            for _ in range(16):
                b.u(8)
    frf = 1
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
                pass
            if fri in (10, 11, 12):
                if b.u(1):
                    b.u(1)
            emdf_info(b, p)
            if b.u(1):               # b_presentation_filter
                b.u(1)
            if ssg == 1:
                gi = b.u(3)
                if gi == 7:
                    gi += b.vb(2)
                p['groups'] = [gi]
                if gi not in groups_seen:
                    groups_seen.append(gi)
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
                        if gi not in groups_seen:
                            groups_seen.append(gi)
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
                emdf_info(b)
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
                if pat in ('11111100', '11111101', '111111100',
                           '111111101'):
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
                    st = b.u(1)
                    gr['b_static_dmx'] = st
                    if not st:
                        raise NotImplementedError('dyn dmx')
                    if b.u(1):
                        raise NotImplementedError('oamd common')
                    nu = b.u(4) + 1
                    if nu == 16:
                        nu += b.vb(3)
                    gr['n_umx'] = nu
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
            gr['content'] = b.u(3)   # content_classifier
            if b.u(1):               # b_language_indicator
                nl = b.u(6)
                for _ in range(nl):
                    b.u(8)
        out.setdefault('groups', []).append(gr)
    # substream_index_table: n_substreams is ALWAYS an explicit field
    # (r511 wrongly derived it from max substream index -> 2-bit desync)
    n_sus = b.u(2)
    if n_sus == 0:
        n_sus = b.vb(2) + 4
    out['n_substreams'] = n_sus
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
    import r516_glue as G
    frames = G.frames_of('ac4-ims.ac4')
    which = [int(x) for x in sys.argv[1:]] or [0, 1, 30, 31, 32, 33, 34,
                                               35, 36]
    for fi in which:
        d = frames[fi]
        try:
            t = parse_toc(d)
        except Exception as e:
            print(f'f{fi}: FAIL {e!r}')
            continue
        print(f'f{fi} len={len(d)}: ver={t["version"]} seq={t["seq"]} '
              f'ifg={t["b_iframe_global"]} npres={t["n_presentations"]} '
              f'pb={t.get("payload_base")} tocB={t["toc_bytes"]} '
              f'sizes={t["sus_sizes"]}')
        for p in t.get('pres', []):
            print(f'   pres: {p}')
        for g in t.get('groups', []):
            print(f'   group: {g}')
