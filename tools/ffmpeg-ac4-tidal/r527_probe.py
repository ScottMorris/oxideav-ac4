#!/usr/bin/env python3
"""R527: for failing short-frame dumps, decode the LFE with the current model,
then scan a window of bit offsets around where ch0's sf_info should start to
find an offset that yields a VALID ch0 header (long_frame consistent, max_sfb
<= num_sfb for the transform, first section cb <= 11). A consistent nonzero
delta => the LFE (or upstream) mis-consumes by that many bits.
"""
import glob
NUM_SFB={2048:63,1920:61,1536:55,1024:49,960:49,768:43,512:36,480:36,384:33,
         256:20,240:20,192:18,128:14,120:14,96:12}
TL=[128,256,512,1024]
def nmsfb(tl): return 6 if 384<=tl<=2048 else (5 if 192<=tl<=256 else 4)

class B:
    def __init__(s,d,p=0): s.d=d; s.p=p
    def u(s,n):
        v=0
        for _ in range(n):
            if s.p>=len(s.d)*8: raise EOFError
            v=(v<<1)|((s.d[s.p>>3]>>(7-(s.p&7)))&1); s.p+=1
        return v
    def align(s):
        if s.p&7: s.p=(s.p+7)&~7

def audio_start(d):
    b=B(d); b.u(15)
    if b.u(1):
        while True:
            b.u(7)
            if not b.u(1): break
    b.align(); return b.p

def decode_lfe_end(d, start):
    """current-model LFE: codec_mode(2)+lfe_msfb(3)+sections(n5)+ref_sf(8)+snf(1).
    returns bit position after the LFE (=where coding_config starts)."""
    b=B(d,start)
    cm=b.u(2); msfb=b.u(3)
    # sections
    k=0; nsec=0; cbs=[]
    while k<msfb:
        cb=b.u(4); cbs.append(cb); sl=1
        while True:
            inc=b.u(5); sl+=inc
            if inc!=31: break
        k+=sl; nsec+=1
        if nsec>10: break
    # spectral: skip (assume all cb>11 or zero carry none) -- but for cb<=11 we
    # would need to read. For probe we only need bit accounting; approximate by
    # assuming no spectral (matches the cb=15 case). ref_sf + snf:
    b.u(8)  # ref_sf
    b.u(1)  # b_snf (assume 0)
    return b.p, cm, msfb, cbs

def try_ch0(d, pos):
    """decode transform_info+psy_info at pos; return dict or None if invalid."""
    try:
        b=B(d,pos)
        lf=b.u(1)
        if lf==0:
            i0=b.u(2); i1=b.u(2); tl=TL[i0]
        else:
            i0=i1=4; tl=2048
        nb=nmsfb(tl)
        msfb=b.u(nb)
        ns=NUM_SFB[tl]
        # read first section cb (n_sect_bits: idx<=2 ->3 else 5; long idx=4->5)
        gidx = 4 if lf==1 else i0
        nsb = 3 if gidx<=2 else 5
        # skip grouping bits (approx 0 for this probe; short frames have some,
        # but first-section cb check is robust to small grouping)
        cb0=b.u(4)
        valid = (msfb<=ns) and (cb0<=11) and (msfb>=1)
        return dict(lf=lf,tl=tl,msfb=msfb,ns=ns,cb0=cb0,valid=valid)
    except EOFError:
        return None

if __name__=='__main__':
    # a set of dumps; we'll flag which are 'short' (ch0 long=0 at model pos)
    files=sorted(glob.glob('kw4/sub*.bin'))
    deltas={}
    checked=0; shortcnt=0
    for fn in files:
        d=open(fn,'rb').read()
        st=audio_start(d)
        lfe_end,cm,lmsfb,cbs=decode_lfe_end(d,st)
        cc_start=lfe_end
        # coding_config(2) then three/two/etc -> sf_info at cc_start+2 (approx;
        # real path has chparam etc but sf_info is first in *_channel_data)
        base=try_ch0(d, cc_start+2)
        if base is None: continue
        checked+=1
        if base['lf']==0:  # model says short
            shortcnt+=1
            if base['valid']: continue   # model already valid
            # scan for a nearby offset that yields a valid ch0
            found=None
            for delta in range(-8,17):
                r=try_ch0(d, cc_start+2+delta)
                if r and r['valid'] and r['msfb']>=4:
                    found=(delta,r); break
            key=found[0] if found else 'none'
            deltas[key]=deltas.get(key,0)+1
    print(f'checked={checked} model-short={shortcnt}')
    print('delta distribution (bits to shift ch0 for a VALID header):')
    for k in sorted(deltas, key=lambda x:(x=='none',x)):
        print(f'  delta {k}: {deltas[k]}')
