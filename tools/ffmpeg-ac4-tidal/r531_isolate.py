import numpy as np, re
N=2048
sp=np.fromfile('/tmp/spec_base.bin',dtype=np.float32).astype(np.float64)
nf=sp.size//(8*N); sp=sp[:nf*8*N].reshape(nf,8,N)[1:]; nf=sp.shape[0]
dEsum=np.log(np.sum(sp**2,axis=(1,2))+1e-9)
fail=set()
for ln in open('lp0.log'):
    m=re.search(r'NEVERFAIL frame (\d+)',ln)
    if m: fail.add(int(m.group(1))-1)
# jumps between CONSECUTIVE frames that are BOTH clean long (not failed)
clean_jumps=[]; all_jumps=[]
for f in range(1,nf):
    all_jumps.append(abs(dEsum[f]-dEsum[f-1]))
    if f not in fail and (f-1) not in fail:
        clean_jumps.append(abs(dEsum[f]-dEsum[f-1]))
print(f'frame-to-frame |d logE| (nats):')
print(f'  ALL consecutive pairs:        median={np.median(all_jumps):.2f} mean={np.mean(all_jumps):.2f}')
print(f'  CLEAN-long consecutive pairs: median={np.median(clean_jumps):.2f} mean={np.mean(clean_jumps):.2f}  (n={len(clean_jumps)})')
print(f'  reference (from prev run):    ~0.15   |  clean audio ~0.1')
# distribution of clean long-frame absolute energies (are they sane or exploding?)
cleanf=[f for f in range(nf) if f not in fail]
ce=dEsum[cleanf]
print(f'\nclean-long log-energy: min={ce.min():.1f} p10={np.percentile(ce,10):.1f} med={np.median(ce):.1f} p90={np.percentile(ce,90):.1f} max={ce.max():.1f}')
print(f'(spread p90-p10 = {np.percentile(ce,90)-np.percentile(ce,10):.1f} nats = {np.exp(np.percentile(ce,90)-np.percentile(ce,10)):.0f}x energy swing across clean frames)')
