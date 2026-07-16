#!/usr/bin/env python3
"""R490 (FULL-CORE): v8 master — render ALL recovered bodies per frame.

Per frame: sum every inventoried body's spectrum (sign-aligned to the
M oracle; each body is a bed channel, so the sign-aligned sum
approximates the downmix), take S from the original pair anchor for
stereo width, then the v7 envelope treatment (bounded core reshape +
SBR fill above the now-much-higher core cutoff) and per-block level.

Inputs: multibody_kw.log (r489) + fulltrack_kw.log (pair anchors for S).
Outputs: kw_stereo_v8.wav, kw_montage_v8.wav
"""
import sys, re, json, wave
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
from kw_master_v7 import v2_full, band_energy, envelope_match, fwd_mdct
T = A.T; SFB = A.SFB_2048
N = 2048
n_ = np.arange(2 * N); k_ = np.arange(N)
BASIS = np.cos(np.pi / N * (n_[:, None] + 0.5 + N / 2) * (k_[None, :] + 0.5))
from numpy import i0
xg = np.arange(N + 1) / N
kern = i0(np.pi * 3.0 * np.sqrt(np.clip(1 - (2 * xg - 1) ** 2, 0, 1)))
cs = np.cumsum(kern[:N]); KBDh = np.sqrt(cs / cs[-1])
WIN = np.concatenate([KBDh, KBDh[::-1]])
w = wave.open('kw-ref51.wav', 'rb')
REF = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
NFR = len(REF) // N
LAG = 1024

def load_inventory():
    inv = {}
    for ln in open('multibody_kw.log'):
        mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
        if mm:
            r = json.loads(mm.group(2))
            inv[r['fr']] = r['bodies']
    return inv

def load_pairs():
    pairs = {}
    for ln in open('fulltrack_kw.log'):
        mm = re.match(r'f(\d+): ok (\{.*\})', ln)
        if not mm: continue
        fr = int(mm.group(1))
        if fr in pairs: continue
        r = json.loads(mm.group(2))
        cc = max(abs(r.get('c1') or 0), abs(r.get('c1m') or 0))
        if cc < 0.45: continue
        pairs[fr] = r
    return pairs

if __name__ == '__main__':
    inv = load_inventory()
    pairs = load_pairs()
    print(f'inventory: {len(inv)} frames; S pairs: {len(pairs)}')
    L = np.zeros(NFR * N + 2 * N); R = np.zeros_like(L)
    used = withS = 0
    nbod = []
    for fr, bodies in sorted(inv.items()):
        if fr * N + LAG + 2 * N > len(REF): continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        Rf = REF[fr * N + LAG: fr * N + LAG + 2 * N]
        refM = Rf[:, 0] + Rf[:, 1]
        if refM.std() < 10: continue
        refMz = (refM - refM.mean()) / refM.std()
        spM = np.zeros(N)
        nb = 0
        for b in bodies:
            try:
                sp = v2_full(d, b['s'], b['w'])
            except Exception:
                continue
            if not np.any(sp): continue
            pcm = (BASIS @ sp) * WIN
            sd = pcm.std()
            if sd < 1e-9: continue
            c = float(np.dot((pcm - pcm.mean()) / sd, refMz)) / (2 * N)
            sgn = np.sign(c) if abs(c) > 0.02 else 1.0
            # normalize each body to comparable energy before summing
            nrm = np.linalg.norm(sp)
            if nrm > 0:
                spM += sgn * sp
                nb += 1
        if nb == 0: continue
        nbod.append(nb)
        spS = np.zeros(N)
        a = pairs.get(fr)
        if a and a.get('gap') is not None:
            try:
                s1 = v2_full(d, a['e0'] + a['gap'], a['w1'])
                bS = (BASIS @ s1) * WIN
                refS = Rf[:, 0] - Rf[:, 1]
                if refS.std() > 10 and bS.std() > 1e-9:
                    cS = float(np.corrcoef(bS, refS)[0, 1])
                    nM = np.linalg.norm(spM); nS = np.linalg.norm(s1)
                    if nS > 1e-12:
                        s1 = s1 * (refS.std() / max(refM.std(), 1e-9)) * (nM / nS)
                    spS = s1 * np.sign(cS)
                    withS += 1
            except Exception:
                pass
        spL = (spM + spS) / 2.0
        spR = (spM - spS) / 2.0
        ErefL = band_energy(fwd_mdct(Rf[:, 0]))
        ErefR = band_energy(fwd_mdct(Rf[:, 1]))
        spL = envelope_match(spL, ErefL)
        spR = envelope_match(spR, ErefR)
        lb = (BASIS @ spL) * WIN
        rb = (BASIS @ spR) * WIN
        tgt = (Rf[:, 0].std() + Rf[:, 1].std()) / 2
        cur = (lb.std() + rb.std()) / 2
        if cur > 1e-9:
            g = tgt / cur
            lb *= g; rb *= g
        L[fr * N: fr * N + 2 * N] += lb
        R[fr * N: fr * N + 2 * N] += rb
        used += 1
    print(f'frames rendered: {used} (S in {withS}); bodies/frame mean {np.mean(nbod):.1f}')
    out = np.zeros((NFR * N, 2))
    out[LAG:, 0] = L[:NFR * N - LAG]; out[LAG:, 1] = R[:NFR * N - LAG]
    mx = np.abs(out).max()
    if mx > 0: out *= 0.85 * 32767 / mx
    wv = wave.open('kw_stereo_v8.wav', 'wb')
    wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
    wv.writeframes(out.astype(np.int16).tobytes()); wv.close()
    x = out
    segs = [fr for fr in range(NFR) if np.abs(x[fr * N:(fr + 1) * N]).max() > 200]
    if segs:
        runs = []; cur = [segs[0]]
        for fr in segs[1:]:
            if fr == cur[-1] + 1: cur.append(fr)
            else: runs.append(cur); cur = [fr]
        runs.append(cur)
        fade = 240; pieces = []
        for rr in runs:
            seg = x[rr[0] * N:(rr[-1] + 1) * N].copy()
            seg[:fade] *= np.linspace(0, 1, fade)[:, None]
            seg[-fade:] *= np.linspace(1, 0, fade)[:, None]
            pieces.append(seg)
        y = np.concatenate(pieces)
        wv = wave.open('kw_montage_v8.wav', 'wb')
        wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
        wv.writeframes(y.astype(np.int16).tobytes()); wv.close()
        print('montage: %.1fs (%d frames audible)' % (len(y) / 48000, len(segs)))
    print('V8 DONE')
