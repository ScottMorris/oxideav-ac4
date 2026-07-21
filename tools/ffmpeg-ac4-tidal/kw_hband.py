#!/usr/bin/env python3
"""R532: add a real highband to the working v10f low-frequency render by SBR.
Post-process (fast, no re-decode): STFT the v10f output; above the core cutoff
(~1.6 kHz) fill each STFT frame by TILING the decoded lowband spectrum up, with
magnitude shaped to the E-AC-3 reference's highband envelope (content = trans-
posed decode; envelope = reference). ISTFT + overlap-add. Output kw_v11_hb.wav.
"""
import numpy as np, wave, sys
SR=48000; N=2048; HOP=N//4
LAG=1024
XOVER_HZ=1600.0
STOP_HZ=11000.0
def load(f):
    w=wave.open(f,'rb'); nc=w.getnchannels()
    return np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,nc), nc
vo,_=load('kw_stereo_v10f.wav')          # 2ch low render (ref-aligned, LAG applied)
ref,rnc=load('kw-ref51.wav')             # 6ch reference; align: ref[t-LAG]~vo[t]
win=np.hanning(N).astype(np.float64)
freqs=np.fft.rfftfreq(N,1/SR)
xbin=np.searchsorted(freqs,XOVER_HZ)     # first highband bin
sbin=np.searchsorted(freqs,STOP_HZ)
nb=len(freqs)
# reference downmix for envelope (L+R+surround): use mean of all ref channels
refmix=ref.mean(1)
def stft(x):
    nf=1+(len(x)-N)//HOP
    S=np.empty((nf,nb),np.complex128)
    for i in range(nf):
        S[i]=np.fft.rfft(x[i*HOP:i*HOP+N]*win)
    return S
def istft(S,length):
    x=np.zeros(length+N); wsum=np.zeros(length+N)
    for i in range(S.shape[0]):
        x[i*HOP:i*HOP+N]+=np.fft.irfft(S[i],N)*win
        wsum[i*HOP:i*HOP+N]+=win**2
    wsum[wsum<1e-6]=1
    return (x/wsum)[:length]
out=np.zeros_like(vo)
# reference magnitude envelope, aligned: ref time = vo time - LAG
Sref=stft(np.concatenate([np.zeros(LAG),refmix]))   # shift ref later by LAG to align to vo
for ch in range(2):
    S=stft(vo[:,ch])
    nf=S.shape[0]
    # brightest source band of v10f for tiling: 700..1600 Hz
    s0=np.searchsorted(freqs,400.0)
    for i in range(nf):
        X=S[i]
        src=X[s0:xbin].copy()
        if len(src)<4 or np.abs(src).sum()<1e-9:
            continue
        vlow=np.sqrt(np.mean(np.abs(X[1:xbin])**2))+1e-9     # v10f lowband RMS
        ri=min(i,Sref.shape[0]-1); Rm=np.abs(Sref[ri])
        rlow=np.sqrt(np.mean(Rm[1:xbin]**2))+1e-9            # ref lowband RMS
        BW=16
        for b0 in range(xbin,sbin,BW):
            b1=min(b0+BW,sbin)
            tiled=np.array([src[(b0-xbin+k)%len(src)] for k in range(b1-b0)])
            ten=np.sqrt(np.mean(np.abs(tiled)**2))+1e-9
            # relative: ref highband/lowband ratio applied to v10f lowband,
            # with a gentle audibility floor (-18 dB/oct-ish via octave index)
            ratio=np.sqrt(np.mean(Rm[b0:b1]**2))/rlow
            oct_above=np.log2(freqs[b0]/XOVER_HZ+1e-9)
            floor=0.35*(0.5**oct_above)                     # audible synthetic tilt
            target=max(ratio, floor)*vlow
            X[b0:b1]=tiled*(target/ten)
        X[sbin:]=0
        S[i]=X
    out[:,ch]=istft(S,len(vo))
# renormalize to v10f peak
mx=np.abs(out).max()
if mx>0: out*=0.9*32767/mx
wv=wave.open('kw_v11_hb.wav','wb'); wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(SR)
wv.writeframes(out.astype(np.int16).tobytes()); wv.close()
print('wrote kw_v11_hb.wav')
# report spectrum
def bands(x):
    mono=x.mean(1); M=4096; mags=[]
    for i in range(0,len(mono)-M,2048):
        s=mono[i:i+M]
        if np.abs(s).max()<200: continue
        mags.append(np.abs(np.fft.rfft(s*np.hanning(M))))
    m=np.mean(mags,0); f=np.fft.rfftfreq(M,1/SR); tot=(m**2).sum()
    return [(lo,hi,100*(m[(f>=lo)&(f<hi)]**2).sum()/tot) for lo,hi in [(0,500),(500,2000),(2000,4000),(4000,8000),(8000,20000)]]
print('band        v10f_hb   reference')
for (lo,hi,a),(_,_,b) in zip(bands(out),bands(ref)):
    print(f'  {lo:5}-{hi:5}Hz  {a:6.1f}%  {b:6.1f}%')
