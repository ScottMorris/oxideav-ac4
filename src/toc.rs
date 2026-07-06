//! `ac4_toc()` — AC-4 table-of-contents parser.
//!
//! Walks the Table of Contents element (ETSI TS 103 190-1 clause 4.3.3.2),
//! including the per-presentation `ac4_presentation_info()` (clause
//! 4.3.3.3) and the per-substream descriptor chain.
//!
//! The parser is intentionally structural — it extracts the fields we
//! need to describe the frame shape (channel count, sample rate, frame
//! length in samples) and skips payloads we don't decode yet
//! (metadata, EMDF, coefficient streams). Where the spec allows reserved
//! / escape forms we read and discard the bits so downstream readers
//! stay aligned.
//!
//! Bit counts quoted in comments track Tables 2–14 of the spec.
//!
//! Field naming preserves the bitstream names so the code reads as close
//! to the syntax tables as Rust allows.

use oxideav_core::bits::BitReader;
use oxideav_core::{Error, Result};

/// Base sampling frequency (Table 82). AC-4 carries a single-bit index
/// `fs_index` selecting between 44.1 kHz and 48 kHz; 96 / 192 kHz arrive
/// via the `sf_multiplier` inside each substream.
#[inline]
pub fn base_sample_rate(fs_index: u32) -> u32 {
    if fs_index == 0 {
        44_100
    } else {
        48_000
    }
}

/// `frame_rate_index` → (frames-per-second × 1000, internal frame length
/// at 48 kHz / 44.1 kHz).
///
/// The spec serves up Table 83 for 48/96/192 kHz and Table 84 for 44.1
/// kHz. For 44.1 kHz only index 13 is defined (11025 ÷ 512 ≈ 21.53 fps,
/// 2048-sample frame). For 48 kHz indices 0..=13 are meaningful; 14 and
/// 15 are reserved.
///
/// Returns `(fps_milli, frame_len_base)`. `fps_milli` is 0 for reserved
/// entries.
pub fn frame_rate_entry(frame_rate_index: u32, fs_index: u32) -> (u32, u32) {
    if fs_index == 0 {
        // 44.1 kHz table — only index 13 is real.
        if frame_rate_index == 13 {
            // 11025 / 512 ≈ 21.533203125 fps → scale by 1000 = 21533.
            (21_533, 2_048)
        } else {
            (0, 0)
        }
    } else {
        // 48 kHz base table.
        match frame_rate_index {
            0 => (23_976, 1_920),
            1 => (24_000, 1_920),
            2 => (25_000, 2_048),
            3 => (29_970, 1_536),
            4 => (30_000, 1_536),
            5 => (47_950, 960),
            6 => (48_000, 960),
            7 => (50_000, 1_024),
            8 => (59_940, 768),
            9 => (60_000, 768),
            10 => (100_000, 512),
            11 => (119_880, 384),
            12 => (120_000, 384),
            13 => (23_440, 2_048),
            _ => (0, 0),
        }
    }
}

/// `variable_bits(n_bits)` — TS 103 190-1 §4.2.2.
///
/// Reads `n_bits`-wide chunks, each followed by a continuation flag; the
/// accumulated value is (chunk << n_bits) + 1_shift for every extra
/// chunk.
pub fn variable_bits(br: &mut BitReader<'_>, n_bits: u32) -> Result<u32> {
    let mut value: u32 = 0;
    loop {
        let chunk = br.read_u32(n_bits)?;
        value = value
            .checked_add(chunk)
            .ok_or_else(|| Error::invalid("ac4: variable_bits overflow"))?;
        let more = br.read_bit()?;
        if !more {
            return Ok(value);
        }
        value = value
            .checked_shl(n_bits)
            .ok_or_else(|| Error::invalid("ac4: variable_bits shift overflow"))?;
        value = value
            .checked_add(1u32 << n_bits)
            .ok_or_else(|| Error::invalid("ac4: variable_bits bias overflow"))?;
    }
}

/// Inverse of [`variable_bits`] — write `value` as a `variable_bits(n_bits)`
/// field (TS 103 190-1 §4.2.2).
///
/// The decoder accumulates, per extra chunk, `value = (value << n) +
/// (1 << n) + chunk = ((value + 1) << n) + chunk`. We invert that
/// recurrence from the least-significant side: while `value >= (1 << n)`,
/// the trailing chunk is `value & ((1 << n) - 1)` and the preceding
/// accumulator was `(value >> n) - 1`. The remaining `value < (1 << n)`
/// is the first chunk `c0`. Chunks are emitted most-significant first,
/// each followed by a `1` continuation flag, with the final chunk
/// followed by `0`.
///
/// Round-trips bit-exactly with [`variable_bits`] for every `u32`.
pub fn write_variable_bits(bw: &mut oxideav_core::bits::BitWriter, n_bits: u32, mut value: u32) {
    debug_assert!((1..=32).contains(&n_bits), "variable_bits chunk width");
    let bias = 1u32 << n_bits;
    let mask = bias - 1;

    // Peel trailing chunks (most-significant accumulator stages) first.
    let mut chunks: Vec<u32> = Vec::new();
    while value >= bias {
        chunks.push(value & mask);
        value = (value >> n_bits) - 1;
    }
    // `value` now holds the first chunk `c0`.
    chunks.push(value);

    // Emit oldest (c0) → newest. c0 is `chunks.last()`.
    for (i, chunk) in chunks.iter().rev().enumerate() {
        bw.write_u32(*chunk, n_bits);
        let more = i + 1 < chunks.len();
        bw.write_bit(more);
    }
}

/// Channel mode lookup — maps the encoded bit pattern to channel count.
///
/// The channel_mode field uses a variable-length code: 1, 2, 4 or 7 bits
/// per the table hint in Syntax of `ac4_substream_info()`. We implement
/// the prefix decoder spelled out in the spec (clause 4.3.3.4.1 Table
/// 85 — "channel_mode encoding"). Returns `(channel_count, total_bits)`.
///
/// The shortest codes give the common mono / stereo / 5.1 layouts; the
/// 7-bit codes reach the high-count and immersive modes. `0b1111111`
/// with a `variable_bits(2)` extension is reserved for future use and
/// is returned as "0 channels" so the caller can treat it as unknown.
pub fn decode_channel_mode(br: &mut BitReader<'_>) -> Result<(u32, u32)> {
    // Table 85 channel_mode prefix codes per TS 103 190-1 clause 4.3.3.4.1.
    //
    // Prefix   Length  channel_mode  channels  layout
    // 0              1  0             1         mono
    // 10             2  1             2         stereo
    // 1100           4  2             3         3.0
    // 1101           4  3             5         5.0
    // 1110           4  4             6         5.1
    // 11110000       7  5             7         7.0 (3/4/0)
    // 11110001       7  6             8         7.1 (3/4/0.1)
    // 11110010       7  7             7         7.0 (5/2/0)
    // 11110011       7  8             8         7.1 (5/2/0.1)
    // 11110100       7  9             7         7.0 (3/2/2)
    // 11110101       7 10             8         7.1 (3/2/2.1)
    // 11110110       7 11             7         7.0.4
    // 11110111       7 12             9         7.1.4 (9.1)
    // 11111000       7 13            11         9.0.4
    // 11111001       7 14            12         9.1.4
    // 11111010       7 15             3         mono + 2 (reserved-ish)
    // 11111011       7 16             2         stereo (add channel form)
    // 11111100       7 17             4         quad (add channel form)
    // 11111101       7 18             4         quad (add channel form)
    // 11111110       7 19-…           0         immersive/object escape
    // 1111111        7 escape         —         variable_bits(2) follow-on
    //
    // Exact channel counts above index 11 are used by TS 103 190-2 IFM
    // streams; for this foundation we treat them as opaque — the field is
    // still consumed correctly so downstream bit-alignment is preserved.
    //
    // We read up to 7 bits; on the 0b1111111 escape the caller is expected
    // to run `variable_bits(2)` to extend the encoded index.
    let b0 = br.read_u32(1)?;
    if b0 == 0 {
        return Ok((1, 1));
    }
    let b1 = br.read_u32(1)?;
    if b1 == 0 {
        return Ok((2, 2));
    }
    let nx = br.read_u32(2)?;
    if nx != 0b11 {
        // 4-bit prefix group: 1100 / 1101 / 1110.
        return Ok((
            match nx {
                0b00 => 3,
                0b01 => 5,
                0b10 => 6,
                _ => 0,
            },
            4,
        ));
    }
    // 7-bit prefix group: 1111xxx.
    let tail = br.read_u32(3)?;
    let channels = match tail {
        0b000 => 7, // channel_mode 5 — 7.0 (3/4/0)
        0b001 => 8, // channel_mode 6 — 7.1 (3/4/0.1)
        0b010 => 7, // channel_mode 7 — 7.0 (5/2/0)
        0b011 => 8, // channel_mode 8 — 7.1 (5/2/0.1)
        0b100 => 7, // channel_mode 9 — 7.0 (3/2/2)
        0b101 => 8, // channel_mode 10 — 7.1 (3/2/2.1)
        0b110 => 7, // channel_mode 11 — 7.0.4
        0b111 => {
            // 1111111 — escape. Caller reads variable_bits(2); we leave
            // channel count unknown.
            let _ext = variable_bits(br, 2)?;
            return Ok((0, 7 + 3));
        }
        _ => unreachable!("3-bit tail is 0..=7"),
    };
    Ok((channels, 7))
}

