#!/usr/bin/env python3
"""Spec-exact ASF body parser + alignment scanner for AC-4 substreams.

Implements (from ETSI TS 103 190-1 V1.4.1): asf_section_data (Table 39),
asf_spectral_data (Table 40, incl. sign-then-ext order and Pseudocode 20),
asf_scalefac_data (Table 41 / Pseudocode 21), asf_snf_data (Table 42 /
Pseudocode 23 gating), sf_info_lfe, and enough of sf_info/chparam to walk a
long-frame five_channel_data. Used to brute-force where elements really
start in a captured substream, with strict validity checks the production
try-and-bail decoder doesn't apply.
"""
import re
import sys

C1 = '/home/scott/Documents/ac4-spec/part1_pkg/ts_103190_tables.c'


def load_tables():
    src = open(C1).read()
    t = {}
    for m in re.finditer(r'const\s+\w+\s+(\w+)\[(\d+)\]\s*=\s*\{([^}]*)\}', src, re.S):
        name, n, body = m.group(1), int(m.group(2)), m.group(3)
        try:
            vals = [int(v, 0) for v in re.findall(r'-?0[xX][0-9a-fA-F]+|-?\d+', body)]
        except ValueError:
            continue
        if len(vals) == n:
            t[name] = vals
    return t


T = load_tables()
CB_DIM = {1: 4, 2: 4, 3: 4, 4: 4, 5: 2, 6: 2, 7: 2, 8: 2, 9: 2, 10: 2, 11: 2}
UNSIGNED = {1: False, 2: False, 3: True, 4: True, 5: False, 6: False,
            7: True, 8: True, 9: True, 10: True, 11: True}
CB_MOD = {1: 3, 2: 3, 3: 3, 4: 3, 5: 9, 6: 9, 7: 8, 8: 8, 9: 13, 10: 13, 11: 17}
CB_OFF = {1: 1, 2: 1, 3: 0, 4: 0, 5: 4, 6: 4, 7: 0, 8: 0, 9: 0, 10: 0, 11: 0}

SFB_2048 = [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 52, 60, 68, 76, 84, 92, 100,
            108, 116, 124, 136, 148, 160, 172, 188, 204, 220, 240, 260, 284, 308, 336,
            364, 396, 432, 468, 508, 552, 600, 652, 704, 768, 832, 896, 960, 1024,
            1088, 1152, 1216, 1280, 1344, 1408, 1472, 1536, 1600, 1664, 1728, 1792,
            1856, 1920, 1984, 2048]
NUM_SFB = {2048: 63}


class Bits:
    def __init__(self, data, bitpos=0):
        self.d = data
        self.p = bitpos

    def u(self, n):
        v = 0
        for _ in range(n):
            if self.p >= len(self.d) * 8:
                raise EOFError
            byte = self.d[self.p >> 3]
            v = (v << 1) | ((byte >> (7 - (self.p & 7))) & 1)
            self.p += 1
        return v


def huff(bits, lens, cws):
    acc = 0
    n = 0
    maxlen = max(lens)
    while n <= maxlen:
        acc = (acc << 1) | bits.u(1)
        n += 1
        for i, (l, c) in enumerate(zip(lens, cws)):
            if l == n and c == acc:
                return i
    raise ValueError("no huffman match")


def parse_sections(bits, width, max_sfb, strict=True):
    esc = (1 << width) - 1
    sects = []
    k = 0
    while k < max_sfb:
        cb = bits.u(4)
        if strict and cb > 11:
            raise ValueError(f"cb {cb} invalid")
        slen = 1
        while True:
            incr = bits.u(width)
            slen += incr
            if incr != esc:
                break
        if strict and k + slen > max_sfb:
            raise ValueError(f"section overrun {k}+{slen}>{max_sfb}")
        sects.append((k, min(k + slen, max_sfb), cb))
        k += slen
    return sects


