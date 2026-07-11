import sys
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
from ac4scan import T, Bits, huff
DATA = open(sys.argv[1], 'rb').read()
def bit(i): return (DATA[i//8] >> (7-(i%8))) & 1
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
def framing(b, iframe):
    ic = read_ic(b); nrl=nrr=0
    if ic=='FF': ne = 1<<b.u(1)
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
    return ne, (2 if ne>1 else 1)
def ec(b, P, kind, n_env, dirs, fmask, sm):
    for env in range(n_env):
        if kind=='S':
            n = 6 if (fmask>>env)&1 else 3
            # xover-shifted tables: hires = 6-x, lores = ceil/2
            hires = 6-P['xov']; lores=(hires+1)//2
            n = hires if (fmask>>env)&1 else lores
            fam='ASPX_HCB_ENV_%s_%s' % (sm, P['qtab']); f0b=P['f0s']
        else:
            n = 1; fam='ASPX_HCB_NOISE_%s' % sm; f0b=P['f0n']
        if dirs[env]==0:
            b.u(f0b)
            l,c=hcb(fam+'_DF')
            for _ in range(n-1): huff(b,l,c)
        else:
            l,c=hcb(fam+'_DT')
            for _ in range(n): huff(b,l,c)
def t2ch(b, P, iframe):
    if iframe: P['xov']=b.u(3)
    ne, nn = framing(b, iframe)
    bal = b.u(1)
    ne1, nn1 = ne, nn
    if not bal: ne1, nn1 = framing(b, iframe)
    sd0=[b.u(1) for _ in range(ne)]; nd0=[b.u(1) for _ in range(nn)]
    sd1=[b.u(1) for _ in range(ne1)]; nd1=[b.u(1) for _ in range(nn1)]
    b.u(2)
    if not bal: b.u(2)
    hires = 6-P['xov']
    if b.u(1):
        for _ in range(hires): b.u(1)
    if b.u(1):
        for _ in range(hires): b.u(1)
    if b.u(1):
        if b.u(1):
            for _ in range(hires): b.u(1)
        if b.u(1):
            for _ in range(hires): b.u(1)
    if b.u(1):
        cp=b.u(1); tl=tr=0
        if not cp: tl=b.u(1); tr=b.u(1)
        if cp or tl:
            for _ in range(NATS): b.u(1)
        if tr:
            for _ in range(NATS): b.u(1)
    sm1='BALANCE' if bal else 'LEVEL'
    ec(b,P,'S',ne,sd0,P['m0'],'LEVEL'); ec(b,P,'S',ne1,sd1,P['m1'],sm1)
    ec(b,P,'N',nn,nd0,0,'LEVEL'); ec(b,P,'N',nn1,nd1,0,sm1)
def t1ch(b, P, iframe):
    if iframe: P['xov']=b.u(3)
    ne, nn = framing(b, iframe)
    sd=[b.u(1) for _ in range(ne)]; nd=[b.u(1) for _ in range(nn)]
    hires = 6-P['xov']
    b.u(2)
    if b.u(1):
        for _ in range(hires): b.u(1)
    if b.u(1):
        for _ in range(hires): b.u(1)
    if b.u(1):
        for _ in range(NATS): b.u(1)
    ec(b,P,'S',ne,sd,P['m0'],'LEVEL'); ec(b,P,'N',nn,nd,0,'LEVEL')

# P-frame 4-trailer block: [tstart..wall), slots [0,0,0,4]
tstart, wall = int(sys.argv[2]), int(sys.argv[3])
fams = [('30',5,7),('30',6,7),('30',7,5),('15',5,5),('15',6,5),('15',6,7),('15',7,7)]
for (q,f0s,f0n) in fams:
    good=[]
    for m in range(4096):
        masks=[(m>>(3*i))&7 for i in range(4)]
        b=Bits(DATA,tstart)
        try:
            P=dict(qtab=q,f0s=f0s,f0n=f0n,xov=0,m0=masks[0]&7,m1=masks[0]>>0)
            P['xov']=0; P['m0']=masks[0]; P['m1']=masks[0]
            t2ch(b,P,False)
            P['xov']=0; P['m0']=masks[1]; P['m1']=masks[1]
            t2ch(b,P,False)
            P['xov']=0; P['m0']=masks[2]; P['m1']=masks[2]
            t1ch(b,P,False)
            P['xov']=4; P['m0']=masks[3]; P['m1']=masks[3]
            t2ch(b,P,False)
        except Exception:
            continue
        if 0 <= wall-b.p <= 8:
            good.append(masks)
    print("family q=%s f0=%d/%d: %d closing mask-sets" % (q,f0s,f0n,len(good)))