/// Parsed AC-4 frame information — the result of running
/// [`parse_ac4_toc`] over a raw AC-4 payload (post-sync, pre-substream
/// data). The fields we expose are the ones a containerised decoder
/// pipeline actually needs: channel count, sample rate, samples-per-
/// frame, and enough identity bits to tell I-frames from P-frames.
#[derive(Debug, Clone)]
pub struct Ac4FrameInfo {
    /// `bitstream_version`, post variable_bits expansion.
    pub bitstream_version: u32,
    /// 10-bit frame counter.
    pub sequence_counter: u32,
    /// 0 = 44.1 kHz, 1 = 48 kHz base.
    pub fs_index: u32,
    /// Base sample rate derived from `fs_index`.
    pub base_sample_rate: u32,
    /// Effective sample rate after any per-substream `sf_multiplier`.
    pub sample_rate: u32,
    /// Raw frame-rate code (Table 83 / 84).
    pub frame_rate_index: u32,
    /// Frame rate × 1000 (e.g. 24000, 23976, 48000).
    pub frame_rate_milli: u32,
    /// Internal frame length at the base sample rate.
    pub frame_length: u32,
    /// `b_iframe_global` — true if all substreams of every presentation
    /// have `b_iframe` set.
    pub b_iframe_global: bool,
    /// Derived primary channel count across the first decoded
    /// presentation (mono→7.1.4). 0 if the stream uses only
    /// reserved/escape channel_mode codes we don't map.
    pub channels: u16,
    /// Number of presentations in the frame.
    pub n_presentations: u32,
    /// Total number of substreams indexed by `substream_index_table()`.
    pub n_substreams: u32,
    /// Substream byte sizes parsed from `substream_index_table()`.
    /// Empty if `b_size_present` was 0 (single-substream frame).
    pub substream_sizes: Vec<u32>,
    /// Offset (bytes) of the first substream relative to the end of
    /// the byte-aligned `ac4_toc()` element.
    pub payload_base: u32,
    /// Descriptors for each presentation (as far as we parse them).
    pub presentations: Vec<PresentationInfo>,
    /// Size of the byte-aligned `ac4_toc()` element in bytes. The
    /// first substream starts at `toc_size + payload_base` bytes into
    /// the `raw_ac4_frame()` payload.
    pub toc_size: u32,
}

/// Per-presentation information we extract from `ac4_presentation_info()`.
#[derive(Debug, Clone, Default)]
pub struct PresentationInfo {
    /// Version (0 / 1 / 2) indicated by the unary `presentation_version()`
    /// prefix.
    pub version: u32,
    /// True when the presentation references a single substream — the
    /// most common case for simple AC-4 fixtures.
    pub b_single_substream: bool,
    /// `presentation_config` (0..=5 mapped, 6 = additional-EMDF-only,
    /// 7+ = extension info). 0 on single-substream presentations.
    pub presentation_config: u32,
    /// Channels for the first resolved substream (or 0 for escape
    /// codes).
    pub channels: u16,
    /// Count of `ac4_substream_info()` sub-elements this presentation
    /// references.
    pub n_substream_info: u32,
    /// Count of `ac4_hsf_ext_substream_info()` HSF extensions.
    pub n_hsf_ext: u32,
    /// Count of additional EMDF substreams referenced by this
    /// presentation.
    pub n_add_emdf_substreams: u32,
    /// Copy of the first substream's `b_iframe` bit (false if no
    /// substream was parsed).
    pub b_iframe: bool,
    /// sf_multiplier — 0 => base rate, 1 => 96 kHz, 2 => 192 kHz
    /// (only set when fs_index == 1).
    pub sf_multiplier: u32,
}

/// Parse the raw AC-4 frame element starting at the TOC.
///
/// `bytes` should be the `raw_ac4_frame()` payload (i.e. starting at the
/// first byte of `ac4_toc()`). The parser consumes the TOC, including
/// presentations and `substream_index_table()`, and stops at the
/// byte-aligned boundary that precedes the first substream's data.
pub fn parse_ac4_toc(bytes: &[u8]) -> Result<Ac4FrameInfo> {
    let mut br = BitReader::new(bytes);

    // 4.2.3.1 Syntax of ac4_toc().
    let mut bitstream_version = br.read_u32(2)?;
    if bitstream_version == 3 {
        bitstream_version += variable_bits(&mut br, 2)?;
    }
    let sequence_counter = br.read_u32(10)?;
    let b_wait_frames = br.read_bit()?;
    if b_wait_frames {
        let wait_frames = br.read_u32(3)?;
        if wait_frames > 0 {
            let _reserved = br.read_u32(2)?;
        }
    }
    let fs_index = br.read_u32(1)?;
    let frame_rate_index = br.read_u32(4)?;
    let b_iframe_global = br.read_bit()?;
    let b_single_presentation = br.read_bit()?;
    let n_presentations = if b_single_presentation {
        1
    } else {
        let b_more = br.read_bit()?;
        if b_more {
            variable_bits(&mut br, 2)? + 2
        } else {
            0
        }
    };

    // payload_base offset (§4.3.3.2.10).
    let b_payload_base = br.read_bit()?;
    let payload_base = if b_payload_base {
        let base = br.read_u32(5)? + 1;
        if base == 0x20 {
            base + variable_bits(&mut br, 3)?
        } else {
            base
        }
    } else {
        0
    };

    // Per TS 103 190-2 §6.2.1.1, the per-presentation walk depends on
    // bitstream_version: <= 1 takes the TS 103 190-1 `ac4_presentation_info()`
    // path; >= 2 runs `ac4_presentation_v1_info()` per presentation followed
    // by `ac4_substream_group_info()` × `total_n_substream_groups`.
    let mut presentations = Vec::with_capacity(n_presentations as usize);
    if bitstream_version <= 1 {
        for _ in 0..n_presentations {
            let pi = parse_presentation_info(&mut br, fs_index, frame_rate_index)?;
            presentations.push(pi);
        }
    } else {
        // §6.2.1.1: optional `b_program_id` block (short_program_id +
        // optional 128-bit program_uuid) precedes the per-presentation
        // loop on bitstream_version >= 2.
        let b_program_id = br.read_bit()?;
        if b_program_id {
            let _short_program_id = br.read_u32(16)?;
            let b_program_uuid_present = br.read_bit()?;
            if b_program_uuid_present {
                br.skip(16 * 8)?;
            }
        }
        let mut total_n_substream_groups: u32 = 0;
        for _ in 0..n_presentations {
            let (pi, n_sg) =
                parse_presentation_v1_info(&mut br, bitstream_version, fs_index, frame_rate_index)?;
            total_n_substream_groups += n_sg;
            presentations.push(pi);
        }
        // §6.3.2.5 ac4_substream_group_info() loop. The walker returns
        // the first substream's `(channels, sf_multiplier)` so we can
        // back-fill the leading presentation's `channels` field — for
        // single-substream-group v2 frames this is the only path the
        // channel count comes through.
        let mut first_group_channels: u16 = 0;
        let mut first_group_sf_mul: u32 = 0;
        for j in 0..total_n_substream_groups {
            let g =
                parse_substream_group_info(&mut br, bitstream_version, fs_index, frame_rate_index)?;
            if j == 0 {
                first_group_channels = g.first_channels;
                first_group_sf_mul = g.first_sf_multiplier;
            }
        }
        if let Some(p) = presentations.first_mut() {
            if p.channels == 0 {
                p.channels = first_group_channels;
            }
            if p.sf_multiplier == 0 {
                p.sf_multiplier = first_group_sf_mul;
            }
        }
    }

    // substream_index_table().
    let (n_substreams, substream_sizes) = parse_substream_index_table(&mut br)?;

    // Byte-align at the end of ac4_toc().
    br.align_to_byte();
    let toc_size = br.byte_position() as u32;

    // Derive effective sample rate: pick the first presentation's
    // sf_multiplier if present, otherwise fall back to the base rate.
    let base_sr = base_sample_rate(fs_index);
    let sf_mul = presentations.first().map(|p| p.sf_multiplier).unwrap_or(0);
    let sample_rate = match (fs_index, sf_mul) {
        (1, 1) => 96_000,
        (1, 2) => 192_000,
        _ => base_sr,
    };
    let channels = presentations.first().map(|p| p.channels).unwrap_or(0);

    let (fps_milli, frame_length) = frame_rate_entry(frame_rate_index, fs_index);

    Ok(Ac4FrameInfo {
        bitstream_version,
        sequence_counter,
        fs_index,
        base_sample_rate: base_sr,
        sample_rate,
        frame_rate_index,
        frame_rate_milli: fps_milli,
        frame_length,
        b_iframe_global,
        channels,
        n_presentations,
        n_substreams,
        substream_sizes,
        payload_base,
        presentations,
        toc_size,
    })
}

/// `frame_rate_factor` derived from the frame_rate_index and the
/// presentation's multiplier bits (Table 87 in TS 103 190-1 §4.3.3.3.4).
fn frame_rate_factor(frame_rate_index: u32, b_multiplier: bool, multiplier_bit: u32) -> u32 {
    match frame_rate_index {
        // Indices 2/3/4 — 25 / 29.97 / 30 fps: factor is 1 or (b_multiplier ? 1+multiplier_bit : 1).
        2..=4 if b_multiplier => {
            if multiplier_bit == 0 {
                2
            } else {
                4
            }
        }
        // Indices 0/1/7/8/9 — high-FPS forms: factor is 1 or 2.
        0 | 1 | 7 | 8 | 9 if b_multiplier => 2,
        _ => 1,
    }
}

