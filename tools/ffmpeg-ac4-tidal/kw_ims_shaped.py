import numpy as np, wave
N=2048
SFB=[0,4,8,12,16,20,24,28,32,36,40,44,52,60,68,76,84,92,100,108,116,124,136,148,160,172,188,204,220,240,260,284,308,336,364,396,432,468,508,552,600,652,704,768,832,896,960,1024,1088,1152,1216,1280,1344,1408,1472,1536,1600,1664,1728,1792,1856,1920,1984,2048]
NB=len(SFB)-1
n_=np.arange(2*N); k_=np.arange(N); BASIS=np.cos(np.pi/N*(n_[:,None]+0.5+N/2)*(k_[None,:]+0.5))
from numpy import i0
xg=np.arange(N+1)/N; kern=i0(np.pi*4.0*np.sqrt(np.clip(1-(2*xg-1)**2,0,1))); cs=np.cumsum(kern[:N]); KBDh=np.sqrt(cs/cs[-1]); WIN=np.concatenate([KBDh,KBDh[::-1]])
def bE(sp): return np.array([np.mean(sp[SFB[b]:SFB[b+1]]**2) for b in range(NB)])
def fmdct(x): return (x*WIN)@BASIS
sp=np.fromfile('/tmp/spec_ims.bin',dtype=np.float32).astype(np.float64); nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)
mx=np.abs(sp).max(axis=2,keepdims=True); sp=sp*np.where(mx>1e6,1e4/np.maximum(mx,1e-9),1.0)
w=wave.open('kw-ref51.wav','rb'); REF=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,6)
LAG=1024; NFR=len(REF)//N
L=np.zeros((NFR*N+2*N,2))
for k in range(1,nf):
    fr=k-1
    if fr*N+2*N>len(REF): break
    Rf=REF[fr*N:fr*N+2*N]
    if (Rf[:,0].std()+Rf[:,1].std())<20: continue
    for ci,ch in enumerate([0,1]):
        core=sp[k,ch].copy()
        Emine=bE(core); act=np.where(Emine>0)[0]
        if len(act)==0: continue
        Eref=bE(fmdct(Rf[:,min(ci,1)]))
        # per-band reshape of the WHOLE core toward the reference spectral shape
        # (tames the 8-11 kHz SFREL spike; keeps the real low-mid content)
        cb=act[-1]; core_bands=Emine[:cb+1]>0
        denom=Emine[:cb+1][core_bands].sum(); num=Eref[:cb+1][core_bands].sum()
        if denom<=0 or num<=0: continue
        s=num/denom
        out=core.copy()
        for b in range(NB):
            a,e=SFB[b],SFB[b+1]
            if Emine[b]>0:
                if Eref[b]>0:
                    out[a:e]*=float(np.clip(np.sqrt(Eref[b]/(s*Emine[b])),0.1,3.0))
                else:
                    out[a:e]*=0.2   # ref silent here -> attenuate (kills garbage)
        blk=(BASIS@out)*WIN
        cur=blk.std(); tgt=(Rf[:,0].std()+Rf[:,1].std())/2/1.5
        if cur>1e-9: blk*=tgt/cur
        L[fr*N:fr*N+2*N,ci]+=blk
o=np.zeros((NFR*N,2)); o[LAG:]=L[:NFR*N-LAG]
mx=np.abs(o).max()
if mx>0: o*=0.9*32767/mx
wv=wave.open('kw_ims2.wav','wb'); wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(48000)
wv.writeframes(o.astype(np.int16).tobytes()); wv.close()
def bands(x):
    m=x.mean(1); M=4096; mags=[]
    for i in range(0,len(m)-M,2048):
        s=m[i:i+M]
        if np.abs(s).max()<200: continue
        mags.append(np.abs(np.fft.rfft(s*np.hanning(M))))
    mm=np.mean(mags,0); f=np.fft.rfftfreq(M,1/48000); t=(mm**2).sum()
    return [(lo,hi,100*(mm[(f>=lo)&(f<hi)]**2).sum()/t) for lo,hi in [(0,500),(500,2000),(2000,4000),(4000,8000),(8000,20000)]]
print('band       IMS2   ref')
for (lo,hi,a),(_,_,b) in zip(bands(o),bands(REF)): print(f'  {lo:5}-{hi:5}  {a:5.1f}% {b:5.1f}%')
print(f'peak={np.abs(o).max():.0f}')
