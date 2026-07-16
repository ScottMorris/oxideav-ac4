#!/usr/bin/env python3
"""R488: v7 master = v5 anchor pipeline + SBR-lite highband + 63-band
reference envelope matching (documented assist).

Per frame, per stereo side: build L/R MDCT spectra from the anchored
M/S bodies (v2+snf decode, ref-assisted polarity), then:
  1. forward-MDCT the reference L/R at the same lag -> band energies
     over the 63 SFB bands (bins 0..2048)
  2. bands where our spectrum is empty but the ref has energy: fill by
     copying our own lowband bins up (SBR-style transposition, source
     at half frequency), cascading upward
  3. scale every band to the reference band energy (gain clip 8x)
IMDCT, KBD window, OLA. Assists: anchor positions, polarity, 63-band
per-frame envelope (replaces v4's single level match). Fine spectral
structure remains our decode.
"""
import sys, re, json, wave
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
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
NB = 63          # bands cover bins 0..2048
GAIN_CLIP = 8.0


def v2_full(d, P, width, noise_fill=True):
    bits = A.Bits(d, P)
    m = bits.u(5)
    if m > 56 or m < 1: raise ValueError
    esc = (1 << width) - 1; sects = []; k = 0
    while k < m:
        cb = bits.u(4); ln = 1; li = bits.u(width)
        while li == esc: ln += esc; li = bits.u(width)
        ln += li
        if k + ln > 127: raise ValueError
        sects.append((k, k + ln, cb)); k += ln
    mqi = [0] * m; q = np.zeros(SFB[m])
    for (s, e, cb) in sects:
        if cb == 0 or cb > 11 or e > m: continue
        q2, mq2 = A.parse_spectra(bits, [(s, e, cb)], SFB, m)
        for xx in range(s, e): mqi[xx] = max(mqi[xx], mq2[xx])
        for i2 in range(SFB[s], SFB[e]): q[i2] = q2[i2]
    ref_sf = bits.u(8)
    sfb_cb = [0] * m
    for (s, e, cb) in sects:
        for xx in range(s, min(e, m)): sfb_cb[xx] = cb
    sf = ref_sf; first = False; sf_per = [None] * m
    lens, cws = T['ASF_HCB_SCALEFAC_LEN'], T['ASF_HCB_SCALEFAC_CW']
    for sfb in range(m):
        if sfb_cb[sfb] == 0 or mqi[sfb] == 0: continue
        if first:
            sf += A.huff(bits, lens, cws) - 60
            if not (0 <= sf <= 255): raise ValueError
        else: first = True
        sf_per[sfb] = sf
    snf_codes = [None] * m
    if bits.u(1):
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for sfb in range(m):
            if sfb_cb[sfb] == 0 or mqi[sfb] == 0:
                snf_codes[sfb] = A.huff(bits, l2, c2)
    sp = np.zeros(N)
    for sfb in range(m):
        if sf_per[sfb] is None: continue
        g2 = 2.0 ** (0.25 * (sf_per[sfb] - ref_sf))
        for kk in range(SFB[sfb], SFB[sfb + 1]):
            sp[kk] = np.sign(q[kk]) * abs(q[kk]) ** (4 / 3.) * g2
    if noise_fill:
        rng = np.random.default_rng(1234)
        prev = None
        for sfb in range(m):
            a, b = SFB[sfb], SFB[sfb + 1]
            if sf_per[sfb] is not None and np.any(sp[a:b]):
                e2 = float(np.sum(sp[a:b] ** 2)) / (b - a)
                if e2 > 0 and prev is None:
                    prev = 0.5 * np.log2(e2 + 1e-30)
        level = prev if prev is not None else -10.0
        for sfb in range(m):
            code = snf_codes[sfb]
            if code is None: continue
            delta = code - 17
            if delta == -17: continue
            level += delta
            amp = 2.0 ** (0.5 * level)
            a, b = SFB[sfb], SFB[sfb + 1]
            sp[a:b] += amp * rng.standard_normal(b - a)
    return sp


def load_anchors():
    anchors = {}
    for src in ('kwjoint4.out', 'kwmel.out', 'kwfull.out'):
        try:
            for ln in open(src):
                mm = re.match(r'f(\d+): JOINT sb=(\d+) e0=(\d+) gap=(\d+) w1=(\d+)', ln)
                if mm and int(mm.group(1)) not in anchors:
                    anchors[int(mm.group(1))] = dict(
                        sb=int(mm.group(2)), e0=int(mm.group(3)),
                        gap=int(mm.group(4)), w1=int(mm.group(5)), w0=3)
        except FileNotFoundError:
            pass
    n_old = len(anchors)
    for ln in open('fulltrack_kw.log'):
        mm = re.match(r'f(\d+): (ok|noS) (\{.*\})', ln)
        if not mm: continue
        fr = int(mm.group(1))
        if fr in anchors: continue
        r = json.loads(mm.group(3))
        cc = max(abs(r.get('c1') or 0), abs(r.get('c1m') or 0))
        if mm.group(2) == 'ok' and cc < 0.45: continue
        anchors[fr] = dict(sb=r['sb'], e0=r['e0'], gap=r.get('gap'),
                           w1=r.get('w1'), w0=r['w0'])
    print(f'anchors: {n_old} old + {len(anchors) - n_old} sweep = {len(anchors)}')
    return anchors


