#!/usr/bin/env python3
"""R500: joint header-grammar enumeration over ALL frames (task #12).

For each candidate header layout (a field sequence), decode every clean
P-frame from its exact LFE end and score ACROSS frames:
  - parse ok rate
  - coding_config concentration (real config fields are skewed)
  - msfb concentration (top-2 mass) and mode value
  - long/short fraction (walk census says ~60/40)
  - section-parse success at the predicted first-section position,
    under the header's own msfb (self-consistency)
  - delta between predicted section start and the validated harvest
    first-body position (constant offset = win)

Candidate dimensions:
  lfe_w      : LFE section-length width 3|5
  pre        : 0..2 unknown flag bits before coding_config
  cc         : coding_config present 0|1
  msfb_bits  : 5|6|7
  matsel     : chel_matsel u(4) present 0|1
  nchp       : number of chparam_info blocks 0|2|3|5
Header order per spec: [pre][cc][transform_info][msfb(+dual)][matsel]
[chparam*n], then sections begin.
chparam: sap_mode u(2); 1 -> msfb-bit bitmap; 3 -> parse fail (rare).
transform_info: b_long u(1); if short: tl0 u(2), tl1 u(2); if
tl0 != tl1: second msfb field (b_different_framing).
"""
import sys, json, re, itertools
from collections import Counter
import numpy as np
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A

T = A.T


def lfe_end(d, P, w):
    """exact LFE end for clean shape; returns end or None if not clean."""
    b = A.Bits(d, P)
    m = b.u(3)
    if m != 3:
        return None
    k = 0
    nsect = 0
    while k < m:
        cb = b.u(4)
        esc = (1 << w) - 1
        ln = 1
        li = b.u(w)
        while li == esc:
            ln += esc
            li = b.u(w)
        ln += li
        if cb <= 11 and cb != 0:
            return None      # payload section -> not the clean shape
        k += ln
        nsect += 1
        if nsect > 4 or k > 127:
            return None
    b.u(8)                   # ref_sf
    if b.u(1):
        return None          # snf present -> excluded (variant-free set)
    return b.p


def parse_header(d, E, hp):
    """returns dict(fields) or None."""
    b = A.Bits(d, E)
    out = {}
    if hp['pre']:
        out['pre'] = b.u(hp['pre'])
    if hp['cc']:
        out['cc'] = b.u(2)
        if out['cc'] == 0:
            out['m2'] = b.u(1)
    lng = b.u(1)
    out['long'] = lng
    dual = 0
    if not lng:
        tl0 = b.u(2); tl1 = b.u(2)
        out['tl'] = (tl0, tl1)
        dual = 1 if tl0 != tl1 else 0
    out['msfb'] = b.u(hp['msfb_bits'])
    if dual:
        out['msfb2'] = b.u(hp['msfb_bits'])
    if hp['matsel']:
        out['matsel'] = b.u(4)
    for i in range(hp['nchp']):
        sm = b.u(2)
        if sm == 1:
            b.u(max(out['msfb'], 1))
        elif sm == 3:
            return None
    out['sect_start'] = b.p
    return out


def sections_ok(d, P, msfb, w, wall):
    """parse a section list under msfb; True if it terminates sanely."""
    if msfb < 1 or msfb > 56:
        return False
    try:
        b = A.Bits(d, P)
        k = 0
        n = 0
        esc = (1 << w) - 1
        while k < msfb:
            cb = b.u(4)
            ln = 1
            li = b.u(w)
            while li == esc:
                ln += esc
                li = b.u(w)
            ln += li
            k += ln
            n += 1
            if k > 127 or n > 24 or b.p > wall:
                return False
        return True
    except Exception:
        return False


if __name__ == '__main__':
    firsts = {}
    for ln in open('backext_kw.log'):
        mm = re.match(r'f(\d+): PRE (\{.*\})', ln)
        if mm:
            r = json.loads(mm.group(2)); firsts[r['fr']] = r['first']
    walls = {}
    for ln in open('multibody_kw.log'):
        mm = re.match(r'f(\d+): BODIES (\{.*\})', ln)
        if mm:
            r = json.loads(mm.group(2)); walls[r['fr']] = r['wall']

    frames = []
    for fr in sorted(walls):
        if fr % 24 == 0:
            continue
        try:
            d = open(f'kw4/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        frames.append((fr, d))
    print(f'{len(frames)} P-frames loaded')

    HPS = []
    for pre, cc, mb, mat, nchp in itertools.product(
            (0, 1, 2), (1, 0), (5, 6, 7), (1, 0), (0, 2, 3, 5)):
        HPS.append(dict(pre=pre, cc=cc, msfb_bits=mb, matsel=mat, nchp=nchp))

    results = []
    for lfe_w in (5, 3):
        # precompute clean LFE ends
        ends = {}
        for fr, d in frames:
            e = lfe_end(d, 18, lfe_w)
            if e is not None:
                ends[fr] = e
        for hp in HPS:
            n = ok = 0
            ccs = Counter(); msfbs = Counter(); longs = 0
            sec_ok = 0
            deltas = Counter()
            for fr, d in frames:
                if fr not in ends:
                    continue
                E = ends[fr]
                n += 1
                out = parse_header(d, E, hp)
                if out is None:
                    continue
                ok += 1
                if 'cc' in out:
                    ccs[out['cc']] += 1
                msfbs[out['msfb']] += 1
                longs += out['long']
                wall = walls.get(fr, len(d) * 8)
                sw = 5 if out['long'] else 3
                if sections_ok(d, out['sect_start'], out['msfb'], sw, wall):
                    sec_ok += 1
                f1 = firsts.get(fr)
                if f1 and f1 < 400:
                    dd = out['sect_start'] - f1
                    if -64 <= dd <= 64:
                        deltas[dd] += 1
            if ok < 200:
                continue
            top2 = sum(x for _, x in msfbs.most_common(2)) / max(ok, 1)
            ccmax = (max(ccs.values()) / sum(ccs.values())) if ccs else -1
            dtop = deltas.most_common(1)
            score = (sec_ok / ok) + top2 + (ccmax if ccmax >= 0 else 0.5)
            results.append((score, lfe_w, hp, ok, n, sec_ok / ok, top2,
                            msfbs.most_common(3), ccmax, longs / ok,
                            dtop))
    results.sort(key=lambda r: -r[0])
    print('\nTOP 12 hypotheses:')
    for (sc, w, hp, ok, n, so, t2, mtop, ccm, lf, dt) in results[:12]:
        print(f'score {sc:.2f} lfe_w{w} pre{hp["pre"]} cc{hp["cc"]} '
              f'mb{hp["msfb_bits"]} mat{hp["matsel"]} chp{hp["nchp"]} | '
              f'ok {ok}/{n} secOK {so:.2f} msfb_top2 {t2:.2f} '
              f'{mtop} ccmax {ccm:.2f} long {lf:.2f} dtop {dt}')
