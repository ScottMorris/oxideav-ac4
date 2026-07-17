import numpy as np, struct, wave
def loadf32(fn):
    d=open(fn,'rb').read(); i=d.find(b'data'); n=struct.unpack('<I',d[i+4:i+8])[0]
    return np.frombuffer(d[i+8:i+8+n],dtype=np.float32).astype(np.float64).reshape(-1,8)
mine=loadf32('/tmp/kw_f32.wav').mean(1)
w=wave.open('kw-ref51.wav','rb');rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(float).reshape(-1,rc).mean(1)
SR=48000; NFFT=2048; HOP=512
def melspec(x,nmel=32,fmax=6000):
    m=(len(x)-NFFT)//HOP
    win=np.hanning(NFFT)
    # mel filterbank
    def hz2mel(f):return 2595*np.log10(1+f/700)
    def mel2hz(mel):return 700*(10**(mel/2595)-1)
    mels=np.linspace(hz2mel(50),hz2mel(fmax),nmel+2)
    hz=mel2hz(mels); bins=np.floor((NFFT+1)*hz/SR).astype(int)
    fb=np.zeros((nmel,NFFT//2+1))
    for j in range(nmel):
        for k in range(bins[j],bins[j+1]): 
            if bins[j+1]>bins[j]: fb[j,k]=(k-bins[j])/(bins[j+1]-bins[j])
        for k in range(bins[j+1],bins[j+2]):
            if bins[j+2]>bins[j+1]: fb[j,k]=(bins[j+2]-k)/(bins[j+2]-bins[j+1])
    S=np.zeros((m,nmel))
    for i in range(m):
        P=np.abs(np.fft.rfft(x[i*HOP:i*HOP+NFFT]*win))**2
        S[i]=np.log(fb@P+1e-6)
    return S
Sm=melspec(mine); Sr=melspec(ref)
n=min(len(Sm),len(Sr)); Sm=Sm[:n]; Sr=Sr[:n]
# normalize each mel band over time (z-score) so master loudness diffs cancel
def z(S):
    return (S-S.mean(0))/(S.std(0)+1e-9)
Zm=z(Sm); Zr=z(Sr)
# cross-correlate the flattened mel-spectrograms over time lag
def frame_corr(A,B):
    a=A.ravel()-A.mean(); b=B.ravel()-B.mean()
    d=a.std()*b.std(); return float((a*b).mean()/d) if d>1e-9 else 0
best=(0,0)
for lag in range(-400,401,2):
    if lag>=0: c=frame_corr(Zm[lag:n],Zr[:n-lag] if lag else Zr)
    else: c=frame_corr(Zm[:n+lag],Zr[-lag:])
    if abs(c)>abs(best[1]):best=(lag,c)
print('MEL-SPECTROGRAM corr (master-invariant), best lag %d frames (%.2fs): corr %.3f'%(best[0],best[0]*HOP/SR,best[1]))
# null control: reverse mine in time
Zmr=Zm[::-1]
bn=0
for lag in range(-400,401,2):
    if lag>=0: c=frame_corr(Zmr[lag:n],Zr[:n-lag] if lag else Zr)
    else: c=frame_corr(Zmr[:n+lag],Zr[-lag:])
    if abs(c)>abs(bn):bn=c
print('null (time-reversed) best corr: %.3f'%bn)
