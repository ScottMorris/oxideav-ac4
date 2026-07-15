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
