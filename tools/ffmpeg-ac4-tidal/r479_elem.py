#!/usr/bin/env python3
"""R479: walk-guided element parser + end-position validation.

Walk events give element markers (mono/2ch/5ch@pos) and, per body,
'POS msfb@p' / 'AUDIT ... sect@s' / 'POS end@e'. Parse each element
top-down with ac4asf, matching every sf_info's msfb_pos to the walk
and every sf_data's end to the walk. Report exact-match rate. This
proves the parser is bit-faithful and the positions are ground truth.
"""
import sys, json, re
sys.path.insert(0, '/home/scott/source/oxideav-ac4/tools')
import ac4scan as A
import ac4asf

TRACK = sys.argv[1] if len(sys.argv) > 1 else 'kw'
WALK = {'kw': 'kw_mp4_walk.json', 'spk': 'spk_walk.json'}[TRACK]
DUMP = {'kw': 'kw4', 'spk': 'spk4'}[TRACK]
walk = json.load(open(WALK))


def events(rec):
    out = []
    for e in rec['ev']:
        m = re.match(r'POS (mono\(lfe=(\d)\)|2ch|5ch|4ch|1ch)@(\d+)', e)
        if m:
            out.append(('elem', m.group(1).split('(')[0], int(m.group(3)),
                        int(m.group(2)) if m.group(2) else None)); continue
        m = re.match(r'POS msfb=(\d+)@(\d+)', e)
        if m: out.append(('msfb', int(m.group(1)), int(m.group(2)))); continue
        m = re.match(r'AUDIT m=(\d+) g=(\d+) long=(\d+) sect@(\d+)', e)
        if m: out.append(('audit', int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4)))); continue
        m = re.match(r'POS end@(\d+)', e)
        if m: out.append(('end', int(m.group(1)))); continue
        m = re.match(r'POS (aspx_\w+|spec|scf|snf)@(\d+)', e)
        if m: out.append((m.group(1), int(m.group(2)))); continue
    return out


def parse_element(d, evs, i):
    """Parse one element starting at evs[i]=('elem',...). Return
    (list of (msfb_check, end_check), next_i) where checks are
    (mine, walk) tuples; advance i past the element's bodies."""
    kind = evs[i][1]; pos = evs[i][2]; lfe = evs[i][3]
    checks = []
    bits = A.Bits(d, pos)
    cfgs = []
    try:
        if kind == 'mono':
            if lfe == 1:
                cfg = ac4asf.parse_sf_info_lfe(bits)
            else:
                bits.u(1)   # spec_frontend
                cfg = ac4asf.parse_sf_info(bits)
            cfgs = [cfg]
        elif kind == '5ch':
            cfg = ac4asf.parse_sf_info(bits)
            cfgs = [cfg] * 5
            # five_channel_info: chel_matsel(4) + 5 chparam_info -- skip
            # via walk: jump to first body sect (handled below)
        elif kind == '2ch':
            proc = bits.u(1)
            if proc:
                cfg = ac4asf.parse_sf_info(bits, 0, 0, 0)
                cfgs = [cfg, cfg]
            else:
                c0 = ac4asf.parse_sf_info(bits, 0, 0, 0)
                c1 = ac4asf.parse_sf_info(bits, 0, 0, 0)
                cfgs = [c0, c1]
        elif kind == '4ch':
            cfg = ac4asf.parse_sf_info(bits)
            cfgs = [cfg] * 4
        else:
            return checks, i + 1
    except Exception as ex:
        return [('ERR', str(ex))], i + 1

    # collect the msfb POS events for this element (right after markers)
    j = i + 1
    walk_msfbs = []
    while j < len(evs) and evs[j][0] == 'msfb':
        walk_msfbs.append(evs[j][2]); j += 1
    # msfb checks
    for ci, cfg in enumerate(dict.fromkeys(id(c) for c in cfgs)):
        pass
    uniq = []
    seen = set()
    for c in cfgs:
        if id(c) not in seen:
            seen.add(id(c)); uniq.append(c)
    for ci, c in enumerate(uniq):
        if ci < len(walk_msfbs):
            checks.append(('msfb', c.msfb_pos, walk_msfbs[ci]))

    # now bodies: pair sf_data with AUDIT/end events
    bi = 0
    while j < len(evs) and evs[j][0] == 'audit' and bi < len(cfgs):
        sect = evs[j][4]
        # find matching end
        k = j + 1; endpos = None
        while k < len(evs):
            if evs[k][0] == 'end': endpos = evs[k][1]; break
            if evs[k][0] == 'audit': break
            k += 1
        cfg = cfgs[bi]
        try:
            b2 = A.Bits(d, sect)
            data = ac4asf.parse_sf_data(b2, cfg)
            checks.append(('end', data.end, endpos))
        except Exception as ex:
            checks.append(('end_err', str(ex), endpos))
        bi += 1
        # advance j to the end event then continue
        j = k + 1 if endpos is not None else k
    return checks, j


if __name__ == '__main__':
    nrec = int(sys.argv[2]) if len(sys.argv) > 2 else 60
    msfb_ok = msfb_tot = 0
    end_ok = end_tot = 0
    errs = []
    for wi in range(2, min(nrec + 2, len(walk))):
        fr = wi - 1
        try:
            d = open(f'{DUMP}/sub{fr:04d}.bin', 'rb').read()
        except FileNotFoundError:
            continue
        evs = events(walk[wi])
        i = 0
        while i < len(evs):
            if evs[i][0] == 'elem':
                checks, i = parse_element(d, evs, i)
                for c in checks:
                    if c[0] == 'msfb':
                        msfb_tot += 1; msfb_ok += (c[1] == c[2])
                    elif c[0] == 'end':
                        end_tot += 1
                        if c[1] == c[2]: end_ok += 1
                        elif len(errs) < 12: errs.append((fr, 'end', c[1], c[2]))
                    elif c[0] in ('end_err', 'ERR'):
                        end_tot += 1
                        if len(errs) < 12: errs.append((fr, c[0], c[1], c[2] if len(c) > 2 else None))
            else:
                i += 1
    print(f'{TRACK}: msfb exact {msfb_ok}/{msfb_tot} ({100*msfb_ok/max(msfb_tot,1):.0f}%)')
    print(f'{TRACK}: sf_data end exact {end_ok}/{end_tot} ({100*end_ok/max(end_tot,1):.0f}%)')
    print('sample mismatches:')
    for e in errs:
        print('  ', e)
