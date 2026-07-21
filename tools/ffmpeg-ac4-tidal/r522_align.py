#!/usr/bin/env python3
"""R522: verify kw4/sub*.bin dumps are byte-exact audio-substream payloads.

Extract real AC-4 samples from kw-fixedstco.mp4 (stsz/stco/stsc), parse each
frame's TOC (r516_toc), and compare the audio-substream byte size (last
sus_size, or the presentation substream) to the size of the matching dump.
If sizes match frame-for-frame, the dumps are aligned (RAWSUB ruled out).
"""
import struct, sys
import r516_toc as TOC

MP4 = 'kw-fixedstco.mp4'
data = open(MP4, 'rb').read()


def find_boxes(buf, start, end, path=''):
    """Yield (type, payload_start, payload_end) for boxes in [start,end)."""
    p = start
    while p + 8 <= end:
        size = struct.unpack('>I', buf[p:p+4])[0]
        typ = buf[p+4:p+8].decode('latin1')
        hdr = 8
        if size == 1:
            size = struct.unpack('>Q', buf[p+8:p+16])[0]
            hdr = 16
        elif size == 0:
            size = end - p
        yield typ, p + hdr, p + size, path
        p += size


CONTAINERS = {'moov', 'trak', 'mdia', 'minf', 'stbl', 'md(t)'}


def walk(buf, start, end, want, acc):
    for typ, ps, pe, _ in find_boxes(buf, start, end):
        if typ == want:
            acc.append((ps, pe))
        if typ in ('moov', 'trak', 'mdia', 'minf', 'stbl'):
            walk(buf, ps, pe, want, acc)


def get_box(want):
    acc = []
    walk(data, 0, len(data), want, acc)
    return acc


# stsz: sample sizes
stsz = get_box('stsz')
ps, pe = stsz[0]
# version(1)+flags(3), sample_size(4), sample_count(4), then sizes
sample_size = struct.unpack('>I', data[ps+4:ps+8])[0]
sample_count = struct.unpack('>I', data[ps+8:ps+12])[0]
sizes = []
if sample_size == 0:
    off = ps + 12
    for i in range(sample_count):
        sizes.append(struct.unpack('>I', data[off:off+4])[0])
        off += 4
else:
    sizes = [sample_size] * sample_count
print(f'stsz: count={sample_count} first sizes={sizes[:5]}')

# stco: chunk offsets
stco = get_box('stco')
ps, pe = stco[0]
nchunk = struct.unpack('>I', data[ps+4:ps+8])[0]
choff = [struct.unpack('>I', data[ps+8+4*i:ps+12+4*i])[0] for i in range(nchunk)]

# stsc: samples-per-chunk
stsc = get_box('stsc')
ps, pe = stsc[0]
nentry = struct.unpack('>I', data[ps+4:ps+8])[0]
stsc_entries = []
for i in range(nentry):
    o = ps + 8 + 12*i
    first_chunk = struct.unpack('>I', data[o:o+4])[0]
    spc = struct.unpack('>I', data[o+4:o+8])[0]
    stsc_entries.append((first_chunk, spc))
print(f'stco: nchunk={nchunk} first offs={choff[:3]}  stsc={stsc_entries[:3]}')

# Build sample -> file offset map
sample_offsets = []
si = 0
for ci in range(nchunk):
    # samples in this chunk (1-based chunk index)
    spc = 1
    for (fc, s) in stsc_entries:
        if ci + 1 >= fc:
            spc = s
    base = choff[ci]
    for _ in range(spc):
        if si >= sample_count:
            break
        sample_offsets.append(base)
        base += sizes[si]
        si += 1
print(f'built {len(sample_offsets)} sample offsets')

# For a handful of frames, parse TOC and compare audio-substream size vs dump.
import os
def dump_size(fr):
    p = f'kw4/sub{fr:04d}.bin'
    return os.path.getsize(p) if os.path.exists(p) else None

print('\nfr  smpLen  tocB  sus_sizes                 audioSus  dumpLen  match')
for fr in [0, 1, 2, 23, 24, 25, 48, 100, 500, 1000, 1409]:
    if fr >= len(sample_offsets):
        continue
    off = sample_offsets[fr]
    d = data[off:off + sizes[fr]]
    try:
        t = TOC.parse_toc(d)
    except Exception as e:
        print(f'{fr}: TOC FAIL {e!r}')
        continue
    ss = t['sus_sizes']
    tocb = t['toc_bytes']
    # audio substream = the largest sus_size (tiny=metadata, big=audio)
    audio = max(ss) if ss else None
    dl = dump_size(fr)
    match = (audio == dl)
    print(f'{fr:<4}{sizes[fr]:<8}{tocb:<6}{str(ss):<26}{str(audio):<10}{str(dl):<9}{match}')
