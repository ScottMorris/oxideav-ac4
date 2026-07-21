import numpy as np, wave, re
N=2048
SFB=np.array([0,4,8,12,16,20,24,28,32,36,40,44,52,60,68,76,84,92,100,108,116,124,136,148,
     160,172,188,204,220,240,260,284,308,336,364,396,432,468,508,552,600,652,704,768,832,896,960,1024])
MB=len(SFB)-1
sp=np.fromfile('/tmp/spec_base.bin',dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)[1:]; nf=sp.shape[0]
def bandE(m2): return np.log(np.stack([m2[...,SFB[i]:SFB[i+1]].sum(-1) for i in range(MB)],-1)+1e-9)
decb=bandE(sp**2)
w=wave.open('kw-ref51.wav','rb'); rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,rc)
HOP=N//2; win=np.sin(np.pi*(np.arange(N)+0.5)/N); nh=(ref.shape[0]-N)//HOP
idx=np.arange(N)[None,:]+(np.arange(nh)*HOP)[:,None]
rb=bandE(np.abs(np.fft.rfft(ref[idx,1]*win,axis=1))[:,:1024]**2)  # ref ch1
rz=rb-rb.mean(-1,keepdims=True); rs=rb.std(-1)+1e-9
ach=3
da=decb[:,ach,:]; dz=da-da.mean(-1,keepdims=True); ds=da.std(-1)+1e-9
fail=set()
for ln in open('lp0.log'):
    m=re.search(r'NEVERFAIL frame (\d+)',ln)
    if m: fail.add(int(m.group(1))-1)
longf=np.array([f for f in range(nf) if f not in fail])
shortf=np.array([f for f in range(nf) if f in fail])
def pf(maxdh):
    out=np.full(nf,np.nan)
    for f in range(nf):
        if ds[f]<1e-6: continue
        base=int(f*N/HOP); bc=0
        for dh in range(-maxdh,maxdh+1):
            h=base+dh
            if 0<=h<nh:
                c=abs(np.dot(dz[f],rz[h])/(MB*ds[f]*rs[h])); bc=max(bc,c)
        out[f]=bc
    return out
for md,lab in [(1,'tight +/-1hop'),(2,'+/-2hop'),(8,'loose +/-8hop')]:
    p=pf(md)
    lv=p[longf]; lv=lv[~np.isnan(lv)]; sv=p[shortf]; sv=sv[~np.isnan(sv)]
    print(f'{lab:16}: LONG mean={lv.mean():.3f} med={np.median(lv):.3f} | SHORT mean={sv.mean():.3f} med={np.median(sv):.3f} | sep={lv.mean()-sv.mean():+.3f}')
