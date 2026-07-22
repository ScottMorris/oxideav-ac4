import numpy as np, wave, sys
# render with correct KBD alpha=3.0 window + decoder-side COMPANDING (expansion)
N=2048; SLOT=64; ALPHA=0.65; EXP=(1-ALPHA)/ALPHA   # 0.5385
sp=np.fromfile('/tmp/spec_ims.bin',dtype=np.float32).astype(np.float64); nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)
mx=np.abs(sp).max(axis=2,keepdims=True); sp=sp*np.where(mx>1e6,1e4/np.maximum(mx,1e-9),1.0)
n_=np.arange(2*N); k_=np.arange(N); BASIS=np.cos(np.pi/N*(n_[:,None]+0.5+N/2)*(k_[None,:]+0.5))
from numpy import i0
xg=np.arange(N+1)/N; kern=i0(np.pi*3.0*np.sqrt(np.clip(1-(2*xg-1)**2,0,1))); cs=np.cumsum(kern[:N]); KBDh=np.sqrt(cs/cs[-1]); WIN=np.concatenate([KBDh,KBDh[::-1]])
L=(nf-1)*N; out=np.zeros((L+2*N,2))
for ci,ch in enumerate([0,1]):
    t=(sp[1:,ch,:]@BASIS.T)*WIN
    for f in range(nf-1): out[f*N:f*N+2*N,ci]+=t[f]
out=out[:L]
# COMPANDING (decoder expansion): per 64-sample slot, gain = L_slot^EXP,
# normalised per-frame so overall frame energy is preserved. Applied per ch.
comp=out.copy()
nfr=L//N
for ci in range(2):
    x=out[:,ci]
    for f in range(nfr):
        fr=x[f*N:(f+1)*N]
        nsl=N//SLOT
        Lv=np.array([np.mean(np.abs(fr[s*SLOT:(s+1)*SLOT]))+1e-6 for s in range(nsl)])
        g=Lv**EXP
        g=g/np.mean(g)                     # preserve avg level
        # smooth gain across slot boundaries to avoid clicks (linear interp)
        gg=np.interp(np.arange(N), (np.arange(nsl)+0.5)*SLOT, g, left=g[0], right=g[-1])
        comp[f*N:(f+1)*N,ci]=fr*gg
# gentle AGC per frame
o=comp
rms=np.array([np.sqrt(np.mean(o[f*N:(f+1)*N]**2))+1 for f in range(nfr)])
lg=np.log(rms); sm=lg.copy()
for _ in range(3): sm[1:-1]=0.5*sm[1:-1]+0.25*(sm[:-2]+sm[2:])
gain=np.exp(np.median(lg))/np.exp(sm); gain=np.clip(gain,0.3,4)
for f in range(nfr): o[f*N:(f+1)*N]*=gain[f]
LAG=1024; oo=np.zeros((L,2)); oo[LAG:]=o[:L-LAG]
mx=np.abs(oo).max()
if mx>0: oo*=0.9*32767/mx
wv=wave.open('kw_compand.wav','wb'); wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
wv.writeframes(oo.astype(np.int16).tobytes()); wv.close()
print(f'kw_compand.wav peak={np.abs(oo).max():.0f}')
# transient sharpness metric: crest factor (peak/rms) — higher = sharper transients
for lab,sig in [('no-compand(kw_native)',out),('companded',comp)]:
    cf=np.mean([np.abs(sig[f*N:(f+1)*N]).max()/(np.sqrt(np.mean(sig[f*N:(f+1)*N]**2))+1) for f in range(nfr)])
    print(f'{lab}: mean crest factor {cf:.2f}')
