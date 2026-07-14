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

## Round 421d — the middle is NOT an ASF body

- Exact-accounting enumeration over all 17 one-element frames: no
  ASF sf_data body (LFE-shape or mono-shape, w3/w5, any start
  10-44) fits the middle region on ANY frame. The ~230-330 content
  bits are parametric, not spectral — consistent with announcement
  DECAY frames physically being reverb/noise tails (A-SPX-style
  envelopes / noise params, no tonal core).
- f29/f259 (same phrase, repeated) share a 63-BIT common prefix —
  the element's parameter header (static across repeats); the
  divergence at bit 63 marks where time-varying (dt-coded) payload
  begins. Frame anatomy so far:
  [head 10+6][param header to ~bit 63][time-varying payload]
  [tail token 14b][~8-25 end bits][zero fill]
- Next enumeration target: the param header [16..63) (47 bits,
  shared) and the aspx-envelope hypothesis for the payload (official
  ASPX_HCB_ENV_* codebooks are loaded in ac4scan's T dict — chain
  candidate env codewords from ~63 to the tail token).

## Round 421e — the payload is A-SPX dt-coded envelope data

- Chaining official ASPX_HCB_ENV_LEVEL_15_DT codewords through the
  one-element frames lands EXACTLY on the tail token in 14/14
  token-bearing frames (from any start phase 16-23 — the codebook
  self-synchronizes, so phase isn't pinned yet, but the token
  boundary is codeword-aligned with an ENV_15_DT stream).
- Physical fit: announcement-decay frames = envelope levels ramping
  down = delta-TIME coding, no spectral core. Matches the no-ASF-
  body theorem (r421d).
- Element picture: [head+flags][param header ~to bit 63][ENV_15_DT
  envelope payload][14-bit tail token (codeword-aligned; likely the
  final envelope codewords or a noise/end field)][end bits][fill].
- To pin the phase/start exactly: enumerate the [16..63) param
  header against known A-SPX header fields (framing class, num_env,
  freq res, dt dirs) such that the implied band-count x num_env
  codeword count EXACTLY spans [payload_start..token]; validate on
  the f29/f259 identical-prefix pair first.

## Round 422 — the completeness theorem (read before ANY codeword claim)

