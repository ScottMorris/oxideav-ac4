# ffmpeg AC-4 decoder — Tidal/Atmos work tree (round 434)

Base: ffmpeg 6.1.2 + the community AC-4 patchset
(github.com/funnymanva/ffmpeg-with-ac4). This ac4dec.c adds:

- COMPLETE channel_element_7x per TS 103 190-1 Table 33
  (coding_config 0/1 groups, b_use_sap + additional pair,
  ASPX_ACPL_1 master tail, cc0/2 mono, 4 A-SPX trailers,
  ACPL data) — upstream had empty stubs for 7.1 content.

## Deviation knobs (env-gated)

- AC4_CB15 — THE round-434 breakthrough. Accept section codebooks
  12-15 as valid "no spectral payload" codebooks instead of
  erroring. Dolby encoders emit them constantly (believer LFE and
  beyond). believer.mp4: 4782 -> 735 failing frames of 4805
  (85% clean, full-song 8-channel synthesis).
- AC4_OVERSHOOT_SKIP — sections whose sect_end overshoots max_sfb
  carry no spectral lines. Measured on FULL runs: 735 errors with
  skip vs 4335 without. (ffmpeg's stale sect_sfb_offset arrays
  previously faked this for the LFE channel only.)
- AC4_MSFB5 — 5-bit max_sfb on long bodies. With CB15 the effect
  is now second-order for believer; keep for Tidal experiments.
- AC4_W3_LONG — 3-bit section widths on long frames. Interaction
  with MSFB5 unresolved.
- AC4_SFSIGNED — 8-bit two's-complement wrap of the scalefactor
  chain. Kills the 2^(huge) explosions but also crushes believer's
  legit >127 chains: absolute SF calibration still open. Believer
  content decodes with plausible per-channel structure either way.

## Crash-proofing (this file only, all measured necessary)

- ext_decode: clamp escape runs (get_bits n<=25 assert).
- asf_section_data: bound k+sect_len<=127, i<=125.
- asf_spectral_data: reject line ranges outside 0..2048.
- spectral_reordering / spectral_synthesis: hard bounds on
  spec_reord/scaled_spec indexing (SIGSEGV on garbage configs).
- ac4_toc: clamp nb_presentations/total_groups to array sizes
  (Tidal TOC desync produced counts like 5745 -> OOB writes).
- emdf_info / variable_bits: bit-exhaustion guards.
- audio_data channel_mode 7+ still av_assert0 — next de-fuse.

## Round-434 findings

1. believer.mp4 decodes 4070/4805 frames FULLY (parse hits the
   substream wall bit-exact) with CB15+OVERSHOOT_SKIP(+MSFB5).
   Full 8ch synthesis runs; levels are wrong (SF absolute law
   open; some wall-exact frames still explode to 2^60+ — ref_sf
   position or per-frame gain field suspected).
2. believer LFE grammar: pure noise-fill. Sections (often cb
   12-15, or overshooting cb<=11) + NO spectral lines + 8-bit ref
   + snf chain. Wall-exact across thousands of frames.
3. Tidal music/speaker files: the remaining wall is the TOC layer,
   not the audio grammar. ffmpeg's v2 TOC parse desyncs per frame
   (presentations/channel_mode/groups scatter randomly; speaker
   file aborts on channel_mode 35). The audio substreams
   themselves are already perfectly extracted by our Rust
   dump_subs — NEXT: bypass harness that wraps spk4/kw4 substream
   dumps in a minimal spec-clean TOC and feeds ffmpeg, isolating
   audio_data from Tidal's TOC.
4. METHOD WARNING (bitten 3x): never count errors on a run that
   crashed — a truncated run undercounts. Check exit code first.
   ("W3_LONG 1734 decoded", "17 errors", "speaker 0 errors" were
   all truncation artifacts.)

Positions-trace: -loglevel trace now logs POS tags (audio start,
element entries, sect/spec/scf/snf/end per channel) — diffable
against python hand-parses.

Build: scratchpad/build_ffac4.sh (docker, ~1 min incremental).

## Round 435 — TOC-bypass harness (AC4_RAWSUB)

- ac4dec.c: AC4_RAWSUB=<channel_mode> env skips ac4_toc entirely;
  packet = [flags byte: bit0=iframe][substream starting at
  audio_size field]. wrap_subs.py wraps dump_subs .bin dumps into
  AC40 sync framing (iframe cadence or explicit list).
- VALIDATED: believer dumps through the harness = corr +1.000 vs
  the mp4 path on all channels. The harness is bit-faithful.
- Tidal speaker/Kraftwerk substreams now run through audio_data:
  effectively 0 clean frames (expected — A-SPX deviations not yet
  ported), but failures are per-frame diagnosable with POS traces.
- KEY TRACE (speaker f33, P-frame): codec_mode=1 ASPX, LFE parses
  clean (26..57), coding_config=3 -> five_channel_data, desync in
  the chparam/sap region ~bit 188. War truth: body0 (5-bit msfb +
  w3 sections M/S pair) sits at packet bit 1696. ~1500 bits of
  parameter data (war E-chains = aspx envelopes?) sit BEFORE the
  bodies — Tidal's element puts A-SPX data before spectral bodies,
  or the element isn't a Table-33 7X at all. NEXT: hand-map
  57..1696 against war aspx grammar (kwjoint positions known for
  210 Kraftwerk frames too).