def parse_spectra(bits, sects, sfbo, max_sfb):
    end = sfbo[max_sfb]
    q = [0] * end
    for (s, e, cb) in sects:
        if cb == 0 or cb > 11:
            continue
        lens = T[f'ASF_HCB_{cb}_LEN']
        cws = T[f'ASF_HCB_{cb}_CW']
        dim, unsig = CB_DIM[cb], UNSIGNED[cb]
        mod, off = CB_MOD[cb], CB_OFF[cb]
        k = sfbo[s]
        while k < sfbo[e]:
            idx = huff(bits, lens, cws)
            if dim == 4:
                vals = []
                rem = idx
                for m in (mod ** 3, mod ** 2, mod):
                    v = rem // m - off
                    rem -= (v + off) * m
                    vals.append(v)
                vals.append(rem - off)
            else:
                v1 = idx // mod - off
                v2 = idx - (v1 + off) * mod
                vals = [v1, v2 - off if off else v2]
                if off:
                    vals = [v1, (idx - (v1 + off) * mod) - off]
            if unsig:
                signs = []
                for v in vals:
                    signs.append(bits.u(1) if v != 0 else 0)
                vals = [-v if sgn else v for v, sgn in zip(vals, signs)]
            if cb == 11:
                out = []
                for v in vals:
                    if abs(v) == 16:
                        n_ext = 0
                        while bits.u(1):
                            n_ext += 1
                        ext = bits.u(n_ext + 4) + (1 << (n_ext + 4))
                        out.append(-ext if v < 0 else ext)
                    else:
                        out.append(v)
                vals = out
            for t_i, v in enumerate(vals):
                if k + t_i < end:
                    q[k + t_i] = v
            k += dim
    mqi = [max((abs(v) for v in q[sfbo[i]:sfbo[i + 1]]), default=0)
           for i in range(max_sfb)]
    return q, mqi


def parse_scalefac(bits, sects, mqi, max_sfb, strict=True):
    ref = bits.u(8)
    sfb_cb = [0] * max_sfb
    for (s, e, cb) in sects:
        for i in range(s, e):
            sfb_cb[i] = cb
    sf = ref
    first = False
    lens, cws = T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']
    for sfb in range(max_sfb):
        if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
            continue
        if first:
            idx = huff(bits, lens, cws)
            sf += idx - 60
            if strict and not (0 <= sf <= 255):
                raise ValueError(f"sf {sf} out of range")
        else:
            first = True
    return ref


def parse_snf(bits, sects, mqi, max_sfb):
    if not bits.u(1):
        return
    sfb_cb = [0] * max_sfb
    for (s, e, cb) in sects:
        for i in range(s, e):
            sfb_cb[i] = cb
    lens, cws = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
    for sfb in range(max_sfb):
        if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
            huff(bits, lens, cws)


def parse_long_body(bits, width, max_sfb, sfbo, strict=True):
    sects = parse_sections(bits, width, max_sfb, strict)
    q, mqi = parse_spectra(bits, sects, sfbo, max_sfb)
    parse_scalefac(bits, sects, mqi, max_sfb, strict)
    parse_snf(bits, sects, mqi, max_sfb)
    nz = sum(1 for v in q if v)
    return sects, nz


def try_lfe(data, start_bit, width):
    bits = Bits(data, start_bit)
    max_sfb = bits.u(3)
    if max_sfb == 0:
        raise ValueError("lfe max_sfb 0")
    sects, nz = parse_long_body(bits, width, max_sfb, SFB_2048, strict=True)
    return bits.p, max_sfb, sects, nz


def try_5ch(data, start_bit, width):
    """sf_info(ASF,0,0) long-frame + five_channel_info + 5 bodies."""
    bits = Bits(data, start_bit)
    b_long = bits.u(1)
    if not b_long:
        raise ValueError("not long frame")
    max_sfb = bits.u(6)
    if not (20 <= max_sfb <= 63):
        raise ValueError(f"max_sfb {max_sfb} implausible")
    # five_channel_info: chel_matsel 4b + 5x chparam_info
    matsel = bits.u(4)
    for _ in range(5):
        sap = bits.u(2)
        if sap == 1:
            for _ in range(max_sfb):
                bits.u(1)
        elif sap == 3:
            raise ValueError("sap_data unhandled")
    bodies = []
    for _ in range(5):
        sects, nz = parse_long_body(bits, width, max_sfb, SFB_2048, strict=True)
        bodies.append(nz)
    return bits.p, max_sfb, matsel, bodies


if __name__ == "__main__":
    path = sys.argv[1]
    data = open(path, 'rb').read()
    mode = sys.argv[2] if len(sys.argv) > 2 else 'lfe'
    lo, hi = (int(x) for x in sys.argv[3].split(':')) if len(sys.argv) > 3 else (16, 120)
    fn = try_lfe if mode == 'lfe' else try_5ch
    for width in (5, 3):
        for sb in range(lo, hi):
            try:
                res = fn(data, sb, width)
            except Exception:
                continue
            print(f"width={width} start={sb}: end={res[0]} ({res[0]-sb} bits) "
                  f"max_sfb={res[1]} detail={res[2:]}"[:200])
