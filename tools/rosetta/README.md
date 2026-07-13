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
