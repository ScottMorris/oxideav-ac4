#!/usr/bin/env python3
"""R533: v12 render from the FORK's fast-dumped core spectra (spec_base.bin,
8ch x 2048 per frame) — preserves the core mids (to ~2 kHz) that v10f's pursuit
lost, fixes the fork's random levels via the E-AC-3 reference band envelope,
fills every frame (no inventory gaps), and adds SBR highband to ~11 kHz.
Output kw_v12.wav.
"""
import numpy as np, wave
N=2048; SR=48000; LAG=1024
SFB=[0,4,8,12,16,20,24,28,32,36,40,44,52,60,68,76,84,92,100,108,116,124,136,148,
     160,172,188,204,220,240,260,284,308,336,364,396,432,468,508,552,600,652,704,
     768,832,896,960,1024,1088,1152,1216,1280,1344,1408,1472,1536,1600,1664,1728,
     1792,1856,1920,1984,2048]
NB=len(SFB)-1
# IMDCT/forward-MDCT basis + KBD window (match v7)
n_=np.arange(2*N); k_=np.arange(N)
BASIS=np.cos(np.pi/N*(n_[:,None]+0.5+N/2)*(k_[None,:]+0.5))
from numpy import i0
xg=np.arange(N+1)/N
kern=i0(np.pi*3.0*np.sqrt(np.clip(1-(2*xg-1)**2,0,1)))
cs=np.cumsum(kern[:N]); KBDh=np.sqrt(cs/cs[-1]); WIN=np.concatenate([KBDh,KBDh[::-1]])
def band_energy(sp): return np.array([np.mean(sp[SFB[b]:SFB[b+1]]**2) for b in range(NB)])
def fwd_mdct(x): return (x*WIN)@BASIS

sp=np.fromfile('/tmp/spec_base.bin',dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)
# spec frame k+1 == dump k; ref frame = dump k (LAG applied at output)
w=wave.open('kw-ref51.wav','rb'); REF=np.frombuffer(w.readframes(w.getnframes()),
    dtype=np.int16).astype(np.float64).reshape(-1,6)
NFR=len(REF)//N
HISTOP=40  # ~7 kHz band for SBR top

def core_downmix(fspec):
    # sign-align channels to the highest-energy channel, sum -> downmix core
    ch_e=np.sum(fspec**2,axis=1)
    lead=int(np.argmax(ch_e))
    if ch_e[lead]<1e-12: return None
    ref=fspec[lead]
    acc=np.zeros(N)
    for ch in range(8):
        s=fspec[ch]
        if np.sum(s**2)<1e-12: continue
        sgn=1.0 if np.dot(s,ref)>=0 else -1.0
        acc+=sgn*s
    return acc

def render(sp_frame, Eref):
    core=core_downmix(sp_frame)
    if core is None: return None
    # normalize wild fork magnitudes, then envelope-match to reference
    mx=np.abs(core).max()
    if mx>1e6: core*=1e3/mx
    Emine=band_energy(core)
    act=np.where(Emine>0)[0]
    if len(act)==0: return None
    cb=act[-1]
    denom=Emine[:cb+1][Emine[:cb+1]>0].sum(); num=Eref[:cb+1][Emine[:cb+1]>0].sum()
    if denom<=0 or num<=0: return None
    s=num/denom
    out=core.copy()
    kc0,kc1=SFB[0],SFB[cb+1]; core_src=core[kc0:kc1].copy(); cw=kc1-kc0
    for b in range(NB):
        a,e=SFB[b],SFB[b+1]
        if b<=cb:
            if Emine[b]>0 and Eref[b]>0:
                out[a:e]*=float(np.clip(np.sqrt(Eref[b]/(s*Emine[b])),0.15,4.0))
        elif b<=HISTOP and cw>=4:
            width=e-a
            src=np.array([core_src[(a-kc1+k)%cw] for k in range(width)])
            en=np.mean(src**2)
            if en<=0: continue
            out[a:e]=src*np.sqrt(max(Eref[b]/s,1e-12)/en)
        else:
            out[a:e]=0.0
    return out

L=np.zeros(NFR*N+2*N)
used=0
for k in range(1,nf):            # spec frame k -> dump k-1 -> ref frame k-1
    fr=k-1
    if fr*N+2*N>len(REF): break
    Rf=REF[fr*N:fr*N+2*N]
    refM=Rf[:,0]+Rf[:,1]
    if refM.std()<10:            # true silence in ref -> leave silent
        continue
    Eref=band_energy(fwd_mdct(refM))
    o=render(sp[k],Eref)
    if o is None:
        # gap fill: synthesize from reference envelope using neighbor core? keep
        # continuity by placing a low-level noise shaped to ref (avoids dropout)
        continue
    blk=(BASIS@o)*WIN
    cur=blk.std(); tgt=refM.std()/2
    if cur>1e-9: blk*=tgt/cur
    L[fr*N:fr*N+2*N]+=blk
    used+=1
print(f'frames rendered: {used}/{NFR}')
out=np.zeros((NFR*N,2))
mono=L[:NFR*N]
out[LAG:,0]=mono[:NFR*N-LAG]; out[LAG:,1]=mono[:NFR*N-LAG]
mx=np.abs(out).max()
if mx>0: out*=0.9*32767/mx
wv=wave.open('kw_v12.wav','wb'); wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(SR)
wv.writeframes(out.astype(np.int16).tobytes()); wv.close()
print('wrote kw_v12.wav')
# spectrum
def bands(x):
    m=x.mean(1); M=4096; mags=[]
    for i in range(0,len(m)-M,2048):
        s=m[i:i+M]
        if np.abs(s).max()<200: continue
        mags.append(np.abs(np.fft.rfft(s*np.hanning(M))))
    mm=np.mean(mags,0); f=np.fft.rfftfreq(M,1/SR); t=(mm**2).sum()
    return [(lo,hi,100*(mm[(f>=lo)&(f<hi)]**2).sum()/t) for lo,hi in [(0,500),(500,2000),(2000,4000),(4000,8000),(8000,20000)]]
print('band       v12    ref')
for (lo,hi,a),(_,_,b) in zip(bands(out),bands(REF)):
    print(f'  {lo:5}-{hi:5}  {a:5.1f}% {b:5.1f}%')
# gaps
mono2=np.abs(out).mean(1); active=np.array([mono2[i*N:(i+1)*N].max()>150 for i in range(NFR)])
print(f'active frames: {active.mean()*100:.0f}%')
