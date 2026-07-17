#!/usr/bin/env python3
"""R516: race the glue variants on ims content frames (32..36+).

Anchor = end of the silent LFE (same anatomy as Tidal). At that bit,
try candidate grammars for what follows and apply the survivor test:
  - full parse, no exception
  - sane geometry: msfb <= num_sfb_48(tl) for every window group
    (36 @512-short, 63 @2048-long)
  - spectra in range and NONZERO (these are content frames)
  - end lands inside the frame leaving sane room for aspx/acpl tail

Variants:
  2ch      : fork two_channel_data (IMS-native stereo pair)
  3ch      : fork three_channel_data (the cc='01' 3+2 reading)
  mono     : single spec_frontend mono_data
  v2w3/v2w5: harvest flat v2 body chain (msfb5+sections w=3/5)
Sub-variants: pre-skip h=0..4 bits; msfb width default / +1 bit.
"""
import sys
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf
import ac4frame

T = A.T
SFB = A.SFB_2048


def frames_of(path):
    d = open(path, 'rb').read()
    out = []
    p = 0
    while p < len(d) - 4:
        sync = int.from_bytes(d[p:p + 2], 'big')
        if sync not in (0xAC40, 0xAC41):
            break
        sz = int.from_bytes(d[p + 2:p + 4], 'big')
        hdr = 4
        if sz == 0xFFFF:
            sz = int.from_bytes(d[p + 4:p + 7], 'big')
            hdr = 7
        out.append(d[p + hdr:p + hdr + sz])
        p += hdr + sz + (2 if sync == 0xAC41 else 0)
    return out


def silent_lfe_at(d, p):
    """parse LFE at bit p; return end if it is the silent shape."""
    try:
        b = A.Bits(d, p)
        cfg = ac4asf.parse_sf_info_lfe(b)
        if cfg.max_sfb[0] < 1 or cfg.max_sfb[0] > 4:
            return None
        data = ac4asf.parse_sf_data(b, cfg)
    except Exception:
        return None
    if data.snf_exists:
        return None
    if int(np.abs(data.quant).max(initial=0)) != 0:
        return None
    return b.p, cfg.max_sfb[0], data


def anchors(d, lo=900, hi=1400):
    """candidate LFE anchors (start, end, msfb, ref_sf)."""
    out = []
    nbits = len(d) * 8
    for p in range(lo, min(hi, nbits - 40)):
        r = silent_lfe_at(d, p)
        if r is None:
            continue
        end, msfb, data = r
        # ref_sf is the raw u(8) after spectra; recover it
        out.append((p, end, msfb))
    return out


def sane_geom(cfg):
    for g in range(cfg.num_window_groups):
        tl, _ = cfg.get_transf_length(g)
        if cfg.get_max_sfb(g) > ac4asf.num_sfb_48(tl):
            return False
    if cfg.total_lines > 4096:
        return False
    return True


def summarize(pairs):
    """pairs: list of (cfg, data). Return None if insane, else info."""
    nz = 0
    geoms = []
    for cfg, data in pairs:
        if not sane_geom(cfg):
            return None
        if int(np.abs(data.quant).max(initial=0)) > 20000:
            return None
        nz += int((data.quant != 0).sum())
        g0 = cfg.get_transf_length(0)[0]
        geoms.append((cfg.long_frame, g0, cfg.get_max_sfb(0),
                      cfg.num_window_groups))
    return dict(nz=nz, geoms=geoms)


def try_2ch(d, p):
    b = A.Bits(d, p)
    out = {}
    ac4frame.two_channel_data(b, out, (0, 1))
    return [out[0], out[1]], b.p


def try_3ch(d, p):
    b = A.Bits(d, p)
    out = {}
    ac4frame.three_channel_data(b, out, (0, 1, 2))
    return [out[0], out[1], out[2]], b.p


def try_mono(d, p):
    b = A.Bits(d, p)
    r = ac4frame.mono_data(b, 0)
    return [r], b.p


