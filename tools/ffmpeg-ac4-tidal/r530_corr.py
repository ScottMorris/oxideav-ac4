#!/usr/bin/env python3
"""R530: per-frame correlation of the AC-4 decode vs the E-AC-3 reference
(kw-ref51.wav, different master, same performance). Establishes an audio oracle
to tell a correct transient-frame decode from garbage. Splits per-frame corr by
long vs failing(transient) frames."""
import numpy as np, wave, sys, re
N=2048
SPEC=sys.argv[1] if len(sys.argv)>1 else '/tmp/spec_base.bin'
sp=np.fromfile(SPEC,dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)
# drop priming frame 0 so spec-frame index == dump index (spec k+1 == dump k)
sp=sp[1:]; nf=sp.shape[0]
# clamp absurd magnitudes (SFREL can explode)
mx=np.abs(sp).max(axis=2,keepdims=True)
sp=sp*np.where(mx>1e6,1e4/np.maximum(mx,1e-9),1.0)
# IMDCT basis + KBD window (matches r516_corr3)
n_=np.arange(2*N); k_=np.arange(N)
BASIS=np.cos(np.pi/N*(n_[:,None]+0.5+N/2)*(k_[None,:]+0.5))
from numpy import i0
xg=np.arange(N+1)/N
kern=i0(np.pi*5.0*np.sqrt(np.clip(1-(2*xg-1)**2,0,1)))
cs=np.cumsum(kern[:N]); KBDh=np.sqrt(cs/cs[-1])
WIN=np.concatenate([KBDh,KBDh[::-1]])
L=nf*N
w=wave.open('kw-ref51.wav','rb'); rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,rc)

def synth(ch):
    t=(sp[:,ch,:]@BASIS.T)*WIN
    out=np.zeros(L+N)
    for f in range(nf): out[f*N:f*N+2*N]+=t[f]
    return out[:L]

# failing (transient proxy) frames from lp0.log NEVERFAIL (dump idx = ctr-1)
fail=set()
try:
    for ln in open('lp0.log'):
        m=re.search(r'NEVERFAIL frame (\d+)',ln)
        if m: fail.add(int(m.group(1))-1)
except FileNotFoundError: pass

# global lag: correlate synth ch0 vs each ref channel, pick best & its lag
y0=synth(0)
def best_ref_lag(y):
    m=min(len(y),ref.shape[0]); a=y[:m]-y[:m].mean()
    best=(-1,0,0)
    for rc_ in range(rc):
        b=ref[:m,rc_]-ref[:m,rc_].mean()
        if b.std()<1: continue
        # coarse lag via fft xcorr
        n=1<<int(np.ceil(np.log2(2*m)))
        fa=np.fft.rfft(a,n); fb=np.fft.rfft(b,n)
        cc=np.fft.irfft(fa*np.conj(fb),n); ML=4096
        cc=np.concatenate([cc[-ML:],cc[:ML+1]])/(m*a.std()*b.std()+1e-9)
        i=int(np.argmax(np.abs(cc))); c=abs(cc[i]); lag=i-ML
        if c>best[2]: best=(rc_,lag,c)
    return best
rcb,lag,gc=best_ref_lag(y0)
print(f'ch0 best ref ch={rcb} global lag={lag} corr={gc:.3f}')

# per-frame correlation at global lag (+/- small local search)
y=y0
pf=np.zeros(nf)
for f in range(nf):
    a=y[f*N:(f+1)*N]
    if a.std()<1e-6: pf[f]=0; continue
    best=0
    for dl in range(-64,65,16):
        st=f*N+lag+dl
        if st<0 or st+N>ref.shape[0]: continue
        b=ref[st:st+N,rcb]
        if b.std()<1: continue
        c=abs(np.corrcoef(a,b)[0,1]); best=max(best,c)
    pf[f]=best
allf=np.arange(nf)
longf=np.array([f for f in allf if f not in fail])
shortf=np.array([f for f in allf if f in fail])
def stats(idx):
    v=pf[idx]; v=v[~np.isnan(v)]
    return f'n={len(v)} meanCorr={v.mean():.3f} median={np.median(v):.3f} frac>0.2={np.mean(v>0.2):.2f}'
print('LONG frames :', stats(longf))
print('SHORT/fail  :', stats(shortf))
np.save('/tmp/pf_base.npy', pf)
print('saved per-frame corr to /tmp/pf_base.npy')
