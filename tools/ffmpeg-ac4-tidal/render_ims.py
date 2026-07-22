import numpy as np, wave, sys
spec, outwav = sys.argv[1], sys.argv[2]
N=2048
sp=np.fromfile(spec,dtype=np.float32).astype(np.float64); nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)
mx=np.abs(sp).max(axis=2,keepdims=True); sp=sp*np.where(mx>1e6,1e4/np.maximum(mx,1e-9),1.0)
n_=np.arange(2*N); k_=np.arange(N); BASIS=np.cos(np.pi/N*(n_[:,None]+0.5+N/2)*(k_[None,:]+0.5))
from numpy import i0
xg=np.arange(N+1)/N; kern=i0(np.pi*3.0*np.sqrt(np.clip(1-(2*xg-1)**2,0,1))); cs=np.cumsum(kern[:N]); KBDh=np.sqrt(cs/cs[-1]); WIN=np.concatenate([KBDh,KBDh[::-1]])
L=(nf-1)*N; out=np.zeros((L+2*N,2))
for ci,ch in enumerate([0,1]):
    t=(sp[1:,ch,:]@BASIS.T)*WIN
    for f in range(nf-1): out[f*N:f*N+2*N,ci]+=t[f]
out=out[:L]
# self-contained level smoothing (AGC): per-frame RMS -> smoothed target gain
nfr=L//N
rms=np.array([max(np.sqrt(np.mean(out[f*N:(f+1)*N]**2)),1.0) for f in range(nfr)])
# smooth the loudness envelope so frame-to-frame jumps are gentle (kills pops)
logr=np.log(rms); sm=np.copy(logr)
for _ in range(4):
    sm[1:-1]=0.5*sm[1:-1]+0.25*(sm[:-2]+sm[2:])
target=np.exp(np.median(logr))          # aim for a steady overall level
g=target/np.exp(sm)
g=np.clip(g,0.2,5.0)
for f in range(nfr):
    out[f*N:(f+1)*N]*=g[f]
mx=np.abs(out).max()
if mx>0: out*=0.9*32767/mx
wv=wave.open(outwav,'wb'); wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
wv.writeframes(out.astype(np.int16).tobytes()); wv.close()
print(f'{outwav}: {L/48000:.1f}s, peak={np.abs(out).max():.0f}')