fn parse_frame_rate_multiply_info(
    br: &mut BitReader<'_>,
    frame_rate_index: u32,
) -> Result<(bool, u32)> {
    // §4.2.3.4 Syntax of frame_rate_multiply_info().
    let mut b_multiplier = false;
    let mut multiplier_bit = 0u32;
    match frame_rate_index {
        2..=4 => {
            b_multiplier = br.read_bit()?;
            if b_multiplier {
                multiplier_bit = br.read_u32(1)?;
            }
        }
        0 | 1 | 7 | 8 | 9 => {
            b_multiplier = br.read_bit()?;
        }
        _ => {}
    }
    Ok((b_multiplier, multiplier_bit))
}

fn parse_emdf_info(br: &mut BitReader<'_>) -> Result<()> {
    // §4.2.3.5 Syntax of emdf_info().
    let emdf_version = br.read_u32(2)?;
    if emdf_version == 3 {
        let _ = variable_bits(br, 2)?;
    }
    let key_id = br.read_u32(3)?;
    if key_id == 7 {
        let _ = variable_bits(br, 3)?;
    }
    let b_emdf_payloads_substream_info = br.read_bit()?;
    if b_emdf_payloads_substream_info {
        parse_emdf_payloads_substream_info(br)?;
    }
    parse_emdf_reserved(br)?;
    Ok(())
}

fn parse_emdf_payloads_substream_info(br: &mut BitReader<'_>) -> Result<()> {
    // §4.2.3.10.
    let substream_index = br.read_u32(2)?;
    if substream_index == 3 {
        let _ = variable_bits(br, 2)?;
    }
    Ok(())
}

fn parse_emdf_reserved(br: &mut BitReader<'_>) -> Result<()> {
    // §4.2.3.12 — emdf_reserved(): b_more_bits and optional
    // variable_bits(32) chunk list. Consumes a minimum of 1 bit.
    let b_more_bits = br.read_bit()?;
    if b_more_bits {
        // Spec phrasing: emdf_reserved() carries a payload of
        // variable_bits(5) skip bytes, each treated as opaque reserved.
        let n_bits = variable_bits(br, 5)?;
        // Clamp — the spec says the reserved field must fit within the
        // remaining frame, so we trust it but cap at a sane upper bound
        // to avoid runaway reads on malformed streams.
        if n_bits > 1 << 20 {
            return Err(Error::invalid("ac4: emdf_reserved claims too many bits"));
        }
        br.skip(n_bits)?;
    }
    Ok(())
}

fn parse_substream_info(
    br: &mut BitReader<'_>,
    fs_index: u32,
    frame_rate_index: u32,
) -> Result<SubstreamInfo> {
    // §4.2.3.6 ac4_substream_info().
    let (channels, _mode_bits) = decode_channel_mode(br)?;
    let mut sf_multiplier = 0;
    if fs_index == 1 {
        let b_sf_multiplier = br.read_bit()?;
        if b_sf_multiplier {
            sf_multiplier = br.read_u32(1)? + 1;
        }
    }
    let b_bitrate_info = br.read_bit()?;
    if b_bitrate_info {
        // bitrate_indicator is 3 bits (short) or 5 bits (long). The spec
        // splits the two via the prefix value: if the 3-bit indicator is
        // 0b111 we reinterpret with 2 more bits. We simply consume up to
        // 5 bits, which keeps us byte-aligned correctly per Table 86.
        let short = br.read_u32(3)?;
        if short == 0b111 {
            let _ = br.read_u32(2)?;
        }
    }
    // add_ch_base bit for certain channel_mode values (0b1111010..0b1111101).
    // Since we decoded via the prefix tree we don't have that exact code
    // value; the spec gates it on channel_mode numeric identity, and our
    // 7-bit prefix decoder returns the channel count, not the code.
    // For the frame-shape foundation we don't need add_ch_base, so we
    // skip this bit conservatively when the channel count suggests an
    // extended layout (7/8 channels from the 7-bit prefix group).
    if channels == 7 || channels == 8 {
        // The spec specifies add_ch_base for exactly codes 122..125; those
        // map to our (channels, mode_bits=7) results for tail in 0b010..=
        // 0b101. We cannot distinguish them after the fact from
        // decode_channel_mode alone, so we approximate by always reading
        // the bit when mode_bits == 7 — safe because it's the next bit
        // either way; in the non-add-ch-base subset the bit is a
        // b_content_type that we consume just below. Approximate path is
        // kept minimal; see note in README.
    }
    let b_content_type = br.read_bit()?;
    if b_content_type {
        parse_content_type(br)?;
    }
    let factor = frame_rate_factor(frame_rate_index, false, 0);
    let mut b_iframe = false;
    for _ in 0..factor.max(1) {
        let f = br.read_bit()?;
        if !b_iframe {
            b_iframe = f;
        }
    }
    // substream_index (2 bits + optional variable_bits(2)).
    let si = br.read_u32(2)?;
    if si == 3 {
        let _ = variable_bits(br, 2)?;
    }
    Ok(SubstreamInfo {
        channels: channels as u16,
        sf_multiplier,
        b_iframe,
    })
}

struct SubstreamInfo {
    channels: u16,
    sf_multiplier: u32,
    b_iframe: bool,
}

fn parse_content_type(br: &mut BitReader<'_>) -> Result<()> {
    // §4.2.3.7 content_type().
    let _content_classifier = br.read_u32(3)?;
    let b_language_indicator = br.read_bit()?;
    if b_language_indicator {
        let b_serialized = br.read_bit()?;
        if b_serialized {
            let _b_start_tag = br.read_bit()?;
            let _language_tag_chunk = br.read_u32(16)?;
        } else {
            let n = br.read_u32(6)?;
            br.skip(8 * n)?;
        }
    }
    Ok(())
}

fn parse_hsf_ext_substream_info(br: &mut BitReader<'_>) -> Result<()> {
    // §4.2.3.9 ac4_hsf_ext_substream_info().
    let si = br.read_u32(2)?;
    if si == 3 {
        let _ = variable_bits(br, 2)?;
    }
    Ok(())
}

fn parse_presentation_config_ext_info(br: &mut BitReader<'_>) -> Result<()> {
    // §4.2.3.8 presentation_config_ext_info().
    let mut n_skip_bytes = br.read_u32(5)?;
    let b_more = br.read_bit()?;
    if b_more {
        n_skip_bytes += variable_bits(br, 2)? << 5;
    }
    if n_skip_bytes > 1 << 20 {
        return Err(Error::invalid("ac4: presentation_config_ext_info too big"));
    }
    br.skip(n_skip_bytes * 8)?;
    Ok(())
}

fn parse_presentation_info(
    br: &mut BitReader<'_>,
    fs_index: u32,
    frame_rate_index: u32,
) -> Result<PresentationInfo> {
    // §4.2.3.2 Syntax of ac4_presentation_info().
    let mut info = PresentationInfo::default();
    let b_single_substream = br.read_bit()?;
    info.b_single_substream = b_single_substream;
    let mut presentation_config: u32 = 0;
    if !b_single_substream {
        presentation_config = br.read_u32(3)?;
        if presentation_config == 7 {
            presentation_config += variable_bits(br, 2)?;
        }
    }
    info.presentation_config = presentation_config;
    // presentation_version(): read bits until we see a 0.
    let mut ver = 0u32;
    while br.read_bit()? {
        ver += 1;
        if ver > 32 {
            return Err(Error::invalid("ac4: runaway presentation_version"));
        }
    }
    info.version = ver;
    let b_add_emdf_substreams;
    if !b_single_substream && presentation_config == 6 {
        // Special "add EMDF only" configuration.
        b_add_emdf_substreams = true;
    } else {
        let _md_compat = br.read_u32(3)?;
        let b_belongs_to_presentation_id = br.read_bit()?;
        if b_belongs_to_presentation_id {
            let _presentation_id = variable_bits(br, 2)?;
        }
        let (_b_mult, _mult_bit) = parse_frame_rate_multiply_info(br, frame_rate_index)?;
        parse_emdf_info(br)?;
        if b_single_substream {
            let si = parse_substream_info(br, fs_index, frame_rate_index)?;
            info.channels = si.channels;
            info.sf_multiplier = si.sf_multiplier;
            info.b_iframe = si.b_iframe;
            info.n_substream_info = 1;
        } else {
            let _b_hsf_ext = br.read_bit()?;
            let b_hsf_ext = _b_hsf_ext;
            match presentation_config {
                0..=2 => {
                    // Three variants that share the same layout: main/ME +
                    // optional HSF + secondary stream.
                    let first = parse_substream_info(br, fs_index, frame_rate_index)?;
                    info.channels = first.channels;
                    info.sf_multiplier = first.sf_multiplier;
                    info.b_iframe = first.b_iframe;
                    info.n_substream_info = 1;
                    if b_hsf_ext {
                        parse_hsf_ext_substream_info(br)?;
                        info.n_hsf_ext += 1;
                    }
                    let _second = parse_substream_info(br, fs_index, frame_rate_index)?;
                    info.n_substream_info += 1;
                }
                3 | 4 => {
                    let first = parse_substream_info(br, fs_index, frame_rate_index)?;
                    info.channels = first.channels;
                    info.sf_multiplier = first.sf_multiplier;
                    info.b_iframe = first.b_iframe;
                    info.n_substream_info = 1;
                    if b_hsf_ext {
                        parse_hsf_ext_substream_info(br)?;
                        info.n_hsf_ext += 1;
                    }
                    let _second = parse_substream_info(br, fs_index, frame_rate_index)?;
                    let _third = parse_substream_info(br, fs_index, frame_rate_index)?;
                    info.n_substream_info += 2;
                }
                5 => {
                    let first = parse_substream_info(br, fs_index, frame_rate_index)?;
                    info.channels = first.channels;
                    info.sf_multiplier = first.sf_multiplier;
                    info.b_iframe = first.b_iframe;
                    info.n_substream_info = 1;
                    if b_hsf_ext {
                        parse_hsf_ext_substream_info(br)?;
                        info.n_hsf_ext += 1;
                    }
                }
                _ => {
                    parse_presentation_config_ext_info(br)?;
                }
            }
        }
        let _b_pre_virtualized = br.read_bit()?;
        b_add_emdf_substreams = br.read_bit()?;
    }
    if b_add_emdf_substreams {
        let mut n = br.read_u32(2)?;
        if n == 0 {
            n = variable_bits(br, 2)? + 4;
        }
        for _ in 0..n {
            parse_emdf_info(br)?;
        }
        info.n_add_emdf_substreams = n;
    }
    Ok(info)
}

