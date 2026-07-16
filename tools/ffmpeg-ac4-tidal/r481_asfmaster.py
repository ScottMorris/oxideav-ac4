#!/usr/bin/env python3
"""R481: ac4asf-based twin-peak harvester + master (LONG bodies).

Uses the corrected short/multigroup parser (ac4asf) restricted to long
single-transform bodies (which IMDCT to a clean 2048 block). Twin-peak:
body0 vs M=L+R oracle (fixed lag), body1 vs S=L-R oracle. Compares
bandwidth + correlation to the v5 (v2_parse) master.

Usage: r481_asfmaster.py harvest <fr...>     # scan+print per frame
       r481_asfmaster.py build               # full-track master
"""
import sys, json, re, wave, os
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf
from multiprocessing import Pool
N = 2048
n_ = np.arange(2 * N); k_ = np.arange(N)
BASIS = np.cos(np.pi / N * (n_[:, None] + 0.5 + N / 2) * (k_[None, :] + 0.5))
from numpy import i0
xg = np.arange(N + 1) / N
kern = i0(np.pi * 3.0 * np.sqrt(np.clip(1 - (2 * xg - 1) ** 2, 0, 1)))
cs = np.cumsum(kern[:N]); KBDh = np.sqrt(cs / cs[-1])
WIN = np.concatenate([KBDh, KBDh[::-1]])
w = wave.open('kw-ref51.wav', 'rb')
_R = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
NFR = len(_R) // N
M_OR = _R[:, 0] + _R[:, 1]
S_OR = _R[:, 0] - _R[:, 1]


def parse_long(d, sb):
    """parse_sf_info+data at sb; return (spectrum2048, end, max_sfb) or None
    for long single-group bodies only."""
    bits = A.Bits(d, sb)
    cfg = ac4asf.parse_sf_info(bits)
    if cfg.long_frame != 1:
        return None
    m = cfg.get_max_sfb(0)
    if not (1 <= m <= 63):
        return None
    data = ac4asf.parse_sf_data(bits, cfg)
    sp = ac4asf.core_spectrum(cfg, data)
    if (sp != 0).sum() < 10:
        return None
    return sp, data.end, m


def pcm_of(sp):
    return (BASIS @ sp) * WIN


def corr_oracle(fr, pcm, oracle, fixed=True):
    if pcm.std() < 1e-9:
        return 0.0, 0
    best = (0.0, 0)
    rng = range(1000, 1081, 8) if fixed else range(984, 1073, 8)
    for lag in rng:
        st = fr * N + lag
        if st < 0 or st + 2 * N > len(oracle):
            continue
        r = oracle[st:st + 2 * N]
        if r.std() < 10:
            continue
        c = float(np.corrcoef(pcm, r)[0, 1])
        if abs(c) > abs(best[0]):
            best = (c, lag)
    return best


def harvest(fr):
    try:
        d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
    except FileNotFoundError:
        return fr, None
    wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
    if fr * N + 1104 + 2 * N > len(_R):
        return fr, None
    if M_OR[fr * N + 1040: fr * N + 1040 + 2 * N].std() < 10:
        return fr, None
    # stage 1: best body0 vs M
    cands = []
    for sb in range(16, max(17, wall - 200)):
        try:
            pr = parse_long(d, sb)
        except Exception:
            continue
        if pr is None:
            continue
        sp, e0, m = pr
        pcm = pcm_of(sp)
        c0, l0 = corr_oracle(fr, pcm, M_OR, fixed=True)
        if abs(c0) < 0.42:
            continue
        cands.append((abs(c0), sb, e0, m, c0, sp))
    if not cands:
        return fr, None
    cands.sort(key=lambda x: -x[0])
    seen = set(); ded = []
    for c in cands:
        if c[2] in seen:
            continue
        seen.add(c[2]); ded.append(c)
    cands = ded
    best = None
    for c0abs, sb, e0, m, c0, sp0 in cands[:16]:
        # stage 2: S body scan after e0 (small header gap 0..48)
        for g in range(0, 49):
            try:
                pr = parse_long(d, e0 + g)
            except Exception:
                continue
            if pr is None:
                continue
            sp1, e1, m1 = pr
            pcm1 = pcm_of(sp1)
            cS, lS = corr_oracle(fr, pcm1, S_OR, fixed=False)
            cSm, _ = corr_oracle(fr, pcm1, M_OR, fixed=False)
            cc = max(abs(cS), abs(cSm))
            if cc < 0.42:
                continue
            score = abs(c0) + cc
            if best is None or score > best[0]:
                best = (score, sb, e0, g, m, m1, c0, cS, cSm, e1)
    if best is None:
        c0abs, sb, e0, m, c0, sp0 = cands[0]
        return fr, {'fr': fr, 'sb': sb, 'e0': e0, 'm0': m, 'c0': round(c0, 3), 'gap': None}
    score, sb, e0, g, m, m1, c0, cS, cSm, e1 = best
    return fr, {'fr': fr, 'sb': sb, 'e0': e0, 'm0': m, 'c0': round(c0, 3),
                'gap': g, 'm1': m1, 'cS': round(cS, 3), 'cSm': round(cSm, 3), 'e1': e1}