- **All 60 Huffman codebooks in the official tables are
  Kraft-complete (sum 2^-len = 1.0)** — every book parses ANY bit
  string without error. Consequences:
  1. Parse-success chains can NEVER discriminate grammar (this
     retroactively explains every lottery of the campaign, including
     r421e's ENV_15_DT "landings" — hereby demoted to unproven).
  2. Even decoded-value statistics are weakly discriminating: under
     random bits, P(codeword) = 2^-len approximates the book's
     design distribution (small deltas dominate either way).
  3. The ONLY valid oracles: (a) exact count accounting (header
     fields must imply the codeword count that exactly spans a
     fenced region), (b) content validation vs the reference,
     (c) CROSS-FRAME VALUE COHERENCE — dt-coded values accumulate
     across consecutive frames into smooth per-band decay
     trajectories on real data vs random walks on misparses.
     Frames 679-683 (consecutive decay) are the trajectory corpus.
- Payload identity of the one-element frames: OPEN again (was
   "ENV_15_DT" — unproven). The param-header count-accounting +
  trajectory oracle is the decisive instrument pair.

## Round 422b — first 5-sigma payload identification (trajectory oracle)

- The cross-frame trajectory oracle (mean dt-delta z-score vs the
  book's random-bit distribution) on consecutive decay frames
  679-683: **ASPX_HCB_ENV_LEVEL_30_DT @ payload phase 20, ~4-5
  bands: mean delta -1.5/frame, z = -4.8** — envelope levels
  ramping down. Adjacent phases (19-22) and band counts (4-5)
  cluster coherently. First statistically-proper payload content
  identification of the campaign (survives the completeness
  theorem: the oracle uses VALUE statistics vs the design
  distribution, not parse success).
- Implied anatomy refinement: header = audio bits [0..~20) (the
  10-bit common head + ~10 flag/param bits), then ~4-5-band
  fine-quant (30-level) envelope dt deltas.
- Next: (1) verify decoded delta magnitudes against the 5.1
  reference's actual decay slope (dB per frame — an absolute
  physical check); (2) extend the oracle to joint layouts
  (sig+noise, multiple envelopes) and exact count accounting to
  the tail token; (3) decode absolute envelope values from an
  I-frame (F0) start and reconstruct the first parametric audio.

## Round 423 — r422b demoted; the real structure map (speaker track)

- **DEMOTION of r422b**: frames 679-683 are DIGITAL SILENCE in the
  5.1 reference (exact zeros; the announcement decay ends at 678,
  bitstream/reference alignment is frame-exact). The 5-sigma
  "decay ramp" came from a structured countdown field decoded
  through a Huffman book — the trajectory oracle's random-bits
  null is INVALID on structured silent-frame content. Value-
  statistics oracles need a null built from other frames' bits,
  not random bits.
- **I-frame cadence**: every 24 frames exactly (42 I-frames in the
  first 999). All share 16-bit head `0111110111111101`; silent
  I-frames carry `00001110` at [16..24). P-frames split
  `010111` (fronts-active, 598) vs `010001` (surrounds, 359) —
  bits 3-5 are per-group presence flags, tracking current-frame
  activity at ~82.5% (sticky at transitions, no lead/lag).
- **The "tail token" is content, not syntax**: universal quiet-
  frame ender E = `11111 00 11111 0` (+gate +optional trailer),
  = two 5-bit raw floor values 31 + separators — the A-SPX noise
  block at minimum. 688/999 speaker frames end with E (peak
  15-22 bits from wall) vs 3/201 Kraftwerk music frames —
  exactly the speech-vs-music noise-floor prediction.
- f0 (track start, silent I-frame) is 112 audio bits total:
  [16b I-head][8b class][7b zeros][`101000000`x3 = stride-9
  units, exact-fit ENV_LEVEL_15_F0 pairs][12b zeros][E@70]
  [gate=1][22b trailer, exact-fits ALPHA_FINE_DF x8 / DRC_HCB x7
  — unconfirmed][6b pad]. Silent P-skeleton f1 carries TWO of
  the same stride-9 units.
- Fade P-frames: one early field's codeword length shifts by 1
  bit every ~2 frames (the r422b "countdown") — a dt-coded value
  walking during fades. Silent-gap frames alternate small/large
  (period-2 refresh, ~60-bit extra block every 2nd frame).
- Duplicate-announcement frames share 107-227-bit prefixes
  (param header block); divergence = content payload start.
- WAR GRAMMAR LOCKS ON SPEAKER TRACK: bed_decode resync locks
  954/999 add-pairs, 972/999 LFE, 0 hard fails. Content
  validation vs the 5.1 reference in progress.

## Round 424 — FIRST CONTENT-EXACT BODY DECODE (PCM corr 0.987)

- Full speaker-track corpus dumped: spk4/ = all 2412 substreams
  (examples/dump_subs.rs). Silent I-frames beyond 999: f1560,
  f1704, f2112 (two E-chains: the noise block is a chain of
  (00 + 5-bit floor-31) links, count varies by frame).
- **The find**: python strict scan x PCM oracle on pure-L
  announcement frames (dequant |q|^(4/3) x 2^(0.25 sf), IMDCT,
  sine window): f60 body at bit 1602 (= resync H 1224 + 378)
  scores **PCM corr +0.987 vs reference L at lag 1024** (the MDCT
  half-frame delay). Grammar: [5-bit max_sfb=18][w3 sections
  (cb 3,9,4)][spectra][8-bit ref_sf=233 (signed -23)][sf chain]
  [snf] — the rosetta w3 grammar, on a core body.
- body1 (S of the M/S pair) immediately follows: [1909..2128),
  m=11, corr -0.53 vs L — M/S confirmed structurally.
- H+200..700 single-body sweep: 9/17 L-frames |corr|>=0.42
  (null ~0.1). But naive per-frame swept-lag stitching collapses
  at a single global lag (mean +0.077) — most sub-0.55 hits are
  the position lottery; only lag-1024-consistent hits are real.
- Rust bed_decode on the speaker track: LFE band-energy corr
  0.77 mean (89% of frames >0.5) = spectrum right, waveform
  phase wrong; pair walk decodes different near-silent data
  (bounds path diverges from the true body0 position).
- Honest state: content-exact decode is PROVEN possible (0.987);
  making it per-frame reliable needs the deterministic element
  grammar (chparam block between H and body0, window-shape
  handling for grouped frames), not more scanning.

## Round 424b — fixed-lag joint scan + stitch (session close)

- Final scan (body0 at fixed lag 1024±32, body1 free-lag
  structural confirm, select by |c0|): 28 joint hits across the
  L (33-94) and R (108-165) announcements, lags clustering
  992-1056.
- Stitch: L 15/62 frames decoded, R 13/58; per-frame fixed-lag
  |corr| mean 0.390 (L) / 0.429 (R); 73-77% of decoded frames
  >0.3. Segment-level corr dilutes to ~0.05 (sparse coverage +
  sign/TDAC interactions at coverage gaps).
- The resistant frames are the LOUD mid-announcement stretch
  (f42-57 class): denser spectra / grouped short windows — the
  long-only strict parser cannot reach them. Deterministic
  chparam grammar + grouped-window body support = the two
  blockers between here and full-coverage decode.
- Gate status: two channels decode with mean-of-decoded >0.3,
  but only ~25% frame coverage — NOT the gate. No audio
  delivered. stitch2_L.wav / stitch2_R.wav (decoded vs ref,
  side-by-side) banked in the scratchpad for the record.

## Round 425 — KRAFTWERK DECODES UNDER THE ROSETTA GRAMMAR

- The python chain (strict w3 sections + 5-bit max_sfb bodies,
  |q|^(4/3) x 2^(0.25 sf) dequant, IMDCT, sine window) applied to
  KRAFTWERK frame 6: body at bit 9865 scores **-0.826 vs the
  M=(L+R) reference at lag 1056**, with three more bodies in the
  same frame (4614, 5016, 10847) at the same lag cluster
  1040-1088 — the same pipeline delay found on the speaker track
  (1024). Music, not announcements.
- Implication: the frames carry MULTIPLE w3-grammar bodies
  (likely the front-group channel cores) beyond the war's proven
  w5 additional pair. The war's wall-closure proofs stand, but
  the primary audible content appears to live in w3 bodies the
  Rust walker never decodes — explaining flat-zero Rust PCM
  correlations at every lag on both tracks while python scores
  0.8-0.99 at the right positions.
- Full 200-frame Kraftwerk joint scan (body0 vs M fixed-lag
  1024-1104, body1 vs S free) running.
- Speaker C announcement: joint hits on 9/10 of the first C
  frames (quieter speech = long windows = high parser reach).

## Round 426 — Kraftwerk melodic frames decode; session close

- Melodic region (frames 1000-1119, choir/verse): 40/113 joint
  hits after fixing the band-template to the pipeline lag (+1024)
  — the prefilter at lag 0 had rejected the -0.826 f6 body.
  Intro region (0-199, geiger/noise): 6 hits (expected — noise
  fill content).
- Stereo stitch of 46 M/S pairs: per-frame M correlation at the
  single global lag: **mean +0.289, median +0.330, 57% > 0.3**.
  Segment corr ~0 purely from sparse coverage (46 frames over an
  1104-frame span). Caveat: block signs oracle-derived; the
  bitstream sign law is still owed.
- Speaker-track correction: the "resistant" frames are the PINK
  NOISE bursts — noise-fill content is waveform-uncorrelatable in
  principle vs an independent E-AC-3 encode. On VOICE frames,
  coverage is ~94% (15/16). C announcement: 15/45 joint hits.
- Inter-body gaps run 0-21 bits (python snf tail under-consumes);
  bodies are adjacent modulo that slack.
- State: the rosetta w3 grammar + python chain decodes real music
  at the target per-frame quality bar on the frames it reaches.
  Remaining to the gate: position determinism (the chparam/element
  grammar), the sign law, snf tail closure, noise-fill synthesis,
  then the Rust port.

## Round 427 — inter-body field forensics (honest negatives)

- Gap histogram across 89 confirmed adjacent-body pairs: 0-25
  bits, mode ~0-11. Gap size does NOT correlate with any body0
  internal (m, sections, zero-bands, snf gate/count) — the field
  belongs to the NEXT body (sf_info/window header?) or is
  optional per-body trailing data.
- Tail-variant search (snf band-set x sf-first x pad): the
  CURRENT tail grammar is the best of 24 variants (8/75 exact
  adjacency) — the tail model is right; the gap is real structure.
- Gaps >= 12 are NOT empty mini-bodies (0/24 exact-fit).
- Greedy body-train chaining derails immediately after body0
  (completeness: several false continuations per hop; f60's
  18-body "train" contains exactly ONE content-real body).
- Time-chaining by envelope continuity is insufficient (speech
  envelopes too smooth: 8 candidates > 0.93 in f61, true body
  not top-8). Reference-free walking NEEDS the deterministic
  pre-body grammar — next session's target, fresh context.
- Full-track Kraftwerk fixed-lag scan (all 1406 in-reference
  frames) launched for the coverage stitch.

## Round 428 — full-minute Kraftwerk sweep (session close)

- Fixed-lag joint scan over all 1406 in-reference frames (the
  first 60s of Radioactivity): **210 frames yield confirmed M/S
  body pairs (15%); per-frame M correlation at ONE global lag:
  mean +0.266, median +0.308, 52% > 0.3.** Same quality bar as
  the melodic-region pilot — the decode generalizes track-wide.
- ~9 seconds of real decoded Kraftwerk audio (scattered), banked
  as kwstitch.wav (decoded M | reference M side-by-side) in the
  scratchpad. Gate still unmet: coverage is grammar-blocked, not
  scan-blocked. Signs still oracle-derived.
- Scan crashed harmlessly at the ref boundary (last ~8 frames).
- Next session: the pre-body/element grammar (position
  determinism), the sign law, then the Rust port of the proven
  python chain.
