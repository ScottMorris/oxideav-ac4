#!/usr/bin/env python3
"""R473: train/evaluate the true-body prior.

Leave-frames-out CV: train class-weighted logistic regression on 2/3
of frames, report rank-of-true-body per held-out frame (rank among
that frame's candidates sorted by score). Success = true bodies
in top-30 of ~2-6k candidates on most frames.
"""
import sys, json
import numpy as np
from bodyfeat import FEATNAMES

dat = np.load('bodyfeat_spk.npz')
X, y, meta = dat['X'], dat['y'], dat['meta']
frames = meta[:, 0]
print(f'{len(X)} rows, {int(y.sum())} positives, {len(set(frames))} frames')

# standardize
mu = X.mean(0); sd = X.std(0) + 1e-9
Xs = (X - mu) / sd

def train_logistic(Xt, yt, iters=400, lr=0.3, l2=1e-3):
    w = np.zeros(Xt.shape[1] + 1)
    Xb = np.hstack([Xt, np.ones((len(Xt), 1))])
    # class weights: balance
    wpos = len(yt) / (2.0 * max(yt.sum(), 1))
    wneg = len(yt) / (2.0 * max((1 - yt).sum(), 1))
    sw = np.where(yt == 1, wpos, wneg)
    for _ in range(iters):
        z = Xb @ w
        p = 1 / (1 + np.exp(-np.clip(z, -30, 30)))
        g = Xb.T @ (sw * (p - yt)) / len(yt) + l2 * w
        w -= lr * g
    return w

ufr = sorted(set(frames))
rng = np.random.default_rng(11)
rng.shuffle(ufr)
folds = [ufr[i::3] for i in range(3)]
ranks = []
for fi, hold in enumerate(folds):
    tr = ~np.isin(frames, hold)
    w = train_logistic(Xs[tr], y[tr])
    for f in hold:
        sel = frames == f
        if y[sel].sum() == 0: continue
        Xf = np.hstack([Xs[sel], np.ones((int(sel.sum()), 1))])
        sc = Xf @ w
        order = np.argsort(-sc)
        yy = y[sel][order]
        for r_ in np.where(yy == 1)[0]:
            ranks.append((int(f), int(r_), int(sel.sum())))
ranks.sort()
rr = np.array([r for (_, r, _) in ranks])
print(f'true-body ranks (n={len(rr)}): median {int(np.median(rr))}, '
      f'p75 {int(np.percentile(rr, 75))}, top-30: {(rr < 30).mean() * 100:.0f}%, '
      f'top-100: {(rr < 100).mean() * 100:.0f}%')
# feature weights on full data
w = train_logistic(Xs, y)
imp = sorted(zip(FEATNAMES, w[:-1]), key=lambda x: -abs(x[1]))
print('top features:', [(n, round(v, 2)) for n, v in imp[:10]])
np.savez('bodyclf.npz', w=w, mu=mu, sd=sd)
print('saved bodyclf.npz')
