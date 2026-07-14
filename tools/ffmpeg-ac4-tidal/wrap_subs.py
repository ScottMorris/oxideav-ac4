#!/usr/bin/env python3
"""Wrap substream dumps (starting at the audio_size field) into a raw
.ac4 sync-frame stream for the AC4_RAWSUB TOC-bypass decoder mode.
Frame: AC 40 [16-bit size] [flags byte: bit0=iframe] [substream bytes].
Usage: wrap_subs.py <dumpdir> <out.ac4> [iframe_list_file|cadence:N]"""
import sys, os, glob, struct
dumpdir, out = sys.argv[1], sys.argv[2]
ifr = sys.argv[3] if len(sys.argv) > 3 else 'cadence:1024000'
files = sorted(glob.glob(os.path.join(dumpdir, 'sub*.bin')))
if ifr.startswith('cadence:'):
    n = int(ifr.split(':')[1])
    iset = set(range(0, len(files), n))
else:
    iset = set(int(x) for x in open(ifr).read().split())
w = open(out, 'wb')
for i, fn in enumerate(files):
    d = open(fn, 'rb').read()
    payload = bytes([1 if i in iset else 0]) + d
    if len(payload) > 0xFFFE:
        payload = payload[:0xFFFE]
    w.write(b'\xAC\x40' + struct.pack('>H', len(payload)) + payload)
w.close()
print(f'{out}: {len(files)} frames, {len(iset & set(range(len(files))))} iframes')
