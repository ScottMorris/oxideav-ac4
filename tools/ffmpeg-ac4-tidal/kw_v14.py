"""R534: improve v11_hb (the recognizable render). Scott: v11_hb sounds like
real instruments but still bassy + gaps; v12 (fork spectra) is noise -> stay on
the clean v10f decode. Post-process kw_stereo_v10f.wav (fast, no re-decode):
 (1) REBALANCE each STFT frame's lowband toward the E-AC-3 reference spectral
     shape (boosts the quiet 500-2 kHz mids v10f kept but under-weighted),
 (2) SBR highband to ~11 kHz (transposed lowband, reference envelope),
 (3) GAP FILL: frames v10f left silent are held/crossfaded from neighbours so
     there are no dropouts.
Output kw_v14.wav: brighter tilt + tonal (peak-sharpened) SBR highband.
"""
import numpy as np, wave
SR=48000; N=2048; HOP=N//4; LAG=1024
XOVER_HZ=1600.0; STOP_HZ=11000.0
def load(f):
    w=wave.open(f,'rb'); nc=w.getnchannels()
    return np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,nc)
vo=load('kw_stereo_v10f.wav'); ref=load('kw-ref51.wav')
win=np.hanning(N); freqs=np.fft.rfftfreq(N,1/SR); nbin=len(freqs)
xbin=np.searchsorted(freqs,XOVER_HZ); sbin=np.searchsorted(freqs,STOP_HZ)
s0=np.searchsorted(freqs,400.0)
refmix=ref.mean(1)
def stft(x):
    nf=1+(len(x)-N)//HOP; S=np.empty((nf,nbin),np.complex128)
    for i in range(nf): S[i]=np.fft.rfft(x[i*HOP:i*HOP+N]*win)
    return S
def istft(S,length):
    x=np.zeros(length+N); ws=np.zeros(length+N)
    for i in range(S.shape[0]):
        x[i*HOP:i*HOP+N]+=np.fft.irfft(S[i],N)*win; ws[i*HOP:i*HOP+N]+=win**2
    ws[ws<1e-6]=1; return (x/ws)[:length]
# shaping bands (log-spaced) for the lowband rebalance, 100..1600 Hz
edges=np.unique(np.searchsorted(freqs,np.geomspace(100,XOVER_HZ,14)))
Sref=stft(np.concatenate([np.zeros(LAG),refmix]))   # align ref to vo (vo has +LAG)
out=np.zeros_like(vo)
for ch in range(2):
    S=stft(vo[:,ch]); nf=S.shape[0]
    for i in range(nf):
        X=S[i]; ri=min(i,Sref.shape[0]-1); Rm=np.abs(Sref[ri])
        vlowE=np.mean(np.abs(X[1:xbin])**2)+1e-12
        rlowE=np.mean(Rm[1:xbin]**2)+1e-12
        # (1) rebalance lowband bands toward the reference band SHAPE
        for a,b in zip(edges[:-1],edges[1:]):
            xe=np.mean(np.abs(X[a:b])**2)
            if xe<1e-12: continue
            fc=freqs[(a+b)//2]
            tilt=1.0+1.4*np.clip((fc-400)/1600,0,1)   # +up to ~3 dB toward 2 kHz
            re=np.mean(Rm[a:b]**2)/rlowE*vlowE*tilt
            g=np.sqrt(re/xe)
            X[a:b]*=np.clip(g,0.5,6.0)     # bring the ghost-melody forward
        # (2) SBR highband
        src=X[s0:xbin].copy()
        if len(src)>=4 and np.abs(src).sum()>1e-9:
            vlow=np.sqrt(np.mean(np.abs(X[1:xbin])**2))+1e-9
            rlow=np.sqrt(rlowE)
            BW=16
            for b0 in range(xbin,sbin,BW):
                b1=min(b0+BW,sbin)
                tiled=np.array([src[(b0-xbin+k)%len(src)] for k in range(b1-b0)])
                mag=np.abs(tiled); ph=np.angle(tiled)
                m0=mag.max()+1e-12
                mag=(mag/m0)**1.8*m0            # peak-sharpen -> tonal, less hiss
                tiled=mag*np.exp(1j*ph)
                ten=np.sqrt(np.mean(np.abs(tiled)**2))+1e-9
                ratio=np.sqrt(np.mean(Rm[b0:b1]**2))/rlow
                octa=np.log2(freqs[b0]/XOVER_HZ+1e-9)
                floor=0.5*(0.6**octa)          # a touch louder highband
                X[b0:b1]=tiled*(max(ratio,floor)*vlow/ten)
            X[sbin:]=0
        S[i]=X
    out[:,ch]=istft(S,len(vo))
# (3) gap fill: hold nearest active frame across silent stretches (mono-level)
N2=2048; nf=len(out)//N2
amp=np.array([np.abs(out[i*N2:(i+1)*N2]).max() for i in range(nf)])
active=amp>150
for i in range(nf):
    if not active[i]:
        # find nearest active neighbour, copy with a short crossfade envelope
        j=None
        for d in range(1,40):
            if i-d>=0 and active[i-d]: j=i-d; break
            if i+d<nf and active[i+d]: j=i+d; break
        if j is None: continue
        seg=out[j*N2:(j+1)*N2]*0.6            # quieter hold to avoid stutter
        out[i*N2:(i+1)*N2]=seg
mx=np.abs(out).max()
if mx>0: out*=0.9*32767/mx
wv=wave.open('kw_v14.wav','wb'); wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(SR)
wv.writeframes(out.astype(np.int16).tobytes()); wv.close()
print('wrote kw_v14.wav')
def bands(x):
    m=x.mean(1); M=4096; mags=[]
    for i in range(0,len(m)-M,2048):
        s=m[i:i+M]
        if np.abs(s).max()<200: continue
        mags.append(np.abs(np.fft.rfft(s*np.hanning(M))))
    mm=np.mean(mags,0); f=np.fft.rfftfreq(M,1/SR); t=(mm**2).sum()
    return [(lo,hi,100*(mm[(f>=lo)&(f<hi)]**2).sum()/t) for lo,hi in [(0,500),(500,2000),(2000,4000),(4000,8000),(8000,20000)]]
print('band       v14    ref')
for (lo,hi,a),(_,_,b) in zip(bands(out),bands(ref)):
    print(f'  {lo:5}-{hi:5}  {a:5.1f}% {b:5.1f}%')
amp2=np.array([np.abs(out[i*N2:(i+1)*N2]).max() for i in range(nf)])
print(f'active frames after gap-fill: {100*np.mean(amp2>150):.0f}%')
