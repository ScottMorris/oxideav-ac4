import sys
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
from ac4scan import T, Bits, huff
NATS = 16
def hcb(name): return T[name+'_LEN'], T[name+'_CW']
def read_ic(b):
    if b.u(1)==0: return 'FF'
    if b.u(1)==0: return 'FV'
    if b.u(1)==0: return 'VF'
    return 'VV'
def cl2(n):
    k=0
    while (1<<k)<n: k+=1
    return k
TAB={1:[0,16],2:[0,8,16],4:[0,4,8,12,16]}
def framing(b, iframe, prev_stop):
    ic = read_ic(b); nrl=nrr=0; vbl=vbr=0; rl=[]; rr=[]
    if ic=='FF':
        ne = 1<<b.u(1)
        sig = TAB[ne][:]
        ptr = -1
    else:
        if ic=='FV':
            vbr=b.u(2); nrr=b.u(2)
            rr=[2*b.u(2)+2 for _ in range(nrr)]
            ne=nrr+1
        elif ic=='VF':
            if iframe: vbl=b.u(2)
            nrl=b.u(2)
            rl=[2*b.u(2)+2 for _ in range(nrl)]
            ne=nrl+1
        else:
            if iframe: vbl=b.u(2)
            nrl=b.u(2)
            rl=[2*b.u(2)+2 for _ in range(nrl)]
            vbr=b.u(2); nrr=b.u(2)
            rr=[2*b.u(2)+2 for _ in range(nrr)]
            ne=nrl+nrr+1
        ptr = b.u(cl2(ne+2)) - 1
        sig=[0]*(ne+1)
        if ic=='FV':
            sig[0]=0; sig[ne]=vbr+NATS
            for t in range(nrr):
                sig[ne-t-1]=sig[ne-t]-rr[t]
        elif ic=='VF':
            sig[0]= vbl if iframe else prev_stop-NATS
            sig[ne]=NATS
            for t in range(nrl): sig[t+1]=sig[t]+rl[t]
        else:
            sig[0]= vbl if iframe else prev_stop-NATS
            for t in range(nrl): sig[t+1]=sig[t]+rl[t]
            sig[ne]=vbr+NATS
            for t in range(nrr):
                sig[ne-t-1]=sig[ne-t]-rr[t]
    # Pseudocode 77 mode 2
    th = NATS/6.0+3.25
    fres=[(1 if ((a< ptr and NATS>8) or (sig[a+1]-sig[a])>th) else 0) for a in range(ne)]
    return ne, (2 if ne>1 else 1), fres, sig[ne]
def ec(b,P,kind,n_env,dirs,fres,sm):
    hires=6-P['xov']; lores=(hires+1)//2
    for env in range(n_env):
        if kind=='S':
            n = hires if fres[env] else lores
            fam='ASPX_HCB_ENV_%s_%s'%(sm,P['q']); f0b=P['f0s']
        else:
            n=1; fam='ASPX_HCB_NOISE_%s'%sm; f0b=P['f0n']
        if dirs[env]==0:
            b.u(f0b)
            l,c=hcb(fam+'_DF')
            for _ in range(n-1): huff(b,l,c)
        else:
            l,c=hcb(fam+'_DT')
            for _ in range(n): huff(b,l,c)
def t2ch(b,P,iframe,prev):
    if iframe: P['xov']=b.u(3)
    ne,nn,fr0,stop0 = framing(b,iframe,prev)
    bal=b.u(1)
    if not bal:
        ne1,nn1,fr1,stop1 = framing(b,iframe,prev)
    else:
        ne1,nn1,fr1 = ne,nn,fr0
    sd0=[b.u(1) for _ in range(ne)]; nd0=[b.u(1) for _ in range(nn)]
    sd1=[b.u(1) for _ in range(ne1)]; nd1=[b.u(1) for _ in range(nn1)]
    b.u(2)
    if not bal: b.u(2)
    hires=6-P['xov']
    if b.u(1): b.u(hires)
    if b.u(1): b.u(hires)
    if b.u(1):
        if b.u(1): b.u(hires)
        if b.u(1): b.u(hires)
    if b.u(1):
        cp=b.u(1); tl=tr=0
        if not cp: tl=b.u(1); tr=b.u(1)
        if cp or tl: b.u(NATS)
        if tr: b.u(NATS)
    sm1='BALANCE' if bal else 'LEVEL'
    ec(b,P,'S',ne,sd0,fr0,'LEVEL'); ec(b,P,'S',ne1,sd1,fr1,sm1)
    ec(b,P,'N',nn,nd0,[0]*nn,'LEVEL'); ec(b,P,'N',nn1,nd1,[0]*nn1,sm1)
def t1ch(b,P,iframe,prev):
    if iframe: P['xov']=b.u(3)
    ne,nn,fr,stop = framing(b,iframe,prev)
    sd=[b.u(1) for _ in range(ne)]; nd=[b.u(1) for _ in range(nn)]
    hires=6-P['xov']
    b.u(2)
    if b.u(1): b.u(hires)
    if b.u(1): b.u(hires)
    if b.u(1): b.u(NATS)
    ec(b,P,'S',ne,sd,fr,'LEVEL'); ec(b,P,'N',nn,nd,[0]*nn,'LEVEL')

DATA=None
def bit(i): return (DATA[i//8]>>(7-(i%8)))&1
anchors = [
 # (file, start, end, iframe, kinds, xovers)
 ('subs/sub00.bin', 17341, 17698, True,  '2212', None),
 ('subs/sub01.bin', 13462, 13720, False, '2212', [0,0,0,4]),
 ('subs/sub02.bin', 12979, 13304, False, '2212', [0,0,0,4]),
 ('subs/sub03.bin', 12399, 12808, False, '2212', [0,0,0,4]),
]
import os
S=os.path.dirname(os.path.abspath(sys.argv[0]))
fams=[('30',5,7),('30',6,7),('30',7,5),('15',5,5),('15',6,5),('15',6,7),('15',7,7),('30',6,5),('30',7,7),('15',5,7),('15',7,5),('30',5,5)]
for (q,f0s,f0n) in fams:
    res=[]
    for (fn, st, en, ifr, kinds, xv) in anchors:
        globals()['DATA'] = open(os.path.join(S,fn),'rb').read()
        best=None
        for prev in (16,17,18,19,20,21,22):
            b=Bits(DATA,st)
            try:
                for i,k in enumerate(kinds):
                    P=dict(q=q,f0s=f0s,f0n=f0n,xov=(0 if xv is None else xv[i]))
                    if k=='2': t2ch(b,P,not not ifr,prev)
                    else: t1ch(b,P,not not ifr,prev)
            except Exception:
                continue
            d = en-b.p
            if best is None or abs(d)<abs(best): best=d
        res.append(best)
    print("q=%s f0=%d/%d: deltas %s" % (q,f0s,f0n,res))