def band_energy(sp):
    return np.array([np.mean(sp[SFB[b]:SFB[b + 1]] ** 2) for b in range(NB)])


def envelope_match(sp, Eref):
    """Core-preserving reshape + SBR fill above the core cutoff.

    Core bands (up to the highest band our decode populated): bounded
    gain (1/2.5 .. 2.5) toward the ref shape, in a COMMON scale (so the
    absolute forward/inverse MDCT scale cancels). Above the cutoff:
    copy-up fill scaled exactly to the ref band energy in that same
    common scale."""
    sp = sp.copy()
    Emine = band_energy(sp)
    act = np.where(Emine > 0)[0]
    if len(act) == 0:
        return sp
    cb = act[-1]                      # core cutoff band
    core = Emine[:cb + 1] > 0
    denom = Emine[:cb + 1][core].sum()
    num = Eref[:cb + 1][core].sum()
    if denom <= 0 or num <= 0:
        return sp
    s = num / denom                   # common energy scale ref<->mine
    for b in range(NB):
        a, e = SFB[b], SFB[b + 1]
        if b <= cb:
            if Emine[b] > 0 and Eref[b] > 0:
                g = np.sqrt(Eref[b] / (s * Emine[b]))
                sp[a:e] *= float(np.clip(g, 0.4, 2.5))
        else:
            if Eref[b] <= 0 or a < 16:
                continue
            src_a = a // 2
            seg = sp[src_a:src_a + (e - a)]
            if len(seg) < (e - a):
                seg = np.pad(seg, (0, (e - a) - len(seg)))
            en = np.mean(seg ** 2)
            if en > 0:
                sp[a:e] = seg * np.sqrt(Eref[b] / (s * en))
    return sp


def fwd_mdct(x):
    return (x * WIN) @ BASIS


if __name__ == '__main__':
    anchors = load_anchors()
    L = np.zeros(NFR * N + 2 * N); R = np.zeros_like(L)
    used = withS = filled = 0
    for fr, a in sorted(anchors.items()):
        if fr * N + LAG + 2 * N > len(REF): continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        Rf = REF[fr * N + LAG: fr * N + LAG + 2 * N]
        refM = Rf[:, 0] + Rf[:, 1]
        if refM.std() < 10: continue
        try:
            spM = v2_full(d, a['sb'], a.get('w0', 3))
        except Exception:
            continue
        blkM = (BASIS @ spM) * WIN
        if blkM.std() < 1e-9: continue
        cM = float(np.corrcoef(blkM, refM)[0, 1])
        if abs(cM) < 0.25: continue
        spM = spM * np.sign(cM)
        spS = np.zeros(N)
        if a.get('gap') is not None:
            try:
                s1 = v2_full(d, a['e0'] + a['gap'], a['w1'])
                bS = (BASIS @ s1) * WIN
                refS = Rf[:, 0] - Rf[:, 1]
                if refS.std() > 10 and bS.std() > 1e-9:
                    cS = float(np.corrcoef(bS, refS)[0, 1])
                    nM = np.linalg.norm(spM); nS = np.linalg.norm(s1)
                    if nS > 1e-12:
                        # S level assist: ref S/M ratio x our M scale (as v4)
                        s1 = s1 * (refS.std() / max(refM.std(), 1e-9)) * (nM / nS)
                    spS = s1 * np.sign(cS)
                    withS += 1
            except Exception:
                pass
        spL = (spM + spS) / 2.0
        spR = (spM - spS) / 2.0
        # reference band envelopes (forward MDCT of ref L/R)
        ErefL = band_energy(fwd_mdct(Rf[:, 0]))
        ErefR = band_energy(fwd_mdct(Rf[:, 1]))
        spL = envelope_match(spL, ErefL)
        spR = envelope_match(spR, ErefR)
        lb = (BASIS @ spL) * WIN
        rb = (BASIS @ spR) * WIN
        # absolute level: time-domain per-block match to ref (fixes the
        # forward/inverse MDCT scale mismatch in the band energies)
        tgt = (Rf[:, 0].std() + Rf[:, 1].std()) / 2
        cur = (lb.std() + rb.std()) / 2
        if cur > 1e-9:
            g = tgt / cur
            lb *= g; rb *= g
        L[fr * N: fr * N + 2 * N] += lb
        R[fr * N: fr * N + 2 * N] += rb
        used += 1
    print(f'frames rendered: {used} (S in {withS})')
    out = np.zeros((NFR * N, 2))
    out[LAG:, 0] = L[:NFR * N - LAG]; out[LAG:, 1] = R[:NFR * N - LAG]
    mx = np.abs(out).max()
    if mx > 0: out *= 0.85 * 32767 / mx
    wv = wave.open('kw_stereo_v7.wav', 'wb')
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
        wv = wave.open('kw_montage_v7.wav', 'wb')
        wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
        wv.writeframes(y.astype(np.int16).tobytes()); wv.close()
        print('montage: %.1fs (%d frames audible)' % (len(y) / 48000, len(segs)))
    print('V7 DONE')
