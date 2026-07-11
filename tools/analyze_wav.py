#!/usr/bin/env python3
"""Quick verification of a decoded AC-4 WAV.

1. Per-channel RMS in 500 ms windows -> which channels are active when
   (for the channel-ID test track, channels light up one at a time).
2. Per-channel spectral centroid / high-frequency ratio -> LFE should be
   band-limited to ~120 Hz.
3. Boundary-spike detector: sample-to-sample delta outliers bucketed by
   position within the 2048-sample frame; window-transition bugs show as
   spikes piling up at bucket edges.
"""
import sys
import wave
import struct
import numpy as np


def read_wav(path):
    # wave module chokes on WAVE_FORMAT_EXTENSIBLE sometimes; parse manually.
    with open(path, "rb") as f:
        data = f.read()
    assert data[:4] == b"RIFF" and data[8:12] == b"WAVE"
    pos = 12
    fmt = None
    pcm = None
    while pos + 8 <= len(data):
        cid = data[pos:pos + 4]
        sz = struct.unpack("<I", data[pos + 4:pos + 8])[0]
        body = data[pos + 8:pos + 8 + sz]
        if cid == b"fmt ":
            fmt = body
        elif cid == b"data":
            pcm = body
        pos += 8 + sz + (sz & 1)
    tag, nch, rate, _, _, bits = struct.unpack("<HHIIHH", fmt[:16])
    mask = None
    if tag == 0xFFFE and len(fmt) >= 40:
        mask = struct.unpack("<I", fmt[20:24])[0]
    x = np.frombuffer(pcm, dtype="<i2").astype(np.float64) / 32768.0
    x = x[: (len(x) // nch) * nch].reshape(-1, nch)
    return x, nch, rate, tag, mask


def main(path):
    x, nch, rate, tag, mask = read_wav(path)
    print(f"{path}: {nch} ch @ {rate} Hz, fmt tag 0x{tag:04X}, "
          f"mask {hex(mask) if mask is not None else 'none'}, "
          f"{x.shape[0]/rate:.1f} s")

    # --- channel activity timeline (500 ms windows) ---
    win = rate // 2
    nwin = x.shape[0] // win
    rms = np.zeros((nwin, nch))
    for w in range(nwin):
        seg = x[w * win:(w + 1) * win]
        rms[w] = np.sqrt((seg ** 2).mean(axis=0))
    print("\nper-channel overall RMS (dBFS):")
    overall = np.sqrt((x ** 2).mean(axis=0))
    for c in range(nch):
        db = 20 * np.log10(overall[c] + 1e-12)
        print(f"  ch{c}: {db:6.1f} dB")

    print("\nchannel activity timeline (each row=2s, col=channel, '#'=loudest):")
    step = max(1, nwin // 40)
    for w in range(0, nwin, step):
        row = rms[w:w + step].mean(axis=0)
        peak = row.max()
        marks = "".join(
            "#" if (peak > 1e-4 and v > 0.5 * peak) else
            ("+" if v > 0.1 * peak and peak > 1e-4 else ".")
            for v in row)
        print(f"  {w * 0.5:6.1f}s  {marks}")

    # --- spectral character per channel ---
    print("\nper-channel energy above 300 Hz vs total (LFE should be ~0):")
    n = min(x.shape[0], rate * 60)
    for c in range(nch):
        seg = x[:n, c]
        if (seg ** 2).sum() < 1e-9:
            print(f"  ch{c}: (silent)")
            continue
        spec = np.abs(np.fft.rfft(seg)) ** 2
        freqs = np.fft.rfftfreq(len(seg), 1 / rate)
        hi = spec[freqs > 300].sum() / spec.sum()
        print(f"  ch{c}: {100 * hi:5.1f}% energy above 300 Hz")

    # --- boundary spike detector ---
    print("\nboundary-spike histogram (delta outliers by position in 2048-frame, 8 buckets):")
    frame = 2048
    mono = x.mean(axis=1)
    d = np.abs(np.diff(mono))
    # robust threshold: 8 * median absolute delta over the whole track
    med = np.median(d[d > 0]) if (d > 0).any() else 0
    if med == 0:
        print("  (track too quiet)")
        return
    idx = np.nonzero(d > 25 * med)[0]
    buckets = np.zeros(8, dtype=int)
    for i in idx:
        buckets[(i % frame) * 8 // frame] += 1
    total = len(idx)
    print(f"  outliers: {total} of {len(d)} deltas (thresh 25x median)")
    print("  bucket counts (bucket 0 spans the frame boundary):", buckets.tolist())


if __name__ == "__main__":
    main(sys.argv[1])