- MINIMAL-BUILD TRAP: no null muxer — "-f null" dies at muxer
  init and error counts reflect the PROBE PHASE ONLY. Always
  decode to -f wav. (Counting-trap rule, incident #4.)

## Round 436 — TABLE 33 HOLDS FOR TIDAL; speaker parse near-complete

- MSFB5 is the master key for Tidal speaker frames through the
  harness: without it 40/40 hard-fail; with it (+CB15) NO hard
  sect failures — frames walk the FULL Table-33 structure.
- THE WAR MYSTERY RESOLVED: the "pre-body parameter region" is
  five_channel_data's chparam/sap_data chains + the first
  channels' sf_datas. Our scanned "bodies" ARE the 5ch sf_datas
  (f33: ffmpeg ch3 sect@1641 vs war body0@1664 = same region).
  M/S pairs = sap_mode=1 channel pairs. No pre-body aspx block.
- f33 walk: LFE 26..57 | cc=3 five_channel 59..2395 | add pair
  2396..2784 | aspx tail 2784..~3040 (2ch 39b, 2ch 91b, 1ch 49b,
  2ch ...) vs wall 3144 — remaining miss ~100-180 bits INSIDE
  aspx_data (war F0 widths/sticky-xover apply here). underread
  19 nonzero tail bytes on f33; many frames similar.
- W3_LONG NOT needed for this walk (war w3-bodies observation vs
  5-bit ffmpeg sections both "work" — unresolved, empiricism won).
- Kraftwerk kw4 dumps: 40/40 fail immediately after codec_mode=1
  on iframes — aspx_config divergence (music-class config).
  NEXT: error logging inside aspx_config + war aspx deviations.
- Packet offset in harness: demuxer strips sync; packet bit =
  dump bit + 8 (flags byte).

## Round 437 — both tracks converge on aspx_data

- Kraftwerk f0 (iframe, 17688-bit audio): LFE + three_channel +
  msp pairs with different_framing short windows ALL walk clean
  to bit 4613, then dies INSIDE the first aspx_data_2ch. 13099
  bits of aspx payload follow (music-sized envelopes).
- Speaker f33: same story, tail-sized (aspx blocks at 2784..~3040
  vs wall 3144).
- => the LAST grammar face for both = aspx_data_2ch/1ch. Port the
  war deviations as knobs: mixed raw/huff F0 (LEVEL_30=6,
  LEVEL_15=7, BAL_30=4, BAL_15=5, NOISE=5, NBAL=4), sticky xover
  slots [0,0,0,4], P-frame FIXFIX+1env->Fine qmode override.

## Round 438 — Kraftwerk aspx breakthrough + F0 raw knob

- AC4_F0_RAW: war round-407h law — first FREQ value of each aspx
  env chain is FIXED-WIDTH RAW (SIG lvl 15/30 = 7/6 bits, SIG bal
  15/30 = 5/4, NOISE lvl 5, bal 4), not the Table-58 F0 huffman.
  (quant_mode index 0 = 15-step in ffmpeg's vlc arrays.)
- Master-table init fix: ffmpeg only built sbg_master when config
  CHANGED on an iframe; channels whose first aspx parse followed
  an unchanged config had empty tables -> sbx=0 rejects. Now
  rebuilt whenever num_sbg_master==0. Kraftwerk 40/40 -> 25 fails
  (15 frames parse to completion) on kw40 subset.
- REMAINING KW WALL: aspx_config MISPARSE. All sbx=0 rejects show
  start_freq=7 stop=1 scale=1 xover=7 (start_freq pegged at max =
  garbage). ffmpeg reads 15 fixed bits; Tidal's aspx_config layout
  deviates. ANSWER KEY: Rust fork parse_aspx_config (src/aspx.rs
  ~L609, war-proven on these exact frames). NEXT ROUND: diff the
  two field-by-field, port as AC4_ASPX_CFG knob.
- goto-fail LESSON: never insert av_log between a braceless if and
  its return (instrumentation script did; caught same run).

## Round 439 — sticky xover confirmed; A-JOC reframe

- AC4_XOVER_STICKY: war slots [0,0,0,4] — no per-block 3-bit xover
  field on iframes; fixed per aspx slot (s->aspx_slot counter,
  reset per frame). ALL Kraftwerk sbx=0 rejects eliminated.
- KRAFTWERK REFRAME: bed walk ends ~4.9k bits; 60-80% of each
  frame (10k+ bits) remains after the aspx trailers = A-JOC OBJECT
  stream (music = bed + Atmos objects; war kwjoint bodies at high
  offsets were object audio). Bed-only speaker track nearly
  closes (best frames underread 19 bytes); Kraftwerk needs the
  fork's ajoc.rs knowledge next.
- Speaker loud frames still underread 1000+ bytes: aspx env
  blocks consume ~100 bits vs real payloads — envelope grammar
  (framing/num_env/freq_res or ec_data chains) still partly off.
  Loud rejects: "invalid aspx num env 6-7 (class 3=VARVAR)" —
  cap>5 may be Tidal-legal, check war VARVAR handling.
- Shell trap: `env VARS cmd > log 2>&1` inside a for-loop gave
  phantom 40/40 counts; `2> log` form counts correctly.

## Round 440 — env pow2 law; speaker within bytes of closure

- AC4_ENV_POW2: war law — FIXFIX aspx_num_env = 1 << tmp (1/2/4/8),
  not 1 + tmp (ffmpeg). Env arrays bumped ([9]/[10]/[8]) for 8-env
  frames. Caps relaxed to 8 under the knob.
- FIXFIX+1env -> qmode 0 override: ALREADY spec/ffmpeg behavior,
  not a deviation. Removed from the todo list.
- FULL SPEAKER RUN (all knobs: RAWSUB CB15 OVERSHOOT_SKIP MSFB5
  F0_RAW XOVER_STICKY ENV_POW2): 718 hard errors, 1845/2412 parse
  to completion, 89 frames within 4 BYTES of the wall, 122 within
  32. f33 = overread 2 bytes (16 bits). Residue = one small field
  in the aspx tail (candidates: rel_bord value semantics raw vs
  2n+2 — same bits but wrong borders could shift atsg/ec chains
  on later frames; noise ec_data chains; b_aspx extras).
- Two failure classes remain: (a) near-miss tails (aspx grammar
  residue), (b) frames whose channel data ends far too early
  (upstream sf misparse, likely sap_data/ms chains on loud
  frames).
- Kraftwerk unchanged: bed closes the same way; the 60-80% tail
  is A-JOC objects (separate element, fork ajoc.rs has grammar).

## Rounds 441-443 — aspx parity audit, aligned-output metering, 7X processing

- r441: per-block aspx POS tracing (framing/delta_dir/ec_data).
  f33 surplus isolated to block 4 (additional pair) ec chains.
  AC4_XOVER_SLOT3=N tunable — sweep 0..6 barely moves counts (not
  the lever). hfgen_iwc_2ch + framing grammars verified IDENTICAL
  to fork war implementations (tna/ah/fic/tic all match).
- r442: AC4_NEVER_FAIL — overread rewinds to wall instead of
  dropping; failed substreams zero their spectra; decoder emits
  ALL frames = timeline-aligned output for honest full-pipeline
  A/B. FIRST FULL-PIPELINE BASELINE vs E-AC-3 reference (per-frame
  |c| @ lag 1024): L/R ~0.04, C ~0.08, LFE ~0.10, Ls/Rs ~0.08 —
  weak; lowpass metering no better, so not a high-band masking
  issue. Parse-complete != decode-correct (expected).
- r443: mode 5/6 had NO post-parse M/S-SAP inverse AT ALL
  (processing switch stopped at mode 4). Added
  m7channel_processing: cc0 pairs (0,1),(2,3); cc1 pair (3,4);
  SIMPLE/ASPX additional pair (5,6) via mdct_stereo_proc[2].
  No metric change yet — speaker is almost all coding_config=3
  (five_channel + chel_matsel): the 5-channel matsel inverse
  (which pairs the matrix picks) is UNIMPLEMENTED for cc2/cc3 in
  both 5x and 7x paths. THAT is the next synthesis gap, plus SF
  level law. Note: war python proved bodies decode at 0.987 — the
  channel data is right; remaining gaps are processing-side.

## Round 444 — matsel cascade engine

- Table 179 structure CRACKED from row analysis: five_channel
  matrix = fixed pair cascade s=P0(I0,I1); t=P1(s0,I2);
  u=P2(I3,I4); (O0,O3)=P3(t0,u0); (O1,O4)=P4(s1,u1); O2=t1,
  with matsel permuting routings (row1 = P0 output swap; row2 =
  input re-route). chparam i lives in ssch[i] = P_i params.
- Implemented two_channel_processing_p (param/buffer split) +
  five_channel_cascade (row-0 routing for ALL matsels — routing
  refinement pending) + four_channel_cascade (Table 177, fixed,
  no matsel). Knob: AC4_MATSEL. Speaker matsel distribution:
  15 dominant (941), then 7/0/8/11/1 — scattered, so per-matsel
  routing matters. Plan: EMPIRICAL routing search per matsel
  value using reference correlation as oracle (no OCR needed).
- sap_mode 2 = fullband M/S: already handled in ffmpeg (verified).
- Meter after cascade: STILL flat (~0.04-0.10). Conclusion: the
  blocker is upstream of matrixing — suspect per-band SF gains
  (wrong gains scramble in-frame spectral shape; war python with
  proven sf chain hit 0.987 on same content). DECISIVE NEXT STEP:
  parse-level A/B of ONE war-proven body (speaker f33 body0 at
  dump bit 1656, c0=-0.642, or f60@1602 c0=+0.987) — dump ffmpeg's
  per-band sf/spectrum for that channel and diff against the
  python chain values. That pinpoints the exact divergent field.

## Round 445 — per-channel misalignment mapped; gap fields = last unknown

Correlation-oracle body scan around ffmpeg's five_channel sf_data
boundaries on speaker f33 (python, war grammar, ±40 bits):
- ch0@223: no correlating body (quiet ch)
- ch1@383: true body @412 w5 (off +29, corr 0.311)
- ch2@1034: none
- ch3@1633: true body @1656 w3 (off +23, corr 0.629) = war body0
- ch4@2346: true body @2330 w3 m=1 (off -16, corr 0.844);
  war body1 @1871 reparses better as w3 m=2 @1878 (corr 0.727)
OFFSETS NON-MONOTONE (+29/+23/-16) => unknown small fields BETWEEN
per-channel sf_datas (the war's 0-25-bit inter-body "gap mystery"
= real per-channel leading/trailing fields ffmpeg never reads).
This is the LAST grammar unknown for the bed. Mixed w3/w5 widths
confirmed WITHIN one element (per-channel, not per-element).
Toolchain note: the correlation-anchored scan (this experiment)
gives per-channel truth positions on any frame with reference —
use it to solve the gap-field structure across many frames
(accumulate (channel, gap-size, preceding-body properties) pairs
and look for the field grammar; msfb/w choice may live IN the gap).

## Round 446 — gap-field forensics (negative + new attack vector)

- Gap bit-strings dumped for 34 war body pairs: contents look
  random (no prefix code, no fixed flags; all-zero and all-one
  both occur). Confirms war r427. Gap sizes 0-13 in this sample.
- NEW ATTACK unavailable during the war: ffmpeg now walks the
  whole element and reports per-channel context (sap_mode, msfb,
  num_windows, matsel). Next: full-corpus regression of gap size
  against NEXT-channel properties (sap_mode? msfb value? w3/w5
  choice?) across all 238 pairs (jointbest + kwjoint4 + kwmel +
  kwfull) — the gap may encode the per-channel width/msfb
  selector that explains mixed w3/w5 within one element.
- Also confirmed: war body1@1871 reparses better as w3 m=2 @1878
  (corr .727 vs .320 for the old w5 read) — the war's w1=5 reads
  may themselves be position-lottery artifacts; re-audit with
  fixed-lag oracle when re-anchoring.

## Round 447 — gap regression: comprehensive negative, new design

- Full-corpus regression (238 pairs): gap size uniform over the
  scan range 0-25 -> corpus contaminated by position-lottery
  body1 hits (weak |c1| filter in the war scans). No byte
  alignment (body0/body1 starts uniform mod 8). No gap<->w1 or
  gap<->m1 relation.
- Oracle-strict rebuild (|c0|>=0.5, body1 required |c1|>=0.5 at
  fixed lag): only 3/40 frames yield verifiable body1 (near-mono
  content). Gaps 7/17/10, w1=3 all, body0 snf_bits 1/1/5 — too
  few for regression. THE GAP FIELD REMAINS UNSOLVED analytically.
- NEXT DESIGN (wall-constrained chaining INSIDE ffmpeg): unlike
  the war's blind scans, ffmpeg now knows the full element around
  the sf_datas. Add AC4_BODY_RESYNC: at each five_channel sf_data
  start, try offsets 0..25 x {w3,w5}; accept the first parse that
  (a) is grammar-valid, (b) keeps the remaining channels' chain
  able to land within the audio wall (backtracking, like the
  fork's r408 RSDBG machinery but with the full Table-33 context).
  The wall + downstream-structure constraints should collapse the
  ambiguity that killed greedy chaining. Validate on f33 (known
  anchors 1656/1878) before trusting.

## Round 448 — f33 FIVE-BODY CHAIN RECOVERED (DAG search + anchor)

Memoized DAG chain search (gaps 0-25, w in {3,5}, m=0 allowed)
over speaker f33, validated by the war anchor:
  ch0 body@218  w3 m=19 end=842   (lead gap from ~193)
  ch1 body@865  w3 m=26 end=1255  (gap 23)
  ch2 body@1273 w3 m=4  end=1338  (gap 18)
  ch3 body@1349 w3 m=16 end=1647  (gap 11)  [alt: @1358 w5 m=22]
  ch4 body@1656 w3 m=10 end=1869  (gap 9)   <- war anchor, corr .629
Then gap 2-9 -> body@1871/1878 = ADDITIONAL PAIR (w5/w3) -> aspx.
KEY READS: (1) body@1656 is ch4 (LAST of five), not ch3;
(2) all five bed sf_datas parse w3 with leading 5-bit msfb;
(3) inter-body gaps shrink monotonically (25,23,18,11,9) in this
frame — gaps look like PER-CHANNEL PREFIX FIELDS (hypothesis:
interleaved chparam/sap data before each sf_data — Tidal moves
chparam_info from five_channel_info into each channel, or an
unknown per-channel header). ffmpeg ch0 miss is small (~5 bits =
per-channel msfb). NEXT: decode the 5 gap bit-strings of f33
against chparam grammar ([2b sap_mode][ms_used m bits][sap_data]),
try alignments; replicate chain on 2-3 more anchored frames
(f60 speaker, kw strong frames) to confirm gap monotonicity is
coincidence or structure. Then port resync/true grammar to ffmpeg.

## Round 449 — gap-field data bank (two-frame witness)

f33 chain gaps: (lead~25) 23, 18, 11, 9 | f60 chain: body@220 w3
m=1 e257; @281 w3 m=10 e412 (gap 24); @433 w3 m=27 e1326 (21);
@1343 w5 m=19 e1587 (17); @1602 w3 m=18 e1903 (15) = war 0.987
anchor as LAST-ish channel. GAPS MONOTONICALLY DECREASE IN BOTH
FRAMES — structural, not noise.
Gap bit-strings (start..end dump bits):
 f33: ch1 842..865  10000001001011000001000
      ch2 1255..1273 011011100010010110
      ch3 1338..1349 11100000011
      ch4 1647..1656 101101100
 f60: ch1 257..281  (24b) ch2 412..433 (21b)
      ch3 1326..1343 (17b) ch4 1587..1602 (15b)
Huffman-run fits (SCF/SNF books, skip 0-2): inconsistent across
witnesses (f33ch2, f60ch1, f60ch4 fit nothing) -> NOT a plain
huffman chain. Hypotheses still open: per-channel interleaved
chparam+transform_info combos; pair-tree metadata; width law
tied to decreasing quantity (remaining channels? cumulative?).
NEXT: (a) third+ witness frames (kw strong 1194/1372/1392 via
DAG w/ kw anchors; kw = 3ch config -> different channel count =
discriminating test for "remaining channels" hypotheses);
(b) try parsing gaps as [transform_info][psy pre-msfb fields] for
SHORT-window channels (5+grouping bits) mixed with chparam;
(c) once gap law falls: port to ffmpeg five_channel_data, align,
re-meter, GATE.

## Rounds 450-451 — THE IMS STEREO PAIR REFRAME

- KW third witness (f1194): 7-body chain, gaps (25)6,6,6,17,4,24 —
  monotone-decrease BROKEN => spk "monotone law" was path-selection
  artifact (completeness theorem, again). Only anchors are proven.
- CHAIN-CONSENSUS TEST (471 five-chains over 18 anchored speaker
  frames, per-position correlation vs all ref channels):
  positions 0-3 DEAD (|c| 0.01-0.10); position 4 (anchor) 0.49 vs
  L AND 0.58 vs R. THE WAR BODIES ARE NOT BED CHANNELS:
  ** anchor pair = ADDITIONAL STEREO PAIR (ssch5/6) = the IMS
  stereo render ** — correlates with both L and R because it IS
  the stereo mix. Bed channels are quiet on the speaker track.
  This explains: exactly-two adjacent bodies everywhere; pair
  gaps 0-25; mixed per-channel w3/w5 (own sf_infos in
  two_channel_data).
- INTERLEAVE HYPOTHESIS ([sf_info1] between the pair's sf_datas):
  gap=1 cases fit 'long' 1-bit sf_info EXACTLY (f140 c0=0.928,
  kw1104); full-corpus fit only 15/238 BUT war gaps/e0 ends are
  unreliable (snf tail under-read suspected; war body1 positions
  weak — f60 war body1 only 0.097 at fixed pos/lag).
- NEXT (rounds 452+): (a) audit parse_snf tail against gap=0/1
  ground truth (find the missing end bits); (b) interleaved
  [sf_info][sf_data][sf_info][sf_data] two_channel layout knob in
  ffmpeg (AC4_2CH_INTERLEAVE); (c) build BIG M-body corpus via
  harness tail-region oracle (add pair sits just before the aspx
  trailer whose position ffmpeg knows); (d) M/S stitch under
  ffmpeg synthesis -> GATE attempt. The gate may not need the bed
  at all — the IMS pair IS the stereo listening target!

## Rounds 452-453 — GATE DRY-RUN: numbers met (with one asterisk)

- f60 S-partner hunt (free-lag, e0..e0+34): best = war's @1910 w5
  but only |c|=0.138 — f60 is near-mono; S genuinely tiny. M
  carries the content: M-only stereo reconstruction is viable on
  such frames.
- M-ONLY GATE DRY-RUN (28 jointbest M bodies, python chain, sign
  from stored c0, GLOBAL fixed lag 1024, active-ref frames only):
    L: n=15 mean +0.389 median +0.376 (73% > 0.3)
    R: n=14 mean +0.368 median +0.376 (64% > 0.3)
  Two channels > 0.3 mean per-frame = DISCIPLINE GATE NUMBERS MET
  on the anchored corpus. ASTERISK: per-frame signs taken from
  reference-derived c0 (leakage — not gate-legit); coverage = 28
  frames of the 133-frame scanned range (voiced segments).
- REMAINING HONEST BLOCKERS: (1) deterministic sign law — signs
  mix ± across frames; war lag0 also varied 992-1072 per frame:
  both smell like WINDOW-SEQUENCE PHASE (we OLA with fixed sine
  2048; true frames have start/stop window shapes + possibly
  variable transform splits). Fix = model window sequence from
  sf_info in the python chain (or drive ffmpeg synthesis with
  aligned bodies). (2) Coverage: scan all 2412 speaker frames for
  M bodies (position prior: tail region before aspx; M template
  fixed-lag). (3) Believer contradiction to interleave hypothesis
  noted: believer msp=0 grouped layout is wall-exact-validated,
  so Tidal's pair layout deviation (if any) is Tidal-specific.

## Round 454 — phase wobble PROVEN; the knot identified

- Role table (28 anchors, L/R/M/S per frame): speaker announce
  frames are single-channel; body tracks the ACTIVE channel
  (M-of-pair consistent). Old sign(c0) flips = lag artifacts.
- Fine lag sweep (step 1, 960..1090): peak lags SPREAD the whole
  range (several pinned at sweep edges, e.g. f140 c=0.936 with
  peak <=960; f60 0.987 at 1020); signs still mixed 18+/10-.
  => NOT sweep quantization: per-frame TRANSFORM PLACEMENT.
  Bodies with short-window configs place energy at different
  offsets; our fixed single-2048 sine IMDCT smears/shifts them.
- THE KNOT: pair sf_info (immediately before each body = the
  "gap" field) carries the window config; window config fixes
  phase; phase fixes sign+lag; sign+lag close the honest gate.
  ONE implementation unlocks all: backward-parse sf_info at
  body0_start - {1 | 6+n_grp_bits_a[idx0][idx1]} bits, then
  window-correct IMDCT/OLA (needs short-window SFB tables +
  grouped spectrum layout — fork has BlockSwitchOla + tables).
- PLAN r455: implement window-exact python recon for the 28
  anchors (enumerate sf_info candidates that end exactly at
  body0 start; pick the one that locks peak to a constant lag);
  expect lag to collapse to single value & signs to unify ->
  re-run gate WITHOUT assists -> listening master.

## Round 455 — window knot: two negatives, sharper aim

- Wide lag sweep (700..1350): peak offsets -86..+325, NOT clean
  window multiples — speech pitch-period aliases contaminate lag
  evidence (voiced content self-correlates at F0 periods). Lag
  cannot diagnose window config directly on voice.
- Backward sf_info enumeration at 28 anchor bodies (long-bit /
  self-consistent short forms, slack 0-5, shift 0-1): coverage
  never beats ~19/28 with 'long' hits at chance level (~50%).
  => the field immediately before bodies is NOT bare sf_info in
  the simplified [b_long | 0+idx+idx+diff+grouping] form. Layout
  likely richer (psy_info extras: dual_maxsfb/side fields, or
  chparam interleaved, or msfb belongs to a larger psy block).
- NEXT TOOL (strongest available): BELIEVER AS PAIR-LAYOUT
  ROSETTA. Believer's additional pair is wall-exact through
  ffmpeg (SIMPLE mode) — dump exact bit spans of msp/sf_info0/
  sf_info1/sf_data0/sf_data1 on believer frames (POS logs already
  in ac4dec.c), then structurally align the Tidal pair region
  around known anchors (body0 position fixed) and diff the two
  layouts field-by-field. Model Table 37/38 BIT-EXACT (incl.
  dual_maxsfb/side_limited paths) before re-attempting.
- Window-exact IMDCT implementation still pending (grouped
  spectrum + short SFB tables) — needed for the honest gate
  regardless of how the layout question resolves.

## Round 456 — believer pair layout BIT-MAPPED (Rosetta landed)

Believer clean frame 68 (wall-exact, POS msfb logging):
- msp=0 pair: [2ch@273][ti0][msfb0=2 ends@280][ti1][msfb1=1
  ends@286][sections0@286 CONTIGUOUS][...sf_data0 end@310]
  [sf_data1@310 NO PREFIX, GAP 0][end@328]
- msp=1 pair: [2ch@328][ti][msfb=26 ends@339][~5b chparam]
  [sections0@344][sf_data0 end@2133][sf_data1@2133 gap 0]
=> In believer, sections abut the OTHER channel's msfb (grouped
sf_infos), and the pair's second sf_data has no prefix and zero
gap. TIDAL WAR PAIRS MATCH NEITHER SHAPE (both bodies carry
[5b msfb][sections] prefixes, gaps 0-25 nonzero) => Tidal
genuinely deviates: per-sf_data msfb prefix (war "msfb
everywhere") + a small variable trailing field after each
sf_data (snf-tail under-read OR an inter-sf_data field).
- Python-vs-ffmpeg span audit attempted on believer; blocked by
  dump-index/offset bookkeeping (cb-15 fails = misalignment, not
  grammar). NEXT: redo audit with harness-wrapped blv.ac4 (packet
  bits = dump bits + 8, no ambiguity); then measure the exact
  missing-tail grammar on believer sf_datas with the python
  parser; apply to Tidal pair; THEN window-exact IMDCT for gate.

## Round 457 — WAR SCANNER SELECTION BIAS EXPOSED; v2 parser near-exact

- ti-interleave hypothesis dead in pure form: f140's single gap
  bit = 0 (not b_long=1); gap-size set {1,7,9,10,13,14,16,21}
  fits none of the strong anchors' actual bits.
- AUDIT LINES added to sf_data (m/groups/long/sect-pos per
  channel) — believer harness now emits self-contained
  calibration data (blvh2.txt, 1444 sf_datas).
- ** WAR PYTHON GRAMMAR IS A STRICT SUBSET: ** on believer's
  validated spans it FAILS 102/107 with 'cb 15 invalid' and
  'section overrun' — the war scanner REJECTED cb12-15 and
  overshoot bodies by construction (selection bias). All war
  anchors are from the subset; Tidal bodies using cb15/overshoot
  were invisible to every war scan.
- parse_body_v2 (believer semantics: cb0-15, overshoot-skip,
  snf over max_sfb): 0 -> 8/107 exact matches, remainder
  clustered at ±1..±9 bits = replication details (spectra
  decode-per-section structure, snf clamps, sign handling).
- NEXT (r458): make v2 bit-exact against ffmpeg's asf functions
  (target ~100% on believer audit), then RE-SCAN Tidal anchors
  with v2: body ends move, more bodies become visible (cb15
  class), gaps re-measured on correct ends -> pair layout;
  plus window-exact IMDCT for the gate.

## Round 458 — v2 BIT-EXACT (89/102); TWIN-PEAK GAP HARVEST

- OFF-BY-ONE FOUND: ffmpeg probe double-decodes packet 0 → trace
  frame i = dump i-1. Map via audio_size matching (offset votes
  {1:200}). r456's failed audits were THIS, not grammar.
- python v2 vs ffmpeg on believer long single-group spans:
  ** 89/102 EXACT, zero nonzero deltas ** (13 exceptions left to
  classify). The v2 grammar (cb0-15, overshoot-skip, snf) is
  bit-perfect. => war body END positions were CORRECT; Tidal pair
  gaps are REAL structure absent from believer.
- TWIN-PEAK METHOD: on single-channel frames, body1 (S) correlates
  as strongly as body0 (M=S=L). f60's TRUE S: gap=11 w5 m=26 at
  |c|=0.988 (war's gap-7 was 4 bits off). Sixteen clean gap
  samples harvested (jointbest anchors, v2 grammar, c>=0.45):
  f33 g9 w3 m2 | f36 g10 w3 m3 | f39 g19 | f58 g23 | f60 g11 w5
  m26 | f61 g26 | f62 g24 | f63 g31 | f64 g5 | f108 g22 | f109 g3
  | f110 g6 | f134 g5 | f135 g16 | f136 g16 | f140 g0 (flush!).
  Bit-strings in transcript. Gap range 0-31, no simple width law
  yet; f140 g0 = believer-style flush exists in Tidal too.
- NEXT (r459): (a) classify the 13 v2 exceptions; (b) twin-peak
  over ALL 2412 speaker frames (not just war anchors) => hundreds
  of gap samples + the full M/S corpus for the gate; (c) gap
  grammar regression on the clean set (vs w1/m1/sap candidates);
  (d) window-exact IMDCT. The gate corpus and the gap law now
  come from the SAME scan.

## Round 459 — sign law: two falsifications, one conclusion

- Spectral parity alternation ((-1)^k = half-hop shift): flips
  and improves SOME frames (f135 -0.339 -> +0.488, f136, f110)
  but degrades others (f134, f140) — per-frame window property,
  not a global law. NOT the sign law alone.
- Overlap-consistency decoder-side sign resolver (maximize OLA
  constructive overlap with previous frame): 9/15 = chance.
  Wrong windows break TDAC cancellation, which the test needs.
- CONCLUSION: sign + lag + parity all reduce to WINDOW-EXACT
  SYNTHESIS. No shortcut exists. Implement AC-4 §5.5 window
  sequence in the python chain by porting the Rust fork's
  mdct.rs / BlockSwitchOla shapes (TDAC round-trip tested,
  session 1) — window params per frame from the pair's sf_info
  (transform_info now REACHABLE: body positions known to the bit
  via twin-peak; the sf_info sits in the gap fields).
- Note: frames improved by parity alternation (f110/135/136) are
  candidates for short/split transform configs — use as test set
  for window hypotheses.

## Round 460 — KBD confirmed; long-frame model near-perfect

- AC-4 windows are KBD (Table 186: alpha=3 @ 2048), NOT sine.
  Python chain upgraded: mean |c| 0.408 -> 0.411 across 28
  anchors; f60 0.987 -> 0.993 (long-frame model now near-exact).
- Signs UNCHANGED under KBD => sign is not window shape; the
  negative-c frames are genuinely different transform structures
  (short/split configs — the same frames parity-alternation
  helped). The long-KBD model is correct for long frames; short
  frames need the full grouped-spectrum reconstruction.
- STATUS toward the gate: magnitude side solid (|c| mean 0.411 on
  anchors, well over 0.3); positions still come from reference-
  assisted war scans — a fully honest master needs bitstream-
  derived positions (ffmpeg harness alignment + the gap law).
  Polarity flips are near-inaudible; gate ruling on signed vs |c|
  is Scott's call.

## Round 461 — gap corpus at 29 clean samples (both tracks)

- Kraftwerk twin-peak harvest (74 strong anchors, S-template,
  KBD windows): 13 clean gap samples incl gap=0 (f1320) and
  gap=1 bits='1' (f1361). Saved: kw_gaps.json (fr,gap,w1,c,bits).
- Combined corpus: 29 samples, gaps 0-31, both tracks. Spot
  hypothesis checks ([ti][sap] width combos): individual fits
  exist (f109 g3=long+sap, f60 g11=short grp3+sap) but bit-level
  consistency FAILS cross-sample (f33 g9 bits contradict its own
  idx-implied grouping width).
- NEXT: systematic solver — enumerate candidate field-sequence
  templates (products of: ti forms, sap_mode variants, ms_used
  with band-count from {m0,m1,const,aspx-derived}, b_-flags,
  huffman chains) against ALL 29 samples requiring exact width +
  self-consistent field values on every sample. Corpus is small
  enough to brute a large template space. Also grow corpus via
  twin-peak on non-anchored frames (needs bitstream body0 finding
  or full-position scan).

## Round 462 — template solver + alignment: both negative; ASPX-tail deduction

- Full-context corpus (29 samples with gap/w0/w1/m0/m1/bits):
  banked as gap_corpus.json.
- Mechanical template solver (~20k field-sequence templates from
  {flags, U2-U5, transform_info, sap variants with band sources
  m0/m1/min/half, gated forms}, exact width + parse-consistency
  on all 29): ZERO exact fits; best 5/29 (chance). Byte/word
  alignment padding: dead (mod-k tests at chance).
- LOGICAL CORNER: gap=0 samples (f140, kw1320) prove NO
  bit-consuming grammar can live in the gap unconditionally —
  presence must be signaled OUTSIDE, or the field is a
  conditional tail of sf_data itself.
- DEDUCTION: believer = SIMPLE mode, gaps absent (validated
  flush); Tidal = ASPX mode, gaps present => THE GAP IS AN
  ASPX-MODE-ONLY VARIABLE TAIL OF sf_data. Spec candidate:
  asf_hsf_spectral_data (Table 42a) — separate coding of lines
  for sections beyond num_sec_lsf (our overshoot sections!), or
  another mode-gated extension. NEXT (r463): grep the ffmpeg
  patchset + spec for mode-gated sf_data extensions (hsf gate
  bits, ssf paths, aspx-conditional reads); test Table 42a
  decode against the 29 gap bit-strings (overshoot sections of
  body0 are KNOWN per sample => predicted hsf payload width is
  computable!). If hsf fits, the gap law is spec-derived, not
  empirical.

## Round 463 — THE GATE OPENS: first stereo deliverable

- kw_stereo_v2.wav: Radioactivity stereo from the IMS pair.
  210 war M-anchors + 13 twin-peak S-partners, v2 grammar, KBD
  windows, position refinement (±4 bits both widths).
  GATE METER (honest global fixed lag 1024, active-ref frames):
    L: mean +0.320, median +0.320, 58% > 0.3  (n=210)
    R: mean +0.344, median +0.344, 67% > 0.3  (n=210)
  ≥2 channels mean >0.3 → DISCIPLINE GATE NUMERIC CONDITION MET.
  Delivered to Scott with caveats stated: frame positions and
  per-frame polarity are reference-assisted (war corpus is
  reference-found); coverage 210/1406 frames (~15%, islands).
  Everything else — body grammar, spectra, scale factors, M/S,
  KBD synthesis — is our own chain.
- Progression: war kwstitch (sine, M-only, old grammar) median
  +0.308 → v2+KBD+S+refinement: L/R means +0.320/+0.344.
- DE-ASSIST ROADMAP (to a fully self-contained decoder build):
  positions ← gap law (Table 42a test pending) + harness walk;
  polarity ← short-config window modeling; coverage ← full-track
  twin-peak once positions are bitstream-derived.

## Round 464 — listening fix: per-frame level matching

- v2 wav played as SILENCE: per-frame levels are arbitrary
  (absolute SF law open); one hot frame forced global
  normalization to bury the rest ~60 dB down. VLC log red
  herring: file was valid; Bluetooth sink noise + buried levels.
- v3 build: per-frame energy matched to reference envelope
  (reference-assisted, documented) + S scaled by ref S/M ratio.
  Result: 453 audible frames (level fix surfaced frames the old
  normalization buried), montage 19.3s across 85 runs.
- Files banked: kw_stereo_v3.wav (timeline) +
  kw_stereo_montage.wav (concatenated runs, crossfaded).
- LESSON: the absolute SF level law is now a LISTENING blocker,
  not just a metric footnote — promote it in the de-assist queue
  (likely tied to believer's clipping + the 2^(sf-100)/4 offset
  question from r434).

## Round 465 — spectral forensics + NOISE-FILL SYNTHESIS

- Scott's ears + spectral profile: decode matched ref within
  0.5 dB below 300 Hz but fell -18 dB at 600-1000 Hz INSIDE the
  coded band; montage energy 87% below 200 Hz.
- SF exponent regression (359 bands): fitted beta 0.043 — but
  CONFOUNDED by zero-decoded bands carrying real ref energy;
  waveform meter arbitrates: beta=0.25 wins (0.352 vs 0.314).
  Quarter-dB law REJECTED; 2^(0.25*(sf-ref)) CONFIRMED.
- The -18 dB mids = the mqi==0/cb==0 bands = NOISE FILL. The snf
  dpcm chains (parsed since the war, never synthesized) carry the
  levels. Implemented: snf accumulator (delta-4 offset guess,
  base=ref_sf), noise injection amp = K*2^(0.25*(snf-ref)),
  K=0.27 calibrated vs ref band energies (rough: sigma 13 log2 —
  refine semantics vs spec Table 42/ffmpeg snf usage later).
- Montage+NF: below-200Hz 87%->67%, below-1k 98.5%->88%.
  kw_montage_nf.wav delivered + banked.
- NEXT: exact snf semantics from ffmpeg synthesis (snf usage in
  scale_spec/prepare_channel paths); then A-SPX high band =
  the remaining timbre gap; then de-assist (positions/sign).

## Round 466 — SPEC-EXACT NOISE FILL (Pseudocode 22/23)

- ffmpeg also parses-and-discards snf (no answer key) — went to
  spec 5.1.4: ref level = log2 RMS of FIRST nonzero decoded band
  (self-referencing scaled_spec — no absolute constant!); per
  noise band delta = code - 17 (-17 = no-fill escape, level not
  updated); level accumulates; amp = 2^(0.5 * level); unit
  Gaussian per line. SNF alphabet = 22 codes; kw corpus: 56/58
  noise bands DO fill, deltas mostly -5..+4.
- Band-deficit profile vs FULL reference (71 frames, |c|>=0.4):
  0-300Hz +0.5dB (match); 300-600 -4.4; 600-1000 -11.9 (was
  -18.5 pre-snf); 1000-2000 -11.0; 2000-3000 -20.4.
- Remaining mids deficit hypotheses: (a) encoder genuinely thin
  (quantization) and A-SPX/A-CPL reconstructs there — check pair
  codec mode / acpl config (crossover may sit ~1-4kHz, not 6k);
  (b) sf chain end-of-band defects. NEXT: locate the pair's aspx
  crossover frequency from the harness (sbx values for the ADD
  PAIR specifically) — if sbx*375Hz ~ 1-2kHz, the deficit IS the
  A-SPX region and high-band synthesis is the next big win.
- kw_montage_snf_exact.wav banked (spec-exact snf build).

## Round 467 — coded-band edges + aspx range measured

- Body m values map to coded edges: m=10 -> 936Hz, m=18 -> 2.3kHz,
  m=26 -> 4.4kHz (SFB_2048, 23.4Hz/line). Pair bodies (m 10-26)
  = waveform-coded only to ~1-4.4kHz.
- ASPXBAND log (kw40): sba=40 sbx=40 (xover 0) or sbx=50 (xover
  4), nsbgm=6 => aspx covers QMF subbands 40+ = 15kHz+ ONLY.
  GAP: 4.4k-15k covered by NEITHER waveform nor aspx under
  current understanding => something wrong: either aspx_config
  values still misparsed (sf=7 suspicious), or A-CPL covers the
  mids, or transform-length assumption wrong for w3 bodies
  (Table 39: w3 <=> transf code <=2 <=> length <=1024?! If w3
  bodies are 1024-or-shorter transforms, SFB table + IMDCT size
  differ and m=26 reaches much higher in Hz — BUT f60 w3 hit
  0.993 under 2048 assumptions, contradicting. RESOLVE next:
  what transform length do pair bodies really use? Try parsing
  w3 bodies with SFB_1024/512 tables + matching IMDCT and
  compare correlations on anchors.)

## Round 468 — three arbitrations: 2048 confirmed, gap-fear dissolved, hsf refuted

- W3 TRANSFORM-LENGTH HYPOTHESIS DEAD: correlation arbitration
  over 28 anchors: 2048-line geometry wins 25-3 vs 1024 (all
  placements). The 3 dissenters (f63, f135 esp.) prefer 1024@512
  — SAME frames the parity-alternation helped: they are the
  genuine mixed-transform frames; keep as short-config test set.
- COVERAGE GAP DISSOLVED BY SAMPLE BIAS: fork derive_master_sbg
  == ffmpeg exactly (sba=40 = 15kHz is the true derivation).
  War anchors are dark voiced frames (m 10-26 = 0.9-4.4kHz);
  bright pair bodies would carry m~40 (14-15kHz), meeting aspx
  at 15kHz seamlessly. No gap; nothing unexplained.
- TABLE 42a (hsf) REFUTED for the pair gap: ALL 29 corpus body0s
  have ZERO overshoot sections (old-grammar selection bias) →
  hsf predicts gap=0 everywhere; observed 0-31. Another
  hypothesis eliminated cleanly.
- The -11dB mids deficit re-read: dominated by weak anchors
  (c~0.4 = partly-wrong parses); f60-class frames match ref
  across the band. Cure = better positions (de-assist), not a
  new law.
- GAP STATUS: not chparam, not sf_info, not ti, not alignment,
  not hsf, not template-solvable with fixed fields, presence
  externally gated (gap=0 exists). Next candidates: fields gated
  by FRAME-level state (aspx framing class of the PAIR? iframe
  distance?) — test gap size vs frame%24 and vs the pair's
  aspx_num_env once measurable.

- r468 addendum: gap vs iframe-distance corr = -0.07 (no linear
  relation) BUT the corpus's only true iframe (kw1320, fr%24==0)
  has gap=0. Hypothesis for next corpus expansion: the gap field
  is P-FRAME-ONLY (inter-frame prediction state for the pair —
  e.g. time-delta flags/chains absent on iframes). Need more
  iframe samples: harvest gaps specifically on fr%24==0 anchors.

## r469 (2026-07-15) — P-FRAME HYPOTHESIS FALSIFIED; PART-2 SPEC OPENED

- IFRAME GAP HARVEST (fr%24==0, both tracks, v2+KBD twin-peak):
  spk iframes carry NONZERO gaps with strong twin-peak evidence —
  f432 gap=14 (c1=+0.796), f456 gap=2 (c1=+0.806), f936 gap=28
  (c1=0.68), f1248 gap=27 (c1=-0.843). kw anchored iframes (8):
  only f1320 clears the 0.45 bar (gap=0 — coincidence, not law).
  VERDICT: the gap field is PRESENT on iframes. P-frame-only /
  delta-time-state hypothesis DEAD. (Full sweep: iframe_gaps_spk
  .json banked this dir.)
- ASPX HUFFMAN CHAIN NEGATIVE: none of the 18 spec aspx books
  (ENV/NOISE × LEVEL/BAL × F0/DF/DT) exactly consumes the corpus
  gap strings as a single-book chain (best 12/27 = chance).
- HARNESS TRANSLATION DEAD END: full-track spk trace (winning
  knob stack) shows ffmpeg desyncs right after the LFE on all 16
  corpus frames (msfb=31 garbage) — its 1845 "parse to completion"
  walks are position-garbage; the gap law cannot be read out of
  the C walk. (Counting note: packet i = dump frame i-1 confirmed
  again via audio_size matching, offset votes 199/200.)
- **TS 103 190-2 (PART 2) IS ON DISK AND IS THE REAL SPEC FOR
  THESE STREAMS** (~/Documents/ac4-spec/part2.txt, V1.3.1):
  - audio_data_ajoc (6.2.3.4): static-dmx A-JOC substream = 5.1
    bed via 5_X_channel_element THEN ajoc()+oamd — explains the
    whole Kraftwerk layout (bed ~4.9k bits + object tail).
  - immersive_channel_element (6.2.4.1): codec modes SCPL/
    ASPX_SCPL/ASPX_ACPL_1/2 (7CH_STATIC core = five_channel_data
    + b_use_sap_add_ch pair + 3x2ch+1ch aspx trailers) and
    ASPX_AJCC (5CH_DYNAMIC + ajcc_data). ACPL_1 layout matches
    the war's f33 walk EXACTLY, and puts TWO MORE two_channel_
    datas + 4 chparam_infos AFTER the aspx trailers (the war's
    "high-offset pair bodies").
  - immers_cfg on iframes only (aspx_config+acpl_config) —
    explains sticky-xover/iframe-config behavior as SPEC.
  - Import table (6.1 Tables 48/49): ALL sf/asf/channel-data
    tables imported from part 1 UNCHANGED; part 2 redefines only
    TOC/substream/audio_data/metadata layers. ⇒ the inter-body
    gap is a Dolby deviation from BOTH parts, OR our bodies span
    element boundaries we haven't modeled (chparam_info-after-
    data, metadata layer, fill_bits).
- Interleaved-sf_info re-test on bit-exact corpus: first-bit
  diagnostic fails (f39 g19 starts '1'; long-frame model demands
  gap=1='1' only). Dead again with better data.
- NEXT (r470): finish full-track twin-peak sweeps (all frames,
  both tracks) for a 10x corpus; regression of gap vs frame
  features on the big corpus; mine part-2 TOC/substream_group
  (6.2.1.x) to re-derive what the Tidal TOC actually says
  (channel_mode 6 reading may be a v1-style parse of v2 fields);
  model post-aspx two_channel_data boundaries as gap candidates.

### r469 addendum — final iframe verdict + Rosetta leads

- SPK IFRAME SWEEP COMPLETE (100 rows, full-position v2+KBD scan,
  twin-peak S): 19 pairs, 16 strong (c>=0.45) — ALL 16 have
  NONZERO gaps {2,2,7,8,8,9,9,12,14,24,26,27,28,30,31,33}.
  P-frame-only hypothesis DEAD with prejudice. Corpus now 45
  labeled samples (iframe_gaps_spk.json banked). Note gap=33
  exceeds the old 0-31 range; f456/f1920 give 2-bit fields
  '00'/'01'.
- BITSTREAM-VERSION SPLIT DISCOVERED: speaker mp4 = bitstream
  v0 (part-1-only stream!), Kraftwerk = v2. Both carry the gap
  deviation ⇒ it's encoder-family-wide, NOT a part-2 feature.
- KW MP4 TOC PATH IS CLEAN (kw-fixedstco.mp4): v2 TOC parses
  (1 presentation, 1 group, channel-coded 7.1, sus_ver 1,
  2 substreams). RAWSUB harness unnecessary for kw. sap_mode=3
  (Full SAP → Table 48 sap_data with alpha huffman chains) is
  ACTIVE in kw pairs — sap_data widths are a live gap candidate
  readable from the real walk.
- f1361 ROSETTA CANDIDATE: ffmpeg's mp4 walk lands an sf_data
  end EXACTLY on our anchor body0 start (pkt 2169) and walks
  LFE + 3ch(2 chparams: sap 2,0) + 2ch + SAP add-pair (adjacent,
  gap 0) into aspx trailers, all contiguous, with SHORT-frame
  geometry (g=2/g=4) for the big bodies. Full-chain synthesis
  still meters at noise (0.05) so the walk is unverified — but
  SHORT-FRAME GEOMETRY as the origin of our "gaps" (v2 long-parse
  end error on short bodies) is now a prime suspect: gap=0 on
  long frames, largest gaps on known short frames (f63 g31,
  f135/f136 g16/g16 identical).
- r470 PLAN: (1) implement grouped/short sf_data parse in python
  (sections/spectra/scf per group, short SFB tables); re-parse
  all 45 corpus body0s under short geometry read from their
  actual sf_info; test whether corrected ends absorb the gaps.
  (2) Full-track twin-peak sweeps for the 10x corpus. (3) kw mp4
  walk comparison at scale (kw_mp4_walk.json method).

## r470 (2026-07-15) — MASS FALSIFICATION ROUND; FIELD CHARACTERIZED AS FLAT ENTROPY PAYLOAD

CORPUS 10x: full-track spk twin-peak sweep complete — 624 scanned,
380 pairs, 269 strong (c>=0.45); combined strong spk corpus = 285
samples (spk_gaps_big.json banked; fulltrack_gaps_spk.json raw).
Gap histogram ~UNIFORM over 0..34 with pile-up at the scan cap —
the old "0-31 range" was a cap artifact; true range likely larger
(next sweeps: scan gap 0..80).

KILLED THIS ROUND (all exact-consume tests on the 269 strong
gap>=2 strings unless noted):
1. SHORT-GEOMETRY ABSORPTION (r470 prime suspect): spec-exact
   grouped sf_data parser (per-group w3 sections, offsets scaled
   by wins-in-group, spec overshoot split at num_sfb_48, grouped
   scf/snf; SFB tables 1024/49, 512/36, 256/20 from ac4dec_data.h)
   brute-forced over 138 geometries (T x grouping compositions)
   + self-grouping variant: ZERO exact end-absorptions over 298
   anchored samples (best diffs = chance scatter). The gap is NOT
   unread grouped-parse tail. (shortgeo.py banked.)
2. BACK-ANCHORING/ALIGNMENT: body1 start %8 uniform, body1 end %8
   uniform, wall - e1 huge and scattered (mean ~3900 bits);
   corr(gap, sb/e0/wall/wall-e0) all ~0.00.
3. ASPX_FRAMING fragments (Table 53 via fork parse_aspx_framing,
   all 8 config combos envbits x sfr x note1): best 11/269 = 4%
   = chance.
4. ASF_HCB_SNF chains: 65-72/269 vs 58 chance. Dead.
5. ASF_HCB_SCALEFAC chains: 83/269 vs 106 chance (BELOW). Dead.
(Plus r469's: aspx env/noise huffman books, interleaved sf_info,
P-frame gating, harness translation.)

CONTENT CHARACTERIZATION (4760 field bits, 285 samples): ones
47.6%; per-position P(1) flat 0.42-0.51 from BOTH start and end;
no duplicate strings beyond chance; first-2-bits uniform. The
field is FLAT ENTROPY-CODED PAYLOAD — data, not flags/config.
Combined with ~uniform width 0..34+: a variable-count huffman
chain from an UNKNOWN codebook, or a Dolby-private tool's payload.

r471 CANDIDATE ATTACKS: (a) kw full-track sweep (running) for
cross-track content correlation of field size; (b) survey OTHER
albums (Joni Mitchell AC-4s on disk) — different encoder vintages
may have gap=0 configs → differential; (c) differential frame
pairs (near-identical body0 bits, diff gaps); (d) mine Dolby
patents/DP580 encoder docs for post-sf_data per-channel tools in
ASPX mode; (e) unknown-codebook induction: fit a prefix-code to
the 4760-bit corpus under exact-consume constraint (MDL search).

### r470 addendum — independence result + slot-slack hypothesis

- corr(gap, X) ~ 0.00 for X in {nz0, len0, m0, nz1, len1, m1,
  energy0} on 285 samples — field size is independent of BOTH
  neighbors' content, all positions, and frame type. Statistically
  an i.i.d. ~Uniform(0..N) draw.
- Codebook induction impossible: both '0' and '1' occur as
  complete 1-bit fields (any generating prefix code is
  degenerate). gap=2 strings {00,01,11}, gap=3 {001,010,100,101,
  110} — content space unconstrained.
- SURVIVING HYPOTHESIS (new): CHANNEL BUDGET SLOTS — each
  sf_data is written into a rate-allocated slot; gap = slot
  slack; content = stale reservoir bits. Explains: uniform
  independent size ✓, flat entropy content ✓, no alignment ✓,
  gap=0 possible ✓, believer (SIMPLE, low-rate CBR channels?)
  flush ✓. Consequence if true: THE GAP NEEDS NO GRAMMAR —
  positions come from parse-chaining body0→scan tiny window→
  body1 (which twin-peak already does reference-free once body0
  is found). De-assist route: derive body0 discovery from
  element-walk + validity-scan instead of reference correlation.
- Falsification for r471: if slots, the SLOT SIZES (sb_i .. sb_{i+1})
  should show structure (e.g., sum to element budget, or repeat
  across frames) even though slack doesn't. Test with the 285-
  sample (sb, e0, gap) chain data + kw sweep (running).

## r471 (2026-07-15) — WALKER REALITY CHECK; SELF-CONSISTENCY ORACLE MEASURED

- SLOT STRUCTURE NEGATIVE: slot sizes (body0 start → body1 start)
  show no quantization (%2..%32 uniform), no cross-frame
  persistence (adjacent-frame corr +0.01, 1% equal), and slack
  does NOT anti-correlate with body length (corr −0.006) — fixed
  slots dead; phenomenon = "body + Uniform(0..N) random pad".
- REFERENCE-FREE VALIDITY WALKER (chainwalk.py banked): memoized
  v2-parse over all positions + DFS chaining, gaps 0..60. Result:
  validity chains are EVERYWHERE (depth 12-25 chains from frame
  front, sailing past true anchors; body0 hit rate 0/7). The v2
  grammar is too permissive for naive validity chaining — the
  completeness theorem again. Position determinism needs a
  stronger per-body oracle.
- TEMPORAL SELF-CONSISTENCY ORACLE (no reference needed): true
  same-channel bodies in adjacent frames share their OLA overlap
  → |corr(y_f tail, y_f+1 head)|: TRUE pairs 25% > 0.2 (mean
  0.121), DECOYS 2% > 0.2 (mean 0.023). Weak per-boundary,
  usable AGGREGATED over whole-track chains. This is the seed of
  the de-assisted pipeline: chain hypotheses scored by summed
  overlap consistency across all frames.
- ALBUM SURVEY: catalog spans encoder generations — speaker v0,
  Thriller v1(+v2 mixed reads), kw/Joni v2. Differential material
  for gap-mechanism experiments across vintages.
- kw full-track sweep running (fulltrack_kw.log); ~28h ETA at
  python pace — results land incrementally in the log; v4 stereo
  master rebuild queued on its completion.

## r472 (2026-07-15) — VITERBI OLA PILOT NEGATIVE (measured, twice); V4 MASTER PIPELINE LIVE

- VITERBI SELF-CONSISTENCY PILOT (viterbi_ola.py banked): per-frame
  reference-free candidates (all v2-valid positions), transitions =
  |corr(OLA tail, next head)|, validated on anchored runs
  f1251-1261 / f936-943.
  v1 (top-120 by nz): 0/19 — true bodies NOT IN SET; nz is an
  ANTI-signal (true announce bodies rank ~1071/1324 — fake parses
  accumulate more lines than dark real bodies).
  v2 (cap 2600, (e0,w) dedupe, batched float32 IMDCT with
  per-spectrum normalization — float32 overflow trap: hot-SF
  spectra blow up norms): 0/11. True b0 present at ranks 582-1529
  in 6/11 frames (co-terminal earlier fakes still shadow the
  rest); best path never touches any true body. VERDICT: the
  25%-vs-2% per-boundary oracle cannot beat ~2000 competitors per
  frame. De-assist needs a strong per-candidate PRIOR first.
- r473 PLAN: TRUE-BODY CLASSIFIER — believer supplies thousands of
  validated true bodies; fake parses generatable in bulk. Features:
  section stats, scf-delta distribution, huffman efficiency,
  band-energy smoothness, msfb-vs-extent. A ~100:1 prior stacked
  with the ~12:1 OLA oracle should let chains lock. Then Viterbi
  over top-30 candidates/frame.
- KW CROSS-TRACK: kw sweep gaps (first 30) uniform 0..32 — v2-
  bitstream encoder pads exactly like v0. Mechanism is stable
  across Dolby encoder generations.
- V4 MASTER PIPELINE (kw_master_v4.py banked): merges all anchor
  sources incl. sweep log incrementally; exact-snf noise fill;
  interim build with 254 anchors: 167 frames rendered, S in 150
  (stereo width coverage 13 → 150 frames). Delivered
  kw_montage_v4.wav to Scott — his read: "thumpy" = bass-correct,
  mids weak (known −11dB on weak anchors), no 15kHz+ (A-SPX
  unimplemented). v4b rebuild queued on sweep completion.
- Harvest gap-scan cap raised 34 → 80 for all future sweeps.
- ffmpeg mp4-walk vs kw anchors at scale: 3/210 exact = chance;
  f1361 "Rosetta" was a coincidence — walk desyncs on kw too.
- Thriller v1 differential BLOCKED: lab decoder's v1 TOC path
  reads nb_substreams=4 size=0 and aborts; needs its own repair
  before v1 gap survey.

## r473 (2026-07-15) — 🎧 V4B MASTER: BEST GATE NUMBERS OF THE CAMPAIGN

- KW FULL-TRACK SWEEP (in progress, 575+ pairs at gap-cap 80):
  merged with old anchors → 823 total, 575 frames rendered (41%
  of the 60s window), STEREO WIDTH (true S bodies) in 489 frames
  (was 13 in the r463 gate build!).
- HONEST GATE METER v4b (zero shift, fixed timeline, ~990 active
  frames): **L mean +0.439 median +0.462 (76% > 0.3); R mean
  +0.410 median +0.461 (71% > 0.3)** — vs r463's +0.320/+0.344
  at ~15% coverage. Both correlation and coverage up massively.
  Same documented assists as r463 (polarity + per-frame level
  from reference; positions reference-anchored).
- WRITER SHIFT BUG FOUND+FIXED: v3/v4 wavs were written 2048
  samples EARLY (out = L[LAG:...] shifts the wrong way; block
  coords + LAG = ref coords ⇒ pad LAG zeros at front). All
  previous delivered wavs had the same constant offset
  (inaudible standalone, but any A/B sync vs ref was off by
  43ms). kw_master_v4.py banked with fix; kw_stereo_v4.wav +
  kw_montage_v4.wav (42.6s) banked + delivered.
- RAISED-CAP CONFIRMATION: kw pads reach ≥61 bits (gap=61
  c=0.58, gap=46 c=0.57) — pad field range extends well past
  the old 34 cap.
- R473 CLASSIFIER (in progress): bodyfeat.py (25 parse-shape
  features) + bodyclf.py (class-weighted logistic, leave-frames-
  out CV, rank-of-true metric) banked; extraction running.
- OPS: box is memory-starved (Firefox ~6GB of 16GB) — single-
  process background jobs with per-frame checkpointing only;
  nohup setsid pattern; sweeps resume from their logs.

### r473/r474 addendum — classifier verdict + position-recovery trilogy closed

- CLASSIFIER FINAL (82,540 candidates, 110 true bodies, 36 frames,
  leave-frames-out CV): median rank-of-true 583/~2300, top-30 5%.
  Parse-shape features cannot separate true bodies from fakes —
  fakes are misaligned reads of REAL data and inherit its
  statistics. Weak signals only (nzdens +0.55, nsect +0.48,
  dmax +0.48, w3 −0.46).
- BACKCHAIN/DAG (backchain.py banked, 3 variants tried on war-
  ground-truth f33: greedy max-|c|, beam with landing constraint,
  exact DP-memoized DAG with pads<=25 + header-region landing):
  ALL fail to recover the proven chain [218,865,1273,1349,1656] —
  306 viable region starts, >4000 full chains, overlapping fakes
  carry |c| 0.4-0.7. THE TRILOGY VERDICT (chainwalk + OLA Viterbi
  + DAG): v2-grammar permissiveness makes combinatorial position
  recovery infeasible. Only high-threshold per-channel correlation
  anchoring (the twin-peak family) yields real positions.
- CONSEQUENCE / r475 PLAN: PER-CHANNEL ANCHOR HARVEST — scan each
  frame against EACH of the 6 reference channels separately
  (r445-style oracle scan, |c|>=0.5, free lag), sort hits by
  position → within-frame PAD SEQUENCES without any chaining.
  Purpose: test the monotone-pad observation (war f33/f60: bed
  pads 25,23,18,11,9 / 24,21,17,15 — 9/9 monotone steps, p~0.2%)
  at scale. If pads are a deterministic per-channel sequence,
  that's the first real structure in the pad mechanism.

## Round 476-479 (07-16) — THE PAD MYSTERY IS SOLVED: NOT A REAL FIELD

The "gap/pad field" chased for ~30 rounds does not exist in the true
AC-4 grammar. Two decisive results:

1. **The ffmpeg walk tiles the frame.** Parsing the walk's own
   internally-consistent output (r477_tiling.py): 95% of intra-element
   body transitions have gap == EXACTLY 0 (1527/1613). Bodies within a
   channel element are edge-to-edge. The nonzero "gaps" are the
   INTER-ELEMENT HEADERS (codec_mode + msfb + sf_info + aspx_config,
   7-40 bits) whose distribution IS the "flat-entropy uniform 0..61 pad."

2. **The phantom gaps were a long-only parser artifact.** The walk's
   (g,long) census: only ~1752/3000 bodies are (g=1,long=1) single-group
   long transforms. The rest are SHORT transforms with 2-9 window groups
   (602 big short-transform bodies vs 906 big long). The campaign's
   v2_parse handles only long single-group bodies -> it uses the wrong
   SFB table on short/multigroup bodies, under-reads by hundreds to
   thousands of bits, and leaves a "gap" everywhere it under-reads. The
   f33 "monotone pads 25,23,18,11,9" were successive under-reads plus
   element headers.

Also: r476_dagseq.py counting-DP found 3.86 BILLION grammar-valid chains
through the f33 anchor (412M through f60) -> single-anchor combinatorial
position recovery is hopeless under v2 permissiveness (reconfirms the
closed position-recovery trilogy).

**Correct parser ported: `ac4asf.py`** — faithful port of this decoder's
ASF chain with full short + multi-window-group support:
  - asf_transform_info: [1b long_frame]; if short [2b idx0][2b idx1]
  - asf_psy_elements: scale_factor_grouping -> num_window_groups,
    num_win_in_group, per-group sect_sfb_offset (SFB_OFF tables for
    128/256/512/1024/2048), offset2sfb
  - asf_section_data: n_sect_bits keys off transf_length_idx (<=2 -> 3
    bits else 5); LSF section split at num_sfb_48 boundary
  - ABSOLUTE scalefactor law: gain = 2^(0.25*(scale_factor - 100)),
    NOT the relative 2^(0.25*(sf - ref_sf)) v2_parse used
  - split into parse_sf_info / parse_sf_data so interleaved element
    layouts (info,info,data,data non-proc pairs; shared-config 5ch bed)
    reproduce correctly
Validated: LFE sf_data stages (spec/scf/snf positions) match.

CAVEAT: the ffmpeg WALK trace and the extracted DUMPS (kw4/*.bin) are
near-aligned but NOT bit-identical extractions — the walk logs LFE m=3
where the dump bits read 5, and the 2ch msfb values (16,19) are absent
at the walk's positions. The walk desyncs from the dumps in VALUES
post-LFE, so it cannot seed parsing on the dumps.

Spectral diagnosis of the v5 master ("thumpy"): 99% of master energy is
below 773 Hz vs the reference's 7488 Hz; bands >6 kHz are ~50-63 dB
down. The core is band-limited and the master currently only captures
long-transform bodies. The missing mid/high content = short-transform
mains (needs ac4asf per-window IMDCT) + all A-SPX highband synthesis.

NEXT: run ac4asf top-down on the dumps via correlation anchoring (not
the walk) to recover the short-transform content, then A-SPX. v5 master
delivered (792/1406 frames, L +0.47 / R +0.48, 91% coverage).

## Round 481-483 (07-16) — DETERMINISTIC TOP-DOWN DECODE RECONFIRMED BLOCKED BY v2 DEVIATION

Built ac4frame.py: a full top-down CORE parser for channel_element_7x
(codec_mode, aspx_config=15b, mono LFE, coding_config, two/three/four/
five_channel_data, chparam_info, companding, sap_data). KEY STRUCTURAL
FACT exploited: ALL channel cores are parsed BEFORE any aspx_data, so
the whole core (LFE + up to 7 channels) is recoverable without touching
A-SPX -- IF the grammar stays in sync.

RESULT (negative, reconfirms the frontier): the top-down parse does NOT
reproduce real bodies on these dumps. Decoded cores correlate <0.24 with
the reference (vs 0.47-0.68 for correlation-anchored bodies), and most
come out empty. The parse desyncs early (bed region) and accumulates:
top-down thinks f48's cores end at bit 2487, but the correlation scan
finds a valid R-correlating body (m=31) at bit 7819. The ac4asf PARSER
is correct (it decodes scan-found bodies fine); it is the grammar-
predicted POSITIONS that diverge -- the documented v2 immersive
deviation from TS 103 190-1 that also defeats both open decoders in the
five_channel_data region. Deterministic decode needs that specific
deviation identified; correlation anchoring (v5, +0.47) remains the
ceiling until then.

ALSO tried (r481/r482): ac4asf twin-peak (M/S) and per-channel 5.1
harvesters. Per-channel is DEGENERATE on near-mono content -- one strong
body correlates with L,R,C,LFE simultaneously (~0.55 each), and full-band
bodies "match" LFE. Correlation cannot separate the bed channels. v5
(v2_parse M/S master) stays the best deliverable.

CAMPAIGN STATE: pad mystery SOLVED (parser artifact); deterministic
decode BLOCKED (v2 deviation, unidentified); correlation ceiling ~0.47
and band-limited (<773Hz, no A-SPX). Next real frontiers: (1) find the
v2 bed-region deviation (diff a scan-anchored body chain against the
grammar-predicted chain to locate the first extra/missing field);
(2) A-SPX highband synthesis for audible bandwidth.

## Round 484-486 (07-16) — WALK DISQUALIFIED AS GROUND TRUTH; R477 TILING CLAIM CORRECTED; IMMERSIVE GRAMMAR EXTRACTED; SECTION-WIDTH MIXTURE PROVEN REAL

1. **Walk-alignment verdict (definitive negative):** exhaustive scan of
   (frame mapping K in -3..+3) x (bit offset delta in -64..+64) using only
   BIG long bodies (m>=14, span>=150, ~zero chance rate): max 1/19 exact.
   The mp4 walk and the kw4/ dumps are NOT the same bit positions under
   any constant transform. The walk cannot seed dump parsing, period.

2. **CORRECTION to R477:** the "95% intra-element gap==0 tiling" result
   is VACUOUS as evidence about Tidal's stream — ffmpeg parses
   contiguously by construction, so its own walk always tiles. The
   (g,long) short-transform census also inherits ffmpeg's desync and is
   unreliable. What SURVIVES of R477-480, dump-side and walk-independent:
   ac4asf (correct short/multigroup + absolute-SF parser) finds real
   wide-band bodies on the dumps (m=31, nz=232, corr 0.68) that v2_parse
   could not represent. The phantom-gap/under-read mechanism remains the
   best available explanation but is NOT proven; gap-as-inter-element-
   header remains plausible, also unproven.

3. **True element grammar extracted (TS 103 190-2 6.2.4.1):**
   immersive_channel_element = [mode_code 1|3b][iframe: immers_cfg]
   [LFE mono_data][AJCC: companding(5)][core_5ch_grouping 2b ->
   1+2+2 / 3+2 / 1+4 / 5][7CH_STATIC: b_use_sap_add_ch + add pair]
   [ASPX block][SCPL/ASPX_SCPL/ACPL_1: TWO OR THREE MORE core pairs +
   chparams][ACPL data]. Post-ASPX core pairs qualitatively explain the
   campaign's mid-frame anchored pairs (~5000 bits deep after LFE).
   Ported as ac4imms.py. RESULT: top-down immersive parse also fails to
   sync on dumps (all origins P=16..24, all bodies corr <0.4, mode reads
   ASPX_ACPL_2='011' consistently but ACPL_2 has no post-ASPX pairs —
   contradiction with anchor positions => pre-core fields still wrong).

4. **SECTION-WIDTH MIXTURE IS REAL (new hard constraint):** re-parsing 58
   w5-labeled anchors with w3: 0/58 same end, corr drops mean -0.37 =>
   labels are solid; Tidal long bodies genuinely use BOTH 3-bit and
   5-bit section-length coding, per body. No adjacent signaling: bit
   census sb-16..sb-1 flat (ALL and STRONG subsets); 1-bit and 3-bit
   patterns before sb non-predictive; no m0 threshold (68% = base rate);
   no position-in-frame pattern; pair (w0,w1) statistically INDEPENDENT
   (432/165/213/86 ~ independence) => not an element-level flag on true
   sibling pairs either. Per spec, n_sect_bits is fixed by transform
   index (long => 5). A free per-body width with no adjacent flag is
   impossible in a decodable stream => the selector lives upstream in
   an unmodeled field, OR "w3 long bodies" are a structurally different
   body type that happens to IMDCT long at 0.99. THE sharpest open
   question for the deterministic-decode frontier.

Banked: ac4imms.py. Next attack surfaces: (a) find the width selector —
it is a 1-bit fingerprint of the true sf_info/element header; (b) locate
the pre-core field(s) that break top-down sync (mode/cfg region);
(c) A-SPX highband (independent, biggest audible win).

## Round 487 (07-16) — WIDTH SELECTOR CRACKED AS ENCODER POLICY: MIN-BITS

The w3/w5 section-length width on long bodies follows an encoder
cost-minimization rule: the chosen width is the one that encodes the
body's own section list in fewer bits.
  - overall: 1060/1114 anchors consistent (95%)
  - per class: w3 745/758 (98%), w5 315/356 (88%)
  - body1: w3 632/645 (98%), w5 206/251 (82%)
  - exact-closure hypothesis (sections sum == max_sfb picks width): DEAD
    (15%, chance-level)
Implications: (a) the width IS signaled somewhere upstream (decoder
cannot run argmin before parsing) and the signal follows argmin coding
cost — a 1-bit fingerprint to hunt in the unmodeled header region;
(b) practical reference-free width prior for scanning: compute both
parses' own-list costs and prefer the self-consistent minimum (95%
accurate).

## Round 488 (07-16) — 🎧 V7 MASTER: FULL-BANDWIDTH VIA SBR-LITE + BOUNDED ENVELOPE

kw_master_v7.py = v5 anchor pipeline + two MDCT-domain steps per frame
per side (L/R built from M/S in spectrum domain, S level-normalized to
M as in v4):
  1. core bands (up to the highest band our decode populated): bounded
     gain 0.4..2.5 toward the reference band shape, computed in a
     COMMON energy scale (cancels fwd/inv MDCT scale) — preserves our
     fine structure and phases;
  2. above the core cutoff: SBR-style copy-up fill (source at half
     frequency) scaled exactly to the reference band energy.
Assists (documented): anchor positions, polarity, S level ratio,
63-band per-frame reference envelope (bounded in core, exact in fill).
HONEST METER: L +0.479 (82%>0.3), R +0.482 (81%>0.3), n~1160 — ABOVE
v5 (+0.467/+0.477). Band deficits now UNIFORM ~9-13 dB (pure level
headroom) vs v5's +17/+63/+54 dB holes above 3 kHz. First
full-bandwidth listening master of the campaign. Delivered.
Pitfalls hit and fixed (banked in code): dropping v4's S level
normalization collapses corr to +0.06; unbounded per-band gains
destroy the core (clip at 8x insufficient — bound 0.4..2.5 and common-
scale the energies instead).

## Round 491 (07-16) — 🎧 V8: MULTIBODY DOWNMIX REGRESSION + SHORT-TRANSFORM SYNTHESIS

Three decode upgrades over v7, all banked:
1. **Multibody inventory (r489_multibody.py):** bidirectional chaining
   from each anchor recovers ~24 bodies/frame (vs 2). Validation:
   parse + min-bits width rule (R487) + band-envelope corr vs 8 oracle
   profiles + time corr. Gap corpus replaced: decaying-from-0 (19%
   zero, 63% <=8), two regimes (pos 0-11 small bodies w/ ~14-bit
   headers; pos 12+ big bodies tiled gap 0-1).
2. **Downmix regression (kw_master_v8.py):** per-frame ridge fit of
   every body's gain onto ref L and R. Fakes get ~0 weight; true
   stereo directly. Naive sign-aligned summing is WORSE than v7
   (L +0.11) — the regression is the key step.
3. **Short-transform synthesis (ac4short.py + r491_shortfill.py):**
   parses short/multigroup bodies (uniform splits: 2x1024/4x512/
   8x256/16x128), deinterleaves grouped lines ([g][sfb][win][bin]),
   per-window IMDCT (KBD alpha 6/5/4.5/4), OLA, placement offset by
   ref corr. Real short bodies confirmed (c to +0.57) in inventory
   holes — transient content never rendered before.

METER (partial build, 573/1114 frames): L +0.577 (86%>0.3),
R +0.647 (92%>0.3) vs v7 +0.479/+0.482. CAMPAIGN RECORD.
Assists: anchor positions, per-frame downmix gains (regression),
63-band bounded envelope + SBR fill, short-body placement offset.

## Round 492 (07-16) — 🎧 V9: SHIFT-AWARE MATCHING PURSUIT, L +0.71 / R +0.72

Scott on v8-partial: musical structure audible but "still low pitched
and mushy, fading into something." Mush cause: bodies mixed at fixed
lag despite true lags varying ±40 samples (transient smear) + noise
regressors. v9 (kw_master_v9.py) replaces the one-shot ridge with
shift-aware matching pursuit: atoms = every body at shifts -64..64
step 16; greedy pick vs residual, LS refit on the selected set, K<=18
atoms per side. Full 1114-frame inventory (sweep complete).
METER: L +0.710 (86%>0.3, n=1351), R +0.723 (88%>0.3, n=1206) —
campaign trajectory v7 +0.48 -> v8 +0.58/0.65 -> v9 +0.71/0.72.
~50% of waveform variance now matches the reference. Short-fill sweep
still enriching (54 frames of shorts so far); rebuild when complete.

## Round 493 (07-16) — WIDTH VERDICT REFINED; PRE-REGION = THE HOLE; BACKEXT THROUGH SHORTS

Width selector: NO local flag possible — width flips across 41% of
ZERO-GAP boundaries (same rate in strong-only subset); no per-frame
rule (49% all-same ~ chance); adaptive-width grammars (len in
ceil(log2(remaining)) bits, 3 variants) DEAD at 6-9% exact-end vs the
labels. Min-bits (95%) stands. Best remaining explanations: label
noise on small bodies (both widths parse near-equally), or a per-frame
width bitmap in the unparsed header region.

Coverage map from full inventory (n=1097): tails mostly recovered
(19% tile to within 50 bits of wall; median tail 1261) but the
PRE-BODY REGION is the real hole: median 3935 bits unrecovered before
the first chained body — the backward chain only linked LONG bodies,
so one short body breaks the walk. r493_backext.py extends backward
through BOTH long and short bodies (long: minbits + corr/envelope
gates; short: placement corr >= 0.28). Validated: f480 walked back
from 1561 to bit 143 (near frame start), f240 gained 14 bodies at the
old cap. Full sweep running; kw_master_v10.py merges multibody +
shortfill + backext into the matching-pursuit render.

## Round 494 (07-16) — WIDTH BITMAP DEAD; MID-BAND METER; PRE-EMPHASIZED PURSUIT

Width-bitmap hypothesis KILLED with backext data: 76 frames now have
both the header window (bits 16..first_body, first<500) and 14+ known
body widths — searching every window offset for the width sequence
(both polarities): 0 exact hits, best-agreement 0.73 vs null 0.71.
No contiguous per-frame width bitmap in the header (or labels too
noisy). LFE-candidate tiny bodies found as early as @192 (f103, m=3).

NEW INSTRUMENT (meter_ab.py): band-resolved corr. v9 verdict:
broadband +0.694 (85%>0.3) but 1-4kHz MID BAND ONLY +0.104 (14%) —
the intelligibility band is nearly random; this IS the "garbliness."
Mid-band corr is now the primary quality target.

v10e (kw_master_v10e.py): PRE-EMPHASIZED matching pursuit — atom
selection and weight fitting in a whitened domain (y[n]=x[n]-0.95
x[n-1]) so mids compete with bass for atoms; reconstruction with raw
atoms. Building; A/B vs v9 on mid-band corr pending.

## Round 494c (07-16) — BAND-SPLIT PURSUIT WINS THE MID BAND

kw_master_v10f.py: pursuit split at 700 Hz — bass fit (8 atoms) and
mid/high fit (14 atoms) run independently on band-filtered atoms,
reconstructions summed. MID-BAND (1-4 kHz) METER TRAJECTORY:
  v9 (single broadband pursuit):   +0.104 (14%>0.3)
  v10e (pre-emphasized pursuit):   +0.176 (25%)
  v10f (band-split pursuit):       +0.258 (41%)   <- winner
Broadband essentially unchanged (+0.681 vs +0.694). Band-split is the
render default going forward. Interim rebuild with backext 415+/
shortfill 562+ inventories in flight for delivery.

## Round 494d (07-16) — V10G: RAISED ATOM BUDGETS, DOUBLE BEST

Diagnosis confirmed: adding backext bodies didn't move mids at K=14 ->
the pursuit was BUDGET-starved, not content-starved. v10g raises
budgets to 26 mid / 10 bass atoms:
  mid 1-4kHz: +0.290 (48%>0.3)   [v10f +0.259/41%, v9 +0.104/14%]
  broadband:  +0.700 (85%>0.3)   [v10f +0.683]
Both campaign bests. v10g = render default. Delivered.

## Round 495 (07-16) — 🔓 A-SPX GRAMMAR FITS THE FRAME TAILS (0/1500 null)

Census of the full merged tiling (multibody+backext): NO mid-frame
gaps >90 bits (bodies pack tight); the unexplained regions are the
pre-body run-up (median first body @766) and the TAIL (median 1261
bits after last body). Built ac4aspx.py = A-SPX bitstream parser
(Tables 51-58: aspx_framing int_class FIXFIX/FIXVAR/VARVAR/VARFIX,
delta_dir, hfgen_iwc_1ch/2ch tna/ah/fic/tic flags, ec_data/huff_data
F0/DF/DT chains) with band counts (nsb_hi, nsb_noise, env_bits_fixfix,
freq_res_mode, quant_mode) as free params since aspx_config/master
tables unlocated. EXACT-FIT TEST on frame tails, chain [2ch][2ch][1ch]:
15/25 frames consume >=85% of the tail (several land EXACTLY on wall),
NULL-fit rate 0/1500 (same test 700 bits earlier = inside body region).
First contact with REAL highband data (vs synthetic SBR fill). Common
fit params: nsb_hi 10-12, freq_res_mode 3, quant_mode 0. NEXT: lock
the band config per frame, extract real envelopes, replace SBR fill in
the render — the path to killing "video-call mush".

v10g final (full backext): broadband +0.709 (85%), mid +0.293 (49%).

## Round 496 (07-16) — 🔓🔓 A-SPX ENVELOPES DECODE TO SANE VALUES (REAL HIGHBAND DATA)

Locked config across tails: nsb_hi=12, freq_res_mode=3, quant_mode=0
dominant (29/60 tails fit >=90%). Envelope EC decode bug fixed: Huffman
returns codebook INDEX; the F0/DF/DT books are centered on their
shortest codeword (mode = zero-delta), so value = index - argmin(LEN).
Offsets: ENV F0 30, ENV DF/DT 69/70, NOISE F0 7, NOISE DF 29.
RESULT: post-offset envelope values now SANE — n=6228, central 90%
[-17,+15], mean 0.5, median 0 (was min3/max1213/median227 = garbage).
Tight symmetric distribution around 0 = real coded deltas, not noise.
FIRST REAL HIGHBAND DATA READ FROM THE STREAM. Remaining unknown to
render: the QMF master frequency table (maps the 12 high bands to Hz)
— aspx_config start_freq/stop_freq/master_freq_scale still unlocated;
sweep those next, then dequantize (3dB steps) and drive the highband
in place of SBR copy-up fill = v11.

## Round 497 (07-16) — A-SPX ENVELOPE SEMANTICS: NEGATIVE (retracts R496 overclaim)

Extracted per-frame 12-band signal envelopes from tail fits (141/1114
frames, r497_aspxenv.py). VALIDATION vs reference highband band-energy
shape (the decisive test R496 lacked):
  - decoded shape (natural order) vs ref: -0.53, SAME as wrong-frame
    null -0.53
  - reversed band order: +0.52 (74%>0.3) BUT reversed null ALSO +0.53
    => separation -0.007 = ZERO
The +0.52 is an artifact: every frame decodes to a near-identical
RISING ramp (0.9->1.3), which correlates ~+0.5 with any falling
highband regardless of frame. NO per-frame information.
CONCLUSION: the tail STRUCTURALLY parses as A-SPX (0/1500 null on bit
consumption stands) but the decoded envelope VALUES are NOT validated
as real highband data. Either the tail fits are consumption-
coincidental, or the value decode (offset/dequant/band-map/num_env) is
still wrong. R496's "first real highband data" was premature — the
frame-generic ramp is the tell. A-SPX render (v11) BLOCKED until values
decode frame-specifically. Honest reference-assisted highband (v7-v10
envelope fill) remains the render path. NEXT: locate real aspx_config
(start/stop/master_scale) for correct band count + dequant, OR accept
A-SPX as beyond reach without the QMF master tables.

## Round 498 (07-17) — aspx_config LOCATED AND READ; substream front anchored; LFE-first re-confirmed under CB15

THE CONFIG HUNT ENDED AT BIT 18. Spec archaeology (part1 Table 50 +
clauses 4.3.10.1.x + Pseudocode 67-70) gave the full aspx_config field
map and master-table derivation the R496/497 rounds lacked. Then the
substream front turned out to be deterministic:

- sub*.bin = ac4_substream: [audio_size_value 15][b_more 1][element...].
  VERIFIED: size15 == filelen minus metadata on every frame. All prior
  top-down parses that started at bit 0 were 16 bits off.
- bit 16-17 = 7_X codec_mode: reads ASPX (01) on ALL 1410 frames.
- iframes (fr%24==0, PROVEN by config-presence: the 15 config bits
  appear at 18-32 on exactly those 59 frames and the following field
  re-aligns with P-frame bit 18): aspx_config =
    quant_mode_env=1 (3dB), start_freq=7, stop_freq=1, master_scale=1
    => sbg_master=[40,42,44,47,50,53,56], 6 groups, A-SPX = 15-21 kHz;
    interpolation=1 preflat=1 limiter=1 noise_sbg=3 (n_noise=1),
    env_bits_fixfix=0 (num_env in {1,2}), freq_res_mode=2
    (duration-rule freq_res, Pseudocode 77; 16 timeslots).
  ONE config across all 59 iframes. R496/497 fits used nsb_hi=12 —
  structurally wrong band count, fully explaining the semantic null.
- ac4aspx2.py: EXACT A-SPX parser for this config (borders, tsg_ptr,
  mode-2 freq_res, FIXFIX/1-env qmode override, inter-frame
  previous_stop, per-block xover). Element tail per Table 33 ASPX mode:
  aspx_data_2ch, 2ch, 1ch, 2ch (R497 chained only 3 blocks - second
  structural error). Naive tail-window fits are still ambiguous
  (multi-fit, xover disagreement) => tail needs a forward anchor, not
  a search window.
- LFE-first RE-CONFIRMED: P-frame LFE at bit 18 = msfb=3 (constant
  across ~1300 frames), one cb=15 no-payload section with overshoot
  length (round-434 knobs!), ref_sf, snf gate. The LFE is coded
  SILENT throughout Radioactivity. The immersive_channel_element
  detour (mode/ACPL reading of the same bits) is dead: part2 Table 73
  3-bit mode code produced inconsistent iframe/P-frame modes; the
  2-bit 7_X reading is consistent everywhere.
- Even/odd metadata cadence discovered: every even frame carries ~85
  bytes of substream metadata, every odd frame ~4. Audio parse is
  unaffected (metadata sits after audio_size) but any tool that
  assumed iframe<->big-metadata is wrong: iframes are %24.
- THE REMAINING WALL (task #12): the field right after LFE end still
  histograms uniform across frames (not a sane coding_config). With
  the front now bit-exact to ~bit 39-52, the deviation window is
  narrow: suspects are per-body msfb (v2 abandons shared psy_info -
  consistent with 50k+ validated harvest bodies that all start with
  their own 5-bit msfb) and chparam/sap bitmap sizing. Next: bit-DP
  over the [LFE-end .. first-validated-body] window (458 frames have
  it under 200 bits) solving both hypotheses jointly.

Tools banked: ac4aspx2.py (exact parser), r498_aspxcfg.py,
r498_cfg7x.py (config extractors), r498_fit.py (tail fitter),
r498_topdown.py (position-match harness).

## Round 499 (07-17) — two honest negatives sharpen the gate

R499a (r499_hdrwin.py): header-window tiling test. Chaining v2 bodies
from LFE-end reaches the first validated harvest body in 87% of
frames — but a +13-bit mis-offset null reaches it in 82%. The v2 body
grammar is permissive enough that reachability carries ~no
information (reconfirms R476's 3.86-billion-chains result at the
frame front). Constrained field-by-field decode is required.

R499b (r499_aspxsem.py): A-SPX semantic test WITH the exact R498
parser (real 6-band 15-21kHz tables, 3dB quant, duration-rule
freq_res, 4-block tail). 714/1114 frames fit a window-searched tail.
Per-frame decoded signal-envelope level vs reference 15-21kHz
log-energy across 714 frames: corr -0.05; shift nulls -0.01..-0.07.
ZERO separation. Window-searched tail positions do not produce real
values even under the exact grammar.

CONSOLIDATED CONCLUSION: every remaining deterministic goal (A-SPX
values, exact core positions, Rust decoder port) is gated on ONE
thing — the post-LFE header window (~50-150 bits). Next instrument:
joint field-hypothesis enumeration scored across all 1410 frames
(a candidate grammar must explain every frame simultaneously;
cross-frame constancy of config-like fields + landing on validated
body positions are the score).

## Round 500 (07-17) — ENTROPY MAP: v2 P-frames have NO element structure after bit ~40

Three instruments, one decisive:
- r500_hdrenum.py: joint header-grammar enumeration (288 layouts x
  1067 P-frames, scored on cross-frame field sanity). NO winner —
  discriminators saturated (section-parse passes 99% of anything;
  msfb reads hit 1-runs).
- r500_body1.py: semantic body-1 scan at LFE_end+h, h=0..12 vs null
  h=24..36: FLAT 27% validation at every offset INCLUDING nulls.
  => r489-style per-body validation has a ~27% false-positive floor;
  the 54-bodies/frame inventory contains a large phantom fraction
  (the ridge-regression render survives by downweighting them).
- PER-BIT-POSITION ENTROPY MAP across 1351 P-frames (the keeper):
  absolute positions: structure ONLY in bits 16-40 (codec 16-17
  constant; 18-31 low/partial entropy; ~1.00 from bit 41 to the
  wall). LFE-END-ALIGNED: entropy 1.00 from E+0 for 120+ bits.
  CONCLUSIONS: (1) NO coding_config / shared sf_info / chparam /
  per-body fixed headers exist in v2 P-frames — the TS 103 190-1
  Table 33 element structure is ABSENT; (2) all fixed per-frame
  structure fits in ~24 bits (16-40); (3) everything after is one
  unbroken entropy-coded payload run.
  OPEN QUESTION for next round: is the bits-18-40 "silent LFE"
  reading real, or are those 24 structured bits a compact frame
  table-of-contents (with all payloads packed back-to-back after)?
  Iframe variant of the map (offset +15) should discriminate; also
  diff the 24-bit zone against frame-level knowns (frame size,
  body count, long/short mix) to find what it encodes.

## Rounds 501-506 (07-17) — THE FRAME DESCRIPTOR: v2's front decoded to the field level; payload coding still unknown

Six-iteration arc on the 24-bit structured zone:
- R501 bit<->feature correlations: bit22 correlates -0.61 with frame
  size; no clean integer field => variable-length coding.
- R501b prefix conditioning (the breakthrough instrument): grouping
  frames by bits21-25 yields FIVE modes with mode-dependent CONSTANT
  extensions. Payload start per mode (P-frames): 1111->bit 25,
  0111->33, 0110->37, 0011->39, 0101->42 (also rare A='000' variant).
- R502: the "LFE with payload" reading is REFUTED: the non-silent
  codebook groups decode to all-zero quant in 275/275 frames AND the
  bits where spectra should vary are constant across frames. The
  entire "silent LFE" reading of bits 18-40 is now suspect; what is
  certain is the structure, not its name.
- R503 modal readout: descriptor = [3b A in {011,000}][4b MODE]
  [mode-dependent constant extension 0-17 bits].
- R504 iframe cross-check (CONFIRMATION): iframe bits 33-40 have the
  IDENTICAL pattern distribution as P-frame bits 18-25 (config sits
  between codec_mode and descriptor). The descriptor is real,
  position-exact, and tabled.
- R505 mode-anchored semantic body scan: FLAT vs nulls — the payload
  at descriptor-end is NOT a v2 spectral body.
- R506 A-SPX-at-front probe: parse_tail fits at descriptor-end in 28%
  of frames vs 81% at +13-bit null — NOT an A-SPX chain either
  (anti-aligned, if anything).

STATE OF THE WALL: v2 P-frame = [codec_mode 2b][descriptor 9-26b,
fully tabled] + UNKNOWN entropy-coded payload layer that is neither
sf_data-shaped nor aspx-shaped at its start. Harvest-validated
spectral bodies live deeper in the frame (~bit 97+). Next candidate
instruments: (a) decode believer.mp4 (v0/v1, DECODABLE by the fork)
and diff its frame-front bit layout against Tidal v2 to identify the
inserted/changed layer; (b) autocorrelation/grammar-free segmentation
of the payload region to find its code's symbol statistics.

## Round 507 (07-17, overnight) — payload fingerprint: ASF-spectra-flavored; naive element recurrence negative

- Codebook compression fingerprint (grammar-free): decoding the
  dominant-mode payload (bit 25) under each of 60 AC-4 codebooks and
  comparing observed mean code length vs the tree's random-bits
  expectation. ASF spectral books win (cb9 +0.126, cb7/cb1/cb3/cb5
  +0.09..0.10); ASPX books actively anti-fit (-0.14). The payload
  region is ASF-spectra-flavored huffman. CAVEAT: the frame is
  mostly spectra everywhere, and 1-heavy bit bias inflates books
  with 1-run short codes — treat as directional, not positional.
- Recurrence test NEGATIVE: [12 lines under cb X][ref_sf][snf] then
  expect next '011' descriptor — all 11 codebooks at/below the 12.5%
  chance floor. The descriptor-chain structural guess is wrong.
- Believer diff blocked: believer.mp4 no longer on disk (only its
  decoded wav). Prior memory: R458 proved fork bit-exact on believer
  v0/v1; R465 REFUTED Table 42a for MID-FRAME gaps (bodies had no
  overshoot sections) — but the FRAME FRONT is full of overshoot
  sections, so an hsf-STYLE inline tail remains live for the front
  specifically. Official spec hsf lives in a separate 96/192kHz
  substream (Table 17), so any inline variant is v2-custom.
OVERNIGHT STATE: descriptor structure + payload offsets stand
(R501-506); payload coding still unidentified. Strongest next plays:
(1) re-obtain a v0/v1 AC-4 sample (believer) and diff frame fronts;
(2) instrument the fork's C walk on Tidal to log its OWN parse of
bits 16-100 per frame (walk vs dump divergence localizes the first
misread field); (3) position-free spectral matching: IMDCT candidate
decodes of the bit-25 region against the ref's LOW-band (the front
element should be an audible channel, not LFE).

## Round 508 (07-17, overnight) — walk corpus rediscovered; giant front body = fork desync artifact

bedgeo.log/bedfull.log (scratchpad, 51MB/1.1GB) = the fork C walk's
element-by-element trace on the kw file, 189 frames, WALL-MATCHED to
the dumps (f1 13720, f2 13304 — same file, same bit origin, positions
transferable; the R479 "not bit-identical" caveat needs re-scoping).
The walk parses a GIANT front body at in@40 (cb10 sections spanning
63-66 "sfbs" via the known stale-array overshoot bug, spec ~8000
bits). Replicating that decode on dump bits: correlation vs ref =
0.05 = null. VERDICT: the giant front body is the fork's own desync
artifact — it burns most of the frame through garbage right after
the descriptor zone; matches session-3's "both decoders desync in
the five-channel region."
NET OVERNIGHT: descriptor structure + payload offsets stand; payload
coding remains unidentified; the walk corpus is available for
field-level forensics but its front parse is not ground truth.

## Round 509 (07-17) — corrections and closures while v10i builds

- bedgeo/bedfull RE-CLASSIFIED: ~1072 BODY lines per frame and flat
  position alignment vs the validated harvest prove these are a
  prior round's brute-force SCAN corpus, not a single clean C walk.
  The R508 'walk corpus' framing is dead; no walk instrument exists
  on disk for Tidal.
- Iframe-only STRICT A-SPX tail hunt (all-4-xovers-equal, sane
  num_env, end within [wall-400, wall-4]): 6/47 unique fits, 41
  no-fit, xovers inconsistent (3 vs 1) across the six. Third
  independent confirmation: the tail cannot be window-searched; it
  needs the forward anchor (task #12).
- v10i render building: phantom purge (long: bc>=0.60 or |c|>=0.45
  or bc>=0.50&nz>=25; shorts: |c|>=0.40 or nz>=20) + pursuit atoms
  bass 10->12, mid 26->30. Rationale: R500 measured ~27% FP floor
  in the r489 accept test — cleaning the atom pool should help the
  mid band more than adding inventory did (v10h was flat vs v10g).

## Round 510 (07-17) — INSTRUMENTED DECODER REBUILT AND RUN ON THE DUMPS

Rebuilt the fork ffmpeg from scratch (ffmpeg 6.1.2 + community patch
+ fork ac4dec.c, host gcc 13.3 — the binary was lost). Ran via the
AC4_RAWSUB TOC-bypass on the wrapped kw4 dumps with CB15+OVERSHOOT:
- FRONT WALK CONFIRMS R501-506 FIELD MAP on the exact dump bits:
  codec_mode=1 (ASPX), LFE msfb=3 sections@36, coding_config=1
  (3+2), shared sf_info msfb=55 (near-full-band core). The "frame
  descriptor" was [LFE][coding_config][sf_info] all along — my
  python LFE-end computation was the misread, not the concept.
- BUT the walk's 8 bodies/frame occupy only bits ~33..4100 and reach
  aspx_data_2ch@~4128, while the SPECTRALLY VALIDATED harvest bodies
  (~25/frame) fill ~5000..13300. Zero position overlap. The walk's
  bodies decode to full-scale noise (8ch RMS ~20000, corr 0.06 =
  session-3 result reproduced deterministically).
- Decoder WEDGES (infinite loop) inside aspx escape handling after
  ~194 frames at trace level — crash-proofing gap, r510_walk log
  banked (194 frames of field-labeled front walk).
- TOC parse on the real mp4 is GARBAGE (presentations:0, substream
  sizes 332/0/2/704) — the v1 path misreads the v2 TOC. IMPLICATION:
  channel_mode 6 = "7.1" rests on this unreliable parse. The ~25
  validated bodies/frame EXACTLY matches 22_2_channel_element
  (2 mono + 11 pairs = 24 sf_datas + 11 aspx_2ch tails). NEXT: (a)
  fix the v2 TOC parse (or hand-parse the TOC bits properly) to
  settle channel_mode; (b) try 22_2 element grammar against the
  frame; (c) fix the aspx escape wedge and trace all 1410 frames.
- v10i phantom-purge render: NEGATIVE (full +0.704, mid +0.273 vs
  v10h +0.709/+0.298) — hard filtering removes real bodies; the
  ridge regression already handles phantoms better. v10h remains
  champion; purge kept 36140/48595 = 74%, matching the R500
  ~27% FP estimate almost exactly (independent confirmation of the
  FP-floor measurement, even though the filter hurt the render).

## Round 511 (07-17) — THE v2 TOC FALLS: hand-parsed bit-exact, channel_mode SETTLED

Extracted raw frames from the library mp4 (mdat @67701, frames
back-to-back; the mp4's SAMPLE TABLE is broken — ffprobe packets are
misaligned garbage, explaining years of TOC confusion). Anchored the
substream_index_table by brute solve (unique solution, bit 121 on
every frame), then closed the full field map via a cross-frame
entropy profile of the 121-bit TOC head (3000+ frames):

  [ver=2 (2)][seq (10)][b_wait=1, wait_frames, br_code][fs=48k]
  [frame_rate_index=13][b_iframe_global][b_single_presentation=1]
  [payload_base=0][b_program_id=0]
  presentation_v1: [single_group=1][presentation_version=2]
    [md_compat=0][pres_id=0][emdf ver/key=0]
    [EMDF_PROTECTION: 2b+2b -> 5 skip bytes = 40 high-entropy bits
     (the field my first parse missed = the desync)]
    [b_presentation_filter=0][group_index=0][b_pre_virtualized=0]
    [b_add_emdf=0][pres_substream_info: alt=0, ndot, index=0]
  group_info: [substreams_present=1][hsf=0][single_substream=1]
    [b_channel_coded=1][channel_mode = 0b1111001 = 7.1 (3/4/0.1)]
    [sf_mult=0][bitrate_info=0][B_AUDIO_NDOT (bit 111)]
    [substream_index=1][content_type: complete main]
  [n_substreams=2][sizes: pres_substream, audio_substream]

SETTLED: channel_mode = 7.1 CONFIRMED (22.2 hypothesis dead; 7_X
element stands). Substream 0 = 3-10 byte PRESENTATION SUBSTREAM
(sits between TOC and audio; iframes carry the 10-byte variant).
Audio sizes in the table match the kw4 dumps byte-exactly
(extraction validated end-to-end). b_audio_ndot (TOC bit 111) is
the TRUE per-frame iframe flag: matches %24 for all 1410 dump
frames, but 28/5933 frames later in the track DEVIATE — any
full-track work must read ndot from the TOC, not assume cadence.
Chain walk self-validates 5933 frames.
Tool: r511_toc.py. NEXT: parse the presentation substream (10B
iframe variant may hold per-presentation config); fix the fork's
v2 TOC path with this map (removes AC4_RAWSUB crutch); resume the
audio-element wall with ndot ground truth.

## Round 512 (07-17) — element-wall triangulation after the TOC win

- Presentation substream (R511's discovery) parsed by syntax review:
  it is dialnorm/DRC/custom-downmix/loudness metadata
  (6.2.2.3 tail) — valuable for playback polish, NOT a gate for the
  audio element. Deprioritized.
- All-long-frame reconciliation test NEGATIVE: 19 walk frames report
  all bodies long; their aspx-tail positions still scatter (gap to
  wall 1.1k..6.2k bits; one overshoots the wall). The fork's
  desync is NOT confined to short-transform under-read.
- Global tail-gap stats (72/194 walk frames that reach aspx):
  median gap 4999 bits, only 2/194 land in the plausible
  [200,900] fill window = chance. NO fully-reconciling frames
  exist under the fork's current v2 grammar.
CONCLUSION: the v2 deviation lives inside sf_data / sf_info layout
for ALL transform types (values AND lengths wrong), not in the
element ordering (front fields through coding_config are proven) and
not in the TOC (solved). The ~25 validated harvest bodies are best
explained as fragments of the 8 true sf_datas over-segmented by the
permissive v2 parse. NEXT INSTRUMENTS: (1) C-level field diff on
near-reconciling frames (f25/f56-class) — instrument sf_data stages
and compare against harvest fragment boundaries stage by stage;
(2) exploit b_audio_ndot ground truth + exact iframe aspx_config to
constrain the aspx tail from BEHIND the wall on reconciling-ish
frames; (3) revisit the sf_data grammar against believer (v0/v1)
C-walk once believer.mp4 is re-obtained — the sf_data DELTA between
v0/v1 (works) and v2 (fails) is the deviation, by construction.
