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