def v2_body(b, width):
    m = b.u(5)
    if m > 56 or m < 1:
        raise ValueError('msfb')
    esc = (1 << width) - 1
    sects = []
    k = 0
    while k < m:
        cb = b.u(4)
        ln = 1
        li = b.u(width)
        while li == esc:
            ln += esc
            li = b.u(width)
        ln += li
        if k + ln > 127 or len(sects) > 24:
            raise ValueError('sect')
        sects.append((k, k + ln, cb))
        k += ln
    mqi = [0] * m
    q = np.zeros(SFB[m])
    for (s, e, cb) in sects:
        if cb == 0 or cb > 11 or e > m:
            continue
        q2, mq2 = A.parse_spectra(b, [(s, e, cb)], SFB, m)
        for xx in range(s, e):
            mqi[xx] = max(mqi[xx], mq2[xx])
        for i2 in range(SFB[s], SFB[e]):
            q[i2] = q2[i2]
    ref_sf = b.u(8)
    sfb_cb = [0] * m
    for (s, e, cb) in sects:
        for xx in range(s, min(e, m)):
            sfb_cb[xx] = cb
    sf = ref_sf
    first = False
    lens, cws = T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']
    for sfb in range(m):
        if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
            continue
        if first:
            sf += A.huff(b, lens, cws) - 60
            if not (0 <= sf <= 255):
                raise ValueError('sf range')
        else:
            first = True
    if b.u(1):
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for sfb in range(m):
            if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
                A.huff(b, l2, c2)
    nz = int((q != 0).sum())
    if np.abs(q).max(initial=0) > 20000:
        raise ValueError('q range')
    return dict(m=m, nz=nz, nsect=len(sects), end=b.p)


def try_v2chain(d, p, width, nbodies=2):
    b = A.Bits(d, p)
    bodies = []
    for _ in range(nbodies):
        bodies.append(v2_body(b, width))
    return bodies, b.p


MSFB_ORIG = ac4asf.get_msfb_bits


def msfb_plus1(tl):
    return MSFB_ORIG(tl) + 1


def main():
    frames = frames_of('ac4-ims.ac4')
    targets = [int(x) for x in (sys.argv[1:] or
                                ['32', '33', '34', '35', '36'])]
    for fi in targets:
        d = frames[fi]
        nbits = len(d) * 8
        anc = anchors(d)
        # keep anchors whose START is not inside another anchor's parse
        print(f'\n== frame {fi} ({len(d)}B, {nbits} bits): '
              f'{len(anc)} LFE anchors ==')
        for (s, e, m) in anc[:24]:
            print(f'  anchor start={s} end={e} msfb={m}')
        for (s, E, m) in anc:
            for h in range(0, 5):
                p = E + h
                # fork-grammar variants, msfb width default and +1
                for wname, wfn in (('mw5', MSFB_ORIG), ('mw6', msfb_plus1)):
                    ac4asf.get_msfb_bits = wfn
                    for vname, fn in (('2ch', try_2ch), ('3ch', try_3ch),
                                      ('mono', try_mono)):
                        try:
                            pairs, end = fn(d, p)
                        except Exception:
                            continue
                        info = summarize(pairs)
                        if info is None or info['nz'] == 0:
                            continue
                        room = nbits - end
                        if room < 0 or room > 3000:
                            continue
                        print(f'  E={E}+{h} {vname}/{wname}: nz={info["nz"]}'
                              f' geoms={info["geoms"]} end={end} '
                              f'room={room}')
                    ac4asf.get_msfb_bits = MSFB_ORIG
                # v2 flat chain
                for width in (3, 5):
                    for nb in (1, 2, 3):
                        try:
                            bodies, end = try_v2chain(d, p, width, nb)
                        except Exception:
                            continue
                        nz = sum(x['nz'] for x in bodies)
                        if nz == 0:
                            continue
                        room = nbits - end
                        if room < 0 or room > 3000:
                            continue
                        gs = [(x['m'], x['nz']) for x in bodies]
                        print(f'  E={E}+{h} v2w{width}x{nb}: bodies={gs} '
                              f'end={end} room={room}')


if __name__ == '__main__':
    main()
