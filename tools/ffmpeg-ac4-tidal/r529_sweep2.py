#!/usr/bin/env python3
"""R529b: sweep the position of a +1 inserted bit between codec_mode and ch0,
scoring by whether ch0 then decodes as a CLEAN full LONG body (all cbs<=11,
in-range spectral, consumes a plausible number of bits). Full-body validity is
far more discriminating than a header check. Parallel over 1410 dumps."""
import glob, sys
from multiprocessing import Pool
sys.path.insert(0,'/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
T=A.T; SFB=A.SFB_2048
NUM_SFB={2048:63}
def nmsfb(tl): return 6

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

def lfe_marks(d, st):
    """decode LFE (normal grammar) returning bit positions after each element:
       p_after_cm, p_after_msfb, p_after_sect, p_after_ref, p_after_scf, p_end."""
    b=B(d,st); b.u(2); p_cm=b.p
    msfb=b.u(3); p_msfb=b.p
    sects=[]; k=0; esc=31
    while k<msfb:
        cb=b.u(4); sl=1
        while True:
            inc=b.u(5); sl+=inc
            if inc!=esc: break
        sects.append((k,min(k+sl,msfb),cb)); k+=sl
        if len(sects)>16: break
    p_sect=b.p
    q,mqi=A.parse_spectra(b,sects,SFB,msfb)
    b.u(8); p_ref=b.p
    sfbcb=[0]*msfb
    for(s,e,cb)in sects:
        for i in range(s,e):
            if i<msfb:sfbcb[i]=cb
    first=False; L,C=T['ASF_HCB_SCALEFAC_LEN'],T['ASF_HCB_SCALEFAC_CW']
    for sfb in range(msfb):
        if sfbcb[sfb]==0 or mqi[sfb]==0:continue
        if first:A.huff(b,L,C)
        else:first=True
    p_scf=b.p
    if b.u(1):
        L2,C2=T['ASF_HCB_SNF_LEN'],T['ASF_HCB_SNF_CW']
        for sfb in range(msfb):
            if sfbcb[sfb]==0 or mqi[sfb]==0:A.huff(b,L2,C2)
    return [p_cm,p_msfb,p_sect,p_ref,p_scf,b.p]

def ch0_long_body_ok(d, lfe_end):
    """assuming long frame: coding_config(2)[+mode_2ch], then ch0 sf_info(long)
    + full body; return True if body decodes clean (all cb<=11, ends sanely)."""
    b=B(d,lfe_end); cc=b.u(2)
    if cc==0: b.u(1)
    lf=b.u(1)
    if lf!=1: return False
    msfb=b.u(6)                    # long-frame max_sfb (n_msfb_bits=6)
    if not (1<=msfb<=63): return False
    # long frame: n_grp_bits=0, one window group, transf=2048
    try:
        sects=A.parse_sections(b,5,msfb,strict=True)   # n_sect_bits=5
    except Exception:
        return False
    for (s,e,cb) in sects:
        if cb>11: return False     # invalid codebook -> misaligned
    try:
        q,mqi=A.parse_spectra(b,sects,SFB,msfb)
    except Exception:
        return False
    # scalefac
    ref=b.u(8); sfbcb=[0]*msfb
    for (s,e,cb) in sects:
        for i in range(s,e): sfbcb[i]=cb
    first=False; L,C=T['ASF_HCB_SCALEFAC_LEN'],T['ASF_HCB_SCALEFAC_CW']
    try:
        for sfb in range(msfb):
            if sfbcb[sfb]==0 or mqi[sfb]==0: continue
            if first:
                sf=A.huff(b,L,C)
            else: first=True
        if b.u(1):
            L2,C2=T['ASF_HCB_SNF_LEN'],T['ASF_HCB_SNF_CW']
            for sfb in range(msfb):
                if sfbcb[sfb]==0 or mqi[sfb]==0: A.huff(b,L2,C2)
    except Exception:
        return False
    return True

def score(args):
    fn, insert_idx = args   # insert_idx: which LFE mark to add +1 after (-1=none)
    d=open(fn,'rb').read()
    try:
        marks=lfe_marks(d,astart(d))
        end=marks[-1]
        if insert_idx>=0:
            end = marks[insert_idx] + 1  # shift everything after this mark by +1
            # but we must re-decode from that mark... approximation: for the
            # LFE-end shift, only the final position matters for ch0 alignment
            # when the inserted bit is AFTER all LFE content. For interior marks
            # this is a lower bound. Use end = marks[-1]+1 for a pure +1 test.
            end = marks[-1] + 1 if insert_idx==5 else marks[insert_idx]+1
        return 1 if ch0_long_body_ok(d,end) else 0
    except Exception:
        return 0

if __name__=='__main__':
    files=sorted(glob.glob('kw4/sub*.bin'))
    labels=['none(cur)','+1@after_cm','+1@after_msfb','+1@after_sect','+1@after_ref','+1@after_scf','+1@after_snf(LFEend)']
    print(f'{len(files)} dumps. ch0 CLEAN LONG BODY count by +1 insertion point:')
    with Pool(4) as p:
        # baseline none
        base=p.map(score,[(fn,-1) for fn in files]); print(f'  {labels[0]:22}: {sum(base)}')
        for idx in range(6):
            r=p.map(score,[(fn,idx) for fn in files])
            print(f'  {labels[idx+1]:22}: {sum(r)}')
