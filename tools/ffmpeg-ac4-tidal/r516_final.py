import numpy as np, struct, wave
def loadf32(fn):
    d=open(fn,'rb').read(); i=d.find(b'data'); n=struct.unpack('<I',d[i+4:i+8])[0]
    return np.frombuffer(d[i+8:i+8+n],dtype=np.float32).astype(np.float64).reshape(-1,8)
x=loadf32('/tmp/kw_sf.wav')  # SFSIGNED, bounded magnitudes
L,R,C,LFE,Ls,Rs,Lb,Rb=[x[:,i] for i in range(8)]
Lm=L+0.707*C+0.707*Ls+0.707*Lb+0.5*LFE
Rm=R+0.707*C+0.707*Rs+0.707*Rb+0.5*LFE
st=np.stack([Lm,Rm],1)
# global normalize on 99.5th percentile to avoid rare spikes
p=np.percentile(np.abs(st),99.5)
y=np.clip(st/(p+1e-12)*0.7,-1,1)
y-=y.mean(0)
pcm=(y*32767*0.95).astype(np.int16)
w=wave.open('/tmp/kw_core_sfsigned.wav','wb');w.setnchannels(2);w.setsampwidth(2);w.setframerate(48000)
w.writeframes(pcm.tobytes());w.close()
print('wrote kw_core_sfsigned.wav %.1fs, RMS %d'%(len(pcm)/48000, int(np.sqrt((pcm.astype(float)**2).mean()))))
# spectrogram PNG (no matplotlib -> write PGM grayscale)
dm=y.mean(1); NFFT=2048; HOP=1024
m=(len(dm)-NFFT)//HOP
win=np.hanning(NFFT)
S=np.zeros((NFFT//2+1,m))
for i in range(m):
    S[:,i]=np.log(np.abs(np.fft.rfft(dm[i*HOP:i*HOP+NFFT]*win))**2+1e-9)
S=S[:400]  # up to ~9kHz
S=(S-S.min())/(S.max()-S.min()+1e-9)
img=(np.flipud(S)*255).astype(np.uint8)
h,wd=img.shape
with open('/tmp/kw_spec.pgm','wb') as f:
    f.write(b'P5\n%d %d\n255\n'%(wd,h)); f.write(img.tobytes())
print('wrote spectrogram %dx%d'%(wd,h))
