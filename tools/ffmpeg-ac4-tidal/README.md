# ffmpeg AC-4 decoder — Tidal/Atmos work tree (round 433)

Base: ffmpeg 6.1.2 + the community AC-4 patchset
(github.com/funnymanva/ffmpeg-with-ac4). This ac4dec.c adds:

- COMPLETE channel_element_7x per TS 103 190-1 Table 33
  (coding_config 0/1 groups, b_use_sap + additional pair,
  ASPX_ACPL_1 master tail, cc0/2 mono, 4 A-SPX trailers,
  ACPL data) — upstream had empty stubs for 7.1 content.
- Env-gated Tidal deviation knobs:
  AC4_MSFB5   — 5-bit max_sfb on long bodies (deviation: Dolby
                writes n_side_bits-width msfb everywhere;
                believer.mp4 errors drop 646 -> 127, frames
                decode end-to-end)
  AC4_W3_LONG — 3-bit section widths on long frames (helps
                without MSFB5, regresses with it — the two
                interact; open question)
- De-fused hard gates: object/A-JOC TOC stub (skip frame, not
  abort), sequence-counter cascade (debug, not reject), unknown
  substream type (skip by size), compute_window unknown length
  (error, not assert).

Status: believer.mp4 (Tidal-class 7.1, SIMPLE mode) decodes ~23
frames end-to-end with real multichannel audio (levels hot — SF
law pending). Tidal music files (ASPX mode) still fail per-frame:
need the war's A-SPX deviations ported into aspx_data_2ch/1ch
(mixed raw/huff F0, sticky xover slots, qmode override) + signed
SF law. Build: scratchpad/build_ffac4.sh.
