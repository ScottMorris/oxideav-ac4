#!/usr/bin/env python3
"""R477: are the ffmpeg-walk body positions the true positions?

For each frame, read the walk's channel bodies (msfb pos = body start,
plus the AUDIT m and end). Decode with my v2 chain at that EXACT start
and correlate vs all 6 discrete refs (free lag, free sign). If walk
positions give strong correlation, they are ground truth and the
correlation-lottery anchors were the wrong ones all along.

walk[i] <-> dump frame i-1.
Usage: r477_walktest.py <track> [n_frames]
"""
import sys, json, re, wave
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
TRACK = sys.argv[1]
REFWAV = {'kw': 'kw-ref51.wav', 'spk': 'spk-ref51.wav'}[TRACK]
DUMPDIR = {'kw': 'kw4', 'spk': 'spk4'}[TRACK]
WALK = {'kw': 'kw_mp4_walk.json', 'spk': 'spk_walk.json'}[TRACK]
w = wave.open(REFWAV, 'rb')
REF = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
ORN = ['L', 'R', 'C', 'LFE', 'Ls', 'Rs']
walk = json.load(open(WALK))

def v2_parse(d, P, width):
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
    if bits.u(1):
        l2, c2 = T['ASF_HCB_SNF_LEN'], T['ASF_HCB_SNF_CW']
        for sfb in range(m):
            if sfb_cb[sfb] == 0 or mqi[sfb] == 0: A.huff(bits, l2, c2)
    sp = np.zeros(N)
    for sfb in range(m):
        if sf_per[sfb] is None: continue
        g2 = 2.0 ** (0.25 * (sf_per[sfb] - ref_sf))
        for kk in range(SFB[sfb], SFB[sfb + 1]):
            sp[kk] = np.sign(q[kk]) * abs(q[kk]) ** (4 / 3.) * g2
    return sp, bits.p, m

def bestcorr(fr, sp):
    pcm = (BASIS @ sp) * WIN
    if pcm.std() < 1e-9: return 0.0, -1, 0
    best = (0.0, -1, 0)
    for ch in range(6):
        for lag in range(960, 1105, 8):
            st = fr * N + lag
            if st < 0 or st + 2 * N > len(REF): continue
            r = REF[st:st + 2 * N, ch]
            if r.std() < 10: continue
            c = float(np.corrcoef(pcm, r)[0, 1])
            if abs(c) > abs(best[0]): best = (c, ch, lag)
    return best

def walk_bodies(rec):
    """extract [(msfb_pos, m_next, end)] channel bodies from a walk rec.
    Pattern: 'POS msfb=<m>@<p>' gives a body start; the following
    'POS end@<e>' gives its end. Use AUDIT m for the m value."""
    bodies = []
    ev = rec['ev']
    pend_starts = []
    for e in ev:
        m = re.match(r'POS msfb=(\d+)@(\d+)', e)
        if m: pend_starts.append((int(m.group(2)), int(m.group(1)))); continue
        m = re.match(r'AUDIT m=(\d+) g=(\d+) long=(\d+) sect@(\d+)', e)
        if m:
            bodies.append({'sect': int(m.group(4)), 'm': int(m.group(1)),
                           'g': int(m.group(2)), 'long': int(m.group(3)),
                           'start': None})
            continue
        m = re.match(r'POS end@(\d+)', e)
        if m and bodies and bodies[-1].get('end') is None:
            bodies[-1]['end'] = int(m.group(1))
    # attach nearest preceding msfb start to each body
    for b in bodies:
        cand = [p for (p, mm) in pend_starts if p <= b['sect']]
        b['start'] = max(cand) if cand else b['sect']
    return bodies

if __name__ == '__main__':
    nfr = int(sys.argv[2]) if len(sys.argv) > 2 else 60
    NFR = len(REF) // N
    frames = list(range(1, min(nfr + 1, NFR)))
    hits = 0; tot = 0; strong = 0
    perch = {c: [] for c in range(6)}
    for fr in frames:
        wi = fr + 1
        if wi >= len(walk): continue
        d = open(f'{DUMPDIR}/sub{fr:04d}.bin', 'rb').read()
        bodies = walk_bodies(walk[wi])
        if fr * N + 1104 + 2 * N > len(REF): continue
        best_per_body = []
        for b in bodies:
            got = None
            for w in (3, 5):
                for st in (b['start'], b['sect'] - 5, b['sect'], b['start'] - 5):
                    try:
                        sp, e, m = v2_parse(d, st, w)
                    except Exception:
                        continue
                    if (sp != 0).sum() < 8: continue
                    c, ch, lag = bestcorr(fr, sp)
                    if got is None or abs(c) > abs(got[0]):
                        got = (c, ch, lag, st, w, m, e, b.get('end'))
            if got:
                best_per_body.append(got)
                tot += 1
                if abs(got[0]) >= 0.4: hits += 1
                if abs(got[0]) >= 0.55: strong += 1
                perch[got[1]].append(abs(got[0]))
        if fr <= 6:
            print(f'f{fr}: {len(bodies)} bodies; corrs ' +
                  ' '.join(f'{ORN[g[1]]}:{g[0]:+.2f}@{g[3]}(e{g[6]}/we{g[7]})' for g in best_per_body[:10]), flush=True)
    print(f'\nWALK-POSITION DECODE: {tot} bodies, |c|>=0.4: {hits} ({100*hits/max(tot,1):.0f}%), '
          f'|c|>=0.55: {strong} ({100*strong/max(tot,1):.0f}%)')
    for c in range(6):
        v = perch[c]
        if v: print(f'  {ORN[c]}: n={len(v)} mean|c|={np.mean(v):.3f} frac>=.4={np.mean(np.array(v)>=0.4):.2f}')
