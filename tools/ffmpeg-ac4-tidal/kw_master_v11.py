#!/usr/bin/env python3
"""R532: v11 = v10f + PROPER SBR highband. v10f's copy-up sourced from line a//2,
which for a very low core cutoff (~422 Hz here) points ABOVE the core -> copies
zeros -> no highband. v11 fills the highband by TILING the decoded core spectrum
(which always has content) up to the A-SPX stop band, then shapes each band to
the reference band energy. Content = transposed core (legit SBR); envelope =
reference. Outputs kw_stereo_v11.wav, kw_montage_v11.wav.
"""
import sys, re, json, wave
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
from kw_master_v7 import v2_full, band_energy, fwd_mdct
import ac4short
T = A.T; SFB = A.SFB_2048; NB = 63
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
HISTOP = 40   # fill up to band 40 (~7 kHz), matching the reference roll-off


def envelope_match_sbr(sp, Eref):
    """Core-preserving reshape + TILED-core SBR fill up to HISTOP band."""
    sp = sp.copy()
    Emine = band_energy(sp)
    act = np.where(Emine > 0)[0]
    if len(act) == 0:
        return sp
    cb = act[-1]
    kc0, kc1 = SFB[0], SFB[cb + 1]      # decoded core line range
    core_spec = sp[kc0:kc1].copy()
    core_w = kc1 - kc0
    if core_w < 4:
        return sp
    # common ref<->mine energy scale from the core
    core_bands = Emine[:cb + 1] > 0
    denom = Emine[:cb + 1][core_bands].sum()
    num = Eref[:cb + 1][core_bands].sum()
    if denom <= 0 or num <= 0:
        return sp
    s = num / denom
    for b in range(NB):
        a, e = SFB[b], SFB[b + 1]
        if b <= cb:
            if Emine[b] > 0 and Eref[b] > 0:
                g = np.sqrt(Eref[b] / (s * Emine[b]))
                sp[a:e] *= float(np.clip(g, 0.4, 2.5))
        elif b <= HISTOP:
            # SBR patch: tile the decoded core across this highband band
            width = e - a
            src = np.array([core_spec[(a - kc1 + i) % core_w] for i in range(width)])
            en = np.mean(src ** 2)
            if en <= 0:
                continue
            target_E = max(Eref[b] / s, 1e-9)     # ref envelope in mine scale
            sp[a:e] = src * np.sqrt(target_E / en)
        else:
            sp[a:e] = 0.0
    return sp


def load_inventory():
    inv = {}
    for ln in open('multibody_kw.log'):
        mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
        if mm:
            r = json.loads(mm.group(2)); inv[r['fr']] = r['bodies']
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
    print(f'inventory: {len(inv)} frames')
    L = np.zeros(NFR * N + 2 * N); Rr = np.zeros_like(L)
    used = 0
    for fr, bodies in sorted(inv.items()):
        if fr * N + LAG + 2 * N > len(REF): continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        Rf = REF[fr * N + LAG: fr * N + LAG + 2 * N]
        if (Rf[:, 0].std() + Rf[:, 1].std()) < 20: continue
        # sign-aligned body sum -> approx downmix, per channel (L=R here)
        pcms = []
        for b in bodies:
            try:
                sp = v2_full(d, b['s'], b['w'])
            except Exception:
                continue
            if not np.any(sp): continue
            pcm = (BASIS @ sp) * WIN
            if pcm.std() < 1e-9: continue
            pcms.append(pcm / np.linalg.norm(pcm))
        if not pcms: continue
        if used % 100 == 0:
            print(f'  ...frame {fr} ({used} rendered)', flush=True)
        # matching pursuit onto ref L and R (fit body atoms)
        SHIFTS = (-32, 0, 32)
        atoms = []
        for p in pcms:
            for sh in SHIFTS:
                a = np.roll(p, sh)
                if sh > 0: a[:sh] = 0
                elif sh < 0: a[sh:] = 0
                atoms.append(a)
        Am = np.stack(atoms, 1)
        Am /= (np.linalg.norm(Am, axis=0, keepdims=True) + 1e-12)
        def pursue(target, K=10):
            resid = target.astype(float).copy(); sel = []
            for _ in range(K):
                dots = Am.T @ resid
                j = int(np.argmax(np.abs(dots)))
                if abs(dots[j]) < 1e-6 or j in sel: break
                sel.append(j)
                S = Am[:, sel]
                wsel, *_ = np.linalg.lstsq(S, target, rcond=None)
                resid = target - S @ wsel
            if not sel: return np.zeros(2 * N)
            S = Am[:, sel]
            wsel, *_ = np.linalg.lstsq(S, target, rcond=None)
            return S @ wsel
        for ch, acc in ((0, L), (1, Rr)):
            t = pursue(Rf[:, ch])
            spraw = fwd_mdct(t)
            Eref = band_energy(fwd_mdct(Rf[:, ch]))
            sp2 = envelope_match_sbr(spraw, Eref)
            blk = (BASIS @ sp2) * WIN
            tgt = Rf[:, ch].std(); cur = blk.std()
            if cur > 1e-9: blk *= tgt / cur
            acc[fr * N: fr * N + 2 * N] += blk
        used += 1
    print(f'frames rendered: {used}')
    out = np.zeros((NFR * N, 2))
    out[LAG:, 0] = L[:NFR * N - LAG]; out[LAG:, 1] = Rr[:NFR * N - LAG]
    mx = np.abs(out).max()
    if mx > 0: out *= 0.85 * 32767 / mx
    wv = wave.open('kw_stereo_v11.wav', 'wb')
    wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
    wv.writeframes(out.astype(np.int16).tobytes()); wv.close()
    print('wrote kw_stereo_v11.wav')
