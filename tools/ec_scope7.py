import sys
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
from ac4scan import T, Bits
NATS=16

# Fast huffman: dict[(len, code)] -> True per codebook
HC = {}
def hdec(b, name):
    tab = HC.get(name)
    if tab is None:
        lens, cws = T[name+'_LEN'], T[name+'_CW']
        tab = {}
        for l, c in zip(lens, cws):
            tab[(l, c)] = True
        HC[name] = (tab, max(lens))
        tab = HC[name]
    d, mx = tab
    acc = 0; n = 0
    while n < mx:
        acc = (acc<<1) | b.u(1); n += 1
        if (n, acc) in d:
            return
    raise ValueError

def read_ic(b):
    if b.u(1)==0: return 'FF'
    if b.u(1)==0: return 'FV'
    if b.u(1)==0: return 'VF'
    return 'VV'
def cl2(n):
    k=0
    while (1<<k)<n: k+=1
    return k
def framing(b, iframe):
    ic=read_ic(b); nrl=nrr=0
    if ic=='FF': ne=1<<b.u(1)
    elif ic=='FV':
        b.u(2); nrr=b.u(2)
        for _ in range(nrr): b.u(2)
        ne=nrr+1
    elif ic=='VF':
        if iframe: b.u(2)
        nrl=b.u(2)
        for _ in range(nrl): b.u(2)
        ne=nrl+1
    else:
        if iframe: b.u(2)
        nrl=b.u(2)
        for _ in range(nrl): b.u(2)
        b.u(2); nrr=b.u(2)
        for _ in range(nrr): b.u(2)
        ne=nrl+nrr+1
    if ic!='FF': b.u(cl2(ne+2))
    return ne,(2 if ne>1 else 1)
def ec(b,P,kind,n_env,dirs,fmask,sm):
    hires=6-P['xov']; lores=(hires+1)//2
    for env in range(n_env):
        if kind=='S':
            n = hires if (fmask>>env)&1 else lores
            fam='ASPX_HCB_ENV_%s_%s'%(sm,P['q']); f0=P['f0s']
        else:
            n=1; fam='ASPX_HCB_NOISE_%s'%sm; f0=P['f0n']
        if dirs[env]==0:
            b.u(f0)
            for _ in range(n-1): hdec(b,fam+'_DF')
        else:
            for _ in range(n): hdec(b,fam+'_DT')
def t2ch(b,P,iframe,m0,m1):
    if iframe: P['xov']=b.u(3)
    ne,nn = framing(b,iframe)
    bal=b.u(1)
    ne1,nn1=ne,nn
    if not bal: ne1,nn1=framing(b,iframe)
    sd0=[b.u(1) for _ in range(ne)]; nd0=[b.u(1) for _ in range(nn)]
    sd1=[b.u(1) for _ in range(ne1)]; nd1=[b.u(1) for _ in range(nn1)]
    b.u(2)
    if not bal: b.u(2)
    h=6-P['xov']
    if b.u(1): b.u(h)
    if b.u(1): b.u(h)
    if b.u(1):
        if b.u(1): b.u(h)
        if b.u(1): b.u(h)
    if b.u(1):
        cp=b.u(1); tl=tr=0
        if not cp: tl=b.u(1); tr=b.u(1)
        if cp or tl: b.u(NATS)
        if tr: b.u(NATS)
    sm1='BALANCE' if bal else 'LEVEL'
    ec(b,P,'S',ne,sd0,m0,'LEVEL'); ec(b,P,'S',ne1,sd1,m1,sm1)
    ec(b,P,'N',nn,nd0,0,'LEVEL'); ec(b,P,'N',nn1,nd1,0,sm1)
def t1ch(b,P,iframe,m0):
    if iframe: P['xov']=b.u(3)
    ne,nn=framing(b,iframe)
    sd=[b.u(1) for _ in range(ne)]; nd=[b.u(1) for _ in range(nn)]
    h=6-P['xov']
    b.u(2)
    if b.u(1): b.u(h)
    if b.u(1): b.u(h)
    if b.u(1): b.u(NATS)
    ec(b,P,'S',ne,sd,m0,'LEVEL'); ec(b,P,'N',nn,nd,0,'LEVEL')

DATA=open(sys.argv[1],'rb').read()
st, wall = int(sys.argv[2]), int(sys.argv[3])
IFR = sys.argv[4]=='i'
fams=[(q,a,b2) for q in ('30','15') for a in (5,6,7) for b2 in (5,6,7)]
for (q,f0s,f0n) in fams:
    # staged tree: set of positions after each trailer
    pos = {st}
    for stage, kind in enumerate('2212'):
        nxt = set()
        for p in pos:
            for m in range(16):
                b = Bits(DATA, p)
                P = dict(q=q, f0s=f0s, f0n=f0n, xov=0)
                try:
                    if kind=='2': t2ch(b,P,IFR,m,m)
                    else: t1ch(b,P,IFR,m)
                except Exception:
                    continue
                if b.p < wall:
                    nxt.add(b.p)
                elif b.p <= wall and stage==3:
                    nxt.add(b.p)
        pos = nxt
        if not pos: break
    close = [p for p in pos if 0 <= wall-p <= 8]
    print("q=%s f0=%d/%d: stages-ok=%s closures=%d %s" % (q,f0s,f0n,bool(pos),len(close),sorted(close)[:3]))
