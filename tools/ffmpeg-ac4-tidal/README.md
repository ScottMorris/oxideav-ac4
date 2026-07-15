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
