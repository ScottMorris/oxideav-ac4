import sys, itertools
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
from ac4scan import T, Bits, huff

DATA = open(sys.argv[1], 'rb').read()
def bit(i): return (DATA[i//8] >> (7-(i%8))) & 1

HIRES, LORES, NOISE_SBG, NATS = 6, 3, 1, 16

def hcb(name):
    return T[name+'_LEN'], T[name+'_CW']

def read_int_class(b):
    if b.u(1)==0: return 'FF'
    if b.u(1)==0: return 'FV'
    if b.u(1)==0: return 'VF'
    return 'VV'

def ceil_log2(n):
    k=0
    while (1<<k) < n: k+=1
    return k

def decode_framing(b, iframe):
    ic = read_int_class(b)
    nrl=nrr=0
    if ic=='FF':
        tmp = b.u(1)  # envbits = num_env_bits_fixfix+1 = 1
        ne = 1<<tmp
        # freq_res_mode=2: no in-band freq_res bit
    elif ic=='FV':
        b.u(2)              # var_bord_right
        nrr = b.u(2)
        for _ in range(nrr): b.u(2)
        ne = nrr+1
    elif ic=='VF':
        if iframe: b.u(2)   # var_bord_left
        nrl = b.u(2)
        for _ in range(nrl): b.u(2)
        ne = nrl+1
    else:
        if iframe: b.u(2)
        nrl = b.u(2)
        for _ in range(nrl): b.u(2)
        b.u(2)
        nrr = b.u(2)
        for _ in range(nrr): b.u(2)
        ne = nrl+nrr+1
    if ic!='FF':
        b.u(ceil_log2(ne+2))  # tsg_ptr
    return ic, ne, (2 if ne>1 else 1)

def decode_dirs(b, ne, nn):
    sig = [b.u(1) for _ in range(ne)]
    noi = [b.u(1) for _ in range(nn)]
    return sig, noi

def decode_hfgen2(b, balance):
    for _ in range(NOISE_SBG): b.u(2)
    if not balance:
        for _ in range(NOISE_SBG): b.u(2)
    if b.u(1):
        for _ in range(HIRES): b.u(1)
    if b.u(1):
        for _ in range(HIRES): b.u(1)
    if b.u(1):
        if b.u(1):
            for _ in range(HIRES): b.u(1)
        if b.u(1):
            for _ in range(HIRES): b.u(1)
    if b.u(1):
        cp = b.u(1); tl = tr = 0
        if not cp:
            tl = b.u(1); tr = b.u(1)
        if cp or tl:
            for _ in range(NATS): b.u(1)
        if tr:
            for _ in range(NATS): b.u(1)

def decode_ec(b, kind, ne, dirs, fres_mask, qtab, sm, count_rule):
    # kind: 'SIG'/'NOISE'; sm: 'LEVEL'/'BALANCE'
    for env in range(ne):
        if kind=='SIG':
            hi = (fres_mask>>env)&1
            n = HIRES if hi else LORES
            if count_rule=='bal_lores' and sm=='BALANCE':
                n = LORES
            fam = 'ASPX_HCB_ENV_%s_%s' % (sm, qtab)
        else:
            n = NOISE_SBG
            fam = 'ASPX_HCB_NOISE_%s' % sm
        d = dirs[env]
        if d==0:
            l,c = hcb(fam+'_F0'); huff(b,l,c)
            l,c = hcb(fam+'_DF')
            for _ in range(n-1): huff(b,l,c)
        else:
            l,c = hcb(fam+'_DT')
            for _ in range(n): huff(b,l,c)

def decode_trailer_2ch(pos, iframe, qtab, count_rule, fres0, fres1):
    b = Bits(DATA, pos)
    if iframe: b.u(3)
    ic, ne, nn = decode_framing(b, iframe)
    bal = b.u(1)
    if not bal:
        ic1, ne1, nn1 = decode_framing(b, iframe)
    else:
        ne1, nn1 = ne, nn
    sd0, nd0 = decode_dirs(b, ne, nn)
    sd1, nd1 = decode_dirs(b, ne1, nn1)
    decode_hfgen2(b, bal)
    decode_ec(b,'SIG', ne, sd0, fres0, qtab, 'LEVEL',   count_rule)
    decode_ec(b,'SIG', ne1, sd1, fres1, qtab, 'BALANCE' if bal else 'LEVEL', count_rule)
    decode_ec(b,'NOISE', nn, nd0, 0, qtab, 'LEVEL',  count_rule)
    decode_ec(b,'NOISE', nn1, nd1, 0, qtab, 'BALANCE' if bal else 'LEVEL', count_rule)
    return b.p, ne, ne1, bal

T1, TARGET = int(sys.argv[2]), int(sys.argv[3])
IFRAME = sys.argv[4] == 'i'
hits = {}
for qtab in ('30','15'):
    for cr in ('std','bal_lores'):
        for f0a in range(16):
            for f1a in range(16):
                try:
                    x1, nea, ne1a, bala = decode_trailer_2ch(T1, IFRAME, qtab, cr, f0a, f1a)
                except Exception:
                    continue
                if x1+3 >= TARGET: continue
                if IFRAME and (bit(x1) or bit(x1+1) or bit(x1+2)): continue
                for f0b in range(16):
                    for f1b in range(16):
                        try:
                            x2, neb, ne1b, balb = decode_trailer_2ch(x1, IFRAME, qtab, cr, f0b, f1b)
                        except Exception:
                            continue
                        if x2 == TARGET:
                            k = (qtab, cr, f0a&((1<<nea)-1), f1a&((1<<ne1a)-1), x1, f0b&((1<<neb)-1), f1b&((1<<ne1b)-1))
                            if k not in hits:
                                hits[k]=1
                                print("HIT q=%s rule=%s t1(fres %x/%x)->%d t2(fres %x/%x)->%d [ne %d/%d bal %d,%d]" % (qtab,cr,k[2],k[3],x1,k[5],k[6],TARGET,nea,neb,bala,balb))
print("done", len(hits), "unique hits")
