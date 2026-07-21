#!/usr/bin/env python3
"""R529: sweep LFE grammar hypotheses (esp. how cb 12..15 section length is
coded) across all 1410 dumps in parallel, scoring each hypothesis by how many
dumps then produce a CLEAN long-frame ch0 (long_frame=1, valid max_sfb, and a
valid first main-channel section codebook). The winning hypothesis reveals the
missing/misread bit."""
import glob, sys
from multiprocessing import Pool
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
            v=(v<<1)|((s.d[s.p>>3]>>(7-(s.p&7)))&1);s.p+=1
        return v
    def align(s):
        if s.p&7:s.p=(s.p+7)&~7

def astart(d):
    b=B(d);b.u(15)
    if b.u(1):
        while True:
            b.u(7)
            if not b.u(1):break
    b.align();return b.p

def parse_sections_cfg(b, max_sfb, nbits, hicb_mode):
    """hicb_mode controls cb>=12 length handling:
       'normal' = read nbits increment (current fork behaviour)
       'len1'   = cb>=12 has NO increment, length 1
       'fill'   = cb>=12 has NO increment, fills to max_sfb
       'incr3'  = cb>=12 reads a 3-bit increment
    """
    esc=(1<<nbits)-1; sects=[]; k=0
    while k<max_sfb:
        cb=b.u(4)
        if cb>=12 and hicb_mode!='normal':
            if hicb_mode=='len1': sl=1
            elif hicb_mode=='fill': sl=max_sfb-k
            elif hicb_mode=='incr3': sl=1+b.u(3)
            else: sl=1
        else:
            sl=1
            while True:
                inc=b.u(nbits); sl+=inc
                if inc!=esc: break
        sects.append((k,min(k+sl,max_sfb),cb)); k+=sl
        if len(sects)>16: break
    return sects

def decode_lfe(d, st, hicb_mode):
    b=B(d,st); b.u(2)      # codec_mode
    msfb=b.u(3)            # sf_info_lfe max_sfb
    sects=parse_sections_cfg(b,msfb,5,hicb_mode)
    q,mqi=A.parse_spectra(b,sects,SFB,msfb)
    b.u(8)                 # ref_sf
    sfbcb=[0]*msfb
    for(s,e,cb)in sects:
        for i in range(s,e):
            if i<msfb:sfbcb[i]=cb
    first=False; L,C=T['ASF_HCB_SCALEFAC_LEN'],T['ASF_HCB_SCALEFAC_CW']
    for sfb in range(msfb):
        if sfbcb[sfb]==0 or mqi[sfb]==0:continue
        if first:A.huff(b,L,C)
        else:first=True
    if b.u(1):
        L2,C2=T['ASF_HCB_SNF_LEN'],T['ASF_HCB_SNF_CW']
        for sfb in range(msfb):
            if sfbcb[sfb]==0 or mqi[sfb]==0:A.huff(b,L2,C2)
    return b.p

def score_dump(args):
    fn, hicb_mode = args
    d=open(fn,'rb').read()
    try:
        end=decode_lfe(d,astart(d),hicb_mode)
        b=B(d,end); cc=b.u(2)
        if cc==0: b.u(1)   # mode_2ch
        pos=b.p
        # ch0 transform_info + psy max_sfb
        bb=B(d,pos); lf=bb.u(1)
        if lf==0:
            i0=bb.u(2); bb.u(2); tl=TL[i0]
        else:
            tl=2048
        m=bb.u(nmsfb(tl)); ns=NUM_SFB[tl]
        # first main section cb (n_sect_bits: long idx4 ->5)
        # skip grouping bits: for long frame n_grp_bits=0
        cb0=bb.u(4) if lf==1 else 15
        long_valid = (lf==1 and 1<=m<=ns and cb0<=11)
        any_valid  = (1<=m<=ns and (lf==0 or cb0<=11))
        return (1 if long_valid else 0, 1 if any_valid else 0)
    except Exception:
        return (0,0)

if __name__=='__main__':
    files=sorted(glob.glob('kw4/sub*.bin'))
    modes=['normal','len1','fill','incr3']
    print(f'{len(files)} dumps; scoring LFE cb>=12 length hypotheses:')
    print(f'{"mode":8} {"ch0_long_valid":16} {"ch0_any_valid":14}')
    with Pool(8) as p:
        for mode in modes:
            results=p.map(score_dump, [(fn,mode) for fn in files])
            lv=sum(r[0] for r in results); av=sum(r[1] for r in results)
            print(f'{mode:8} {lv:<16} {av:<14}')
