# The Rosetta frames (round 420)

Substream dumps from the **Dolby Atmos Speaker Channel Identification**
track (`~/Music/riptide/Dolby Atmos Audio Test .../Atmos-AC4/01*.mp4`),
same encoder family + identical TOC config as the music tracks
(v2, channel-coded mode 6, substream 1). Dumps carry the 16-bit
audio_size header; audio starts at bit 16.

Why they matter: the track announces one speaker at a time, so most
frames are near-empty — the element structure with ~zero Huffman
payload, at MANY distinct active-channel configurations. A graded
complexity ladder for grammar cracking:

- `sub001.bin` — **11 bytes**. audio_size = 7 bytes = 56 bits for the
  COMPLETE silent P-frame element:
  `01000111 00000000 10100000 01010000 00000001 11100000 00000000`
  (set bits at audio-relative positions 1,5,6,7,16,18,25,27,39-42).
  **Structural theorem: 56 bits cannot contain even three empty ASF
  bodies** (each needs >= 19 bits: 8-bit reference_scale_factor +
  section + snf flag). Silent P-frames therefore carry NO sf_data
  bodies — an element-presence / coded-silence flag layer exists
  that no prior grammar (7X war, A-JOC, immersive probe) modeled.
  The head starts '01' (matches the Kraftwerk 2-bit mode read); the
  following bits differ from Kraftwerk's constant '0111' — the
  "4-bit sticky field" model is mis-split.
- `sub029.bin` / `sub259.bin` — 43 bytes each, first ~48 bits
  IDENTICAL (the same announcement at two different times) —
  cross-frame redundancy separates structure from content.
- `sub031.bin` (44B), `sub002.bin` (54B) — next ladder rungs.
- Metadata tail after the audio wall is constant `0x00 0x20`-ish
  across minimal frames.

Ladder plan: enumerate candidate flag-structures over sub001's 56
bits (exact closure, fill < 8); validate against 029/259/031; then
climb: each next size class adds ONE element with content — its
grammar is isolated by the delta. The full per-frame size histogram
of the track spans 11..~2000 bytes: hundreds of rungs.

## Round 420b findings (the ladder's first rungs)

- All 12 skeleton frames (<60 bytes) share a **10-bit head
  `0100011100`**, then diverge — the post-head region is the flag /
  small-field layer.
- **`sub001` (11B) vs `sub002` (54B): identical through the entire
  56-bit skeleton EXCEPT audio bit 14 (`0` vs `1`) — and sub002's
  extra ~46 bytes sit AFTER the skeleton.** Presence-flag →
  payload-appended architecture, proven by a single-bit diff.
- sub029/sub259 differ only near their tails (same announcement,
  different frame phase); sub345/sub347 are another near-pair.
- f1 set bits at audio positions: 1,5,6,7 (head), 16,18,25,27
  (flags), 39-42 ('1111' run).
- 5.1-reference channel-activity labels per AC-4 frame (2048-sample
  grid): spk-activity.npy (numpy, [nframes x 6] RMS; channels
  L R C LFE Ls Rs). Single-channel announcement segments mapped;
  P-frame sizes correlate with the active channel (L/R median
  ~1200B, C ~940B, Ls/Rs ~715B, silence 213B).

Next rungs: diff the sub029/259 and 345/347 pairs bit-by-bit to
fence flag fields vs payload; then correlate flag bits across all
skeletons with WHICH payloads follow (sizes known); then climb into
the labeled single-channel frames.

## Round 420c findings (head semantics + skeleton anatomy)

- **The head flag bank follows the mix** (999-frame bit/label heat
  map): audio bits 3-4 ≈ 1 on front-channel frames (L/R/C 0.82-0.95)
  and ≈ 0 on surround frames (0.11); bit 10 ≈ 0.92 on surrounds vs
  0.32 fronts. Companding-control-shaped (compressors engage where
  the announcement is). Kraftwerk's constant '010111' P-head = the
  fronts-always-active configuration of the same flag bank.