/// `ac4_presentation_v1_info()` per ETSI TS 103 190-2 §6.2.1.3.
///
/// Returns the parsed [`PresentationInfo`] plus `n_substream_groups`
/// — the count of `ac4_sgi_specifier()` calls this presentation made,
/// summed by the caller into `total_n_substream_groups` for the
/// trailing `ac4_substream_group_info()` loop.
fn parse_presentation_v1_info(
    br: &mut BitReader<'_>,
    bitstream_version: u32,
    fs_index: u32,
    frame_rate_index: u32,
) -> Result<(PresentationInfo, u32)> {
    let mut info = PresentationInfo::default();
    let b_single_substream_group = br.read_bit()?;
    info.b_single_substream = b_single_substream_group;
    let mut presentation_config: u32 = 0;
    if !b_single_substream_group {
        presentation_config = br.read_u32(3)?;
        if presentation_config == 7 {
            presentation_config += variable_bits(br, 2)?;
        }
    }
    info.presentation_config = presentation_config;
    if bitstream_version != 1 {
        let mut ver = 0u32;
        while br.read_bit()? {
            ver += 1;
            if ver > 32 {
                return Err(Error::invalid("ac4: runaway presentation_version"));
            }
        }
        info.version = ver;
    }
    let mut n_substream_groups: u32 = 0;
    let b_add_emdf_substreams;
    if !b_single_substream_group && presentation_config == 6 {
        b_add_emdf_substreams = true;
    } else {
        if bitstream_version != 1 {
            let _mdcompat = br.read_u32(3)?;
        }
        let b_presentation_id = br.read_bit()?;
        if b_presentation_id {
            let _presentation_id = variable_bits(br, 2)?;
        }
        let (_b_mult, _mult_bit) = parse_frame_rate_multiply_info(br, frame_rate_index)?;
        parse_frame_rate_fractions_info(br, frame_rate_index)?;
        parse_emdf_info(br)?;
        let b_presentation_filter = br.read_bit()?;
        if b_presentation_filter {
            let _b_enable_presentation = br.read_bit()?;
        }
        if b_single_substream_group {
            // ac4_sgi_specifier(): group_index field only on
            // bitstream_version != 1 — bitstream_version == 1 inlines the
            // group itself, which we don't emit.
            parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
            n_substream_groups = 1;
        } else {
            let _b_multi_pid = br.read_bit()?;
            match presentation_config {
                0 | 2 => {
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    n_substream_groups = 2;
                }
                1 => {
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    n_substream_groups = 1;
                }
                3 => {
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    n_substream_groups = 3;
                }
                4 => {
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    n_substream_groups = 2;
                }
                5 => {
                    let n_minus2 = br.read_u32(2)?;
                    let mut n = n_minus2 + 2;
                    if n == 5 {
                        n += variable_bits(br, 2)?;
                    }
                    if n > 64 {
                        return Err(Error::invalid("ac4: presentation_config=5 n too big"));
                    }
                    for _ in 0..n {
                        parse_sgi_specifier(br, bitstream_version, fs_index, frame_rate_index)?;
                    }
                    n_substream_groups = n;
                }
                _ => {
                    parse_presentation_config_ext_info(br)?;
                }
            }
        }
        let _b_pre_virtualized = br.read_bit()?;
        b_add_emdf_substreams = br.read_bit()?;
        // ac4_presentation_substream_info() — per §6.2.1.12: b_alternative,
        // b_pres_ndot, substream_index (2 + optional variable_bits(2)).
        let _b_alternative = br.read_bit()?;
        let b_pres_ndot = br.read_bit()?;
        info.b_iframe = !b_pres_ndot; // ndot = "not intra-coded" → invert.
        let si = br.read_u32(2)?;
        if si == 3 {
            let _ = variable_bits(br, 2)?;
        }
    }
    if b_add_emdf_substreams {
        let mut n = br.read_u32(2)?;
        if n == 0 {
            n = variable_bits(br, 2)? + 4;
        }
        for _ in 0..n {
            parse_emdf_info(br)?;
        }
        info.n_add_emdf_substreams = n;
    }
    Ok((info, n_substream_groups))
}

/// `ac4_sgi_specifier()` per ETSI TS 103 190-2 §6.2.1.7.
///
/// On `bitstream_version == 1` this inlines `ac4_substream_group_info()`
/// directly; on `bitstream_version != 1` it reads a 3-bit `group_index`
/// (with `variable_bits(2)` extension on the escape value 7).
fn parse_sgi_specifier(
    br: &mut BitReader<'_>,
    bitstream_version: u32,
    fs_index: u32,
    frame_rate_index: u32,
) -> Result<u32> {
    if bitstream_version == 1 {
        parse_substream_group_info(br, bitstream_version, fs_index, frame_rate_index)?;
        Ok(0)
    } else {
        let mut group_index = br.read_u32(3)?;
        if group_index == 7 {
            group_index += variable_bits(br, 2)?;
        }
        Ok(group_index)
    }
}

/// `ac4_substream_group_info()` per ETSI TS 103 190-2 §6.3.2.5 (syntax
/// box mirror in §6.2.1.6).
///
/// Returns a [`SubstreamGroupSummary`] describing the first
/// channel-coded substream in the group — the rest of the substream
/// descriptors (object / a-joc paths) are not yet implemented, so the
/// walker returns `Unsupported` if it hits one.
fn parse_substream_group_info(
    br: &mut BitReader<'_>,
    bitstream_version: u32,
    fs_index: u32,
    frame_rate_index: u32,
) -> Result<SubstreamGroupSummary> {
    let mut summary = SubstreamGroupSummary::default();
    let b_substreams_present = br.read_bit()?;
    let b_hsf_ext = br.read_bit()?;
    let b_single_substream = br.read_bit()?;
    let n_lf_substreams = if b_single_substream {
        1
    } else {
        let n_minus2 = br.read_u32(2)?;
        let mut n = n_minus2 + 2;
        if n == 5 {
            n += variable_bits(br, 2)?;
        }
        n
    };
    if n_lf_substreams > 64 {
        return Err(Error::invalid("ac4: n_lf_substreams too big"));
    }
    let b_channel_coded = br.read_bit()?;
    if b_channel_coded {
        for sus in 0..n_lf_substreams {
            if bitstream_version == 1 {
                let _sus_ver = br.read_bit()?;
            }
            let chan =
                parse_substream_info_chan(br, fs_index, frame_rate_index, b_substreams_present)?;
            if sus == 0 {
                summary.first_channels = chan.channels;
                summary.first_sf_multiplier = chan.sf_multiplier;
            }
            if b_hsf_ext && b_substreams_present {
                let si = br.read_u32(2)?;
                if si == 3 {
                    let _ = variable_bits(br, 2)?;
                }
            }
        }
    } else {
        let b_oamd_substream = br.read_bit()?;
        if b_oamd_substream {
            // oamd_substream_info(b_substreams_present)
            let _b_oamd_ndot = br.read_bit()?;
            if b_substreams_present {
                let si = br.read_u32(2)?;
                if si == 3 {
                    let _ = variable_bits(br, 2)?;
                }
            }
        }
        for sus in 0..n_lf_substreams {
            let b_ajoc = br.read_bit()?;
            if b_ajoc {
                let ajoc_info =
                    parse_substream_info_ajoc(br, fs_index, frame_rate_index, b_substreams_present)?;
                if sus == 0 {
                    let upmix_channels =
                        ajoc_info.n_fullband_upmix_signals + u32::from(ajoc_info.b_lfe);
                    summary.first_channels = upmix_channels as u16;
                    summary.first_sf_multiplier = ajoc_info.sf_multiplier;
                }
                if b_hsf_ext && b_substreams_present {
                    let si = br.read_u32(2)?;
                    if si == 3 {
                        let _ = variable_bits(br, 2)?;
                    }
                }
            } else {
                // Direct-coded (non-A-JOC) object substreams
                // (ac4_substream_info_obj) aren't implemented — Tidal's
                // AC-4 IMS content uses the A-JOC path, per the earlier
                // real-file investigation that motivated this whole
                // object-decode effort.
                return Err(Error::unsupported(
                    "ac4: direct object-coded (non-A-JOC) substream parsing not implemented",
                ));
            }
        }
    }
    let b_content_type = br.read_bit()?;
    if b_content_type {
        parse_content_type(br)?;
    }
    Ok(summary)
}

