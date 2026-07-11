import sys
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
from ac4scan import T, Bits, huff

DATA = open(sys.argv[1], 'rb').read()
def bit(i): return (DATA[i//8] >> (7-(i%8))) & 1
NATS = 16

def hcb(name): return T[name+'_LEN'], T[name+'_CW']

def read_int_class(b):
    if b.u(1)==0: return 'FF'
    if b.u(1)==0: return 'FV'
    if b.u(1)==0: return 'VF'
    return 'VV'

def ceil_log2(n):
    k=0
    while (1<<k)<n: k+=1
    return k

def decode_framing(b, iframe):
    ic = read_int_class(b)
    nrl=nrr=0
    if ic=='FF':
        ne = 1 << b.u(1)
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
    if ic!='FF': b.u(ceil_log2(ne+2))
    return ic, ne, (2 if ne>1 else 1)

def decode_trailer(pos, iframe, P):
    # P: dict(qtab, hires, lores, noisen, ahlen, ticlen, fres0, fres1)
    b = Bits(DATA, pos)
    if iframe: b.u(3)
    ic, ne, nn = decode_framing(b, iframe)
    bal = b.u(1)
    ne1, nn1 = ne, nn
    if not bal:
        ic1, ne1, nn1 = decode_framing(b, iframe)
    sd0=[b.u(1) for _ in range(ne)]; nd0=[b.u(1) for _ in range(nn)]
    sd1=[b.u(1) for _ in range(ne1)]; nd1=[b.u(1) for _ in range(nn1)]
    # hfgen
    for _ in range(P['noisen']): b.u(2)
    if not bal:
        for _ in range(P['noisen']): b.u(2)
    if b.u(1):
        for _ in range(P['ahlen']): b.u(1)
    if b.u(1):
        for _ in range(P['ahlen']): b.u(1)
    if b.u(1):
        if b.u(1):
            for _ in range(P['ahlen']): b.u(1)
        if b.u(1):
            for _ in range(P['ahlen']): b.u(1)
    if b.u(1):
        cp=b.u(1); tl=tr=0
        if not cp: tl=b.u(1); tr=b.u(1)
        if cp or tl:
            for _ in range(P['ticlen']): b.u(1)
        if tr:
            for _ in range(P['ticlen']): b.u(1)
    # 4 ec blocks
    def ec(kind, n_env, dirs, fmask, sm):
        for env in range(n_env):
            if kind=='S':
                n = P['hires'] if (fmask>>env)&1 else P['lores']
                fam='ASPX_HCB_ENV_%s_%s' % (sm, P['qtab'])
            else:
                n = P['noisen']
                fam='ASPX_HCB_NOISE_%s' % sm
            if dirs[env]==0:
                if P.get('f0raw'):
                    b.u(P['f0raw'] if kind=='S' else P.get('f0raw_n', P['f0raw']))
                else:
                    l,c=hcb(fam+'_F0'); huff(b,l,c)
                l,c=hcb(fam+'_DF')
                for _ in range(n-1): huff(b,l,c)
            else:
                l,c=hcb(fam+'_DT')
                for _ in range(n): huff(b,l,c)
    sm1 = 'BALANCE' if bal else 'LEVEL'
    ec('S', ne, sd0, P['fres0'], 'LEVEL')
    ec('S', ne1, sd1, P['fres1'], sm1)
    ec('N', nn, nd0, 0, 'LEVEL')
    ec('N', nn1, nd1, 0, sm1)
    return b.p

T1, TARGET = int(sys.argv[2]), int(sys.argv[3])
IFRAME = sys.argv[4]=='i'
base = dict(hires=6, lores=3)
hits=0
for qtab in ('30','15'):
    for f0raw in (0,5,6,7):
      for f0raw_n in ((0,) if f0raw==0 else (5,6,7)):
        for noisen in (1,2):
            for ahlen in (6,):
              for ticlen in (16,):
                x1set={}
                for f0 in range(16):
                    for f1 in range(16):
                        P=dict(base, qtab=qtab, noisen=noisen, ahlen=ahlen, ticlen=ticlen, fres0=f0, fres1=f1, f0raw=f0raw, f0raw_n=f0raw_n)
                        try: x1=decode_trailer(T1, IFRAME, P)
                        except Exception: continue
                        if x1+3>=TARGET: continue
                        if IFRAME and (bit(x1) or bit(x1+1) or bit(x1+2)): continue
                        x1set.setdefault(x1, (f0,f1))
                for x1,(f0a,f1a) in x1set.items():
                    for f0 in range(16):
                        for f1 in range(16):
                            P=dict(base, qtab=qtab, noisen=noisen, ahlen=ahlen, ticlen=ticlen, fres0=f0, fres1=f1, f0raw=f0raw, f0raw_n=f0raw_n)
                            try: x2=decode_trailer(x1, IFRAME, P)
                            except Exception: continue
                            if x2==TARGET:
                                hits+=1
                                print("HIT q=%s f0raw=%d/%d noisen=%d | t1 fres(%x,%x)->%d | t2 fres(%x,%x)" % (qtab,f0raw,f0raw_n,noisen,f0a,f1a,x1,f0,f1))
print("done", hits)
