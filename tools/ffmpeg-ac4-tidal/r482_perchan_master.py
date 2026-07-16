#!/usr/bin/env python3
"""R482: per-channel 5.1 reconstruction with ac4asf long bodies.

The content is a 7.1 bed downmixed to the 5.1 E-AC-3 reference. Decode
bodies with the corrected parser and assign each frame's best-correlating
LONG body to EACH of the 6 discrete reference channels (free lag+sign).
Build a 6-channel master + an ITU stereo downmix. Discrete-channel
correlation (r480: up to 0.68) beats the M/S twin-peak (~0.47).

Usage: r482_perchan_master.py sweep <fr...>   -> per-frame per-channel picks
       r482_perchan_master.py build           -> 6ch wav + stereo downmix
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
REF = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64).reshape(-1, 6)
NFR = len(REF) // N
ORN = ['L', 'R', 'C', 'LFE', 'Ls', 'Rs']
LAGS = list(range(984, 1081, 8))
CMIN = 0.45


def parse_long_sp(d, sb):
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


def sweep(fr):
    try:
        d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
    except FileNotFoundError:
        return fr, None
    wall = 16 + int(''.join(f'{x:08b}' for x in d[:2])[0:15], 2) * 8
    if fr * N + LAGS[-1] + 2 * N > len(REF):
        return fr, None
    # precompute normalized ref windows per channel per lag
    RW = np.zeros((len(LAGS), 6, 2 * N))
    RSD = np.zeros((len(LAGS), 6))
    for li, lag in enumerate(LAGS):
        seg = REF[fr * N + lag: fr * N + lag + 2 * N]
        mu = seg.mean(0); sd = seg.std(0); RSD[li] = sd
        RW[li] = ((seg - mu) / np.where(sd < 1e-9, 1, sd)).T
    best = [None] * 6   # per channel: (absc, sb, end, m, c, lag)
    for sb in range(16, max(17, wall - 200)):
        try:
            pr = parse_long_sp(d, sb)
        except Exception:
            continue
        if pr is None:
            continue
        sp, e, m = pr
        pcm = (BASIS @ sp) * WIN
        psd = pcm.std()
        if psd < 1e-9:
            continue
        pz = (pcm - pcm.mean()) / psd
        C = np.einsum('t,lct->lc', pz, RW) / (2 * N)  # (lags,6)
        C[RSD < 10] = 0.0
        for ch in range(6):
            li = int(np.argmax(np.abs(C[:, ch])))
            c = float(C[li, ch])
            if abs(c) < CMIN:
                continue
            if best[ch] is None or abs(c) > best[ch][0]:
                best[ch] = (abs(c), sb, e, m, round(c, 3), LAGS[li])
    picks = {ORN[ch]: dict(sb=b[1], end=b[2], m=b[3], c=b[4], lag=b[5])
             for ch, b in enumerate(best) if b}
    return fr, picks


def build(anchors):
    LAG = 1024
    out6 = np.zeros((NFR * N, 6))
    acc = np.zeros((NFR * N + 2 * N, 6))
    covered = [0] * 6
    for fr, picks in sorted(anchors.items()):
        if fr * N + LAG + 2 * N > len(REF):
            continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        for ch, name in enumerate(ORN):
            if name not in picks:
                continue
            p = picks[name]
            try:
                pr = parse_long_sp(d, p['sb'])
            except Exception:
                continue
            if pr is None:
                continue
            blk = (BASIS @ pr[0]) * WIN
            ref = REF[fr * N + LAG: fr * N + LAG + 2 * N, ch]
            if ref.std() < 10 or blk.std() < 1e-9:
                continue
            c = float(np.corrcoef(blk, ref)[0, 1])
            if abs(c) < 0.30:
                continue
            blk *= np.sign(c) * (ref.std() / blk.std())   # sign + level assist
            acc[fr * N: fr * N + 2 * N, ch] += blk
            covered[ch] += 1
    for ch in range(6):
        out6[LAG:, ch] = acc[:NFR * N - LAG, ch]
    # write 6ch (normalize jointly)
    mx = np.abs(out6).max()
    if mx > 0:
        out6n = out6 * (0.85 * 32767 / mx)
    else:
        out6n = out6
    wv = wave.open('kw_51_v6.wav', 'wb')
    wv.setnchannels(6); wv.setsampwidth(2); wv.setframerate(48000)
    wv.writeframes(out6n.astype(np.int16).tobytes()); wv.close()
    # ITU stereo downmix: Lo = L + .707 C + .707 Ls ; Ro = R + .707 C + .707 Rs
    Lo = out6[:, 0] + 0.707 * out6[:, 2] + 0.707 * out6[:, 4]
    Ro = out6[:, 1] + 0.707 * out6[:, 2] + 0.707 * out6[:, 5]
    st = np.stack([Lo, Ro], 1)
    mx = np.abs(st).max()
    if mx > 0:
        st = st * (0.85 * 32767 / mx)
    wv = wave.open('kw_stereo_v6.wav', 'wb')
    wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
    wv.writeframes(st.astype(np.int16).tobytes()); wv.close()
    print('coverage per channel:', {ORN[c]: covered[c] for c in range(6)})
    return out6


if __name__ == '__main__':
    mode = sys.argv[1]
    if mode == 'sweep':
        frames = [int(x) for x in sys.argv[2:]] if len(sys.argv) > 2 else list(range(NFR))
        with Pool(int(os.environ.get('HARVEST_POOL', '2'))) as p:
            for fr, picks in p.imap(sweep, frames):
                if picks:
                    print(f'f{fr}: ' + json.dumps(picks), flush=True)
    elif mode == 'build':
        anchors = {}
        for ln in open(sys.argv[2] if len(sys.argv) > 2 else 'asf51_kw.log'):
            mm = re.match(r'f(\d+): (\{.*\})', ln)
            if mm:
                anchors[int(mm.group(1))] = json.loads(mm.group(2))
        build(anchors)