def render_master(anchors):
    L = np.zeros(NFR * N + 2 * N); R = np.zeros_like(L)
    LAG = 1024
    used = withS = 0
    for fr, a in sorted(anchors.items()):
        if fr * N + LAG + 2 * N > len(_R):
            continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        Rf = _R[fr * N + LAG: fr * N + LAG + 2 * N]
        refM = Rf[:, 0] + Rf[:, 1]
        if refM.std() < 10:
            continue
        try:
            pr = parse_long(d, a['sb'])
        except Exception:
            continue
        if pr is None:
            continue
        blkM = pcm_of(pr[0])
        if blkM.std() < 1e-9:
            continue
        cM = float(np.corrcoef(blkM, refM)[0, 1])
        if abs(cM) < 0.25:
            continue
        blkM *= np.sign(cM)
        blkS = np.zeros(2 * N)
        if a.get('gap') is not None:
            try:
                pr1 = parse_long(d, a['e0'] + a['gap'])
                if pr1 is not None:
                    bS = pcm_of(pr1[0])
                    refS = Rf[:, 0] - Rf[:, 1]
                    if refS.std() > 10 and bS.std() > 1e-9:
                        cS = float(np.corrcoef(bS, refS)[0, 1])
                        bS *= np.sign(cS)
                        bS *= (refS.std() / max(refM.std(), 1e-9)) * (blkM.std() / max(bS.std(), 1e-9))
                        blkS = bS; withS += 1
            except Exception:
                pass
        lb = (blkM + blkS) / 2; rb = (blkM - blkS) / 2
        tgt = (Rf[:, 0].std() + Rf[:, 1].std()) / 2
        cur = (lb.std() + rb.std()) / 2
        if cur > 1e-9:
            g = tgt / cur; lb *= g; rb *= g
        L[fr * N: fr * N + 2 * N] += lb
        R[fr * N: fr * N + 2 * N] += rb
        used += 1
    out = np.zeros((NFR * N, 2))
    out[LAG:, 0] = L[:NFR * N - LAG]; out[LAG:, 1] = R[:NFR * N - LAG]
    mx = np.abs(out).max()
    if mx > 0:
        out *= 0.85 * 32767 / mx
    wv = wave.open('kw_stereo_v6.wav', 'wb')
    wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
    wv.writeframes(out.astype(np.int16).tobytes()); wv.close()
    print(f'v6: rendered {used} frames (S in {withS})')
    return out


if __name__ == '__main__':
    mode = sys.argv[1]
    if mode == 'harvest':
        frames = [int(x) for x in sys.argv[2:]]
        for fr in frames:
            _, r = harvest(fr)
            print(f'f{fr}: ' + (json.dumps(r) if r else 'none'), flush=True)
    elif mode == 'sweep':
        frames = [int(x) for x in sys.argv[2:]] if len(sys.argv) > 2 else list(range(NFR))
        with Pool(int(os.environ.get('HARVEST_POOL', '2'))) as p:
            for fr, r in p.imap(harvest, frames):
                if r:
                    print(f'f{fr}: ' + json.dumps(r), flush=True)
    elif mode == 'build':
        anchors = {}
        for ln in open('asf_anchors_kw.log'):
            mm = re.match(r'f(\d+): (\{.*\})', ln)
            if mm:
                anchors[int(mm.group(1))] = json.loads(mm.group(2))
        render_master(anchors)
