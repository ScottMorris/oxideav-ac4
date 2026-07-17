import numpy as np, wave, struct
def loadf32(fn):
    d=open(fn,'rb').read(); i=d.find(b'data'); n=struct.unpack('<I',d[i+4:i+8])[0]
    return np.frombuffer(d[i+8:i+8+n],dtype=np.float32).astype(np.float64)
mine=loadf32('/tmp/kw_f32.wav').reshape(-1,8)
w=wave.open('kw-ref51.wav','rb');rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(float).reshape(-1,rc)
md=mine.mean(1); rd=ref.mean(1)
# normalize
md=md/ (np.abs(md).max()+1e-9); rd=rd/(np.abs(rd).max()+1e-9)
def spectral_flatness(x,fr=2048):
    m=len(x)//fr; sf=[]
    for i in range(m):
        seg=x[i*fr:(i+1)*fr]*np.hanning(fr)
        P=np.abs(np.fft.rfft(seg))**2+1e-12
        gm=np.exp(np.log(P).mean()); am=P.mean()
        sf.append(gm/am)
    return np.array(sf)
sfm=spectral_flatness(md); sfr=spectral_flatness(rd)
# only frames with energy
em=np.array([np.abs(md[i*2048:(i+1)*2048]).max() for i in range(len(md)//2048)])
er=np.array([np.abs(rd[i*2048:(i+1)*2048]).max() for i in range(len(rd)//2048)])
print('MY decode spectral flatness (energetic frames): median %.3f'%np.median(sfm[em>0.01]))
print('REF spectral flatness (energetic frames):        median %.3f'%np.median(sfr[er>0.01]))
print('(white noise ~1.0, tonal music ~0.05-0.3)')
# master-invariant spectrogram correlation
def spectrogram(x,fr=2048,hop=1024):
    m=(len(x)-fr)//hop
    S=np.zeros((m,fr//2+1))
    win=np.hanning(fr)
    for i in range(m):
        S[i]=np.log(np.abs(np.fft.rfft(x[i*hop:i*hop+fr]*win))**2+1e-9)
    return S
Sm=spectrogram(md); Sr=spectrogram(rd)
# average spectral envelope over time -> compare shape
envm=Sm.mean(0); envr=Sr.mean(0)
def corr(a,b):
    a=a-a.mean();b=b-b.mean();d=a.std()*b.std();return float((a*b).mean()/d) if d>1e-9 else 0
print('avg spectral envelope shape corr (mine vs ref): %.3f'%corr(envm,envr))
# per-frame total-energy envelope over time, lag search (master-invariant-ish)
te_m=Sm.mean(1); te_r=Sr.mean(1)
n=min(len(te_m),len(te_r))
best=(0,0)
for lag in range(-200,201):
    if lag>=0:c=corr(te_m[lag:n],te_r[:n-lag] if lag else te_r)
    else:c=corr(te_m[:n+lag],te_r[-lag:])
    if abs(c)>abs(best[1]):best=(lag,c)
print('log-energy envelope corr over time: best lag %d corr %.3f'%best)
