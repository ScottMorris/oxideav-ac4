#!/usr/bin/env python3
"""R493: backward chain extension through BOTH long and short bodies.

For each inventoried frame, walk backward from the first chained body:
candidate predecessor = long body (v2_parse) OR short body (ac4short)
whose end lands within MAXGAP of the current start. Validation: long
bodies need minbits + (corr or band-envelope) as in r489; short bodies
need placement corr >= 0.28 vs M/S at 6 offsets.
Logs fN: PRE {...} rows (resumable). These feed the v10 render.
"""
import sys, json, re, wave, os
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf, ac4short
T = A.T; SFB = A.SFB_2048
N = 2048
n_ = np.arange(2 * N); k_ = np.arange(N)
BASIS = np.cos(np.pi / N * (n_[:, None] + 0.5 + N / 2) * (k_[None, :] + 0.5)).astype(np.float32)
from numpy import i0
xg = np.arange(N + 1) / N
kern = i0(np.pi * 3.0 * np.sqrt(np.clip(1 - (2 * xg - 1) ** 2, 0, 1)))
cs = np.cumsum(kern[:N]); KBDh = np.sqrt(cs / cs[-1])
WIN = np.concatenate([KBDh, KBDh[::-1]]).astype(np.float32)
w = wave.open('kw-ref51.wav', 'rb')
REF = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
MOR = REF[:, 0] + REF[:, 1]
SOR = REF[:, 0] - REF[:, 1]
MAXGAP = 90
OFFS = (768, 832, 896, 960, 1024, 1088)

sys.path.insert(0, '.')
from r489_multibody import v2_parse, minbits_ok, band_profiles, make_refwin


def long_pred(d, target, RW, RSD, BP):
    """long body ending in [target-MAXGAP, target]; closest-end first."""
    for P in range(target - 60, max(16, target - 3600), -1):
        for w2 in (3, 5):
            try:
                sp, e, m, sects = v2_parse(d, P, w2)
            except Exception:
                continue
            if not (0 <= target - e <= MAXGAP): continue
            nz = int((sp != 0).sum())
            if nz < 6 or not minbits_ok(sects, w2): continue
            # correlate
            mx = float(np.abs(sp).max())
            if mx <= 0: continue
            pcm = (BASIS @ (sp / mx)) * WIN
            psd = pcm.std()
            c = 0.0
            if psd > 1e-9:
                pz = (pcm - pcm.mean()) / psd
                C = np.einsum('t,lto->lo', pz, RW) / (2 * N)
                C[RSD < 10] = 0.0
                c = float(C.flat[np.argmax(np.abs(C))])
            nb = min(m, 48)
            eb = np.array([np.sum(sp[SFB[i]:SFB[i + 1]] ** 2) for i in range(nb)])
            lb = np.log(eb + 1e-9)
            bc = 0.0
            if nb >= 6 and lb.std() > 1e-9:
                for ch2 in range(8):
                    prof = BP[ch2][:nb]
                    if prof.std() > 1e-9:
                        v = float(np.corrcoef(lb, prof)[0, 1])
                        if v > bc: bc = v
            if abs(c) >= 0.35 or (target - e <= 40 and nz >= 12 and bc >= 0.45):
                return dict(kind='L', s=P, e=e, w=w2, m=m, nz=nz,
                            c=round(c, 3), bc=round(bc, 2))
    return None


def short_pred(d, fr, target, refM, refS):
    for P in range(target - 100, max(16, target - 4200), -1):
        try:
            pr = ac4short.parse_short(d, P)
        except Exception:
            continue
        if pr is None: continue
        cfg, data = pr
        if not (0 <= target - data.end <= MAXGAP): continue
        nz = int((data.quant != 0).sum())
        if nz < 25: continue
        best = (0.0, 0)
        for off in OFFS:
            blk = ac4short.short_block(cfg, data, off)
            if blk.std() < 1e-9: continue
            if refM.std() > 10:
                c = float(np.corrcoef(blk, refM)[0, 1])
                if abs(c) > abs(best[0]): best = (c, off)
            if refS.std() > 10:
                c = float(np.corrcoef(blk, refS)[0, 1])
                if abs(c) > abs(best[0]): best = (c, off)
        if abs(best[0]) >= 0.28:
            return dict(kind='S', s=P, e=data.end, tl=cfg.tl0,
                        W=cfg.num_windows, nz=nz, c=round(best[0], 3),
                        off=best[1])
    return None


if __name__ == '__main__':
    done = set()
    if os.path.exists('backext_kw.log'):
        for ln in open('backext_kw.log'):
            mm = re.match(r'f(\d+): PRE', ln)
            if mm: done.add(int(mm.group(1)))
    inv = {}
    for ln in open('multibody_kw.log'):
        mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
        if mm:
            r = json.loads(mm.group(2)); inv[r['fr']] = r
    out = open('backext_kw.log', 'a')
    frames = [int(x) for x in sys.argv[1:]] if len(sys.argv) > 1 else sorted(inv)
    for fr in frames:
        if fr in done or fr not in inv: continue
        r = inv[fr]; bs = r['bodies']
        if not bs: continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        if fr * N + 1024 + 4096 > len(MOR): continue
        RW, RSD = make_refwin(fr)
        if RW is None: continue
        BP = band_profiles(fr)
        refM = MOR[fr * N + 1024: fr * N + 1024 + 4096]
        refS = SOR[fr * N + 1024: fr * N + 1024 + 4096]
        cur = bs[0]['s']
        added = []
        for step in range(28):
            if cur < 200: break
            b = long_pred(d, cur, RW, RSD, BP)
            if b is None:
                b = short_pred(d, fr, cur, refM, refS)
            if b is None: break
            added.append(b)
            cur = b['s']
        out.write(f'f{fr}: PRE ' + json.dumps({'fr': fr, 'n': len(added),
                  'first': cur, 'added': added}) + '\n')
        out.flush()
    print('BACKEXT DONE')
