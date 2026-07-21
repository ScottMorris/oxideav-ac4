#!/usr/bin/env python3
"""R525: decode the substream dumps DIRECTLY (bypass ffmpeg demuxer + RAWSUB),
core only, to test whether the short-frame failures are real decoder bugs or a
pipeline (demuxer/frame-counter) artifact. For each dump: read audio_size,
walk channel_element_7x (cm, LFE, coding_config, bodies), and flag anomalies
(cb>11, max_sfb>num_sfb) and whether the core consumes a sane bit count.
A-SPX is not parsed; we stop after the core bodies and report position.
"""
import sys, glob, os
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A

NUM_SFB_48 = {2048:63,1920:61,1536:55,1024:49,960:49,768:43,512:36,480:36,
              384:33,256:20,240:20,192:18,128:14,120:14,96:12}
TL_2048 = [128,256,512,1024]           # transf_length[idx], idx 0..3
def n_msfb_bits(tl):
    if 384<=tl<=2048: return 6
    if 192<=tl<=256: return 5
    return 4
def n_msfbl_bits(flb):  # LFE, flb=2048
    return 3 if 1536<=flb<=2048 else 2

class Bits:
    def __init__(s,d): s.d=d; s.p=0
    def u(s,n):
        v=0
        for _ in range(n):
            if s.p>=len(s.d)*8: raise EOFError
            v=(v<<1)|((s.d[s.p>>3]>>(7-(s.p&7)))&1); s.p+=1
        return v
    def align(s):
        if s.p&7: s.p=(s.p+7)&~7

FLB=2048

def read_transform_info(b):
    """returns (long_frame, idx0, idx1, transf_length[0])"""
    lf=b.u(1)
    if lf==0:
        i0=b.u(2); i1=b.u(2)
        return 0,i0,i1,TL_2048[i0]
    return 1,4,4,FLB

def read_psy_info(b, tl0, idx0, idx1):
    """read max_sfb + grouping. returns (max_sfb, num_window_groups, anomaly)."""
    diff_framing = (idx0!=idx1) and (tl0 is not None)
    nb=n_msfb_bits(tl0)
    max_sfb=b.u(nb)
    if diff_framing:
        b.u(n_msfb_bits(TL_2048[idx1]))   # max_sfb[1]
    # n_grp_bits
    # long-frame => 0; else from transf_length. Simplified: derive num_windows
    # from idx (num_windows_0 = 1<<(3-idx)). For a fair anomaly check we only
    # need max_sfb vs num_sfb, so grouping bits are read approximately.
    return max_sfb, diff_framing

def check_body(b, tl, max_sfb, strict_cb=True):
    """parse asf_section_data + spectral + scalefac + snf for one channel/group
    set; returns dict with anomalies. Uses ac4scan's validated parsers on a
    single window group (long-frame-like). For short frames num_window_groups>1
    is not fully modelled — we only validate max_sfb<=num_sfb and cb<=11."""
    num_sfb = NUM_SFB_48[tl]
    anom=[]
    if max_sfb>num_sfb: anom.append(f'msfb{max_sfb}>numsfb{num_sfb}@tl{tl}')
    return anom

def scan_dump(path):
    d=open(path,'rb').read()
    b=Bits(d)
    asz=b.u(15)
    if b.u(1):
        # variable_bits(7)
        ext=0
        while True:
            ext=(ext<<7)|b.u(7)
            if not b.u(1): break
        asz+=ext<<15
    b.align()
    res={'audio_size':asz,'anom':[]}
    try:
        cm=b.u(2); res['cm']=cm
        # iframe? unknown from dump alone; assume flags not available -> try P
        # (aspx_config only on iframe; we can't know, so we just read core).
        # LFE mono_data (channel_mode 7.1)
        lfe_msfb=b.u(n_msfbl_bits(FLB)); res['lfe_msfb']=lfe_msfb
        # LFE sf_data: sections(n=5 for 2048 long) + spectral + scalefac + snf
        secs=A.parse_sections(b, 5, lfe_msfb, strict=False)
        res['lfe_sec0']=secs[0] if secs else None
        for (s0,e0,cb) in secs:
            if cb>11: res['anom'].append(f'LFE cb{cb}')
        # (stop here; the LFE first-section cb is the key tell)
    except Exception as ex:
        res['anom'].append(f'EXC {ex!r}')
    return res

if __name__=='__main__':
    files=sorted(glob.glob('kw4/sub*.bin'))
    n=len(files)
    lfe_cb_bad=0; samples=[]
    from collections import Counter
    cmc=Counter(); lfecbc=Counter()
    for i,fn in enumerate(files):
        r=scan_dump(fn)
        cmc[r.get('cm')]+=1
        if r.get('lfe_sec0'): lfecbc[r['lfe_sec0'][2]]+=1
        if any('cb1' in a or 'cb1' in a for a in r['anom']) or any(a.startswith('LFE cb') for a in r['anom']):
            lfe_cb_bad+=1
            if len(samples)<8: samples.append((i,r))
    print(f'{n} dumps. codec_mode dist: {dict(cmc)}')
    print(f'LFE first-section cb distribution: {dict(sorted(lfecbc.items()))}')
    print(f'dumps with LFE cb>11: {lfe_cb_bad}')
    for i,r in samples:
        print(f'  sub{i:04d}: cm={r.get("cm")} lfe_msfb={r.get("lfe_msfb")} sec0={r.get("lfe_sec0")} anom={r["anom"]}')
