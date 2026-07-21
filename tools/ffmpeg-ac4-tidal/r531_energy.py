import numpy as np, wave, re
N=2048
sp=np.fromfile('/tmp/spec_base.bin',dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)[1:]; nf=sp.shape[0]
# decode per-frame energy (loudness) per channel, and summed
dE=np.log(np.sum(sp**2,axis=2)+1e-9)   # (nf, 8) log-energy per channel
dEsum=np.log(np.sum(sp**2,axis=(1,2))+1e-9)
w=wave.open('kw-ref51.wav','rb'); rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,rc)
# reference per-frame energy at hop N (aligned to AC-4 frames), summed over ch
nfr=ref.shape[0]//N
rE_ch=np.log(np.stack([np.sum(ref[f*N:(f+1)*N,:]**2,axis=0) for f in range(nfr)])+1e-9)  # (nfr, rc)
rEsum=np.log(np.array([np.sum(ref[f*N:(f+1)*N,:]**2) for f in range(nfr)])+1e-9)
def corr_lag(a,b,maxlag=20):
    best=(-9,0)
    for lag in range(-maxlag,maxlag+1):
        if lag>=0: x,y=a[lag:],b[:len(a)-lag]
        else: x,y=a[:lag],b[-lag:]
        m=min(len(x),len(y)); x,y=x[:m],y[:m]
        if x.std()<1e-6 or y.std()<1e-6: continue
        c=np.corrcoef(x,y)[0,1]
        if c>best[0]: best=(c,lag)
    return best
# summed-energy envelope correlation (the R519 level wall vs ground truth)
c,lag=corr_lag(dEsum, rEsum)
print(f'SUMMED per-frame ENERGY envelope: decode vs ref  corr={c:+.3f} lag={lag} frames')
# best per-channel pairing for energy envelope
print('per-channel energy-envelope correlation (best ref ch):')
for ach in range(8):
    best=(-9,-1,0)
    for rch in range(rc):
        c,lag=corr_lag(dE[:,ach], rE_ch[:,rch])
        if c>best[0]: best=(c,rch,lag)
    print(f'  ac4 ch{ach}: bestcorr={best[0]:+.3f} vs ref ch{best[1]} lag={best[2]}')
# R519 metric: frame-to-frame log-energy jump
print(f'\nframe-to-frame |d logE| : decode={np.median(np.abs(np.diff(dEsum))):.2f}  ref={np.median(np.abs(np.diff(rEsum))):.2f}  (clean~0.1)')
