import sys, os, struct, subprocess, tempfile, wave, glob
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ac4_toc  # per-frame TOC parse -> exact audio-substream offset (root-cause fix)
FFDEC="/tmp/claude-1000/-home-scott-source-riptide/380c6dbb-ae05-48c9-8271-8acffb84e19d/scratchpad/ffmpeg-6.1.2/ffmpeg"
FFSYS="/usr/bin/ffmpeg"
SRC=sys.argv[1]
OUT=sys.argv[2]
os.makedirs(OUT, exist_ok=True)
def bx(d,s,e):
    o=[];p=s
    while p+8<=e:
        sz=struct.unpack('>I',d[p:p+4])[0];t=d[p+4:p+8];bs=p+8;be=p+sz
        if sz==1: sz=struct.unpack('>Q',d[p+8:p+16])[0];bs=p+16;be=p+sz
        elif sz==0: be=e
        if be>e or sz<8: break
        o.append((t,bs,be));p=be
    return o
def find(bs,t):
    for b in bs:
        if b[0]==t: return b
def samples(path):
    d=open(path,'rb').read();top=bx(d,0,len(d));moov=find(top,b'moov');mdat=find(top,b'mdat')
    for trak in [b for b in bx(d,moov[1],moov[2]) if b[0]==b'trak']:
        mdia=find(bx(d,trak[1],trak[2]),b'mdia')
        if not mdia: continue
        minf=find(bx(d,mdia[1],mdia[2]),b'minf');stbl=find(bx(d,minf[1],minf[2]),b'stbl')
        sc=bx(d,stbl[1],stbl[2]);stsd=find(sc,b'stsd')
        if d[stsd[1]+12:stsd[1]+16]!=b'ac-4': continue
        stsz=find(sc,b'stsz');p=stsz[1]
        ssz=struct.unpack('>I',d[p+4:p+8])[0];cnt=struct.unpack('>I',d[p+8:p+12])[0]
        sizes=[ssz]*cnt if ssz else [struct.unpack('>I',d[p+12+4*i:p+16+4*i])[0] for i in range(cnt)]
        cur=mdat[1];out=[]
        for s in sizes: out.append(d[cur:cur+s]);cur+=s
        return out
class Bits:
    def __init__(s,d): s.d=d;s.p=0
    def u(s,n):
        v=0
        for _ in range(n): v=(v<<1)|((s.d[s.p>>3]>>(7-(s.p&7)))&1);s.p+=1
        return v
    def vb(s,n):
        v=0
        while True:
            v+=s.u(n)
            if not s.u(1): return v
            v<<=n;v+=1<<n
def iframe(fr):
    b=Bits(fr);ver=b.u(2)
    if ver==3: ver+=b.vb(2)
    b.u(10)
    if b.u(1):
        if b.u(3)>0: b.u(2)
    b.u(1);b.u(4);return b.u(1)
def decode(mp4, wav_out):
    sm=samples(mp4);N=2048
    with tempfile.TemporaryDirectory() as td:
        wrap=os.path.join(td,'w.ac4');pcm=os.path.join(td,'p.bin')
        with open(wrap,'wb') as w:
            for fr in sm:
                toc=ac4_toc.audio_offset(fr)   # exact per-frame strip (was fixed 29/22 -> desynced odd-TOC frames)
                pl=(bytes([1 if iframe(fr) else 0])+fr[toc:])[:0xFFFE]
                w.write(b'\xAC\x40'+struct.pack('>H',len(pl))+pl)
        env=dict(os.environ,AC4_RAWSUB='1',AC4_NEVER_FAIL='1',AC4_CONCEAL='1',AC4_DUMP_PCM=pcm)
        r=subprocess.run([FFDEC,'-y','-hide_banner','-loglevel','error','-i',wrap,
                          os.path.join(td,'x.wav')],env=env,capture_output=True,text=True)
        fails=r.stderr.count('NEVERFAIL')
        a=np.fromfile(pcm,dtype=np.float32).astype(np.float64)
        nf=a.size//(2*N);a=a[:nf*2*N].reshape(nf,2,N)
        if nf>1 and np.array_equal(a[0],a[1]): a=a[1:]
        # conceal decode-glitch frames: NaN/Inf or blown magnitude (>1e6 vs real ~1e4)
        mag=np.nan_to_num(np.abs(a),nan=np.inf,posinf=np.inf).max(axis=(1,2))
        bad=(~np.isfinite(a).all(axis=(1,2)))|(mag>1e6)
        nglitch=int(bad.sum()); a[bad]=0.0
        o=np.stack([np.concatenate(a[:,0,:]),np.concatenate(a[:,1,:])],axis=1)
        o=np.nan_to_num(o,nan=0.0,posinf=0.0,neginf=0.0)
        o[:256]*=np.linspace(0,1,256)[:,None]
        mx=np.abs(o).max();o=o*(0.95*32767/mx) if mx>0 else o
        wv=wave.open(wav_out,'wb');wv.setnchannels(2);wv.setsampwidth(2);wv.setframerate(48000)
        wv.writeframes(np.clip(o,-32768,32767).astype(np.int16).tobytes());wv.close()
        return len(a),fails+nglitch
def flac(wav, mp4, out):
    subprocess.run([FFSYS,'-y','-hide_banner','-loglevel','error','-i',wav,'-i',mp4,
        '-map','0:a:0','-map','1:v:0','-c:a','flac','-compression_level','8',
        '-c:v','copy','-disposition:v:0','attached_pic','-map_metadata','1',out],check=True)
tracks=sorted(glob.glob(os.path.join(SRC,'*.mp4')))
only=[]
for mp4 in tracks:
    base=os.path.splitext(os.path.basename(mp4))[0]
    if only and base.split(' - ')[0] not in only: continue
    with tempfile.TemporaryDirectory() as td:
        wav=os.path.join(td,'t.wav')
        nf,fails=decode(mp4,wav)
        out=os.path.join(OUT,base+'.flac')
        flac(wav,mp4,out)
        sz=os.path.getsize(out)/1048576
        print(f"{base:42s} {nf*2048/48000/60:4.1f}min  fails={fails:2d}  -> {sz:5.1f}MB")
print("DONE ->", OUT)
