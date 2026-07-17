#!/usr/bin/env python3
"""R508: decode the walk's GIANT FRONT BODY from the dumps; semantic test.

bedgeo.log shows the fork walks a body at in@40 spanning most of the
frame (cb10 sections over ~63-66 'sfbs', spec ~8000 bits). Replicate on
dump bits: parse sections at the walk's front position, decode spectra
line-by-line per section codebook (clamped to 2048 lines), IMDCT,
correlate vs ref M/all oracles. Alignment: walk frame f vs dump f and
f-1 (R458 off-by-one). Null: wrong-frame ref.
"""
import sys, re, wave
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A

T = A.T
SFB = A.SFB_2048
N = 2048

n_ = np.arange(2 * N); k_ = np.arange(N)
BASIS = np.cos(np.pi / N * (n_[:, None] + 0.5 + N / 2) *
               (k_[None, :] + 0.5)).astype(np.float32)
from numpy import i0
xg = np.arange(N + 1) / N
kern = i0(np.pi * 3.0 * np.sqrt(np.clip(1 - (2 * xg - 1) ** 2, 0, 1)))
cs = np.cumsum(kern[:N]); KBDh = np.sqrt(cs / cs[-1])
WIN = np.concatenate([KBDh, KBDh[::-1]]).astype(np.float32)
w = wave.open('kw-ref51.wav', 'rb')
_R = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16
                   ).astype(np.float64).reshape(-1, 6)
REF = np.zeros((_R.shape[0], 8))
REF[:, :6] = _R
REF[:, 6] = _R[:, 0] + _R[:, 1]
REF[:, 7] = _R[:, 0] - _R[:, 1]
LAGS = list(range(960, 1105, 8))


def parse_front(lines_iter):
    """collect per-frame front-body records from bedgeo."""
    fronts = {}
    pend = []
    for ln in lines_iter:
        if ln.startswith('BODY'):
            mm = re.match(r'BODY m=(\d+) in@(\d+) sect=(\d+) geom=\[(.*?)\]'
                          r' lsf=\d+ spec=(\d+) sf=(\d+) snf=(\d+) out@(\d+)',
                          ln)
            if mm:
                pend.append(tuple(int(x) if i != 3 else x for i, x in
                                  enumerate(mm.groups())))
        elif ln.startswith('BED f='):
            f = int(re.match(r'BED f=(\d+)', ln).group(1))
            best = None
            for rec in pend:
                if rec[1] <= 64:      # front body (in@ <= 64)
                    if best is None or rec[4] > best[4]:
                        best = rec
            if best is not None:
                fronts[f] = best
            pend = []
    return fronts


def decode_body(d, start_bits, geom, max_lines=2048):
    """decode sections+spectra at start; geom = [(cb, s_sfb, e_sfb)..].
    Returns spectrum (line-indexed via SFB, clamped)."""
    b = A.Bits(d, start_bits)
    sp = np.zeros(N)
    for (cb, s_sfb, e_sfb) in geom:
        if cb < 1 or cb > 11:
            continue
        lo = SFB[min(s_sfb, 56)]
        hi = SFB[min(e_sfb, 56)]
        if hi <= lo:
            continue
        L = T[f'ASF_HCB_{cb}_LEN']; C = T[f'ASF_HCB_{cb}_CW']
        dim = A.CB_DIM[cb]; unsig = A.UNSIGNED[cb]
        mod = A.CB_MOD[cb]; off = A.CB_OFF[cb]
        k = lo
        while k < hi:
            idx = A.huff(b, L, C)
            vals = []
            x = idx
            for _ in range(dim):
                vals.append(x % mod - off); x //= mod
            if unsig:
                for i in range(dim):
                    if vals[i] != 0:
                        if b.u(1):
                            vals[i] = -vals[i]
            for i in range(dim):
                if cb == 11 and abs(vals[i]) == 16:
                    # ext escape: 4-bit prefix count + bits
                    nbits = 4
                    while b.u(1):
                        nbits += 1
                        if nbits > 24:
                            raise ValueError('esc')
                    vals[i] = np.sign(vals[i]) * (b.u(nbits) + (1 << nbits))
                if k + i < N:
                    sp[k + i] = vals[i]
            k += dim
    return sp, b.p


def sect_region_parse(d, in_at, width=5):
    """parse the section list right where the walk read it (sections
    precede spectra? walk logs sect bits then spec: sections START at
    in_at). Returns geom list + position after sections."""
    b = A.Bits(d, in_at)
    # walk geoms show 1-4 sections covering up to ~66 sfbs; msfb field
    # location unknown -> use section list until coverage >= 56 or the
    # section count in the walk's geom
    return b


if __name__ == '__main__':
    fronts = parse_front(open('bedgeo.log'))
    print('walk frames with front body:', len(fronts))
    res = {0: [], -1: []}
    nulls = []
    tested = 0
    for wf, rec in sorted(fronts.items()):
        m, in_at, sectbits, geom_s, spec, sfbits, snf, out = rec
        geom = [tuple(map(int, g)) for g in
                re.findall(r'\((\d+), (\d+), (\d+)\)', geom_s)]
        if spec < 400:
            continue          # want the giant/content bodies
        if tested >= 120:
            break
        for shift in (0, -1):
            fr = wf + shift
            if fr < 0 or fr % 24 == 0:
                continue
            try:
                d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
            except FileNotFoundError:
                continue
            try:
                sp, endp = decode_body(d, in_at + sectbits, geom)
            except Exception:
                continue
            if not np.any(sp):
                continue
            pcm = (BASIS @ (sp / np.abs(sp).max())) * WIN
            pz = (pcm - pcm.mean()) / (pcm.std() + 1e-12)
            def bestc(base):
                best = 0.0
                for lag in LAGS:
                    s = base + lag
                    if s < 0 or s + 2 * N > len(REF):
                        continue
                    seg = REF[s:s + 2 * N]
                    sd = seg.std(0); sd[sd < 1e-9] = 1
                    c = (pz[:, None] * (seg - seg.mean(0)) / sd).mean(0)
                    mx = float(np.abs(c).max())
                    if mx > best:
                        best = mx
                return best
            res[shift].append(bestc(fr * N))
            if shift == 0:
                nulls.append(bestc((fr + 173) * N))
        tested += 1
    for shift in (0, -1):
        a = np.array(res[shift])
        if len(a):
            print(f'align dump=walk{"" if shift==0 else shift}: n={len(a)} '
                  f'mean {a.mean():.3f}  >0.3 {np.mean(a>0.3):.2f}  '
                  f'>0.5 {np.mean(a>0.5):.2f}')
    a = np.array(nulls)
    if len(a):
        print(f'null (+173): n={len(a)} mean {a.mean():.3f}  '
              f'>0.3 {np.mean(a>0.3):.2f}')
