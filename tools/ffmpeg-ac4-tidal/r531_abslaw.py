import numpy as np, wave
N=2048; LN2=np.log(2)
# spec (SFREL scaled) per frame per channel
sp=np.fromfile('/tmp/spec_base.bin',dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)   # ac4_frame_ctr order, includes frame0
# per (ctr,ch) log energy of SFREL scaled_spec
E_srel=np.log(np.sum(sp**2,axis=2)+1e-9)   # (nf,8)
# ref_sf per (ctr,ch): first occurrence per (frame,ch) in sf.txt (col6=ref_sf)
refsf=np.full((nf,8),np.nan)
seen=set()
for ln in open('/tmp/sf.txt'):
    p=ln.split()
    if len(p)<7: continue
    fr=int(p[0]); ch=int(p[1]); rs=int(p[6])
    if fr<nf and (fr,ch) not in seen:
        refsf[fr,ch]=rs; seen.add((fr,ch))
CH=3
# absolute-law energy = SFREL energy + 0.5*ln2*ref_sf (undo the -ref_sf cancel)
E_abs = E_srel[:,CH] + 0.5*LN2*np.nan_to_num(refsf[:,CH])
E_srel_ch = E_srel[:,CH]
# reference energy envelope (ch1, aligned; dump idx = ctr-1)
w=wave.open('kw-ref51.wav','rb'); rc=w.getnchannels()
ref=np.frombuffer(w.readframes(w.getnframes()),dtype=np.int16).astype(np.float64).reshape(-1,rc)
nfr=ref.shape[0]//N
def refE(rch): return np.log(np.array([np.sum(ref[f*N:(f+1)*N,rch]**2) for f in range(nfr)])+1e-9)
def corr_lag(a,b,ml=20):
    best=(-9,0)
    a=np.nan_to_num(a)
    for lag in range(-ml,ml+1):
        # a index ctr -> dump ctr-1 -> ref frame; align by shifting
        x=a[1:]; y=b   # a[ctr] ~ ref[ctr-1]
        if lag>=0: xx,yy=x[lag:],y[:len(x)-lag]
        else: xx,yy=x[:lag],y[-lag:]
        m=min(len(xx),len(yy)); xx,yy=xx[:m],yy[:m]
        if xx.std()<1e-6 or yy.std()<1e-6: continue
        c=np.corrcoef(xx,yy)[0,1]
        if c>best[0]: best=(c,lag)
    return best
print('per-frame ENERGY envelope correlation vs reference (best ref ch):')
for name,E in [('SFREL (current)',E_srel_ch),('ABSOLUTE (+0.5*ln2*ref_sf)',E_abs)]:
    best=(-9,-1,0)
    for rch in range(rc):
        c,lag=corr_lag(E,refE(rch))
        if c>best[0]: best=(c,rch,lag)
    # jump metric
    jump=np.median(np.abs(np.diff(np.nan_to_num(E))))
    print(f'  {name:28}: corr={best[0]:+.3f} (ref ch{best[1]}, lag{best[2]})  frameJump={jump:.2f}')
# also try: is ref_sf itself the loudness? correlate ref_sf vs ref energy
c,lag=corr_lag(refsf[:,CH],refE(1)); print(f'\n  ref_sf[ch3] alone vs ref energy: corr={c:+.3f}')
