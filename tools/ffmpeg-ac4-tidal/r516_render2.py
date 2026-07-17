import numpy as np, struct, wave
def loadf32(fn):
    d=open(fn,'rb').read(); i=d.find(b'data'); n=struct.unpack('<I',d[i+4:i+8])[0]
    return np.frombuffer(d[i+8:i+8+n],dtype=np.float32).astype(np.float64).reshape(-1,8)
x=loadf32('/tmp/kw_f32.wav')
L,R,C,LFE,Ls,Rs,Lb,Rb=[x[:,i] for i in range(8)]
Lm=L+0.707*C+0.707*Ls+0.707*Lb+0.5*LFE
Rm=R+0.707*C+0.707*Rs+0.707*Rb+0.5*LFE
st=np.stack([Lm,Rm],1)
fr=2048; m=len(st)//fr
pk=np.array([np.abs(st[i*fr:(i+1)*fr]).max() for i in range(m)])
ref_level=np.median(pk[pk>1e-6])       # robust central magnitude
print('robust ref level (median peak): %.1f'%ref_level)
# scale so median peak -> 0.25, outlier frames clip
g=0.25/ref_level
y=np.clip(st*g,-1,1)
# soft fade any fully-clipping (blowup) frames to reduce harshness
for i in range(m):
    seg=y[i*fr:(i+1)*fr]
    if np.mean(np.abs(seg)>0.99)>0.5:   # >50% clipped = blowup frame
        y[i*fr:(i+1)*fr]*=0.1
y-=y.mean(0)
pcm=(y*32767*0.95).astype(np.int16)
w=wave.open('/tmp/kw_core_v12.wav','wb');w.setnchannels(2);w.setsampwidth(2);w.setframerate(48000)
w.writeframes(pcm.tobytes());w.close()
print('wrote /tmp/kw_core_v12.wav %.1fs'%(len(pcm)/48000))
print('frames clipping >50%%:', int(sum(np.mean(np.abs(y[i*fr:(i+1)*fr])>0.9)>0.5 for i in range(m))))
