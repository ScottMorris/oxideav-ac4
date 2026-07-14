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