- Common prefixes per (label, head) class end at ~10-13 bits —
  content begins immediately after the head; downstream fields are
  variable-length (heat-map smearing).
- Skeleton anatomy (f1 = 56-bit audio): ones at bits 1,5,6,7 (head
  '01000111'), then 16,18 ('1010'), 25,27 ('0101'), 39-42 ('1111');
  bits 43-55 are zeros = fill → the real element ends by ~bit 43.
  Note the shifted repetition 1010/0101 nine bits apart — two
  similar empty sub-elements?
- **f2's single differing bit (14) leaves audio_size unchanged (7B)**
  — it gates the post-wall metadata block (f2 carries ~45B of
  metadata vs f1's 2B). Grammar constraint: a standalone
  metadata-presence-like flag lives at audio bit 14 in the silent
  configuration.
- Only 2 pure skeletons exist in this track (f1,f2); next rungs are
  the 34-50-byte audio class (~14 frames, one small element each).

Next: the skeleton enumerator — parametrized grammar search over
f1's 43 real bits with hard constraints (field boundary at bit 14
with both values legal and no length change; '000' at 3-5 = all
groups silent; cross-validate every candidate on f2 and the
34-50B class where one element turns on).

## Round 421 — THE UNIFIED P-FRAME HEAD GRAMMAR (measured on two streams)

Strict-parse position scans (spec-exact python, no saturation):

- **Deviation #15: LFE section-length width is 3 bits, not 5**
  (rosetta: 147 w3 successes vs 3 w5).
- The P-frame audio opens `['010'][run-flag field][LFE body]`:
  observed head suffixes after '010' form two run families —
  `1^b` (front-active: '', '111', '1111', '11111', '111111') and
  `0 0 1^c` (surround-active: '0011', '00111', '001110/1') — total
  head length 3-9 bits, then the LFE body IMMEDIATELY (m(3) +
  w3 sections + spectra + scalefac + snf).
- Head family tracks the announced channel deterministically
  (C/R/L → '01011111'@8, Ls/Rs → '0100011'@7, quiet L/R → '010111'@6,
  sparse → '010'@3).
- **Kraftwerk validation: 178/191 P-frames lock a strict LFE under
  this grammar** (positions 3-8 dominate; heads '010' x50,
  '0101111' x34, ...). The war-era LFE-at-substream-22 was reading
  3-19 bits INTO the true LFE body — every downstream element map
  inherited that shift.
- r419's "LFE at 75-199" content hits are hereby demoted (3-sigma
  threshold x ~700 trials/frame = lottery); THIS measurement is
  hundreds of strict+labeled frames on two streams.

Next: (1) decode the run-flag semantics (likely per-group
active/companding flags; enumerate against labels); (2) with the
LFE pinned per frame, chain the NEXT element from lfe_end with the
same strict+label method — the whole P-frame map re-derives
outward; (3) port to Rust (sect width 3 for LFE + head runs),
re-run the bed decoder with true LFE, re-meter at fixed lag.

## Round 421c — the tail token (end anchor)

- A constant 14-bit token `11111001111101` closes 308/957 rosetta
  P-frames, with its last occurrence tightly 17-26 bits before the
  audio wall — the silent/near-silent form of the closing element
  (on busy Kraftwerk frames it only appears as coincidence inside
  Huffman bodies). Internal shape `11111-00-11111-01` = twin
  substructures again (cf. f1's 1010/0101 pair 9 bits apart) —
  pair-of-similar-elements motif recurs at both ends of the frame.
- Combined per-frame anchor set now: head family (start), tail token
  (end, quiet frames), audio wall (exact), channel labels, and the
  one-element class for exact accounting.
- r421b honesty note: the w3/position-scan LFE hypothesis FAILED the
  content gate (no meter improvement) — strict-parse hit rates alone
  cannot confirm grammar; every claim needs content or exact
  accounting. The enumerator over the one-element class (17 frames,
  264-400 content bits, head + one body + tail token + fill) is the
  decisive next move: every bit explained, lottery impossible.
