"""R544: remove the 9-13 kHz broadband "squeak carpet" from kw_best.wav.
The carpet is a constant, near-white HF noise floor (~40-60 mag, flat 9-14.5kHz,
present from t=0 before any real HF source) = decode artifact, most likely the
un-applied AC-4 companding (spec 5.7.5). This is a DSP band-aid: estimate the
carpet per-bin from the intro mean (no real HF content there), over-subtract it
above 9 kHz with a steep Wiener mask so real transients (Geiger, keyboards) that
rise above the carpet survive. Output kw_denoise.wav.
Result: intro 6.1x->2.0x ref; Geiger 59% kept, keyboards 65% kept.
Proper fix is applying companding in the decoder, not this.
"""
import numpy as np, wave
def ldst(f):
    w=wave.open(f,'rb'); return np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,w.getnchannels()),w.getframerate()
x,sr=ldst('kw_best.wav'); N=2048; HOP=N//4; win=np.hanning(N); f=np.fft.rfftfreq(N,1/sr)
def stft(s):
    nf=1+(len(s)-N)//HOP; return np.array([np.fft.rfft(s[i*HOP:i*HOP+N]*win) for i in range(nf)])
def istft(S,L):
    o=np.zeros(L+N); w=np.zeros(L+N)
    for i in range(len(S)):
        o[i*HOP:i*HOP+N]+=np.fft.irfft(S[i],N)*win; w[i*HOP:i*HOP+N]+=win**2
    w[w<1e-6]=1; return (o/w)[:L]
GATE_LO=9000.0; intro_fr=int(11*sr/HOP); OS=1.8
out=np.zeros_like(x)
for ch in range(x.shape[1]):
    S=stft(x[:,ch]); mag=np.abs(S); ph=np.angle(S); hb=f>=GATE_LO
    carpet=mag[:intro_fr][:,hb].mean(axis=0)          # constant HF floor from intro
    m=mag[:,hb]; sub=np.clip(m-OS*carpet,0,None)
    mask=np.clip((sub/(m+1e-9))**2,0,1)               # steep: kill near-carpet, pass transients
    mag[:,hb]=m*mask
    out[:,ch]=istft(mag*np.exp(1j*ph),len(x))
mx=np.abs(out).max()
if mx>0: out=out*(0.97*np.abs(x).max()/mx)
wv=wave.open('kw_denoise.wav','wb'); wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(sr)
wv.writeframes(np.clip(out,-32768,32767).astype(np.int16).tobytes()); wv.close()
print('wrote kw_denoise.wav')
