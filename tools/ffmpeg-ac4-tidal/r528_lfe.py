#!/usr/bin/env python3
"""R528: fully decode the LFE (incl spectral for cb<=11) per dump, then test
whether ch0's transform_info at LFE_end+coding_config yields a VALID long-frame
header (long_frame=1, max_sfb<=63) vs the current 'short' reading, and at what
bit delta. If a consistent small +delta makes ch0 valid-long, the LFE
under-consumes by that many bits (the real bug)."""
import glob, sys
sys.path.insert(0,'/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T=A.T; SFB=A.SFB_2048
NUM_SFB={2048:63,1024:49,512:36,256:20,128:14,1920:61,1536:55,960:49,768:43,480:36,384:33,240:20,192:18,120:14,96:12}
TL=[128,256,512,1024]
def nmsfb(tl): return 6 if 384<=tl<=2048 else (5 if 192<=tl<=256 else 4)

class B:
    def __init__(s,d,p=0): s.d=d;s.p=p
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

def decode_lfe(d, start, iframe):
    """current-model LFE, returns end bit position (coding_config start)."""
    b=B(d,start)
    cm=b.u(2)
    # P-frame: no aspx_config. (iframe handling omitted; test P-frames.)
    # sf_info_lfe: max_sfb 3 bits
    msfb=b.u(3)
    # sf_data: section_data (n_sect_bits=5 for LFE 2048 long)
    sects=[]; k=0
    while k<msfb:
        cb=b.u(4); sl=1
        while True:
            inc=b.u(5); sl+=inc
            if inc!=31: break
        sects.append((k,min(k+sl,msfb),cb)); k+=sl
        if len(sects)>8: break
    # spectral: read for cb 1..11
    q,mqi=A.parse_spectra(b,sects,SFB,msfb)
    # scalefac: ref_sf + chain
    ref=b.u(8); first=False
    sfb_cb=[0]*msfb
    for (s,e,cb) in sects:
        for i in range(s,e):
            if i<msfb: sfb_cb[i]=cb
    lens,cws=T['ASF_HCB_SCALEFAC_LEN'],T['ASF_HCB_SCALEFAC_CW']
    for sfb in range(msfb):
        if sfb_cb[sfb]==0 or mqi[sfb]==0: continue
        if first: A.huff(b,lens,cws)
        else: first=True
    # snf
    if b.u(1):
        lens2,cws2=T['ASF_HCB_SNF_LEN'],T['ASF_HCB_SNF_CW']
        for sfb in range(msfb):
            if sfb_cb[sfb]==0 or mqi[sfb]==0: A.huff(b,lens2,cws2)
    return b.p, cm, msfb, sects

def ch0_valid(d, pos):
    try:
        b=B(d,pos); lf=b.u(1)
        if lf==0:
            i0=b.u(2);i1=b.u(2);tl=TL[i0]
        else:
            tl=2048
        msfb=b.u(nmsfb(tl)); ns=NUM_SFB[tl]
        return lf, msfb, ns, (msfb<=ns and msfb>=1)
    except EOFError:
        return None

if __name__=='__main__':
    files=sorted(glob.glob('kw4/sub*.bin'))
    from collections import Counter
    res=Counter(); lfe_fail=0
    for fn in files:
        d=open(fn,'rb').read()
        try:
            end,cm,lmsfb,lsects=decode_lfe(d,audio_start(d),0)
        except Exception:
            lfe_fail+=1; continue
        # coding_config(2) then sf_info(ch0). Test deltas.
        base=ch0_valid(d,end+2)
        if base is None: continue
        lf0,m0,ns0,ok0=base
        if lf0==0 and not ok0:   # a failing 'short' frame under current model
            # search small deltas for a VALID reading (prefer long_frame=1)
            best=None
            for delta in range(0,6):
                r=ch0_valid(d,end+2+delta)
                if r and r[3]:
                    best=(delta,r[0]); break
            res[best if best else 'none']+=1
    print(f'LFE decode failures: {lfe_fail}')
    print('For failing short frames, (delta, long_frame) that first yields valid ch0:')
    for k in sorted(res, key=lambda x:(x=='none',x)):
        print(f'  {k}: {res[k]}')
PY
