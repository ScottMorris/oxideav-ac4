import numpy as np, wave, re
N=2048
SFB=np.array([0,4,8,12,16,20,24,28,32,36,40,44,52,60,68,76,84,92,100,108,116,124,136,148,
     160,172,188,204,220,240,260,284,308,336,364,396,432,468,508,552,600,652,704,768,832,896,960,1024])
MB=len(SFB)-1
sp=np.fromfile('/tmp/spec_base.bin',dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)[1:]; nf=sp.shape[0]
def bandE(m2): return np.log(np.stack([m2[...,SFB[i]:SFB[i+1]].sum(-1) for i in range(MB)],-1)+1e-9)
da=bandE(sp**2)[:,3,:]; dz=da-da.mean(-1,keepdims=True); ds=da.std(-1)+1e-9
w=wave.open('kw-ref51.wav','rb'); rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,rc)
HOP=N//2; win=np.sin(np.pi*(np.arange(N)+0.5)/N); nh=(ref.shape[0]-N)//HOP
idx=np.arange(N)[None,:]+(np.arange(nh)*HOP)[:,None]
rb=bandE(np.abs(np.fft.rfft(ref[idx,1]*win,axis=1))[:,:1024]**2)
rz=rb-rb.mean(-1,keepdims=True); rs=rb.std(-1)+1e-9
# aligned (tight +/-2) vs shuffled (random ref hop, no time relation)
np.random.seed=None
def score(shuffle):
    cs=[]
    order=list(range(200,900))
    for f in order:
        if ds[f]<1e-6: continue
        if shuffle:
            h=(f*7919+123)% (nh-1)   # deterministic 'random' offset, unrelated to time
            c=abs(np.dot(dz[f],rz[h])/(MB*ds[f]*rs[h]))
        else:
            base=int(f*N/HOP); c=0
            for dh in (-2,-1,0,1,2):
                hh=base+dh
                if 0<=hh<nh: c=max(c,abs(np.dot(dz[f],rz[hh])/(MB*ds[f]*rs[hh])))
        cs.append(c)
    return np.mean(cs), np.median(cs)
am,amed=score(False); sm,smed=score(True)
print(f'ALIGNED (true time):  mean={am:.3f} med={amed:.3f}')
print(f'SHUFFLED (random ref): mean={sm:.3f} med={smed:.3f}')
print(f'SIGNAL above baseline: {am-sm:+.3f}  ({"USABLE" if am-sm>0.08 else "TOO WEAK"})')
