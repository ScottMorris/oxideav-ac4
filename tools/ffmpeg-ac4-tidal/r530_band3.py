import numpy as np, wave, re
N=2048
SFB=np.array([0,4,8,12,16,20,24,28,32,36,40,44,52,60,68,76,84,92,100,108,116,124,136,148,
     160,172,188,204,220,240,260,284,308,336,364,396,432,468,508,552,600,652,704,
     768,832,896,960,1024])  # up to band 48 (0..1024 bin)
MB=len(SFB)-1
sp=np.fromfile('/tmp/spec_base.bin',dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)[1:]; nf=sp.shape[0]
def bandE(mag2):  # mag2: (...,Nbins) -> (...,MB) log band energy
    return np.log(np.stack([mag2[...,SFB[i]:SFB[i+1]].sum(-1) for i in range(MB)],-1)+1e-9)
# decoded band energies from |spec|^2 (spec is MDCT coeffs; use ^2 directly)
decb=bandE(sp**2)  # (nf,8,MB)
# reference: batched STFT, hop=N/2 for finer lag, window sine
w=wave.open('kw-ref51.wav','rb'); rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,rc)
HOP=N//2
win=np.sin(np.pi*(np.arange(N)+0.5)/N)
nh=(ref.shape[0]-N)//HOP
def refband(rch):
    idx=np.arange(N)[None,:]+ (np.arange(nh)*HOP)[:,None]
    fr=ref[idx,rch]*win
    mag2=np.abs(np.fft.rfft(fr,axis=1))**2   # (nh, N/2+1)
    return bandE(mag2[:,:1024])              # (nh, MB) using first 1024 bins
fail=set()
for ln in open('lp0.log'):
    m=re.search(r'NEVERFAIL frame (\d+)',ln)
    if m: fail.add(int(m.group(1))-1)
longf=np.array([f for f in range(nf) if f not in fail])
# normalize decoded
dz=decb-decb.mean(-1,keepdims=True); ds=decb.std(-1)+1e-9
best=None
sample=longf[(longf>100)&(longf<600)]
for rch in range(rc):
    rb=refband(rch); rz=rb-rb.mean(-1,keepdims=True); rs=rb.std(-1)+1e-9
    for ach in range(6):
        cs=[]
        for f in sample[::2]:
            if ds[f,ach]<1e-6: continue
            base=int(f*N/HOP); bc=0
            for dh in range(-6,7):
                h=base+dh
                if 0<=h<nh:
                    c=abs(np.dot(dz[f,ach],rz[h])/(MB*ds[f,ach]*rs[h]))
                    bc=max(bc,c)
            cs.append(bc)
        mc=np.mean(cs) if cs else 0
        if best is None or mc>best[0]: best=(mc,ach,rch,rb,rz,rs)
mc,ach,rch,rb,rz,rs=best
print(f'best: ac4 ch{ach} vs ref ch{rch}  meanBandCorr(long sample)={mc:.3f}')
# per-frame at best pairing
def fc(f):
    if ds[f,ach]<1e-6: return np.nan
    base=int(f*N/HOP); bc=0
    for dh in range(-8,9):
        h=base+dh
        if 0<=h<nh:
            c=abs(np.dot(dz[f,ach],rz[h])/(MB*ds[f,ach]*rs[h])); bc=max(bc,c)
    return bc
pf=np.array([fc(f) for f in range(nf)])
def st(idx):
    v=pf[idx]; v=v[~np.isnan(v)]
    return f'n={len(v)} mean={v.mean():.3f} med={np.median(v):.3f} frac>0.4={np.mean(v>0.4):.2f}'
print('LONG :', st(longf))
print('SHORT:', st(np.array([f for f in range(nf) if f in fail])))
np.save('/tmp/pfband.npy',pf)