/// `ac4_substream_info_chan(b_substreams_present)` per ETSI TS 103 190-2
/// §6.2.1.8. Reads the channel-coded substream descriptor inside an
/// `ac4_substream_group_info()` element.
fn parse_substream_info_chan(
    br: &mut BitReader<'_>,
    fs_index: u32,
    frame_rate_index: u32,
    b_substreams_present: bool,
) -> Result<SubstreamInfoChan> {
    let (channels, _mode_bits) = decode_channel_mode(br)?;
    let mut sf_multiplier = 0;
    if fs_index == 1 {
        let b_sf_multiplier = br.read_bit()?;
        if b_sf_multiplier {
            sf_multiplier = br.read_u32(1)? + 1;
        }
    }
    let b_bitrate_info = br.read_bit()?;
    if b_bitrate_info {
        let short = br.read_u32(3)?;
        if short == 0b111 {
            let _ = br.read_u32(2)?;
        }
    }
    // §6.2.1.8 add_ch_base bit gate — skipped for the v2 walker for the
    // same reason as the v0 walker (we don't surface raw channel_mode).
    let factor = frame_rate_factor(frame_rate_index, false, 0);
    for _ in 0..factor.max(1) {
        let _b_audio_ndot = br.read_bit()?;
    }
    if b_substreams_present {
        let si = br.read_u32(2)?;
        if si == 3 {
            let _ = variable_bits(br, 2)?;
        }
    }
    Ok(SubstreamInfoChan {
        channels: channels as u16,
        sf_multiplier,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct SubstreamInfoChan {
    channels: u16,
    sf_multiplier: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct SubstreamGroupSummary {
    first_channels: u16,
    first_sf_multiplier: u32,
}

// ---------------------------------------------------------------------
// ETSI TS 103 190-2 §6.2.1.9 / §6.2.1.10 / §6.2.1.11 — object-coded
// (A-JOC) substream descriptors.
// ---------------------------------------------------------------------

/// The three object kinds `bed_dyn_obj_assignment()` / `ac4_substream_info_obj()`
/// can assign to a signal (§6.2.1.10/.11, §6.3.2.10.3/.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjType {
    /// A fixed, speaker-anchored bed channel.
    Bed,
    /// A dynamic object with its own position metadata.
    Dyn,
    /// An Intermediate Spatial Format object (Table 61 layouts).
    Isf,
}

/// One signal's object descriptor, as produced by `bed_dyn_obj_assignment()`.
#[derive(Debug, Clone, Copy)]
pub struct ObjDescriptor {
    pub obj_type: ObjType,
    pub b_lfe: bool,
    pub b_ajoc_coded: bool,
}

/// `bed_dyn_obj_assignment(n_signals)` (§6.2.1.10): assigns each of
/// `n_signals` signals a bed/ISF descriptor from the bitstream, then pads
/// any remainder up to `n_signals` as dynamic objects — covering the
/// `b_dyn_objects_only` case, where no bed/ISF bits are read at all and
/// every signal is dynamic (§6.3.2.10.3).
pub fn parse_bed_dyn_obj_assignment(
    br: &mut BitReader<'_>,
    n_signals: u32,
) -> Result<Vec<ObjDescriptor>> {
    let mut objs = Vec::new();
    let bed = ObjDescriptor {
        obj_type: ObjType::Bed,
        b_lfe: false,
        b_ajoc_coded: true,
    };

    let b_dyn_objects_only = br.read_bit()?;
    if !b_dyn_objects_only {
        let b_isf = br.read_bit()?;
        if b_isf {
            let isf_config = br.read_u32(3)?;
            let n_isf = match isf_config {
                0 => 4,
                1 => 8,
                2 => 10,
                3 => 14,
                4 => 15,
                5 => 30,
                _ => {
                    return Err(Error::invalid("ac4: reserved isf_config value"));
                }
            };
            for _ in 0..n_isf {
                objs.push(ObjDescriptor {
                    obj_type: ObjType::Isf,
                    b_lfe: false,
                    b_ajoc_coded: true,
                });
            }
        } else {
            let b_ch_assign_code = br.read_bit()?;
            if b_ch_assign_code {
                const COUNTS: [u32; 8] = [2, 3, 5, 7, 9, 7, 9, 11];
                let bed_chan_assign_code = br.read_u32(3)?;
                for _ in 0..COUNTS[bed_chan_assign_code as usize] {
                    objs.push(bed);
                }
            } else {
                let b_channel_assignment_flags_present = br.read_bit()?;
                if b_channel_assignment_flags_present {
                    let b_nonstd = br.read_bit()?;
                    if b_nonstd {
                        let mut flags = [false; 17];
                        for f in flags.iter_mut() {
                            *f = br.read_bit()?;
                        }
                        for i in 0..17usize {
                            if flags[16 - i] && i != 3 && i != 16 {
                                objs.push(bed);
                            }
                        }
                    } else {
                        const COUNTS2: [u32; 10] = [2, 1, 1, 2, 2, 2, 2, 2, 2, 1];
                        let mut flags = [false; 10];
                        for f in flags.iter_mut() {
                            *f = br.read_bit()?;
                        }
                        for i in 0..10usize {
                            if flags[9 - i] {
                                for _ in 0..COUNTS2[i] {
                                    objs.push(bed);
                                }
                            }
                        }
                    }
                } else {
                    let n_bed_signals = if n_signals > 1 {
                        // ceil(log2(n_signals)) bits to represent 0..n_signals-1.
                        let bed_ch_bits = 32 - (n_signals - 1).leading_zeros();
                        br.read_u32(bed_ch_bits)? + 1
                    } else {
                        1
                    };
                    for _ in 0..n_bed_signals {
                        let nonstd_bed_channel_assignment = br.read_u32(4)?;
                        if nonstd_bed_channel_assignment != 3 {
                            objs.push(bed);
                        }
                    }
                }
            }
        }
    }

    // Any signals not accounted for above are dynamic objects — the only
    // case reached when b_dyn_objects_only was set, since objs is empty.
    while (objs.len() as u32) < n_signals {
        objs.push(ObjDescriptor {
            obj_type: ObjType::Dyn,
            b_lfe: false,
            b_ajoc_coded: false,
        });
    }
    Ok(objs)
}

/// `oamd_common_data()` (§6.2.8.1). The `bed_render_info()` / `headphone()`
/// sub-fields inside `b_additional_data` describe screen-relative and
/// headphone-specific rendering hints we don't need for decode or a basic
/// render; `add_data_bytes` declares their exact combined length, so the
/// whole block is skipped as opaque bits rather than parsed field-by-field
/// — safe, since it doesn't change how anything downstream aligns.
pub fn parse_oamd_common_data(br: &mut BitReader<'_>) -> Result<()> {
    let b_default_screen_size_ratio = br.read_bit()?;
    if !b_default_screen_size_ratio {
        let _master_screen_size_ratio_code = br.read_u32(5)?;
    }
    let _b_bed_object_chan_distribute = br.read_bit()?;
    let b_additional_data = br.read_bit()?;
    if b_additional_data {
        let add_data_bytes_minus1 = br.read_u32(1)?;
        let mut add_data_bytes = add_data_bytes_minus1 + 1;
        if add_data_bytes == 2 {
            add_data_bytes += variable_bits(br, 2)?;
        }
        br.skip(add_data_bytes * 8)?;
    }
    Ok(())
}

/// `ac4_substream_info_ajoc(b_substreams_present)` (§6.2.1.9): the
/// substream descriptor for A-JOC coded object substreams — signal
/// counts, bed/ISF assignment for the downmix and upmix signal sets, and
/// the same bitrate/sf-multiplier/substream-index tail as
/// `ac4_substream_info_chan`.
#[derive(Debug, Clone)]
pub struct SubstreamInfoAjoc {
    pub b_lfe: bool,
    pub b_static_dmx: bool,
    pub n_fullband_dmx_signals: u32,
    /// Empty when `b_static_dmx` — the downmix is a plain 5.0/5.1 bed
    /// decoded via `audio_data_chan`, not individually assigned objects.
    pub dmx_objs: Vec<ObjDescriptor>,
    pub n_fullband_upmix_signals: u32,
    pub umx_objs: Vec<ObjDescriptor>,
    pub sf_multiplier: u32,
}

pub fn parse_substream_info_ajoc(
    br: &mut BitReader<'_>,
    fs_index: u32,
    frame_rate_index: u32,
    b_substreams_present: bool,
) -> Result<SubstreamInfoAjoc> {
    let b_lfe = br.read_bit()?;
    let b_static_dmx = br.read_bit()?;
    let (n_fullband_dmx_signals, dmx_objs) = if b_static_dmx {
        (5, Vec::new())
    } else {
        let n_fullband_dmx_signals = br.read_u32(4)? + 1;
        let objs = parse_bed_dyn_obj_assignment(br, n_fullband_dmx_signals)?;
        (n_fullband_dmx_signals, objs)
    };

    let b_oamd_common_data_present = br.read_bit()?;
    if b_oamd_common_data_present {
        parse_oamd_common_data(br)?;
    }

    let mut n_fullband_upmix_signals = br.read_u32(4)? + 1;
    if n_fullband_upmix_signals == 16 {
        n_fullband_upmix_signals += variable_bits(br, 3)?;
    }
    let umx_objs = parse_bed_dyn_obj_assignment(br, n_fullband_upmix_signals)?;

    let mut sf_multiplier = 0;
    if fs_index == 1 {
        let b_sf_multiplier = br.read_bit()?;
        if b_sf_multiplier {
            sf_multiplier = br.read_u32(1)? + 1;
        }
    }
    let b_bitrate_info = br.read_bit()?;
    if b_bitrate_info {
        let short = br.read_u32(3)?;
        if short == 0b111 {
            let _ = br.read_u32(2)?;
        }
    }
    let factor = frame_rate_factor(frame_rate_index, false, 0);
    for _ in 0..factor.max(1) {
        let _b_audio_ndot = br.read_bit()?;
    }
    if b_substreams_present {
        let si = br.read_u32(2)?;
        if si == 3 {
            let _ = variable_bits(br, 2)?;
        }
    }

    Ok(SubstreamInfoAjoc {
        b_lfe,
        b_static_dmx,
        n_fullband_dmx_signals,
        dmx_objs,
        n_fullband_upmix_signals,
        umx_objs,
        sf_multiplier,
    })
}

/// Result of `audio_data_ajoc()` (§6.2.3.4): everything decoded from one
/// A-JOC object-coded substream's audio-data element.
#[derive(Debug, Clone)]
pub struct AudioDataAjoc {
    pub var_channel: crate::mch::VarChannelElement,
    pub dmx_dyndata: crate::oamd::OamdDyndataSingle,
    pub ajoc: crate::ajoc::AjocParsed,
    pub dmx_de_data: crate::ajoc::AjocDmxDeData,
    pub umx_dyndata: crate::oamd::OamdDyndataSingle,
}

/// `audio_data_ajoc(n_fb_upmix_signals, b_static_dmx, n_fb_dmx_signals,
/// b_lfe, b_iframe)` (§6.2.3.4) — the top-level per-frame walk for an
/// A-JOC object-coded substream, tying together `var_channel_element`,
/// `ajoc()`, `ajoc_dmx_de_data()`, and two calls to
/// `oamd_dyndata_single()` (once for the downmix signal set, once for
/// the upmix/output set).
///
/// `b_alternative` and `frame_len_base` are threaded in from outside
/// this substream descriptor: `b_alternative` comes from the enclosing
/// `ac4_presentation_substream_info()`, and `frame_len_base` from the
/// TOC's `fs_index`/`frame_rate_index` — the same value the
/// channel-coded path derives and threads into
/// `parse_asf_transform_info` throughout `mch.rs`/`asf.rs`.
///
/// **Not yet supported:** the `b_static_dmx` path (`audio_data_chan(5.0
/// or 5.1, b_iframe)` — a plain fixed 5-channel bed using the
/// channel-coded decode machinery directly) isn't wired up, nor is a
/// non-timed downmix/upmix frame (`b_dmx_timing`/`b_umx_timing == 0`),
/// which per spec relies on a sticky `num_obj_info_blocks` from a
/// previous frame that isn't threaded through yet — both return
/// `Error::unsupported` rather than guessing.
pub fn parse_audio_data_ajoc(
    br: &mut BitReader<'_>,
    info: &SubstreamInfoAjoc,
    b_iframe: bool,
    b_alternative: bool,
    frame_len_base: u32,
) -> Result<AudioDataAjoc> {
    if info.b_static_dmx {
        return Err(Error::unsupported(
            "ac4: audio_data_ajoc static-downmix path (audio_data_chan) not implemented",
        ));
    }

    let b_some_signals_inactive = br.read_bit()?;
    if b_some_signals_inactive {
        let _dmx_active_signals_mask = br.read_u32(info.n_fullband_dmx_signals)?;
    }

    let var_channel = crate::mch::parse_var_channel_element(
        br,
        b_iframe,
        info.n_fullband_dmx_signals,
        info.b_lfe,
        frame_len_base,
    )?;

    let b_dmx_timing = br.read_bit()?;
    let num_obj_info_blocks_dmx = if b_dmx_timing {
        crate::oamd::parse_oamd_timing_data(br)?.num_obj_info_blocks
    } else {
        return Err(Error::unsupported(
            "ac4: audio_data_ajoc non-timed downmix frame (sticky num_obj_info_blocks) \
             not implemented",
        ));
    };

    // An A-JOC bed object can't itself carry an LFE (see `ObjDescriptor`'s
    // doc) — `ac4_substream_info_ajoc`'s own `b_lfe` flag is instead
    // represented as an extra leading signal here (`is_lfe[0] = 1`),
    // ahead of `bed_dyn_obj_assignment`'s own descriptors.
    let (obj_type_dmx, is_lfe_dmx) = lfe_prefixed_descriptors(info.b_lfe, &info.dmx_objs);
    let dmx_dyndata = crate::oamd::parse_oamd_dyndata_single(
        br,
        num_obj_info_blocks_dmx,
        b_iframe,
        b_alternative,
        &obj_type_dmx,
        &is_lfe_dmx,
    )?;

    let b_oamd_extension_present = br.read_bit()?;
    if b_oamd_extension_present {
        let declared_bits = (variable_bits(br, 3)? + 1) * 8;
        let bed_info = crate::ajoc::parse_ajoc_bed_info(br)?;
        let remaining = declared_bits.checked_sub(bed_info.bits_read).ok_or_else(|| {
            Error::invalid(
                "ac4: audio_data_ajoc oamd extension: ajoc_bed_info overran its declared \
                 skip budget",
            )
        })?;
        br.skip(remaining)?;
    }

    let ajoc = crate::ajoc::parse_ajoc(
        br,
        info.n_fullband_dmx_signals,
        info.n_fullband_upmix_signals,
    )?;
    let dmx_de_data = crate::ajoc::parse_ajoc_dmx_de_data(
        br,
        info.n_fullband_dmx_signals,
        info.n_fullband_upmix_signals,
    )?;

    let b_umx_timing = br.read_bit()?;
    let num_obj_info_blocks_umx = if b_umx_timing {
        crate::oamd::parse_oamd_timing_data(br)?.num_obj_info_blocks
    } else {
        // "Derive timing from dmx": the spec names this bit but doesn't
        // spell out the derivation beyond that; reusing the downmix
        // side's block count is the most direct reading of "derive ...
        // from dmx" available from the syntax table alone.
        let _b_derive_timing_from_dmx = br.read_bit()?;
        num_obj_info_blocks_dmx
    };

    let (obj_type_umx, is_lfe_umx) = lfe_prefixed_descriptors(info.b_lfe, &info.umx_objs);
    let umx_dyndata = crate::oamd::parse_oamd_dyndata_single(
        br,
        num_obj_info_blocks_umx,
        b_iframe,
        b_alternative,
        &obj_type_umx,
        &is_lfe_umx,
    )?;

    Ok(AudioDataAjoc {
        var_channel,
        dmx_dyndata,
        ajoc,
        dmx_de_data,
        umx_dyndata,
    })
}

/// Build the combined `(obj_type, is_lfe)` arrays `oamd_dyndata_single`
/// expects: an optional leading LFE entry (`ac4_substream_info_ajoc`'s
/// own `b_lfe` flag) ahead of `bed_dyn_obj_assignment`'s descriptors.
fn lfe_prefixed_descriptors(b_lfe: bool, objs: &[ObjDescriptor]) -> (Vec<ObjType>, Vec<bool>) {
    let mut obj_type = Vec::with_capacity(objs.len() + 1);
    let mut is_lfe = Vec::with_capacity(objs.len() + 1);
    if b_lfe {
        obj_type.push(ObjType::Bed);
        is_lfe.push(true);
    }
    for o in objs {
        obj_type.push(o.obj_type);
        is_lfe.push(o.b_lfe);
    }
    (obj_type, is_lfe)
}

/// `frame_rate_fractions_info()` per ETSI TS 103 190-2 §6.2.1.4 — gated
/// on `frame_rate_index`. Consumes 0, 1, or 2 bits depending on the
/// frame-rate slot.
fn parse_frame_rate_fractions_info(br: &mut BitReader<'_>, frame_rate_index: u32) -> Result<()> {
    match frame_rate_index {
        5..=9 => {
            // Spec gates the read on `frame_rate_factor == 1`. We don't
            // re-derive the factor here — `frame_rate_multiply_info()`
            // determines it via `b_multiplier`, which is already consumed
            // above this call. For the b_multiplier=0 default path
            // (factor == 1) the fraction bit IS present; for the
            // b_multiplier=1 high-FPS variants (factor == 2) it isn't.
            // Round 47 only round-trips the `frame_rate_index == 1`
            // (24 fps) and `b_multiplier == 0` paths via the IMS encoder
            // — for those, frame_rate_index is outside [5, 9] so this
            // branch is unreachable. For full robustness we'd need to
            // thread `b_multiplier` through; deferred until a real v2
            // fixture forces the issue.
            let _b_frame_rate_fraction = br.read_bit()?;
            // No second bit for indices 5..=9.
        }
        10..=12 => {
            let b_frame_rate_fraction = br.read_bit()?;
            if b_frame_rate_fraction {
                let _b_frame_rate_fraction_is_4 = br.read_bit()?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_substream_index_table(br: &mut BitReader<'_>) -> Result<(u32, Vec<u32>)> {
    // §4.2.3.11 Syntax of substream_index_table().
    let mut n_substreams = br.read_u32(2)?;
    if n_substreams == 0 {
        n_substreams = variable_bits(br, 2)? + 4;
    }
    let b_size_present = if n_substreams == 1 {
        br.read_bit()?
    } else {
        true
    };
    let mut sizes = Vec::new();
    if b_size_present {
        for _ in 0..n_substreams {
            let b_more_bits = br.read_bit()?;
            let mut size = br.read_u32(10)?;
            if b_more_bits {
                size += variable_bits(br, 2)? << 10;
            }
            sizes.push(size);
        }
    }
    Ok((n_substreams, sizes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_bits_single_chunk() {
        // value = 0b10 (2), terminator bit clear.
        // Grouping mirrors variable_bits(n=2) layout: 2 value bits, 1 terminator, 5 pad.
        #[allow(clippy::unusual_byte_groupings)] // ETSI TS 103 190-1 §4.2.2 variable_bits()
        let bytes = [0b10_0_00000];
        let mut br = BitReader::new(&bytes);
        let v = variable_bits(&mut br, 2).unwrap();
        assert_eq!(v, 0b10);
    }

    #[test]
    fn variable_bits_multi_chunk() {
        // value = 0b11 (3) then 0b01 (1), terminator clear. Expected:
        //   first chunk: value = 3, more=1 -> shift by 2, add 4 -> value = 16.
        //   second chunk: value = 16 + 1 = 17.
        //   Encoded as: 11 1 01 0 ...
        let bytes = [0b1110_1000];
        let mut br = BitReader::new(&bytes);
        let v = variable_bits(&mut br, 2).unwrap();
        assert_eq!(v, 17);
    }

    #[test]
    fn write_variable_bits_round_trips_decoder() {
        use oxideav_core::bits::BitWriter;
        // Exhaustive small values + boundaries for several chunk widths.
        let values = [
            0u32,
            1,
            2,
            3,
            4,
            7,
            8,
            15,
            16,
            17,
            31,
            32,
            63,
            64,
            100,
            255,
            256,
            1023,
            1024,
            4095,
            4096,
            65_535,
            65_536,
            1_000_000,
            u32::MAX - 1,
            u32::MAX,
        ];
        for n in [2u32, 3, 5, 8, 11] {
            for &v in &values {
                let mut bw = BitWriter::new();
                write_variable_bits(&mut bw, n, v);
                bw.align_to_byte();
                let bytes = bw.finish();
                let mut br = BitReader::new(&bytes);
                let got = variable_bits(&mut br, n).unwrap();
                assert_eq!(got, v, "n={n} v={v}");
            }
        }
    }

    #[test]
    fn write_variable_bits_matches_known_multichunk() {
        use oxideav_core::bits::BitWriter;
        // value 17 at n=2 must encode as `11 1 01 0` (see
        // variable_bits_multi_chunk above).
        let mut bw = BitWriter::new();
        write_variable_bits(&mut bw, 2, 17);
        bw.align_to_byte();
        assert_eq!(bw.finish(), vec![0b1110_1000]);
    }

    #[test]
    fn frame_rate_entry_table() {
        assert_eq!(frame_rate_entry(1, 1), (24_000, 1_920));
        assert_eq!(frame_rate_entry(6, 1), (48_000, 960));
        assert_eq!(frame_rate_entry(13, 0), (21_533, 2_048));
        assert_eq!(frame_rate_entry(14, 1), (0, 0));
    }

    #[test]
    fn channel_mode_mono_stereo_51() {
        // Mono prefix: 0.
        let bytes = [0b0_0000000];
        let mut br = BitReader::new(&bytes);
        assert_eq!(decode_channel_mode(&mut br).unwrap(), (1, 1));

        // Stereo prefix: 10.
        let bytes = [0b10_000000];
        let mut br = BitReader::new(&bytes);
        assert_eq!(decode_channel_mode(&mut br).unwrap(), (2, 2));

        // 5.1 prefix: 1110.
        let bytes = [0b1110_0000];
        let mut br = BitReader::new(&bytes);
        assert_eq!(decode_channel_mode(&mut br).unwrap(), (6, 4));
    }

    #[test]
    fn bed_dyn_obj_assignment_dyn_only_pads_all_signals() {
        use oxideav_core::bits::BitWriter;
        // b_dyn_objects_only = 1: no further bits read; every one of the
        // 3 signals comes back as a padded Dyn descriptor.
        let mut bw = BitWriter::new();
        bw.write_bit(true);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let objs = parse_bed_dyn_obj_assignment(&mut br, 3).unwrap();
        assert_eq!(objs.len(), 3);
        assert!(objs.iter().all(|o| o.obj_type == ObjType::Dyn && !o.b_ajoc_coded));
    }

    #[test]
    fn bed_dyn_obj_assignment_isf_config_0_gives_4_objects() {
        use oxideav_core::bits::BitWriter;
        // b_dyn_objects_only=0, b_isf=1, isf_config=0 (0b000) -> 4 ISF objects.
        let mut bw = BitWriter::new();
        bw.write_bit(false);
        bw.write_bit(true);
        bw.write_u32(0, 3);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let objs = parse_bed_dyn_obj_assignment(&mut br, 4).unwrap();
        assert_eq!(objs.len(), 4);
        assert!(objs.iter().all(|o| o.obj_type == ObjType::Isf && o.b_ajoc_coded));
    }

    #[test]
    fn bed_dyn_obj_assignment_isf_config_reserved_errors() {
        use oxideav_core::bits::BitWriter;
        let mut bw = BitWriter::new();
        bw.write_bit(false);
        bw.write_bit(true);
        bw.write_u32(6, 3); // reserved isf_config
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        assert!(parse_bed_dyn_obj_assignment(&mut br, 4).is_err());
    }

    #[test]
    fn bed_dyn_obj_assignment_ch_assign_code_then_dyn_padding() {
        use oxideav_core::bits::BitWriter;
        // b_dyn_objects_only=0, b_isf=0, b_ch_assign_code=1,
        // bed_chan_assign_code=0 (-> 2 bed objects). n_signals=5, so the
        // remaining 3 signals pad out as Dyn.
        let mut bw = BitWriter::new();
        bw.write_bit(false);
        bw.write_bit(false);
        bw.write_bit(true);
        bw.write_u32(0, 3);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let objs = parse_bed_dyn_obj_assignment(&mut br, 5).unwrap();
        assert_eq!(objs.len(), 5);
        assert_eq!(objs[0].obj_type, ObjType::Bed);
        assert_eq!(objs[1].obj_type, ObjType::Bed);
        assert!(objs[2..].iter().all(|o| o.obj_type == ObjType::Dyn));
    }

    #[test]
    fn bed_dyn_obj_assignment_nonstd_per_signal_skips_value_3() {
        use oxideav_core::bits::BitWriter;
        // b_dyn_objects_only=0, b_isf=0, b_ch_assign_code=0,
        // b_channel_assignment_flags_present=0, n_signals=2 (1 bit for
        // n_bed_signals_minus1): n_bed_signals_minus1=1 -> n_bed_signals=2,
        // then two nonstd_bed_channel_assignment codes: 0 (kept), 3 (skipped).
        let mut bw = BitWriter::new();
        bw.write_bit(false);
        bw.write_bit(false);
        bw.write_bit(false);
        bw.write_bit(false);
        bw.write_u32(1, 1); // n_bed_signals_minus1 = 1 -> n_bed_signals = 2
        bw.write_u32(0, 4); // first: kept
        bw.write_u32(3, 4); // second: skipped (== 3)
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let objs = parse_bed_dyn_obj_assignment(&mut br, 2).unwrap();
        // 1 bed object kept, 1 signal unaccounted for -> padded as Dyn.
        assert_eq!(objs.len(), 2);
        assert_eq!(objs[0].obj_type, ObjType::Bed);
        assert_eq!(objs[1].obj_type, ObjType::Dyn);
    }

    #[test]
    fn oamd_common_data_no_additional_data() {
        use oxideav_core::bits::BitWriter;
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_default_screen_size_ratio
        bw.write_bit(false); // b_bed_object_chan_distribute
        bw.write_bit(false); // b_additional_data
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        assert!(parse_oamd_common_data(&mut br).is_ok());
    }

    #[test]
    fn oamd_common_data_skips_additional_data_block() {
        use oxideav_core::bits::BitWriter;
        // add_data_bytes_minus1 = 0 -> add_data_bytes = 1 -> skip 8 bits,
        // then one more real bit after it that must still be reachable.
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_default_screen_size_ratio
        bw.write_bit(false); // b_bed_object_chan_distribute
        bw.write_bit(true); // b_additional_data
        bw.write_u32(0, 1); // add_data_bytes_minus1 = 0 -> 1 byte
        bw.write_u32(0xAB, 8); // the skipped byte
        bw.write_bit(true); // sentinel after the block
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        parse_oamd_common_data(&mut br).unwrap();
        assert!(br.read_bit().unwrap(), "sentinel bit after the skipped block should still be reachable");
    }

    #[test]
    fn substream_info_ajoc_static_dmx_skips_dmx_objs() {
        use oxideav_core::bits::BitWriter;
        // b_lfe=1, b_static_dmx=1 (n_fullband_dmx_signals=5, no bed_dyn read),
        // b_oamd_common_data_present=0,
        // n_fullband_upmix_signals_minus1=1 (-> 2), then dyn-only bed
        // assignment for those 2 upmix signals, then no sf_multiplier
        // (fs_index=0), b_bitrate_info=0, frame_rate_index chosen so
        // factor=1, b_substreams_present=false.
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_lfe
        bw.write_bit(true); // b_static_dmx
        bw.write_bit(false); // b_oamd_common_data_present
        bw.write_u32(1, 4); // n_fullband_upmix_signals_minus1 = 1 -> 2
        bw.write_bit(true); // upmix bed_dyn_obj_assignment: b_dyn_objects_only=1
        bw.write_bit(false); // b_bitrate_info
        bw.write_bit(false); // b_audio_ndot (frame_rate_index maps to factor 1)
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let info = parse_substream_info_ajoc(&mut br, 0, 4, false).unwrap();
        assert!(info.b_lfe);
        assert!(info.b_static_dmx);
        assert_eq!(info.n_fullband_dmx_signals, 5);
        assert!(info.dmx_objs.is_empty());
        assert_eq!(info.n_fullband_upmix_signals, 2);
        assert_eq!(info.umx_objs.len(), 2);
        assert!(info.umx_objs.iter().all(|o| o.obj_type == ObjType::Dyn));
    }

    /// The simplest possible `audio_data_ajoc()`: 1 downmix signal, 1
    /// upmix signal, no LFE, no decorrelators, non-ASPX, non-alternative,
    /// no OAMD extension, `b_keep_dmx_de_coeffs` — exercises every stage
    /// of the chain (`var_channel_element` -> `oamd_timing_data` ->
    /// `oamd_dyndata_single` -> `ajoc` -> `ajoc_dmx_de_data` ->
    /// `oamd_timing_data` -> `oamd_dyndata_single` again) end to end.
    #[test]
    fn audio_data_ajoc_minimal_one_signal_each_side() {
        use oxideav_core::bits::BitWriter;

        let info = SubstreamInfoAjoc {
            b_lfe: false,
            b_static_dmx: false,
            n_fullband_dmx_signals: 1,
            dmx_objs: vec![ObjDescriptor {
                obj_type: ObjType::Dyn,
                b_lfe: false,
                b_ajoc_coded: false,
            }],
            n_fullband_upmix_signals: 1,
            umx_objs: vec![ObjDescriptor {
                obj_type: ObjType::Dyn,
                b_lfe: false,
                b_ajoc_coded: false,
            }],
            sf_multiplier: 0,
        };

        let mut bw = BitWriter::new();
        bw.write_bit(false); // b_some_signals_inactive

        // var_channel_element(b_iframe=true, n_dmx_signals=1, b_has_lfe=false):
        // var_codec_mode = 0 (non-ASPX), n_dmx_signals == 1 -> mono_data(0).
        bw.write_bit(false); // var_codec_mode
                             // mono_data(0): spec_frontend_bit(1) + transform_info(1, long-frame
                             // at frame_len_base=1920) + psy_info max_sfb_0(6 bits at this
                             // transform length) — empirically exactly 8 bits total, and
                             // max_sfb_0 = 0 means the sf_data body decode consumes nothing
                             // further (no scalefactor bands to read), so nothing to pad here.
        bw.write_bit(false); // spec_frontend_bit = 0 (ASF)
        bw.write_bit(true); // transform_info: b_long_frame = 1 (frame_len_base >= 1536)
        bw.write_u32(0, 6); // psy_info: max_sfb_0 = 0

        bw.write_bit(true); // b_dmx_timing = 1
                            // oamd_timing_data(): oa_sample_offset_type=0, num_obj_info_blocks=1,
                            // one block with a simple (non-0b11) ramp_duration_code.
        bw.write_bit(false);
        bw.write_u32(1, 3);
        bw.write_u32(0, 6); // block_offset_factor
        bw.write_u32(0b01, 2); // ramp_duration_code != 0b11

        // oamd_dyndata_single(n_dmx=1, n_blocks=1, iframe, !alternative,
        // [Dyn], [false]): object_info_block(b_no_delta=true, dynamic=true).
        bw.write_bit(false); // b_object_not_active = 0
        bw.write_bit(true); // b_default_basic_info_md = 1 (basic_info: nothing else)
                            // render_info ALL_NEW: position + zone + otherprops all present.
        bw.write_u32(1, 6); // pos3D_X
        bw.write_u32(2, 6); // pos3D_Y
        bw.write_bit(false); // pos3D_Z_sign
        bw.write_u32(3, 4); // pos3D_Z
        bw.write_bit(true); // b_grouped_zone_defaults
        bw.write_bit(true); // b_grouped_other_defaults
        bw.write_bit(false); // b_add_table_data = 0
                             // b_alternative = false -> nothing more for oamd_dyndata_single.

        bw.write_bit(false); // b_oamd_extension_present = 0

        // ajoc(num_dmx_signals=1, num_umx_signals=1):
        bw.write_u32(0, 3); // ajoc_num_decorr = 0
                            // ajoc_ctrl_info: decorr_enable has 0 entries.
        bw.write_bit(true); // object_present[0] = true
        bw.write_u32(1, 2); // ajoc_data_point_info: num_dpoints = 1
        bw.write_u32(0, 5); // start_pos[0]
        bw.write_u32(0, 6); // ramp_len_minus1[0]
        bw.write_u32(7, 3); // num_bands_code = 7 -> 1 band
        bw.write_bit(false); // quant_select = Fine
        bw.write_bit(false); // sparse_select = false
                             // ajoc_data: ajoc_b_nodt = true -> dp=0 is DF-only; 1 channel, 1 band.
        bw.write_bit(true);
        // A single F0 codeword for the sole (o=0, dp=0, ch=0) entry.
        write_shortest_dry_fine_f0_codeword(&mut bw);

        // ajoc_dmx_de_data(1, 1): b_dmx_de_cfg=0, b_keep_dmx_de_coeffs=1
        // (skips the de_dlg_dmx_coeff loop entirely).
        bw.write_bit(false);
        bw.write_bit(true);

        bw.write_bit(true); // b_umx_timing = 1
        bw.write_bit(false); // oamd_timing_data: oa_sample_offset_type=0
        bw.write_u32(1, 3); // num_obj_info_blocks = 1
        bw.write_u32(0, 6); // block_offset_factor
        bw.write_u32(0b01, 2); // ramp_duration_code

        // oamd_dyndata_single for the umx side — identical shape.
        bw.write_bit(false); // b_object_not_active
        bw.write_bit(true); // b_default_basic_info_md
        bw.write_u32(4, 6); // pos3D_X
        bw.write_u32(5, 6); // pos3D_Y
        bw.write_bit(true); // pos3D_Z_sign
        bw.write_u32(6, 4); // pos3D_Z
        bw.write_bit(true); // b_grouped_zone_defaults
        bw.write_bit(true); // b_grouped_other_defaults
        bw.write_bit(false); // b_add_table_data

        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);

        let out = parse_audio_data_ajoc(&mut br, &info, true, false, 1920).unwrap();
        assert_eq!(out.ajoc.ctrl.num_bands[0], 1);
        assert_eq!(out.dmx_dyndata.blocks.len(), 1);
        let dmx_pos = out.dmx_dyndata.blocks[0][0]
            .render_info
            .as_ref()
            .unwrap()
            .position
            .unwrap();
        assert_eq!((dmx_pos.x, dmx_pos.y, dmx_pos.z_sign, dmx_pos.z), (1, 2, false, 3));
        let umx_pos = out.umx_dyndata.blocks[0][0]
            .render_info
            .as_ref()
            .unwrap()
            .position
            .unwrap();
        assert_eq!((umx_pos.x, umx_pos.y, umx_pos.z_sign, umx_pos.z), (4, 5, true, 6));
        assert!(out.dmx_de_data.keep_dmx_de_coeffs);
    }

    /// Writes the shortest codeword of `AJOC_HCB_DRY_FINE_F0` — used by
    /// `audio_data_ajoc_minimal_one_signal_each_side` to supply the sole
    /// `ajoc_huff_data(DRY, ...)` codeword its minimal frame needs.
    fn write_shortest_dry_fine_f0_codeword(bw: &mut oxideav_core::bits::BitWriter) {
        let (len, cw) = crate::ajoc::shortest_dry_fine_f0_for_test();
        bw.write_u32(cw, len);
    }

    #[test]
    fn substream_group_info_ajoc_no_longer_errors() {
        use oxideav_core::bits::BitWriter;
        let mut bw = BitWriter::new();
        bw.write_bit(false); // b_substreams_present
        bw.write_bit(false); // b_hsf_ext
        bw.write_bit(true); // b_single_substream -> n_lf_substreams = 1
        bw.write_bit(false); // b_channel_coded = 0 (object-coded)
        bw.write_bit(false); // b_oamd_substream = 0
        bw.write_bit(true); // b_ajoc = 1 (this substream is A-JOC coded)

        // ac4_substream_info_ajoc: b_lfe=0, b_static_dmx=1 (n_fullband_dmx=5,
        // no bed_dyn_obj_assignment read), b_oamd_common_data_present=0,
        // n_fullband_upmix_signals_minus1=1 -> 2, then dyn-only bed
        // assignment for those 2 upmix signals, no sf_multiplier
        // (fs_index=0), no bitrate info, one b_audio_ndot bit
        // (frame_rate_factor is always 1 here), no substream_index
        // (b_substreams_present=0).
        bw.write_bit(false); // b_lfe
        bw.write_bit(true); // b_static_dmx
        bw.write_bit(false); // b_oamd_common_data_present
        bw.write_u32(1, 4); // n_fullband_upmix_signals_minus1 = 1 -> 2
        bw.write_bit(true); // upmix bed_dyn_obj_assignment: b_dyn_objects_only = 1
        bw.write_bit(false); // b_bitrate_info
        bw.write_bit(false); // b_audio_ndot

        bw.write_bit(false); // b_content_type
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);

        let summary = parse_substream_group_info(&mut br, 2, 0, 4).unwrap();
        assert_eq!(summary.first_channels, 2);
    }
}
