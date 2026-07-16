#!/usr/bin/env python3
"""Band-resolved A/B meter: broadband + 1-4kHz corr per file vs ref."""
import sys, wave
import numpy as np
def load(fn,nch):
    w=wave.open(fn,'rb'); return np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(float).reshape(-1,nch)
R=load('kw-ref51.wav',6)
N=2048
def bandpass(x, lo, hi, sr=48000):
    X=np.fft.rfft(x)
    f=np.fft.rfftfreq(len(x),1/sr)
    X[(f<lo)|(f>hi)]=0
    return np.fft.irfft(X, len(x))
for fn in sys.argv[1:]:
    X=load(fn,2)
    n=min(len(X),len(R))
    res={}
    for band,(lo,hi) in (('full',(20,24000)),('mid',(1000,4000))):
        cs=[]
        for ci in range(2):
            xf=bandpass(X[:n,ci],lo,hi); rf=bandpass(R[:n,ci],lo,hi)
            for fr in range(n//N):
                x=xf[fr*N:(fr+1)*N]; r=rf[fr*N:(fr+1)*N]
                if x.std()<20 or r.std()<20: continue
                cs.append(float(np.corrcoef(x,r)[0,1]))
        cs=np.array(cs)
        res[band]=(cs.mean(),(cs>0.3).mean()*100,len(cs)) if len(cs) else (0,0,0)
    print(f"{fn}: full {res['full'][0]:+.3f} ({res['full'][1]:.0f}%>{0.3}, n={res['full'][2]})  mid1-4k {res['mid'][0]:+.3f} ({res['mid'][1]:.0f}%)")
