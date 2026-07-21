#!/usr/bin/env python3
"""R530b: per-frame BAND-ENERGY correlation (jointbest-style) of the AC-4 decode
vs kw-ref51. For each AC-4 frame: 48-band log-energy of the decoded spectrum vs
the reference's 48-band log-energy (MDCT of ref at the aligned frame, small lag
search). Correct decode ~0.3-0.4; garbage ~0. Tests whether the reference is a
usable oracle by comparing LONG vs SHORT/fail frames."""
import numpy as np, wave, sys, re
N=2048
SFB=[0,4,8,12,16,20,24,28,32,36,40,44,52,60,68,76,84,92,100,108,116,124,136,148,
     160,172,188,204,220,240,260,284,308,336,364,396,432,468,508,552,600,652,704,
     768,832,896,960,1024,1088,1152,1216,1280,1344,1408,1472,1536,1600,1664,1728,
     1792,1856,1920,1984,2048]
MB=48
SPEC=sys.argv[1] if len(sys.argv)>1 else '/tmp/spec_base.bin'
sp=np.fromfile(SPEC,dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)[1:]; nf=sp.shape[0]
# forward MDCT basis for the reference (analysis) and band energies
n_=np.arange(2*N); k_=np.arange(N)
WIN=np.sin(np.pi*(n_+0.5)/(2*N))       # sine window (jointbest used this for ref)
ANA=np.cos(np.pi/N*(n_[:,None]+0.5+N/2)*(k_[None,:]+0.5))  # (2N, N)
w=wave.open('kw-ref51.wav','rb'); rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,rc)

def dec_bands(f, ch):
    s=sp[f,ch,:]
    e=np.array([np.sum(s[SFB[i]:SFB[i+1]]**2) for i in range(MB)])
    return np.log(e+1e-9)
def ref_bands(start, rch):
    if start<0 or start+2*N>ref.shape[0]: return None
    X=((ref[start:start+2*N,rch]*WIN)[:,None]*ANA).sum(0)
    e=np.array([np.sum(X[SFB[i]:SFB[i+1]]**2) for i in range(MB)])
    return np.log(e+1e-9)

fail=set()
for ln in open('lp0.log'):
    m=re.search(r'NEVERFAIL frame (\d+)',ln)
    if m: fail.add(int(m.group(1))-1)

# find best (ac4 ch, ref ch, global lag) on a sample of long frames
longf=[f for f in range(nf) if f not in fail]
sample=longf[100:400]
best=(-1,-1,0,-1)
for ach in range(6):
    for rch in range(rc):
        for lag in range(-2048,2049,256):
            cs=[]
            for f in sample[::5]:
                a=dec_bands(f,ach)
                if a.std()<1e-6: continue
                b=ref_bands(f*N+lag,rch)
                if b is None or b.std()<1e-6: continue
                cs.append(np.corrcoef(a,b)[0,1])
            if len(cs)>10:
                mc=np.nanmean(cs)
                if abs(mc)>abs(best[3]): best=(ach,rch,lag,mc)
ach,rch,lag,mc=best
print(f'best pairing: ac4 ch{ach} vs ref ch{rch} lag={lag} meanBandCorr={mc:.3f}')

# per-frame band corr at that pairing with local lag search
def frame_corr(f):
    a=dec_bands(f,ach)
    if a.std()<1e-6: return np.nan
    best=0
    for dl in range(-512,513,64):
        b=ref_bands(f*N+lag+dl,rch)
        if b is None or b.std()<1e-6: continue
        c=abs(np.corrcoef(a,b)[0,1]); best=max(best,c)
    return best
pf=np.array([frame_corr(f) for f in range(nf)])
def st(idx):
    v=pf[idx]; v=v[~np.isnan(v)]
    return f'n={len(v)} mean={v.mean():.3f} median={np.median(v):.3f} frac>0.35={np.mean(v>0.35):.2f}'
print('LONG frames :', st(np.array(longf)))
print('SHORT/fail  :', st(np.array([f for f in range(nf) if f in fail])))
np.save('/tmp/pfband_base.npy',pf)
