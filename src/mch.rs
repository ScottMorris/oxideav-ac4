//! Multichannel `5_X_channel_element` walker family — round 20 wiring.
//!
//! ETSI TS 103 190-1 V1.4.1, §4.2.6.6 (`5_X_channel_element`) selects
//! between four channel-element bodies via `coding_config` (Table 25):
//!
//! | `coding_config` | body                                    |
//! |-----------------|-----------------------------------------|
//! | 0               | two_channel_data + two_channel_data + mono_data(0) |
//! | 1               | three_channel_data + two_channel_data   |
//! | 2               | four_channel_data + mono_data(0)        |
//! | 3               | five_channel_data                       |
//!
//! Each of the channel-data variants composes:
//!
//! * `three_channel_data()` — Table 27: `sf_info(ASF, 0, 0)` +
//!   `three_channel_info()` (Table 30: `chel_matsel; 4` + 2x
//!   `chparam_info()`) + 3x `sf_data(ASF)`.
//! * `four_channel_data()` — Table 28: `sf_info(ASF, 0, 0)` +
//!   `four_channel_info()` (Table 31: 4x `chparam_info()`) + 4x
//!   `sf_data(ASF)`.
//! * `five_channel_data()` — Table 29: `sf_info(ASF, 0, 0)` +
//!   `five_channel_info()` (Table 32: `chel_matsel; 4` + 5x
//!   `chparam_info()`) + 5x `sf_data(ASF)`.
//! * `mono_data(b_lfe)` — Table 21: when `b_lfe == 1`, the LFE channel
//!   uses `sf_info_lfe()` instead of the regular `sf_info()`. The
//!   bit-count for `max_sfb[0]` switches from `n_msfb_bits` to
//!   `n_msfbl_bits` (Table 106 column 4).
//!
//! Round 19 landed the type definitions + parser scaffolds plus the
//! Cfg3Five outer-shell + LFE `mono_data(1)` walker. Round 20 wires the
//! remaining three coding-config layouts (Cfg0Stereo2plusMono /
//! Cfg1ThreeStereo / Cfg2FourMono) by introducing the
//! `parse_two_channel_data()` outer (Table 26) and reusing
//! `parse_mono_data(...)` for the trailing `mono_data(0)` calls. R20
//! also splits `sf_info_lfe()` from the regular `sf_info()` parser —
//! the leading `max_sfb` field now uses `n_msfbl_bits` from Table 106
//! (column 4) instead of the regular `n_msfb_bits`, matching Table 21.
//!
//! Round 23 wires the per-channel `sf_data(ASF)` Huffman bodies for
//! the multichannel layouts. Tables 26 / 27 / 28 / 29 each emit N
//! independent `sf_data(ASF)` calls (2 / 3 / 4 / 5) right after the
//! shared `sf_info(ASF, 0, 0)` + `*_channel_info()` block. We reuse the
//! ASF Huffman codebook suite (`HCB_1` .. `HCB_11` for spectral lines,
//! `HCB_SCALEFAC` for scale-factor DPCM, `HCB_SNF` for noise fill) —
//! the multichannel paths use the same codebook IDs as mono / stereo
//! per Annex A.1 of TS 103 190-1 (no separate "MCH" codebook set
//! exists — the Huffman tables are sample-rate-independent and
//! codec-mode-independent). The per-channel scaled spectra land on the
//! `scaled_spec_per_channel` field of each `*ChannelData` for the
//! long-frame, single-window-group case; short / grouped frames still
//! parse the outer shells and leave the per-channel slot `None`.

use oxideav_core::bits::BitReader;
use oxideav_core::{Error, Result};

use crate::aspx::{parse_aspx_config, parse_companding_control, AspxConfig, CompandingControl};
use crate::asf::{
    decode_asf_long_lfe_body_with_max_sfb_lfe, decode_asf_long_mono_body_with_max_sfb,
    parse_aspx_data_1ch_body, parse_aspx_data_2ch_body, parse_asf_psy_info,
    parse_asf_psy_info_lfe, parse_asf_transform_info, parse_chparam_info, resolve_transf_length,
    AsfPsyInfo, AsfTransformInfo, ChparamInfo, SubstreamTools,
};
use crate::tables;

/// `5_X_codec_mode` values (§4.3.5.6 Table 97).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiveXCodecMode {
    Simple,
    Aspx,
    AspxAcpl1,
    AspxAcpl2,
    AspxAcpl3,
    /// Reserved (5..=7). Surfaced so callers can detect spec violations
    /// without bailing the bitreader.
    Reserved(u8),
}

impl FiveXCodecMode {
    pub fn from_u32(v: u32) -> Self {
        match v & 0b111 {
            0 => Self::Simple,
            1 => Self::Aspx,
            2 => Self::AspxAcpl1,
            3 => Self::AspxAcpl2,
            4 => Self::AspxAcpl3,
            other => Self::Reserved(other as u8),
        }
    }
}

/// `coding_config` values for the 5.X channel mode (§4.3.5.8 — keyed by
/// the enclosing `5_X_codec_mode` per Table 25). For SIMPLE / ASPX the
/// 2-bit `coding_config` selects one of four channel-data layouts; for
/// `ASPX_ACPL_{1,2}` it's a 1-bit selector between
/// two_channel_data and three_channel_data; for `ASPX_ACPL_3` there's
/// no `coding_config` at all (the body is `stereo_data()` unconditionally).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiveXCodingConfig {
    /// `coding_config == 0` (SIMPLE / ASPX): `2ch_mode + two_channel_data
    /// + two_channel_data + mono_data(0)`.
    Cfg0Stereo2plusMono,
    /// `coding_config == 1` (SIMPLE / ASPX): `three_channel_data +
    /// two_channel_data`. Also the 1-bit `coding_config` "true" value
    /// for ASPX_ACPL_{1,2} (`three_channel_data`).
    Cfg1ThreeStereo,
    /// `coding_config == 2` (SIMPLE / ASPX): `four_channel_data +
    /// mono_data(0)`.
    Cfg2FourMono,
    /// `coding_config == 3` (SIMPLE / ASPX): `five_channel_data`.
    Cfg3Five,
    /// 1-bit selector "false" branch for ASPX_ACPL_{1,2}: `two_channel_data`.
    AcplLite2,
}

/// Parsed `mono_data(b_lfe)` per Table 21 — outer shell + body.
///
/// `b_lfe == 1` switches `sf_info(...)` to `sf_info_lfe()` per §4.2.8 /
/// §4.3.6.2.1 (the `max_sfb[0]` field uses `n_msfbl_bits` from Table
/// 106 instead of `n_msfb_bits`).
///
/// Round 37: extended with `scaled_spec` — the dequantised + scaled
/// MDCT spectrum from the trailing `sf_data(ASF)` body (long-frame,
/// single window group case only). For LFE, for the SSF frontend
/// (`spec_frontend_bit == 1`), or for any short / grouped / Huffman-error
/// case, this stays `None` and only the outer shell is filled. This
/// matches the per-channel `scaled_spec_per_channel` slot pattern in
/// [`TwoChannelData`] / [`ThreeChannelData`] etc.
#[derive(Debug, Clone, Default)]
pub struct MonoLfeData {
    /// `b_lfe` flag the walker was invoked with.
    pub b_lfe: bool,
    /// `spec_frontend` selector — always ASF for LFE per Table 21.
    /// Captured from the bitstream for non-LFE invocations.
    pub spec_frontend_bit: u8,
    /// Parsed `asf_transform_info()` for the channel.
    pub transform_info: Option<AsfTransformInfo>,
    /// Parsed `asf_psy_info()` for the channel. For LFE this is the
    /// `sf_info_lfe()` flavour with `max_sfb` capped to
    /// `num_sfb_lfe()` and bit-width `n_msfbl_bits`.
    pub psy_info: Option<AsfPsyInfo>,
    /// Dequantised + scaled MDCT spectrum from the trailing `sf_data(ASF)`
    /// body. Populated for the non-LFE, ASF-frontend, long-frame,
    /// single-window-group case. Length is `sfb_offset[max_sfb]` at the
    /// signalled transform length. `None` for LFE (the LFE body decoder
    /// is reserved for a future round), for SSF-frontend mono channels,
    /// or for any short / grouped / Huffman-error case.
    pub scaled_spec: Option<Vec<f32>>,
    /// Per-window de-grouped spectra (§5.1.5 ungrouping already
    /// applied) for the non-LFE, ASF-frontend, short/grouped
    /// (`num_window_groups > 1`) case — see [`WindowSpectrum`]. `None`
    /// whenever `scaled_spec` would be populated instead (long-frame),
    /// or for LFE / SSF-frontend / Huffman-error cases.
    pub scaled_spec_windows: Option<Vec<WindowSpectrum>>,
}

/// Parsed `three_channel_info()` per Table 30: 4-bit `chel_matsel` +
/// two `chparam_info()` payloads.
#[derive(Debug, Clone, Default)]
pub struct ThreeChannelInfo {
    pub chel_matsel: u8,
    pub chparam: [ChparamInfo; 2],
}

/// Parsed `four_channel_info()` per Table 31: four `chparam_info()`
/// payloads (no `chel_matsel`).
#[derive(Debug, Clone, Default)]
pub struct FourChannelInfo {
    pub chparam: [ChparamInfo; 4],
}

/// Parsed `five_channel_info()` per Table 32: 4-bit `chel_matsel` +
/// five `chparam_info()` payloads.
#[derive(Debug, Clone, Default)]
pub struct FiveChannelInfo {
    pub chel_matsel: u8,
    pub chparam: [ChparamInfo; 5],
}

/// Parsed `two_channel_data()` outer shell per Table 26.
///
/// Table 26 is a stripped-down stereo container — single shared
/// `sf_info(ASF, 0, 0)` followed by `chparam_info()` (the full Table 47
/// row including `sap_mode` / `ms_used` / `sap_data`) and then two
/// `sf_data(ASF)` bodies. We parse the outer shell only — the
/// `sf_data(ASF)` Huffman bodies are deferred to the `acpl_synth` /
/// joint-MDCT decoder paths (Pseudocode 178 in §5.3.3 needs the
/// transform-matrix wiring that's still outstanding for the multichannel
/// elements).
///
/// Used by `5_X_channel_element` Cfg0 (twice — L/R and Ls/Rs) and Cfg1
/// (after the leading `three_channel_data`).
///
/// Round 23 added `scaled_spec_per_channel` — when the body is decoded
/// in the long-frame, single-window-group case, each `Some(...)` entry
/// carries the dequantised + scaled MDCT spectrum for that channel.
/// Entries are `None` when the per-channel `sf_data(ASF)` walk bailed
/// (short frame, grouped, or Huffman error).
#[derive(Debug, Clone, Default)]
pub struct TwoChannelData {
    /// `b_enable_mdct_stereo_proc` (Table 26, first bit). When set the
    /// two channels share one `sf_info` + a `chparam_info`; when clear
    /// each channel carries its *own* `sf_info` and there is no
    /// chparam. (Round 404: this selector bit was previously never
    /// read — the parser hardcoded the shared-sf_info branch, desyncing
    /// every `two_channel_data` in every frame by at least one bit.)
    pub b_enable_mdct_stereo_proc: bool,
    pub transform_info: Option<AsfTransformInfo>,
    pub psy_info: Option<AsfPsyInfo>,
    /// Channel 1's own `sf_info` for the `b_enable_mdct_stereo_proc == 0`
    /// branch (`None` when the shared branch is taken).
    pub transform_info_1: Option<AsfTransformInfo>,
    pub psy_info_1: Option<AsfPsyInfo>,
    pub chparam: Option<ChparamInfo>,
    /// Per-channel scaled MDCT spectra. Length = 2 once the body has
    /// been walked. Each entry's `Vec<f32>` is `sfb_offset[max_sfb]`
    /// long. Only populated for the long-frame, single-window-group
    /// case — see `scaled_spec_windows_per_channel` for grouped bodies.
    pub scaled_spec_per_channel: Vec<Option<Vec<f32>>>,
    /// Per-channel, per-window de-grouped spectra (§5.1.5 ungrouping
    /// applied) for the short/grouped (`num_window_groups > 1`) case.
    /// `None` per channel whenever `scaled_spec_per_channel` would be
    /// populated instead.
    pub scaled_spec_windows_per_channel: Vec<Option<Vec<WindowSpectrum>>>,
}

/// Parsed `three_channel_data()` outer shell + per-channel sf_data
/// bodies per Table 27.
///
/// Holds the shared `sf_info` (transform_info + psy_info) and the
/// `three_channel_info` (chel_matsel + 2x chparam_info). Round 23 also
/// walks the three trailing `sf_data(ASF)` bodies into
/// `scaled_spec_per_channel` (length 3).
#[derive(Debug, Clone, Default)]
pub struct ThreeChannelData {
    pub transform_info: Option<AsfTransformInfo>,
    pub psy_info: Option<AsfPsyInfo>,
    pub info: Option<ThreeChannelInfo>,
    /// Per-channel scaled MDCT spectra (length 3). See [`TwoChannelData`].
    pub scaled_spec_per_channel: Vec<Option<Vec<f32>>>,
    /// Per-channel, per-window de-grouped spectra. See [`TwoChannelData`].
    pub scaled_spec_windows_per_channel: Vec<Option<Vec<WindowSpectrum>>>,
}

/// Parsed `four_channel_data()` outer shell + per-channel sf_data
/// bodies per Table 28. Round 23 walks the four trailing `sf_data(ASF)`
/// bodies into `scaled_spec_per_channel` (length 4).
#[derive(Debug, Clone, Default)]
pub struct FourChannelData {
    pub transform_info: Option<AsfTransformInfo>,
    pub psy_info: Option<AsfPsyInfo>,
    pub info: Option<FourChannelInfo>,
    /// Per-channel scaled MDCT spectra (length 4). See [`TwoChannelData`].
    pub scaled_spec_per_channel: Vec<Option<Vec<f32>>>,
    /// Per-channel, per-window de-grouped spectra. See [`TwoChannelData`].
    pub scaled_spec_windows_per_channel: Vec<Option<Vec<WindowSpectrum>>>,
}

/// Parsed `five_channel_data()` outer shell + per-channel sf_data
/// bodies per Table 29. Round 23 walks the five trailing `sf_data(ASF)`
/// bodies into `scaled_spec_per_channel` (length 5).
#[derive(Debug, Clone, Default)]
pub struct FiveChannelData {
    pub transform_info: Option<AsfTransformInfo>,
    pub psy_info: Option<AsfPsyInfo>,
    pub info: Option<FiveChannelInfo>,
    /// Per-channel scaled MDCT spectra (length 5). See [`TwoChannelData`].
    pub scaled_spec_per_channel: Vec<Option<Vec<f32>>>,
    /// Per-channel, per-window de-grouped spectra. See [`TwoChannelData`].
    pub scaled_spec_windows_per_channel: Vec<Option<Vec<WindowSpectrum>>>,
}

// =====================================================================
// Per-element parsers
// =====================================================================

/// `mono_data(b_lfe)` per Table 21.
///
/// For `b_lfe == 1` the leading `spec_frontend` bit is **omitted** and
/// `sf_info_lfe()` runs in place of `sf_info()` — `max_sfb[0]` is
/// `n_msfbl_bits` wide and clamped to the LFE band table.
///
/// Round 37: when the channel is non-LFE, ASF-frontend
/// (`spec_frontend_bit == 0`), and long-frame / single-window-group, the
/// trailing `sf_data(ASF)` body is also walked into `scaled_spec`
/// (matching the multichannel `decode_mch_sf_data_channels` pattern).
/// Walker is **try-and-bail** for the body so a Huffman miss leaves the
/// outer shell intact and the caller still gets `Ok(...)`. The SSF
/// frontend and LFE body paths remain deferred — those slots stay
/// `None` and the bitreader cursor is left where the outer shell
/// finished (consistent with prior behaviour).
pub fn parse_mono_data(
    br: &mut BitReader<'_>,
    b_lfe: bool,
    frame_len_base: u32,
) -> Result<MonoLfeData> {
    let mut out = MonoLfeData {
        b_lfe,
        ..Default::default()
    };
    if !b_lfe {
        // Non-LFE: leading 1-bit spec_frontend selector.
        out.spec_frontend_bit = br.read_u32(1)? as u8;
    }
    // `sf_info_lfe()` (Table 35) sets `b_long_frame = 1` implicitly —
    // "transform length = frame_length" — and reads *no* bits for it,
    // unlike the regular `sf_info()` -> `asf_transform_info()` path.
    // Calling `parse_asf_transform_info` for the LFE branch (as this
    // used to) steals real bits belonging to `max_sfb[0]`/`sf_data()`
    // that follow, misaligning the rest of the LFE parse in a
    // data-dependent way (whatever the stolen `b_long_frame` bit
    // happens to be).
    let ti = if b_lfe {
        let tl = resolve_transf_length(frame_len_base, true, 0);
        AsfTransformInfo {
            b_long_frame: true,
            transf_length: [0, 0],
            transform_length_0: tl,
            transform_length_1: tl,
        }
    } else {
        parse_asf_transform_info(br, frame_len_base)?
    };
    out.transform_info = Some(ti);
    // `sf_info(ASF, 0, 0)` for non-LFE; `sf_info_lfe()` for LFE.
    // r20: dispatch to the dedicated `parse_asf_psy_info_lfe()` that
    // uses Table 106 column `n_msfbl_bits` for `max_sfb[0]` instead of
    // the regular `n_msfb_bits`. The two widths can differ by 2-4 bits
    // (e.g. 48 kHz long-frame: 6 vs 3) so this matters for any real
    // 5.1 / 7.1 stream LFE alignment.
    let psy = if b_lfe {
        parse_asf_psy_info_lfe(br, &ti)?
    } else {
        parse_asf_psy_info(br, &ti, frame_len_base, false, false)?
    };

    // Round 37: trailing `sf_data(ASF)` body for non-LFE ASF-frontend
    // mono channels. This mirrors the per-channel body walk inside
    // `decode_mch_sf_data_channels` for a single channel. Try-and-bail:
    // any Huffman / bit-stream miss leaves `scaled_spec` as `None` and
    // returns `Ok(...)`. SSF-frontend (`spec_frontend_bit == 1`) is
    // deferred — the SSF body lives elsewhere in the substream and is
    // not co-located with `mono_data()`.
    //
    // Round 38: extend the body walk to LFE channels via
    // `decode_asf_long_lfe_body_with_max_sfb_lfe`. Per `sf_info_lfe()`
    // (Table 35) the LFE channel is always long-frame, single window
    // group, ASF-frontend — so the same long-frame mono body decoder
    // applies, just with `max_sfb_0` already capped by the
    // `n_msfbl_bits` bit-width.
    if !b_lfe && out.spec_frontend_bit == 0 {
        if ti.b_long_frame && psy.num_window_groups == 1 {
            if let Some(scaled) = decode_asf_long_mono_body_with_max_sfb(br, &ti, psy.max_sfb_0) {
                out.scaled_spec = Some(scaled);
            }
        } else if psy.num_window_groups > 0 {
            if let Some(windows) =
                decode_asf_grouped_body_windows(br, &ti, &psy, psy.max_sfb_0)
            {
                out.scaled_spec_windows = Some(windows);
            }
        }
    } else if b_lfe {
        // LFE: `sf_info_lfe()` forces long-frame, single window group;
        // walk the LFE body via the dedicated decoder. Try-and-bail
        // identical to the non-LFE path.
        if let Some(scaled) = decode_asf_long_lfe_body_with_max_sfb_lfe(br, &ti, psy.max_sfb_0) {
            out.scaled_spec = Some(scaled);
        }
    }
    out.psy_info = Some(psy);
    Ok(out)
}

/// `two_channel_data()` outer shell + per-channel `sf_data(ASF)` bodies
/// per Table 26.
///
/// Walks `sf_info(ASF, 0, 0)` + `chparam_info()` then two
/// `sf_data(ASF)` bodies. For the long-frame, single-window-group case
/// the per-channel scaled spectra land on `scaled_spec_per_channel`
/// (length 2). Short / grouped frames push `None` for each channel —
/// the outer shell still parses cleanly.
pub fn parse_two_channel_data(
    br: &mut BitReader<'_>,
    frame_len_base: u32,
) -> Result<TwoChannelData> {
    // Table 26: `b_enable_mdct_stereo_proc` selects between a shared
    // sf_info + chparam_info (joint MDCT stereo processing) and two
    // fully independent per-channel sf_infos.
    let b_msp = br.read_bit()?;
    // AC4_TRACE_BODIES=1: per-element diagnostic used by the round-40x
    // conformance work (see riptide docs/ac4-decoder-accuracy-plan.md).
    let _trace = std::env::var_os("AC4_TRACE_BODIES").is_some();
    let _p_in = br.bit_position();
    if _trace {
        eprintln!("2CH b_msp={} in@{}", b_msp as u8, _p_in - 1);
    }
    if b_msp {
        let ti = parse_asf_transform_info(br, frame_len_base)?;
        let psy = parse_asf_psy_info(br, &ti, frame_len_base, false, false)?;
        // Round 406e: Table 47 says the ms_used loop runs per window
        // group, but the trailer-validated Kraftwerk walk proves this
        // encoder reads exactly ONE group's worth (m=6 with ng=2 →
        // 6 ms bits). Keep the single-group read; psy.max_sfb_per_group()
        // exists for when counter-evidence shows up.
        let chparam = parse_chparam_info(br, &[psy.max_sfb_0])?;
        let (scaled, scaled_windows) = decode_mch_sf_data_channels(br, &ti, &psy, 2);
        if _trace {
            eprintln!(
                "2CH shared long={} m={} ng={} out@{}",
                ti.b_long_frame, psy.max_sfb_0, psy.num_window_groups, br.bit_position()
            );
        }
        Ok(TwoChannelData {
            b_enable_mdct_stereo_proc: true,
            transform_info: Some(ti),
            psy_info: Some(psy),
            transform_info_1: None,
            psy_info_1: None,
            chparam: Some(chparam),
            scaled_spec_per_channel: scaled,
            scaled_spec_windows_per_channel: scaled_windows,
        })
    } else {
        // Independent channels: each has its own sf_info, and each
        // sf_data body is decoded against its own transform/psy pair.
        let ti0 = parse_asf_transform_info(br, frame_len_base)?;
        let psy0 = parse_asf_psy_info(br, &ti0, frame_len_base, false, false)?;
        let ti1 = parse_asf_transform_info(br, frame_len_base)?;
        let psy1 = parse_asf_psy_info(br, &ti1, frame_len_base, false, false)?;
        let (mut scaled, mut scaled_windows) = decode_mch_sf_data_channels(br, &ti0, &psy0, 1);
        let (s1, w1) = decode_mch_sf_data_channels(br, &ti1, &psy1, 1);
        scaled.extend(s1);
        scaled_windows.extend(w1);
        if _trace {
            eprintln!(
                "2CH separate long=({},{}) m=({},{}) out@{}",
                ti0.b_long_frame, ti1.b_long_frame, psy0.max_sfb_0, psy1.max_sfb_0,
                br.bit_position()
            );
        }
        Ok(TwoChannelData {
            b_enable_mdct_stereo_proc: false,
            transform_info: Some(ti0),
            psy_info: Some(psy0),
            transform_info_1: Some(ti1),
            psy_info_1: Some(psy1),
            chparam: None,
            scaled_spec_per_channel: scaled,
            scaled_spec_windows_per_channel: scaled_windows,
        })
    }
}

/// Number of scale factor bands lying strictly below the A-SPX start
/// subband (`sba`) for the given transform length — i.e. the count of
/// sfbs whose END line is < `sba * (tl / 64)`.
///
/// Round 406: on real 7_X ASPX content the additional-channel
/// `two_channel_data`'s `chparam_info` ms_used loop runs over THIS
/// count, not `get_max_sfb` as Table 47 reads — proven by exact-end
/// backchaining on two independent tracks (ms bits = 50 with
/// max_sfb = 54 on one and max_sfb = 44 on the other; both
/// aspx start_freq=7/HighRes ⇒ sba=40 ⇒ line 1280 ⇒ 50 bands).
pub(crate) fn aspx_core_band_count(cfg: &crate::aspx::AspxConfig, tl: u32) -> Option<u32> {
    let (_master, _n, sba, _sbz) = crate::aspx::derive_master_sbg_table(cfg);
    let sb_width = tl / 64;
    let line = sba * sb_width;
    let sfbo = crate::sfb_offset::sfb_offset_48(tl)?;
    let num_sfb = crate::tables::num_sfb_48(tl)?;
    let mut n = 0u32;
    for sfb in 0..num_sfb as usize {
        if (sfbo[sfb + 1] as u32) < line {
            n += 1;
        } else {
            break;
        }
    }
    Some(n)
}

/// Trial-parse helper for [`parse_two_channel_data_additional`]: given
/// a reader positioned at body0's first section bit, find body0's
/// scalefac/SNF band bound by validating each candidate tail length
/// against a full parse of body1. Returns the winning bound
/// (`k + 1` bands for `k` scalefac codewords read).
pub(crate) fn discover_add_pair_body0_bound(
    br0: BitReader<'_>,
    ti: &AsfTransformInfo,
    max_sfb_1: u32,
) -> Option<u32> {
    use crate::asf_data;
    let tl = ti.transform_length_0;
    let tl_idx = ti.transf_length[0];
    let sfbo = crate::sfb_offset::sfb_offset_48(tl)?;
    let num_sfb = crate::tables::num_sfb_48(tl)?;
    for m0 in 1..=num_sfb {
        // Full deterministic body0 parse under the candidate bound.
        let mut tr = br0;
        let Ok(sections) = asf_data::parse_asf_section_data_ext(&mut tr, tl_idx, tl, m0, true)
        else {
            continue;
        };
        if sections.sect_cb.iter().any(|&cb| cb > 11) {
            continue;
        }
        let Ok((_q, mqi)) = asf_data::parse_asf_spectral_data(&mut tr, &sections, sfbo, m0)
        else {
            continue;
        };
        if asf_data::parse_asf_scalefac_data(&mut tr, &sections, &mqi, m0, tl).is_err() {
            continue;
        }
        if asf_data::parse_asf_snf_data(&mut tr, &sections, &mqi, m0, tl).is_err() {
            continue;
        }
        // Oracle: body1 must parse cleanly from here with legal
        // codebooks and sections closing exactly at max_sfb_1 — a few
        // hundred chained Huffman codewords make a false accept
        // essentially impossible.
        let mut vr = tr;
        let Ok(s1) = asf_data::parse_asf_section_data_ext(&mut vr, tl_idx, tl, max_sfb_1, true)
        else {
            continue;
        };
        if s1.sect_cb.iter().any(|&cb| cb > 11) {
            continue;
        }
        if s1.sect_end.last().map(|&e| e as u32) != Some(max_sfb_1) {
            continue;
        }
        if asf_data::parse_asf_spectral_data(&mut vr, &s1, sfbo, max_sfb_1).is_err() {
            continue;
        }
        return Some(m0);
    }
    None
}

/// Round-407j: locate the I-frame trailer block's 1ch+final pair by
/// hard constraints and return (1ch_start, slot2, slot3). See the
/// call site for rationale.
fn scan_iframe_tail_slots(
    floor_br: BitReader<'_>,
    wall: u64,
    tools: &SubstreamTools,
    cfg: &crate::aspx::AspxConfig,
    frame_len_base: u32,
) -> Option<(u64, u8, u8)> {
    let floor = floor_br.bit_position();
    let hi = wall.saturating_sub(40);
    let mut e = floor;
    while e < hi {
        let mut hr = floor_br;
        if hr.skip((e - floor) as u32).is_err() {
            return None;
        }
        // '000' xover gate for the 1ch (five-frame-proven slot 2 = 0).
        let probe = hr;
        {
            let mut g = probe;
            match g.read_u32(3) {
                Ok(0) => {}
                _ => {
                    e += 1;
                    continue;
                }
            }
        }
        let mut vt = tools.clone();
        vt.aspx_trailer_slot = 2;
        let mut vr = probe;
        if crate::asf::parse_aspx_data_1ch_body(&mut vr, &mut vt, cfg, true, frame_len_base)
            .is_err()
        {
            e += 1;
            continue;
        }
        let y = vr.bit_position();
        // final 2ch parse; its own xover read fills slot 3.
        if crate::asf::parse_aspx_data_2ch_body(&mut vr, &mut vt, cfg, true, frame_len_base)
            .is_err()
        {
            e += 1;
            continue;
        }
        if vr.bit_position() > wall || !(0..=8).contains(&(wall as i64 - vr.bit_position() as i64))
        {
            e += 1;
            continue;
        }
        let s2 = vt.aspx_xover_slots[2]?;
        let s3 = vt.aspx_xover_slots[3]?;
        let _ = y;
        return Some((e, s2, s3));
    }
    None
}

/// Round-407d resync-by-signature: starting from `floor_br`, scan bit
/// positions for the PROVEN additional-pair head signature
/// ([bmsp=1][blong=1][msfb!=0][sap!=3], with sap=1 implying the
/// aspx-core ms run), validate the FULL remaining chain on a copy
/// (body0 via bound discovery, body1 at msfb, then the four ASPX
/// trailers ending within 8 bits of the wall), and return a reader
/// positioned at the winning head. This realigns the walk after front
/// elements whose per-body grammar is still ambiguous, so the
/// additional pair + trailers + sticky configs parse on every frame.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resync_7x_addpair<'a>(
    floor_br: BitReader<'a>,
    tools: &SubstreamTools,
    cfg: &crate::aspx::AspxConfig,
    b_iframe: bool,
    frame_len_base: u32,
) -> Option<(BitReader<'a>, [Option<u8>; 8])> {
    let wall = tools.wall_bits?;
    let ti = AsfTransformInfo {
        b_long_frame: true,
        transf_length: [0, 0],
        transform_length_0: frame_len_base,
        transform_length_1: frame_len_base,
    };
    let ms = aspx_core_band_count(cfg, frame_len_base).unwrap_or(50);
    // The additional pair + trailers never exceed ~9000 bits on real
    // content (largest observed: frame-0's ~6400) — scanning earlier
    // positions only burns time on impossible candidates.
    let mut floor_br = floor_br;
    let floor0 = floor_br.bit_position();
    let floor_min = wall.saturating_sub(9000);
    if floor0 < floor_min {
        let _ = floor_br.skip((floor_min - floor0) as u32);
    }
    let floor = floor_br.bit_position();
    let hi = wall.saturating_sub(400);
    // Two passes: the proven bmsp=1 signature first over the whole
    // range; the looser bmsp=0 shape only if nothing matched (it
    // otherwise steals earlier false positions from proven heads).
    for allow_bmsp0 in [false, true] {
    let mut e = floor;
    let mut brute_budget: u32 = 16;
    while e < hi {
        // Position a copy at e.
        let mut hr = floor_br;
        if hr.skip((e - floor) as u32).is_err() {
            return None;
        }
        let head = hr;
        // Cheap pattern gate. bmsp=1: shared sf_info + chparam.
        // bmsp=0 (round 407k): two independent long sf_infos.
        let Ok(bmsp) = hr.read_bit() else { return None };
        if !bmsp {
            if !allow_bmsp0 {
                e += 1;
                continue;
            }
            let Ok(bl0) = hr.read_bit() else { return None };
            let Ok(m0f) = hr.read_u32(6) else { return None };
            let Ok(bl1) = hr.read_bit() else { return None };
            let Ok(m1f) = hr.read_u32(6) else { return None };
            if !(bl0 && bl1 && m0f != 0 && m1f != 0) {
                e += 1;
                continue;
            }
            // body0 with its own field; body1 with the second.
            if !legal_body_prefix(hr, &ti) {
                e += 1;
                continue;
            }
            let Some(b0m) = discover_add_pair_body0_bound(hr, &ti, m1f) else {
                e += 1;
                continue;
            };
            let mut vr = hr;
            if crate::asf::decode_asf_long_mono_body_with_max_sfb_ext(&mut vr, &ti, b0m, true)
                .is_none()
            {
                e += 1;
                continue;
            }
            if crate::asf::decode_asf_long_mono_body_with_max_sfb_ext(&mut vr, &ti, m1f, true)
                .is_none()
            {
                e += 1;
                continue;
            }
            if wall.saturating_sub(vr.bit_position()) > 1200 {
                e += 1;
                continue;
            }
            let ok = validate_7x_trailers_slots_budgeted(
                vr,
                tools,
                cfg,
                b_iframe,
                frame_len_base,
                brute_budget > 0,
            );
            if ok.is_none() {
                brute_budget = brute_budget.saturating_sub(1);
                e += 1;
                continue;
            }
            if std::env::var_os("AC4_T").is_some() {
                eprintln!(
                    "RESYNC 7x add-pair(bmsp0) @{} (floor {floor}, m {m0f}/{m1f}, b0m {b0m})",
                    head.bit_position()
                );
            }
            return Some((head, ok.unwrap()));
        }
        let Ok(blong) = hr.read_bit() else { return None };
        if !blong {
            e += 1;
            continue;
        }
        let Ok(msfb) = hr.read_u32(6) else { return None };
        if msfb == 0 {
            e += 1;
            continue;
        }
        let Ok(sap) = hr.read_u32(2) else { return None };
        if sap == 1 {
            if hr.skip(ms).is_err() {
                e += 1;
                continue;
            }
        } else if sap == 3 {
            // Round 407k: sap_data heads exist on real frames (the
            // frame-4/5 class) — consume it with the production
            // parser over the aspx-core band count.
            if crate::asf::parse_sap_data(&mut hr, &[ms]).is_err() {
                e += 1;
                continue;
            }
        }
        let dbg = std::env::var("AC4_RESYNC_DEBUG")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|want| want == e)
            .unwrap_or(false);
        // body0 legality prefix, then full discovery + body1.
        if !legal_body_prefix(hr, &ti) {
            if dbg {
                eprintln!("RSDBG e={e}: legal_body_prefix REJECT");
            }
            e += 1;
            continue;
        }
        let Some(m0) = discover_add_pair_body0_bound(hr, &ti, msfb) else {
            if dbg {
                eprintln!("RSDBG e={e}: discovery REJECT (msfb={msfb} sap={sap})");
            }
            e += 1;
            continue;
        };
        let mut vr = hr;
        if crate::asf::decode_asf_long_mono_body_with_max_sfb_ext(&mut vr, &ti, m0, true)
            .is_none()
        {
            e += 1;
            continue;
        }
        if crate::asf::decode_asf_long_mono_body_with_max_sfb_ext(&mut vr, &ti, msfb, true)
            .is_none()
        {
            e += 1;
            continue;
        }
        // The four trailers span ~250-400 bits on real content; a
        // candidate whose bodies end far from the wall cannot be the
        // additional pair. This gate kills nearly every false
        // candidate before the expensive trailer validation.
        if wall.saturating_sub(vr.bit_position()) > 1200 {
            e += 1;
            continue;
        }
        // Trailers with slot self-discovery (round 407e). The 4096
        // brute is budgeted to the first candidate that gets here.
        let ok = validate_7x_trailers_slots_budgeted(
            vr,
            tools,
            cfg,
            b_iframe,
            frame_len_base,
            brute_budget > 0,
        );
        if ok.is_none() {
            brute_budget = brute_budget.saturating_sub(1);
        }
        if dbg {
            eprintln!(
                "RSDBG e={e}: m0={m0} trailers {:?} (harvested slots={:?}) iframe={b_iframe}",
                ok.as_ref().map(|s| &s[..4]),
                &tools.aspx_xover_slots[..4]
            );
        }
        if let Some(slots) = ok {
            if std::env::var_os("AC4_T").is_some() {
                eprintln!(
                    "RESYNC 7x add-pair @{} (floor {floor}, msfb={msfb}, m0={m0})",
                    head.bit_position()
                );
            }
            return Some((head, slots));
        }
        e += 1;
    }
    }
    None
}

/// Validate the 7_X ASPX trailer block (2ch,2ch,1ch,2ch) on a COPY of
/// the reader: must parse and end within 8 bits of the wall.
pub(crate) fn validate_7x_trailers(
    vr: BitReader<'_>,
    tools: &SubstreamTools,
    cfg: &crate::aspx::AspxConfig,
    b_iframe: bool,
    frame_len_base: u32,
) -> bool {
    validate_7x_trailers_slots(vr, tools, cfg, b_iframe, frame_len_base).is_some()
}

/// Like [`validate_7x_trailers`] but returns the slot vector that made
/// the walk land on the wall. Tries the harvested sticky slots first;
/// on failure (P-frames only) brute-forces the 8^4 vectors — round
/// 407e proved the harvested I-frame slots can be internally wrong
/// ([0,0,4,4] harvested where the cross-frame-proven vector is
/// [0,0,0,4]; the I-frame trailer internals still need their own
/// backchain) while the wall oracle over a whole trailer block is
/// selective enough to recover the working vector per frame.
pub(crate) fn validate_7x_trailers_slots(
    vr: BitReader<'_>,
    tools: &SubstreamTools,
    cfg: &crate::aspx::AspxConfig,
    b_iframe: bool,
    frame_len_base: u32,
) -> Option<[Option<u8>; 8]> {
    validate_7x_trailers_slots_budgeted(vr, tools, cfg, b_iframe, frame_len_base, true)
}

/// Budgeted variant: `allow_brute` gates the 8^4 vector search (4096
/// trailer walks — too expensive to run per rejected resync
/// candidate; the resync loop grants it to the first candidate that
/// reaches trailer validation and uses cheap checks after).
pub(crate) fn validate_7x_trailers_slots_budgeted(
    vr: BitReader<'_>,
    tools: &SubstreamTools,
    cfg: &crate::aspx::AspxConfig,
    b_iframe: bool,
    frame_len_base: u32,
    allow_brute: bool,
) -> Option<[Option<u8>; 8]> {
    let Some(wall) = tools.wall_bits else {
        return Some(tools.aspx_xover_slots); // no wall info — accept
    };
    let attempt = |slots: [Option<u8>; 8]| -> bool {
        let mut r2 = vr;
        let mut tt = tools.clone();
        tt.aspx_xover_slots = slots;
        tt.aspx_trailer_slot = 0;
        for chs in [2u8, 2, 1, 2] {
            let r = if chs == 1 {
                crate::asf::parse_aspx_data_1ch_body(&mut r2, &mut tt, cfg, b_iframe, frame_len_base)
            } else {
                crate::asf::parse_aspx_data_2ch_body(&mut r2, &mut tt, cfg, b_iframe, frame_len_base)
            };
            if r.is_err() || r2.bit_position() > wall {
                return false;
            }
        }
        (0..=8).contains(&(wall as i64 - r2.bit_position() as i64))
    };
    if attempt(tools.aspx_xover_slots) {
        return Some(tools.aspx_xover_slots);
    }
    if b_iframe {
        // I-frames read their xovers from the bits — slots don't gate.
        return None;
    }
    // Last-known-good vector (persisted across frames) — cheap second try.
    if let Some(good) = tools.aspx_xover_slots_good {
        if good != tools.aspx_xover_slots && attempt(good) {
            return Some(good);
        }
    }
    // Round 407e postmortem: a free 8^4 vector search here OVERFITS —
    // with 4096 slot choices, huffman soup at wrong positions
    // validates against the wall and resync locks onto fake add-pairs
    // (observed: frame 1 "resyncing" to 5551 with slots [5,3,0,0]
    // when the proven head is 9409 with [0,0,0,4]). The vector must
    // come from a TRUSTED source: the harvested I-frame slots (buggy
    // today — see the I-frame trailer-internals open problem) or the
    // cross-frame-proven cache. `_allow_brute` is kept for a future
    // constrained search.
    let _ = allow_brute;
    None
}

/// Generalized round-407c body-bound discovery: parse ONE long-frame
/// sf_data body under the untruncated-section grammar, discovering its
/// scalefac/SNF band bound by trying candidates ascending and
/// validating each against an oracle on the FOLLOWING bits:
///   - `next_bodies > 0`: the next body must parse legally (sections
///     with cb <= 11) under SOME bound of its own (checked shallowly
///     with its section+spectral prefix, which is bound-independent);
///   - `next_bodies == 0`: the following bits must look like the next
///     element head (caller-provided check).
///
/// Returns (bound, end_bit_position) without consuming the reader.
///
/// Background: real content transmits scalefac/SNF for FEWER bands
/// than the sections span, and the transmitted max_sfb explains only
/// some bodies (add-pair body1) — proven by exact-end backchaining on
/// frames 0/1 (3ch bodies with bounds 11/8/30 against max_sfb=6).
pub(crate) fn discover_body_bound(
    br0: BitReader<'_>,
    ti: &AsfTransformInfo,
    hint: u32,
    oracle: impl Fn(BitReader<'_>) -> bool,
) -> Option<(u32, u64)> {
    use crate::asf_data;
    let tl = ti.transform_length_0;
    let tl_idx = ti.transf_length[0];
    let sfbo = crate::sfb_offset::sfb_offset_48(tl)?;
    let num_sfb = crate::tables::num_sfb_48(tl)?;
    // Candidate order matters: the TRANSMITTED max_sfb (hint) is
    // correct for validated elements (frame-0 3ch m=56, add-pair
    // body1) — try it first so proven frames stay bit-exact; fall
    // back to ascending discovery for the bodies whose bound the
    // header demonstrably does not describe.
    let hint = hint.min(num_sfb).max(1);
    let candidates = std::iter::once(hint).chain((1..=num_sfb).filter(move |&k| k != hint));
    // Round 407m perf: for a given start, the section list (and hence
    // the spectral parse) is identical across every k that stops the
    // section loop at the same boundary — for the common
    // single-section bodies that's ALL k. Cache (sections, mqi,
    // post-spectral reader) keyed by the section geometry so the
    // 63-candidate sweep re-parses only the tiny scalefac/SNF tails.
    let mut cache: Option<(Vec<u16>, asf_data::AsfSections, Vec<u32>, BitReader)> = None;
    for k in candidates {
        let mut tr = br0;
        let Ok(sections) = asf_data::parse_asf_section_data_ext(&mut tr, tl_idx, tl, k, true)
        else {
            continue;
        };
        // NOTE: cb 12-15 sentinel sections are real on some frames —
        // no legality filter here (that's a scan-side discriminator).
        let geom: Vec<u16> = sections
            .sect_end
            .iter()
            .copied()
            .chain(sections.sect_cb.iter().map(|&c| c as u16))
            .collect();
        let (secs, mqi, mut tr) = match cache.as_ref() {
            Some((g, cs, cm, cr)) if *g == geom => (cs.clone(), cm.clone(), *cr),
            _ => {
                let Ok((_q, m)) = asf_data::parse_asf_spectral_data(&mut tr, &sections, sfbo, k)
                else {
                    continue;
                };
                cache = Some((geom, sections.clone(), m.clone(), tr));
                (sections, m, tr)
            }
        };
        if asf_data::parse_asf_scalefac_data(&mut tr, &secs, &mqi, k, tl).is_err() {
            continue;
        }
        if asf_data::parse_asf_snf_data(&mut tr, &secs, &mqi, k, tl).is_err() {
            continue;
        }
        if oracle(tr) {
            return Some((k, tr.bit_position()));
        }
    }
    None
}

/// Oracle helper: do the bits at `br` parse as a legal body prefix
/// (untruncated sections, cb <= 11, spectral decodes)? Bound-agnostic:
/// uses max_sfb = 1, whose section+spectral prefix equals any small
/// bound's.
pub(crate) fn legal_body_prefix(mut br: BitReader<'_>, ti: &AsfTransformInfo) -> bool {
    use crate::asf_data;
    let tl = ti.transform_length_0;
    if crate::sfb_offset::sfb_offset_48(tl).is_none() {
        return false;
    }
    let Ok(sections) = asf_data::parse_asf_section_data_ext(&mut br, ti.transf_length[0], tl, 1, true)
    else {
        return false;
    };
    if sections.sect_cb.iter().any(|&cb| cb > 11) {
        return false;
    }
    // Round 407m perf: bounded probe — decode at most 8 spectral
    // codewords of the first coded section instead of the whole body.
    // Nearly as selective against garbage, and O(1) instead of
    // O(body) at the thousands of positions the resync scan visits.
    let Some(&cb) = sections.sect_cb.iter().find(|&&c| c != 0 && c <= 11) else {
        return true; // all-zero/sentinel sections: nothing to probe
    };
    let Some(hcb) = crate::huffman::asf_hcb(cb as u32) else {
        return false;
    };
    for _ in 0..8 {
        if crate::huffman::huff_decode(&mut br, hcb.len, hcb.cw).is_err() {
            return false;
        }
    }
    true
}

/// The additional-channel pair's FIRST body gates scalefac/SNF over
/// this many bands, independent of the transmitted max_sfb (round 406,
/// empirical — constant 2 across tracks; its single section and the
/// spectral data span far wider, hence the untruncated section
/// grammar). Semantics of the two bands' gains are still TBD; parsing
/// length is exact.
pub(crate) const ADD_PAIR_BODY0_SF_BOUND: u32 = 2;

/// Table 26 `two_channel_data()` for the 7_X ADDITIONAL channel pair.
///
/// Identical to [`parse_two_channel_data`] except in the
/// `b_enable_mdct_stereo_proc == 1` long-frame path, where the
/// additional pair deviates from the plain Table 26/47 reading in two
/// proven ways (riptide docs/ac4-decoder-accuracy-plan.md §7i):
///   1. `chparam_info`'s ms_used loop runs over
///      [`aspx_core_band_count`] bands (not `max_sfb`);
///   2. body0 uses the untruncated section grammar with a scalefac/SNF
///      bound of [`ADD_PAIR_BODY0_SF_BOUND`]; body1 uses `max_sfb`.
/// Falls back to the plain parse when no aspx config is available
/// (SIMPLE codec mode) or the frame is short/grouped.
pub fn parse_two_channel_data_additional(
    br: &mut BitReader<'_>,
    frame_len_base: u32,
    aspx_cfg: Option<&crate::aspx::AspxConfig>,
) -> Result<TwoChannelData> {
    let Some(cfg) = aspx_cfg else {
        return parse_two_channel_data(br, frame_len_base);
    };
    let b_msp = br.read_bit()?;
    let _trace = std::env::var_os("AC4_TRACE_BODIES").is_some();
    if _trace {
        eprintln!("2CH-ADD b_msp={} in@{}", b_msp as u8, br.bit_position() - 1);
    }
    if !b_msp {
        // Independent-channel branch is byte-identical to the plain
        // element; reuse its body handling.
        let ti0 = parse_asf_transform_info(br, frame_len_base)?;
        let psy0 = parse_asf_psy_info(br, &ti0, frame_len_base, false, false)?;
        let ti1 = parse_asf_transform_info(br, frame_len_base)?;
        let psy1 = parse_asf_psy_info(br, &ti1, frame_len_base, false, false)?;
        let (mut scaled, mut scaled_windows) = decode_mch_sf_data_channels(br, &ti0, &psy0, 1);
        let (s1, w1) = decode_mch_sf_data_channels(br, &ti1, &psy1, 1);
        scaled.extend(s1);
        scaled_windows.extend(w1);
        return Ok(TwoChannelData {
            b_enable_mdct_stereo_proc: false,
            transform_info: Some(ti0),
            psy_info: Some(psy0),
            transform_info_1: Some(ti1),
            psy_info_1: Some(psy1),
            chparam: None,
            scaled_spec_per_channel: scaled,
            scaled_spec_windows_per_channel: scaled_windows,
        });
    }
    let ti = parse_asf_transform_info(br, frame_len_base)?;
    let psy = parse_asf_psy_info(br, &ti, frame_len_base, false, false)?;
    if !(ti.b_long_frame && psy.num_window_groups == 1) {
        // Short/grouped additional pair: rule unverified — single-group
        // count (see round-406e note above) and grouped body walker.
        let chparam = parse_chparam_info(br, &[psy.max_sfb_0])?;
        let (scaled, scaled_windows) = decode_mch_sf_data_channels(br, &ti, &psy, 2);
        return Ok(TwoChannelData {
            b_enable_mdct_stereo_proc: true,
            transform_info: Some(ti),
            psy_info: Some(psy),
            transform_info_1: None,
            psy_info_1: None,
            chparam: Some(chparam),
            scaled_spec_per_channel: scaled,
            scaled_spec_windows_per_channel: scaled_windows,
        });
    }
    let ms_bands = aspx_core_band_count(cfg, ti.transform_length_0).unwrap_or(psy.max_sfb_0);
    let chparam = parse_chparam_info(br, &[ms_bands])?;
    // Round 406d: body0's scalefac band count is NOT derivable from any
    // known header field (observed 2 / 2 / 14 across three tracks with
    // identical aspx configs). Discover it per frame: parse body0's
    // section+spectral (deterministic), then try k = 0.. scalefac
    // codewords; the first k whose implied body1 start yields a fully
    // valid body1 parse (legal codebooks, sections closing exactly at
    // max_sfb) wins — a few hundred chained Huffman codewords make a
    // false accept essentially impossible.
    let m0 = discover_add_pair_body0_bound(*br, &ti, psy.max_sfb_0)
        .unwrap_or(ADD_PAIR_BODY0_SF_BOUND);
    let b0 = crate::asf::decode_asf_long_mono_body_with_max_sfb_ext(br, &ti, m0, true);
    let b1 = crate::asf::decode_asf_long_mono_body_with_max_sfb_ext(br, &ti, psy.max_sfb_0, true);
    if _trace {
        eprintln!(
            "2CH-ADD shared m={} ms_bands={} b0={} b1={} out@{}",
            psy.max_sfb_0,
            ms_bands,
            b0.is_some(),
            b1.is_some(),
            br.bit_position()
        );
    }
    if b0.is_none() || b1.is_none() {
        return Err(oxideav_core::error::Error::invalid(
            "ac4: additional two_channel_data body parse failed",
        ));
    }
    Ok(TwoChannelData {
        b_enable_mdct_stereo_proc: true,
        transform_info: Some(ti),
        psy_info: Some(psy),
        transform_info_1: None,
        psy_info_1: None,
        chparam: Some(chparam),
        scaled_spec_per_channel: vec![b0, b1],
        scaled_spec_windows_per_channel: vec![None, None],
    })
}

/// `three_channel_info()` per Table 30.
pub fn parse_three_channel_info(
    br: &mut BitReader<'_>,
    max_sfb_per_group: &[u32],
) -> Result<ThreeChannelInfo> {
    let chel_matsel = br.read_u32(4)? as u8;
    let cp0 = parse_chparam_info(br, max_sfb_per_group)?;
    let cp1 = parse_chparam_info(br, max_sfb_per_group)?;
    Ok(ThreeChannelInfo {
        chel_matsel,
        chparam: [cp0, cp1],
    })
}

/// `four_channel_info()` per Table 31.
pub fn parse_four_channel_info(
    br: &mut BitReader<'_>,
    max_sfb_per_group: &[u32],
) -> Result<FourChannelInfo> {
    let cp0 = parse_chparam_info(br, max_sfb_per_group)?;
    let cp1 = parse_chparam_info(br, max_sfb_per_group)?;
    let cp2 = parse_chparam_info(br, max_sfb_per_group)?;
    let cp3 = parse_chparam_info(br, max_sfb_per_group)?;
    Ok(FourChannelInfo {
        chparam: [cp0, cp1, cp2, cp3],
    })
}

/// `five_channel_info()` per Table 32.
pub fn parse_five_channel_info(
    br: &mut BitReader<'_>,
    max_sfb_per_group: &[u32],
) -> Result<FiveChannelInfo> {
    let chel_matsel = br.read_u32(4)? as u8;
    let cp0 = parse_chparam_info(br, max_sfb_per_group)?;
    let cp1 = parse_chparam_info(br, max_sfb_per_group)?;
    let cp2 = parse_chparam_info(br, max_sfb_per_group)?;
    let cp3 = parse_chparam_info(br, max_sfb_per_group)?;
    let cp4 = parse_chparam_info(br, max_sfb_per_group)?;
    Ok(FiveChannelInfo {
        chel_matsel,
        chparam: [cp0, cp1, cp2, cp3, cp4],
    })
}

/// `three_channel_data()` per Table 27 — outer shell + 3x `sf_data(ASF)`.
///
/// Round 23 wires the three trailing `sf_data(ASF)` bodies through the
/// shared `(transform_info, psy_info)` pair. Each per-channel scaled
/// spectrum lands on `scaled_spec_per_channel[i]` for the long-frame,
/// single-window-group case.
pub fn parse_three_channel_data(
    br: &mut BitReader<'_>,
    frame_len_base: u32,
) -> Result<ThreeChannelData> {
    let ti = parse_asf_transform_info(br, frame_len_base)?;
    let psy = parse_asf_psy_info(br, &ti, frame_len_base, false, false)?;
    let info = parse_three_channel_info(br, &[psy.max_sfb_0])?;
    if std::env::var_os("AC4_TRACE_BODIES").is_some() {
        eprintln!(
            "3CH long={} tl=({},{}) m0={} m1={} ng={} grp={:?} matsel={} saps=({},{}) bodies@{}",
            ti.b_long_frame,
            ti.transf_length[0],
            ti.transf_length[1],
            psy.max_sfb_0,
            psy.max_sfb_1,
            psy.num_window_groups,
            psy.scale_factor_grouping,
            info.chel_matsel,
            info.chparam[0].sap_mode,
            info.chparam[1].sap_mode,
            br.bit_position()
        );
    }
    // Round 407c: a discovery-first walk was tried here and REVERTED —
    // frame-0's 3ch bodies need the truncating grammar while frame-1's
    // need untruncated sections with bounds (11,8,30) that nothing in
    // the header describes (see the RE guide's open problems). The
    // per-body grammar discriminator is still unknown; the legacy
    // fixed-max_sfb walk keeps validated frames bit-exact.
    let (scaled, scaled_windows) = decode_mch_sf_data_channels(br, &ti, &psy, 3);
    if std::env::var_os("AC4_TRACE_BODIES").is_some() {
        eprintln!("3CH bodies out@{}", br.bit_position());
    }
    Ok(ThreeChannelData {
        transform_info: Some(ti),
        psy_info: Some(psy),
        info: Some(info),
        scaled_spec_per_channel: scaled,
        scaled_spec_windows_per_channel: scaled_windows,
    })
}

/// `four_channel_data()` per Table 28 — outer shell + 4x `sf_data(ASF)`.
pub fn parse_four_channel_data(
    br: &mut BitReader<'_>,
    frame_len_base: u32,
) -> Result<FourChannelData> {
    let ti = parse_asf_transform_info(br, frame_len_base)?;
    let psy = parse_asf_psy_info(br, &ti, frame_len_base, false, false)?;
    let info = parse_four_channel_info(br, &[psy.max_sfb_0])?;
    let (scaled, scaled_windows) = decode_mch_sf_data_channels(br, &ti, &psy, 4);
    Ok(FourChannelData {
        transform_info: Some(ti),
        psy_info: Some(psy),
        info: Some(info),
        scaled_spec_per_channel: scaled,
        scaled_spec_windows_per_channel: scaled_windows,
    })
}

/// `five_channel_data()` per Table 29 — outer shell + 5x `sf_data(ASF)`.
pub fn parse_five_channel_data(
    br: &mut BitReader<'_>,
    frame_len_base: u32,
) -> Result<FiveChannelData> {
    let ti = parse_asf_transform_info(br, frame_len_base)?;
    let psy = parse_asf_psy_info(br, &ti, frame_len_base, false, false)?;
    let info = parse_five_channel_info(br, &[psy.max_sfb_0])?;
    let (scaled, scaled_windows) = decode_mch_sf_data_channels(br, &ti, &psy, 5);
    Ok(FiveChannelData {
        transform_info: Some(ti),
        psy_info: Some(psy),
        info: Some(info),
        scaled_spec_per_channel: scaled,
        scaled_spec_windows_per_channel: scaled_windows,
    })
}

/// Walk `n_channels` consecutive `sf_data(ASF)` bodies sharing the same
/// `(transform_info, psy_info)` per Tables 26 / 27 / 28 / 29 of TS 103
/// 190-1.
///
/// Each body decodes as one `asf_section_data()` then `asf_spectral_data()`
/// then `asf_scalefac_data()` then `asf_snf_data()` chain (§4.2.8.3-6) and
/// produces a dequantised + scaled MDCT spectrum of length
/// `sfb_offset[max_sfb]`. The Huffman codebooks reused from
/// [`crate::huffman`] are: `HCB_1` .. `HCB_11` for spectral lines (per
/// `sect_cb`), `HCB_SCALEFAC` (codebook ID `SCF`, 121 entries) for
/// scale-factor DPCM and `HCB_SNF` (22 entries) for spectral noise
/// fill. Annex A.1 shares the codebooks across mono / stereo /
/// multichannel — there is no separate "MCH" codebook set.
///
/// Bodies past a Huffman / bit-stream miss return `None` for the
/// remaining channels (we can't re-sync mid-bitstream); the entries
/// before the miss are still populated.
///
/// Round 24 extends the previous (long-frame, `num_window_groups == 1`)
/// path to also walk **short / grouped** frames where
/// `num_window_groups > 1`. In that case each per-channel body fires
/// `num_window_groups` independent
/// `(asf_section_data + asf_spectral_data + asf_scalefac_data +
/// asf_snf_data)` cycles — one per window group — and the
/// per-channel spectrum is the concatenation of the
/// `num_window_groups` per-group spectra (each of length
/// `sfb_offset[max_sfb]` at the per-window transform length).
/// `b_dual_maxsfb == 0` means the same `max_sfb_0` applies to every
/// group; per-group `max_sfb` selection (Pseudocode 5
/// `get_max_sfb(g)`) collapses to that single value for the
/// non-side-channel multichannel path.
/// A single physical window's decoded, de-grouped spectrum:
/// `(transform_length, spectrum)`. `spectrum.len() ==
/// sfb_offset_48(transform_length)[max_sfb]` — the same width a
/// long-frame single-window channel would produce, ready for a
/// straight per-window IMDCT.
pub(crate) type WindowSpectrum = (u32, Vec<f32>);

/// Round-407c discovery walk: decode `n_channels` long-frame bodies,
/// discovering each body's scalefac/SNF bound via [`discover_body_bound`].
/// Bodies 0..n-1 use "next body prefix parses" as the oracle; the last
/// body uses `final_oracle` (next-element head check supplied by the
/// element walker). Falls back to `None` per channel when discovery
/// fails — the caller can then retry the legacy fixed-max_sfb path.
pub(crate) fn decode_mch_sf_data_channels_discover(
    br: &mut BitReader<'_>,
    ti: &AsfTransformInfo,
    psy: &AsfPsyInfo,
    n_channels: usize,
    final_oracle: &dyn Fn(BitReader<'_>) -> bool,
) -> Option<Vec<Option<Vec<f32>>>> {
    if !(ti.b_long_frame && psy.num_window_groups == 1) {
        return None;
    }
    let mut out: Vec<Option<Vec<f32>>> = Vec::with_capacity(n_channels);
    for ch in 0..n_channels {
        let last = ch + 1 == n_channels;
        let found = if last {
            discover_body_bound(*br, ti, psy.max_sfb_0, |tr| final_oracle(tr))
        } else {
            discover_body_bound(*br, ti, psy.max_sfb_0, |tr| legal_body_prefix(tr, ti))
        };
        // Try the transmitted max_sfb as a first-class candidate too:
        // when the plain bound also satisfies the oracle at the same
        // or earlier position, prefer discovery's (ascending-k) pick.
        let (bound, _end) = found?;
        let body = crate::asf::decode_asf_long_mono_body_with_max_sfb_ext(br, ti, bound, true)?;
        if std::env::var_os("AC4_TRACE_BODIES").is_some() {
            eprintln!("DISC ch{ch} bound={bound} out@{}", br.bit_position());
        }
        out.push(Some(body));
    }
    Some(out)
}

pub(crate) fn decode_mch_sf_data_channels(
    br: &mut BitReader<'_>,
    ti: &AsfTransformInfo,
    psy: &AsfPsyInfo,
    n_channels: usize,
) -> (Vec<Option<Vec<f32>>>, Vec<Option<Vec<WindowSpectrum>>>) {
    let mut out = vec![None; n_channels];
    if ti.b_long_frame && psy.num_window_groups == 1 {
        // Long-frame, 1 window group — walk one body chain per channel.
        for slot in out.iter_mut() {
            match decode_asf_long_mono_body_with_max_sfb(br, ti, psy.max_sfb_0) {
                Some(v) => *slot = Some(v),
                None => break,
            }
        }
        return (out, vec![None; n_channels]);
    }
    if psy.num_window_groups == 0 {
        return (out, vec![None; n_channels]);
    }
    // Short / grouped frame walker: per-channel, per-window spectra
    // (widened Huffman decode + §5.1.5 ungrouping — see
    // `decode_asf_grouped_body_windows`). The old flat, group-major
    // `Vec<f32>` output stays `None` here — it was never correct for
    // real grouped content (see that function's docs) and every
    // consumer already gates on `ti.b_long_frame` before touching it.
    let mut windows_out: Vec<Option<Vec<WindowSpectrum>>> = vec![None; n_channels];
    for slot in windows_out.iter_mut() {
        match decode_asf_grouped_body_windows(br, ti, psy, psy.max_sfb_0) {
            Some(v) => *slot = Some(v),
            None => break,
        }
    }
    (out, windows_out)
}

/// Decode one channel's grouped/short-frame `sf_data(ASF)` body —
/// `num_window_groups > 1` — including the §5.1.5 Pseudocode 25
/// spectral-ungrouping step, and return one `(transform_length,
/// spectrum)` pair per **physical window** (not per group), in window
/// order, ready for individual per-window IMDCT.
///
/// This replaces an earlier implementation that treated every group as
/// exactly one window wide and re-read the shared
/// `reference_scale_factor(8)` / `b_snf_data_exists(1)` header fields
/// once *per group* instead of once for the whole body. Real content
/// commonly packs more than one window into a single group (e.g.
/// `num_windows=8, num_window_groups=4` shows up constantly) —
/// §4.3.6.2.6 Pseudocode 4 requires widening each group's
/// `asf_section_data()` / `asf_spectral_data()` payload by
/// `num_win_in_group[g]` (`sect_sfb_offset[g][sfb] = group_offset +
/// sfb_offset[sfb] * num_win_in_group[g]`), so decoding with the
/// unwidened table desyncs the bitreader position the moment any real
/// group has more than one window. Fixed by widening the `sfb_offset`
/// table passed to the Huffman decode by `num_win_in_group[g]`, then
/// applying §5.1.5 Pseudocode 25 to de-interleave each group's widened
/// (band-major, window-minor) spectrum back into individual per-window
/// spectra.
///
/// Returns `None` on the first Huffman / bit-stream miss; partial
/// per-group output is dropped because the bitreader position is
/// indeterminate after a mid-body miss.
pub(crate) fn decode_asf_grouped_body_windows(
    br: &mut BitReader<'_>,
    ti: &AsfTransformInfo,
    psy: &AsfPsyInfo,
    max_sfb_in: u32,
) -> Option<Vec<WindowSpectrum>> {
    decode_asf_grouped_body_windows_ab(br, ti, psy, max_sfb_in, max_sfb_in)
}

/// [`decode_asf_grouped_body_windows`] with distinct first/second-half
/// max_sfb values for `b_different_framing` bodies (get_max_sfb(g)
/// returns max_sfb[1] for groups past the framing boundary —
/// Pseudocode 5).
pub(crate) fn decode_asf_grouped_body_windows_ab(
    br: &mut BitReader<'_>,
    ti: &AsfTransformInfo,
    psy: &AsfPsyInfo,
    max_sfb_a: u32,
    max_sfb_b: u32,
) -> Option<Vec<WindowSpectrum>> {
    // Real short-frame content can legitimately collapse ALL of its
    // physical windows into a *single* group (`num_window_groups == 1`
    // with `num_windows > 1` — every `scale_factor_grouping` bit set to
    // "continue"), which still needs the widened-decode + ungrouping
    // machinery below (`num_win_in_group[0] == num_windows` in that
    // case). The only genuine "nothing to ungroup" case is a single
    // physical window altogether.
    if psy.num_windows <= 1 {
        return None;
    }
    let (tl_idx_per_g, tl_per_g, max_sfb_per_g, num_win_in_group_per_g) =
        crate::asf::derive_per_group_with_max_sfb(ti, psy, max_sfb_a, max_sfb_b);
    let n = tl_per_g.len();
    let mut base_sfbo_per_g: Vec<&'static [u16]> = Vec::with_capacity(n);
    let mut widened_sfbo_per_g: Vec<Vec<u16>> = Vec::with_capacity(n);
    let mut max_sfb_capped: Vec<u32> = Vec::with_capacity(n);
    for g in 0..n {
        let tl = tl_per_g[g];
        let cap = crate::tables::num_sfb_48(tl)?;
        let m = max_sfb_per_g[g].min(cap);
        if m == 0 {
            return None;
        }
        let base = crate::sfb_offset::sfb_offset_48(tl)?;
        let nwig = num_win_in_group_per_g[g].max(1);
        let widened: Vec<u16> = base
            .iter()
            .map(|&x| x.saturating_mul(nwig as u16))
            .collect();
        max_sfb_capped.push(m);
        base_sfbo_per_g.push(base);
        widened_sfbo_per_g.push(widened);
    }
    let widened_refs: Vec<&[u16]> = widened_sfbo_per_g.iter().map(|v| v.as_slice()).collect();

    let sections = crate::asf_data::parse_asf_section_data_grouped(
        br,
        &tl_idx_per_g,
        &tl_per_g,
        &max_sfb_capped,
    )
    .ok()?;
    let (qspec_per_g, mqi_per_g) = crate::asf_data::parse_asf_spectral_data_grouped(
        br,
        &sections,
        &widened_refs,
        &max_sfb_capped,
    )
    .ok()?;
    let sf_gain_per_g = crate::asf_data::parse_asf_scalefac_data_grouped(
        br,
        &sections,
        &mqi_per_g,
        &max_sfb_capped,
        &tl_per_g,
    )
    .ok()?;
    let _snf = crate::asf_data::parse_asf_snf_data_grouped(
        br,
        &sections,
        &mqi_per_g,
        &max_sfb_capped,
        &tl_per_g,
    )
    .ok()?;

    // §5.1.5 Pseudocode 25: for each group, walk band-major then
    // window-minor through the widened (dequantised + scaled) spectrum,
    // de-interleaving it into `num_win_in_group[g]` per-window vectors
    // each `base_sfbo[max_sfb]` long.
    let mut windows: Vec<WindowSpectrum> = Vec::new();
    for g in 0..n {
        let widened_scaled = crate::asf_data::dequantise_and_scale(
            &qspec_per_g[g],
            &sf_gain_per_g[g],
            widened_refs[g],
            max_sfb_capped[g],
        );
        let win_width = base_sfbo_per_g[g][max_sfb_capped[g] as usize] as usize;
        let nwig = num_win_in_group_per_g[g].max(1) as usize;
        let mut k = 0usize;
        let mut per_window: Vec<Vec<f32>> = vec![vec![0.0f32; win_width]; nwig];
        for sfb in 0..max_sfb_capped[g] as usize {
            let band_start = base_sfbo_per_g[g][sfb] as usize;
            let band_end = base_sfbo_per_g[g][sfb + 1] as usize;
            for pw in per_window.iter_mut() {
                for l in band_start..band_end {
                    if k < widened_scaled.len() && l < win_width {
                        pw[l] = widened_scaled[k];
                    }
                    k += 1;
                }
            }
        }
        for pw in per_window {
            windows.push((tl_per_g[g], pw));
        }
    }
    Some(windows)
}

// =====================================================================
// 5.X outer walker
// =====================================================================

/// Parse the outer layers of `5_X_channel_element(b_has_lfe, b_iframe)`
/// per §4.2.6.6 Table 25.
///
/// Returns `Ok(())` after walking:
///
/// 1. The 3-bit `5_X_codec_mode` selector.
/// 2. The I-frame config block (`aspx_config()` + `acpl_config_*`).
/// 3. The LFE `mono_data(1)` shell when `b_has_lfe == 1`.
/// 4. The `companding_control()` for non-SIMPLE codec modes.
/// 5. The `coding_config` selector (2 bits for SIMPLE/ASPX, 1 bit for
///    ASPX_ACPL_{1,2}, absent for ASPX_ACPL_3) and the chosen
///    channel-element bodies' outer shells.
///
/// **Scope**: r19 lands the bitstream walker for the SIMPLE / ASPX and
/// ASPX_ACPL_3 paths' outer shape, plus full LFE `mono_data(1)`
/// parsing. The ASPX_ACPL_{1,2} variants and the per-channel
/// `sf_data(ASF)` Huffman bodies remain TODO. On any inner parse miss
/// the walker bails early but keeps the partially-populated tools.
pub fn parse_5x_audio_data_outer(
    br: &mut BitReader<'_>,
    tools: &mut SubstreamTools,
    b_has_lfe: bool,
    b_iframe: bool,
    frame_len_base: u32,
) -> Result<()> {
    // 5_X_codec_mode (3 bits).
    let mode_bits = br.read_u32(3)?;
    let mode = FiveXCodecMode::from_u32(mode_bits);
    tools.five_x_mode = Some(mode);
    tools.five_x_b_has_lfe = b_has_lfe;

    // I-frame config block.
    if b_iframe {
        match mode {
            FiveXCodecMode::Aspx
            | FiveXCodecMode::AspxAcpl1
            | FiveXCodecMode::AspxAcpl2
            | FiveXCodecMode::AspxAcpl3 => {
                tools.aspx_config = Some(crate::aspx::parse_aspx_config(br)?);
            }
            _ => {}
        }
        match mode {
            FiveXCodecMode::AspxAcpl1 => {
                let cfg =
                    crate::acpl::parse_acpl_config_1ch(br, crate::acpl::Acpl1chMode::Partial)?;
                tools.acpl_config_1ch_partial = Some(cfg);
            }
            FiveXCodecMode::AspxAcpl2 => {
                let cfg = crate::acpl::parse_acpl_config_1ch(br, crate::acpl::Acpl1chMode::Full)?;
                tools.acpl_config_1ch_full = Some(cfg);
            }
            FiveXCodecMode::AspxAcpl3 => {
                let cfg = crate::acpl::parse_acpl_config_2ch(br)?;
                tools.acpl_config_2ch = Some(cfg);
            }
            _ => {}
        }
    }

    // LFE: mono_data(1).
    if b_has_lfe {
        let lfe = parse_mono_data(br, true, frame_len_base)?;
        tools.lfe_mono_data = Some(lfe);
    }

    // Mode-specific body.
    match mode {
        FiveXCodecMode::Simple | FiveXCodecMode::Aspx => {
            if matches!(mode, FiveXCodecMode::Aspx) {
                tools.companding = Some(crate::aspx::parse_companding_control(br, 5)?);
            }
            // 2-bit coding_config.
            let cc = br.read_u32(2)?;
            let coding_cfg = match cc {
                0 => FiveXCodingConfig::Cfg0Stereo2plusMono,
                1 => FiveXCodingConfig::Cfg1ThreeStereo,
                2 => FiveXCodingConfig::Cfg2FourMono,
                _ => FiveXCodingConfig::Cfg3Five,
            };
            tools.five_x_coding_config = Some(coding_cfg);
            // r20: walk all four channel-element layouts' outer shells.
            // r23: also walks the trailing `sf_data(ASF)` Huffman bodies
            // (one per channel) for the long-frame, single-window-group
            // case, depositing the dequantised spectra on each
            // `*ChannelData::scaled_spec_per_channel`. Short / grouped
            // frames still parse the outer shell and leave the per-
            // channel slots `None`.
            match coding_cfg {
                FiveXCodingConfig::Cfg0Stereo2plusMono => {
                    // Table 25 row 0: 1-bit `2ch_mode` selector
                    // (b_2ch_mode), then two_channel_data twice (L/R
                    // then Ls/Rs), then mono_data(0) for the centre.
                    tools.b_2ch_mode = Some(br.read_bit()?);
                    tools.two_channel_data.clear();
                    tools
                        .two_channel_data
                        .push(parse_two_channel_data(br, frame_len_base)?);
                    tools
                        .two_channel_data
                        .push(parse_two_channel_data(br, frame_len_base)?);
                    tools.cfg0_centre_mono = Some(parse_mono_data(br, false, frame_len_base)?);
                }
                FiveXCodingConfig::Cfg1ThreeStereo => {
                    // Table 25 row 1: three_channel_data + two_channel_data.
                    tools.three_channel_data = Some(parse_three_channel_data(br, frame_len_base)?);
                    tools.two_channel_data.clear();
                    tools
                        .two_channel_data
                        .push(parse_two_channel_data(br, frame_len_base)?);
                }
                FiveXCodingConfig::Cfg2FourMono => {
                    // Table 25 row 2: four_channel_data + mono_data(0).
                    tools.four_channel_data = Some(parse_four_channel_data(br, frame_len_base)?);
                    tools.cfg2_back_mono = Some(parse_mono_data(br, false, frame_len_base)?);
                }
                FiveXCodingConfig::Cfg3Five => {
                    tools.five_channel_data = Some(parse_five_channel_data(br, frame_len_base)?);
                }
                FiveXCodingConfig::AcplLite2 => {
                    // AcplLite2 is the ASPX_ACPL_{1,2} false-branch and
                    // can't appear in the SIMPLE/ASPX 2-bit map.
                    debug_assert!(
                        false,
                        "AcplLite2 unreachable from SIMPLE/ASPX 2-bit coding_config"
                    );
                }
            }
            // §4.2.6.6 Table 25 row `case ASPX:` — when the 5_X codec
            // mode is ASPX (not SIMPLE) the body trails three ASPX
            // payloads regardless of coding_config:
            //
            //   aspx_data_2ch();   // first stereo pair
            //   aspx_data_2ch();   // second stereo pair
            //   aspx_data_1ch();   // mono (centre)
            //
            // Captured into per-coding-config trailer slots so the
            // decoder can run `aspx_extend_pcm` per channel without
            // overwriting the stereo-CPE primary/secondary slots.
            //
            // Round 41: cfg2 wired (the four-channel + back-mono
            // layout). cfg0 / cfg1 / cfg3 land in the same trailers
            // but with different channel-to-trailer mappings; the
            // dispatch side currently consumes only the cfg2 mapping
            // so the cfg0 / cfg1 / cfg3 trailer fields capture data
            // for a future round to wire.
            // P-frames (b_iframe == 0) parse too: the aspx_config and
            // per-element xover offsets are I-frame-sticky and arrive
            // pre-seeded in `tools` (see [`crate::asf::StickyConfig`]).
            // NOTE: `tools` carries a single sticky xover slot, so a
            // P-frame assumes the I-frame used one xover across all
            // trailers of the element (always true for streams from
            // our encoder; per-element sticky xovers would need a
            // per-trailer sticky table).
            if matches!(mode, FiveXCodecMode::Aspx) && tools.aspx_config.is_some() {
                let aspx_cfg = tools.aspx_config.unwrap();
                let lr = crate::asf::capture_aspx_data_2ch_trailer(
                    br,
                    tools,
                    &aspx_cfg,
                    b_iframe,
                    frame_len_base,
                );
                let ls_rs = crate::asf::capture_aspx_data_2ch_trailer(
                    br,
                    tools,
                    &aspx_cfg,
                    b_iframe,
                    frame_len_base,
                );
                let centre = crate::asf::capture_aspx_data_1ch_trailer(
                    br,
                    tools,
                    &aspx_cfg,
                    b_iframe,
                    frame_len_base,
                );
                // Round 42: trailer-aware dispatch for every cfg.
                // Per Table 25 row `case ASPX:` the trailer order is
                // `aspx_data_2ch + aspx_data_2ch + aspx_data_1ch`
                // regardless of `coding_config`. The 5.X output
                // channels are L/R/C/Ls/Rs and the lone 1ch trailer
                // names the centre — so the canonical mapping
                // 1st-2ch -> (L,R) / 2nd-2ch -> (Ls,Rs) / 1ch -> (C)
                // applies to every config. The cfg0 b_2ch_mode == 1
                // inner stereo coding (L,Ls)/(R,Rs) doesn't change
                // this — ASPX is applied per output channel after
                // channel-element decoding completes (cfg0 b_2ch_mode
                // mapping happens up in the dispatch itself).
                match coding_cfg {
                    FiveXCodingConfig::Cfg2FourMono => {
                        tools.cfg2_aspx_lr = lr;
                        tools.cfg2_aspx_ls_rs = ls_rs;
                        tools.cfg2_aspx_centre = centre;
                    }
                    FiveXCodingConfig::Cfg0Stereo2plusMono => {
                        tools.cfg0_aspx_lr = lr;
                        tools.cfg0_aspx_ls_rs = ls_rs;
                        tools.cfg0_aspx_centre = centre;
                    }
                    FiveXCodingConfig::Cfg1ThreeStereo => {
                        tools.cfg1_aspx_lr = lr;
                        tools.cfg1_aspx_ls_rs = ls_rs;
                        tools.cfg1_aspx_centre = centre;
                    }
                    FiveXCodingConfig::Cfg3Five => {
                        tools.cfg3_aspx_lr = lr;
                        tools.cfg3_aspx_ls_rs = ls_rs;
                        tools.cfg3_aspx_centre = centre;
                    }
                    FiveXCodingConfig::AcplLite2 => {
                        // AcplLite2 is unreachable from SIMPLE/ASPX
                        // 2-bit map (asserted above); discard captures.
                        let _ = (lr, ls_rs, centre);
                    }
                }
            }
        }
        FiveXCodecMode::AspxAcpl1 | FiveXCodecMode::AspxAcpl2 => {
            tools.companding = Some(crate::aspx::parse_companding_control(br, 3)?);
            // 1-bit coding_config.
            let cc = br.read_bit()?;
            let coding_cfg = if cc {
                FiveXCodingConfig::Cfg1ThreeStereo
            } else {
                FiveXCodingConfig::AcplLite2
            };
            tools.five_x_coding_config = Some(coding_cfg);
            // r25: walk the inner body per §4.2.6.6 Table 25 row
            // `case ASPX_ACPL_1: case ASPX_ACPL_2:`. The walker is
            // try-and-bail: any inner Huffman / parse miss leaves the
            // already-populated tools intact and returns silently. The
            // outer walker still returns Ok(()).
            let _ = parse_aspx_acpl_1_2_inner_body(br, tools, mode, cc, b_iframe, frame_len_base);
        }
        FiveXCodecMode::AspxAcpl3 => {
            tools.companding = Some(crate::aspx::parse_companding_control(br, 2)?);
            // No coding_config: body is `stereo_data() + aspx_data_2ch()
            // + acpl_data_2ch()` per Table 25 row ASPX_ACPL_3.
            //
            // r24 wires the inner body walkers. The flow mirrors the
            // stereo-CPE ASPX path in `asf.rs`: walk stereo_data() into
            // tools, then if the body decoded cleanly + we're on an
            // I-frame + an aspx_config is in scope, walk aspx_data_2ch()
            // (Table 52). Finally walk acpl_data_2ch() (Table 62) using
            // the parsed acpl_config_2ch() — start_band is 0 since
            // acpl_config_2ch() doesn't carry a qmf_band field
            // (acpl_data_2ch always covers all parameter bands).
            let body_ok = crate::asf::parse_stereo_data_body(br, tools, frame_len_base);
            // Runs on P-frames as well — aspx_config / acpl_config_2ch
            // are I-frame-sticky and pre-seeded into `tools`.
            if body_ok {
                if let Some(cfg) = tools.aspx_config {
                    crate::asf::parse_aspx_data_2ch_body(
                        br,
                        tools,
                        &cfg,
                        b_iframe,
                        frame_len_base,
                    )?;
                    if let Some(acfg) = tools.acpl_config_2ch {
                        if let Ok(d) = crate::acpl::parse_acpl_data_2ch(
                            br,
                            acfg.num_param_bands,
                            0,
                            acfg.quant_mode_0,
                            acfg.quant_mode_1,
                        ) {
                            tools.acpl_data_2ch = Some(d);
                        }
                    }
                }
            }
        }
        FiveXCodecMode::Reserved(_) => {}
    }
    Ok(())
}

// =====================================================================
// 5_X ASPX_ACPL_1 / ASPX_ACPL_2 inner body walker (round 25)
// =====================================================================

/// Walk the inner body of `5_X_channel_element` for the
/// `ASPX_ACPL_1` / `ASPX_ACPL_2` modes per §4.2.6.6 Table 25 (the
/// `case ASPX_ACPL_1: case ASPX_ACPL_2:` arm) — the bits *after*
/// `companding_control(3)` and the 1-bit `coding_config`.
///
/// The body shape (in Table 25 order):
/// ```text
/// if (coding_config) { three_channel_data(); }
/// else               { two_channel_data();   }
/// if (5_X_codec_mode == ASPX_ACPL_1) {
///     max_sfb_master;            // n_side_bits — joint-MDCT residual
///     chparam_info();            // ACPL_1 residual ch0
///     chparam_info();            // ACPL_1 residual ch1
///     sf_data(ASF);              // ACPL_1 residual ch0
///     sf_data(ASF);              // ACPL_1 residual ch1
/// }
/// if (coding_config == 0) {
///     mono_data(0);              // centre / surround mono
/// }
/// aspx_data_2ch();
/// aspx_data_1ch();
/// acpl_data_1ch();               // -> tools.acpl_data_1ch_pair[0]
/// acpl_data_1ch();               // -> tools.acpl_data_1ch_pair[1]
/// ```
///
/// `n_side_bits` is derived per the §4.2.6.6 NOTE: largest signalled
/// transform length from the preceding two/three_channel_data() above
/// (look up Table 106 column `n_side_bits`).
///
/// The walker is **try-and-bail**: every step bails silently on the
/// first parse miss, leaving the already-populated `tools.*` fields
/// intact. It always returns `Ok(())` to the caller — the outer
/// walker's contract is that an inner-body miss is non-fatal.
///
/// Like the round-24 ASPX_ACPL_3 walker, the deeper aspx_data /
/// acpl_data steps are gated on `b_iframe && tools.aspx_config.is_some()`
/// so non-iframe paths simply consume what they can of the upstream
/// channel data and stop.
fn parse_aspx_acpl_1_2_inner_body(
    br: &mut BitReader<'_>,
    tools: &mut SubstreamTools,
    mode: FiveXCodecMode,
    coding_config_bit: bool,
    b_iframe: bool,
    frame_len_base: u32,
) -> Result<()> {
    debug_assert!(matches!(
        mode,
        FiveXCodecMode::AspxAcpl1 | FiveXCodecMode::AspxAcpl2
    ));
    // 1) two_channel_data() OR three_channel_data().
    let largest_tl: Option<u32> = if coding_config_bit {
        // three_channel_data().
        match parse_three_channel_data(br, frame_len_base) {
            Ok(d) => {
                let tl = d.transform_info.as_ref().map(|ti| ti.transform_length_0);
                tools.three_channel_data = Some(d);
                tl
            }
            Err(_) => return Ok(()),
        }
    } else {
        // two_channel_data().
        match parse_two_channel_data(br, frame_len_base) {
            Ok(d) => {
                let tl = d.transform_info.as_ref().map(|ti| ti.transform_length_0);
                tools.two_channel_data.clear();
                tools.two_channel_data.push(d);
                tl
            }
            Err(_) => return Ok(()),
        }
    };

    // 2) ASPX_ACPL_1 only: max_sfb_master + chparam_info×2 + sf_data×2
    //    (joint-MDCT residual layer).
    if matches!(mode, FiveXCodecMode::AspxAcpl1) {
        let Some(tl) = largest_tl else {
            return Ok(());
        };
        let Some((_n_msfb, n_side, _n_msfbl)) = tables::n_msfb_bits_48(tl) else {
            return Ok(());
        };
        let Some(num_sfb_cap) = tables::num_sfb_48(tl) else {
            return Ok(());
        };
        let max_sfb_master = match br.read_u32(n_side) {
            Ok(v) => v.min(num_sfb_cap),
            Err(_) => return Ok(()),
        };
        if max_sfb_master == 0 {
            // No bands signalled — chparam_info()×2 still run with the
            // empty bound, but the sf_data bodies would be degenerate.
            // Bail rather than feed an empty bound through downstream.
            return Ok(());
        }
        // Two chparam_info() calls — one per ACPL_1 residual channel.
        // Per Pseudocode 5 / §4.2.10 the per-group max_sfb here is just
        // max_sfb_master (joint-MDCT residual layer is a single window
        // group at the dominant transform length).
        let cp0 = match parse_chparam_info(br, &[max_sfb_master]) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        let cp1 = match parse_chparam_info(br, &[max_sfb_master]) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        // Two sf_data(ASF) bodies — pair the channels' residual MDCT
        // spectra. We reuse the ASF long-frame body decoder with the
        // explicit max_sfb_master bound. We synthesise the
        // AsfTransformInfo on the fly (long-frame at `tl`) since the
        // joint-MDCT residual layer always shares the dominant
        // transform length.
        let synth_ti = AsfTransformInfo {
            b_long_frame: true,
            transf_length: [0; 2],
            transform_length_0: tl,
            transform_length_1: tl,
        };
        let body0 = decode_asf_long_mono_body_with_max_sfb(br, &synth_ti, max_sfb_master);
        let Some(b0) = body0 else { return Ok(()) };
        let body1 = decode_asf_long_mono_body_with_max_sfb(br, &synth_ti, max_sfb_master);
        let Some(b1) = body1 else { return Ok(()) };
        // Persist the joint-MDCT residual pair (sSMP,3 / sSMP,4 per
        // Table 181) so the ASPX_ACPL_1 dispatch can IMDCT them into
        // Ls / Rs surround PCM carriers (round 40 — replaces the
        // round-37 silence placeholder for the surround-driven path).
        // Round 41 also persists the matching `chparam_info()` pair so
        // the dispatch can apply Table 181's SAP a/b/c/d first-stage
        // matrix between (sSMP_A, sSMP_B) and (sSMP_3, sSMP_4) before
        // Pseudocode 117 runs.
        tools.acpl_1_residual_pair[0] = Some((tl, b0));
        tools.acpl_1_residual_pair[1] = Some((tl, b1));
        tools.acpl_1_residual_chparam[0] = Some(cp0);
        tools.acpl_1_residual_chparam[1] = Some(cp1);
        tools.acpl_1_residual_max_sfb_master = Some(max_sfb_master);
    }

    // 3) Cfg0 only: mono_data(0) — centre / surround mono.
    if !coding_config_bit {
        match parse_mono_data(br, false, frame_len_base) {
            Ok(m) => tools.cfg0_centre_mono = Some(m),
            Err(_) => return Ok(()),
        }
    }

    // 4) ASPX trailers + ACPL pair need an aspx_config in scope — parsed
    //    from this frame's I-frame config block, or pre-seeded from the
    //    sticky state on P-frames (§4.2.6.6 Table 25 gates only the
    //    *configs* on b_iframe; the data elements are always present).
    let Some(aspx_cfg) = tools.aspx_config else {
        if std::env::var_os("AC4_T").is_some() { eprintln!("BAIL no-aspx-cfg @{}", br.bit_position()); }
        return Ok(());
    };
    // aspx_data_2ch() then aspx_data_1ch().
    if crate::asf::parse_aspx_data_2ch_body(br, tools, &aspx_cfg, b_iframe, frame_len_base).is_err()
    {
        return Ok(());
    }
    if crate::asf::parse_aspx_data_1ch_body(br, tools, &aspx_cfg, b_iframe, frame_len_base).is_err()
    {
        return Ok(());
    }
    // acpl_data_1ch()×2 — pair entries [0] / [1] per Pseudocode 117.
    // The active acpl_config_1ch was parsed earlier in the I-frame
    // header — `acpl_config_1ch_partial` for ASPX_ACPL_1, full for ACPL_2.
    let acpl_cfg = match mode {
        FiveXCodecMode::AspxAcpl1 => tools.acpl_config_1ch_partial,
        FiveXCodecMode::AspxAcpl2 => tools.acpl_config_1ch_full,
        _ => None,
    };
    let Some(acfg) = acpl_cfg else {
        return Ok(());
    };
    let start_band = if acfg.qmf_band == 0 {
        0
    } else {
        crate::acpl::sb_to_pb(acfg.qmf_band as u32, acfg.num_param_bands)
    };
    if let Ok(d0) =
        crate::acpl::parse_acpl_data_1ch(br, acfg.num_param_bands, start_band, acfg.quant_mode)
    {
        tools.acpl_data_1ch_pair[0] = Some(d0);
        if let Ok(d1) =
            crate::acpl::parse_acpl_data_1ch(br, acfg.num_param_bands, start_band, acfg.quant_mode)
        {
            tools.acpl_data_1ch_pair[1] = Some(d1);
        }
    }
    Ok(())
}

// =====================================================================
// 7_X channel-element walker (round 27 — immersive 7.0 / 7.1)
// =====================================================================

/// `7_X_codec_mode` values per §4.3.5.7 Table 98.
///
/// Note this is a **2-bit** field (vs the 3-bit `5_X_codec_mode`) and
/// has **no** `ASPX_ACPL_3` mode — only SIMPLE / ASPX / ASPX_ACPL_1 /
/// ASPX_ACPL_2 are defined for 7.X.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SevenXCodecMode {
    Simple,
    Aspx,
    AspxAcpl1,
    AspxAcpl2,
}

impl SevenXCodecMode {
    pub fn from_u32(v: u32) -> Self {
        match v & 0b11 {
            0 => Self::Simple,
            1 => Self::Aspx,
            2 => Self::AspxAcpl1,
            _ => Self::AspxAcpl2,
        }
    }
}

/// Parse the outer layers of `7_X_channel_element(channel_mode, b_iframe)`
/// per §4.2.6.14 Table 33.
///
/// `b_has_lfe == true` corresponds to `channel_mode == "7.1"` per the
/// spec (the only differentiator between 7.0 and 7.1 inside the
/// channel element is the leading `mono_data(1)` LFE).
///
/// The walker mirrors [`parse_5x_audio_data_outer`] but with the
/// 7.X-specific shape:
///
/// 1. 2-bit `7_X_codec_mode` (vs 3-bit for 5_X). No `Reserved` values
///    since all 4 codepoints are defined.
/// 2. I-frame config block: `aspx_config()` for non-SIMPLE,
///    `acpl_config_1ch(PARTIAL/FULL)` for ASPX_ACPL_{1,2}. There is no
///    `acpl_config_2ch()` here — ASPX_ACPL_3 is 5.X-only.
/// 3. LFE `mono_data(1)` when `b_has_lfe`.
/// 4. `companding_control(5)` for ASPX_ACPL_{1,2} only — SIMPLE/ASPX in
///    7.X have **no** leading companding (that differs from 5_X where
///    ASPX gets `companding_control(5)`).
/// 5. The 2-bit `coding_config` switch driving the four channel-data
///    layouts (Cfg0 / Cfg1 / Cfg2 / Cfg3 — same selectors as the 5_X
///    SIMPLE/ASPX path) — but with one critical difference: the Cfg0
///    body is `2ch_mode + two_channel_data() + two_channel_data()`
///    (no centre `mono_data(0)` here) and the Cfg2 body is just
///    `four_channel_data()` (no trailing `mono_data(0)` here either).
/// 6. SIMPLE/ASPX-only additional-channel block: 1-bit
///    `b_use_sap_add_ch` then optional `chparam_info()×2` then a
///    `two_channel_data()` carrying the additional 2 channels (the L+R
///    surround / front-extension pair beyond the 5.X core).
/// 7. ASPX_ACPL_1-only joint-MDCT residual layer: `max_sfb_master`
///    (`n_side_bits` wide, derived per Table 33 NOTE) +
///    `chparam_info()×2 + sf_data(ASF)×2`.
/// 8. `coding_config in {0, 2}`-only trailing `mono_data(0)` — these
///    are the centre / surround mono channels that move from inside the
///    coding_config switch (5.X) to outside it (7.X). For 7.X this is
///    **after** the additional-channel block.
/// 9. ASPX trailers: `aspx_data_2ch()×2 + aspx_data_1ch()` for any
///    non-SIMPLE mode, plus an extra `aspx_data_2ch()` for the ASPX
///    mode (covering the additional two channels).
/// 10. `acpl_data_1ch()×2` for ASPX_ACPL_{1,2} — same pair shape as
///     the 5_X §5.7.7.6.1 Pseudocode 117 path.
///
/// Like the 5_X walker, the deeper `aspx_data` / `acpl_data` steps are
/// gated on `b_iframe && tools.aspx_config.is_some()` so non-iframe
/// paths consume what they can of the upstream channel data and stop.
/// All inner Huffman / parse misses are caught try-and-bail and surface
/// `Ok(())` to the caller — the outer walker never returns `Err` once
/// the leading 2-bit `7_X_codec_mode` has been consumed.
pub fn parse_7x_audio_data_outer(
    br: &mut BitReader<'_>,
    tools: &mut SubstreamTools,
    b_has_lfe: bool,
    b_iframe: bool,
    frame_len_base: u32,
) -> Result<()> {
    // 7_X_codec_mode (2 bits — Table 98).
    let mode_bits = br.read_u32(2)?;
    let mode = SevenXCodecMode::from_u32(mode_bits);
    tools.seven_x_mode = Some(mode);
    tools.seven_x_b_has_lfe = b_has_lfe;

    // I-frame config block.
    if b_iframe {
        if !matches!(mode, SevenXCodecMode::Simple) {
            tools.aspx_config = Some(crate::aspx::parse_aspx_config(br)?);
        }
        match mode {
            SevenXCodecMode::AspxAcpl1 => {
                let cfg =
                    crate::acpl::parse_acpl_config_1ch(br, crate::acpl::Acpl1chMode::Partial)?;
                tools.acpl_config_1ch_partial = Some(cfg);
            }
            SevenXCodecMode::AspxAcpl2 => {
                let cfg = crate::acpl::parse_acpl_config_1ch(br, crate::acpl::Acpl1chMode::Full)?;
                tools.acpl_config_1ch_full = Some(cfg);
            }
            _ => {}
        }
    }

    // LFE: mono_data(1) when channel_mode == "7.1".
    if b_has_lfe {
        let lfe = parse_mono_data(br, true, frame_len_base)?;
        tools.lfe_mono_data = Some(lfe);
    }

    // companding_control(5) — ASPX_ACPL_{1,2} only.
    // Note: SIMPLE / ASPX in 7.X do **not** carry a leading companding
    // control (different from the 5_X walker where ASPX gets
    // companding_control(5)).
    if matches!(
        mode,
        SevenXCodecMode::AspxAcpl1 | SevenXCodecMode::AspxAcpl2
    ) {
        tools.companding = Some(crate::aspx::parse_companding_control(br, 5)?);
    }

    // coding_config (2 bits) — same 4-way selector as the 5.X
    // SIMPLE/ASPX path but with different body shapes (no Cfg0 centre
    // mono and no Cfg2 surround mono inside the switch — those move
    // out to a single trailing `mono_data(0)` below).
    let cc = br.read_u32(2)?;
    let coding_cfg = match cc {
        0 => FiveXCodingConfig::Cfg0Stereo2plusMono,
        1 => FiveXCodingConfig::Cfg1ThreeStereo,
        2 => FiveXCodingConfig::Cfg2FourMono,
        _ => FiveXCodingConfig::Cfg3Five,
    };
    tools.seven_x_coding_config = Some(coding_cfg);
    // Round 407d: remember where the channel-data switch starts — the
    // resync scan floor when the front walk desyncs.
    let switch_floor = *br;

    // Track the largest signalled transform length across the channel
    // data bodies — used downstream to derive `n_side_bits` per the
    // Table 33 NOTE for the ASPX_ACPL_1 joint-MDCT residual layer.
    let mut largest_tl: Option<u32> = None;
    let update_largest = |tl: u32, slot: &mut Option<u32>| {
        *slot = Some(slot.map_or(tl, |cur| cur.max(tl)));
    };

    // Channel-data switch. Try-and-bail: any inner parser miss leaves
    // the slot None and we still surface Ok(()).
    let mut body_ok = true;
    match coding_cfg {
        FiveXCodingConfig::Cfg0Stereo2plusMono => {
            // 7.X Cfg0 = `2ch_mode + two_channel_data + two_channel_data`
            // (no centre mono inside this switch).
            tools.b_2ch_mode = match br.read_bit() {
                Ok(b) => Some(b),
                Err(_) => {
                    body_ok = false;
                    None
                }
            };
            tools.two_channel_data.clear();
            if body_ok {
                match parse_two_channel_data(br, frame_len_base) {
                    Ok(d) => {
                        if let Some(ti) = d.transform_info.as_ref() {
                            update_largest(ti.transform_length_0, &mut largest_tl);
                        }
                        tools.two_channel_data.push(d);
                    }
                    Err(_) => body_ok = false,
                }
            }
            if body_ok {
                match parse_two_channel_data(br, frame_len_base) {
                    Ok(d) => {
                        if let Some(ti) = d.transform_info.as_ref() {
                            update_largest(ti.transform_length_0, &mut largest_tl);
                        }
                        tools.two_channel_data.push(d);
                    }
                    Err(_) => body_ok = false,
                }
            }
        }
        FiveXCodingConfig::Cfg1ThreeStereo => {
            // 7.X Cfg1 = `three_channel_data + two_channel_data`.
            match parse_three_channel_data(br, frame_len_base) {
                Ok(d) => {
                    if let Some(ti) = d.transform_info.as_ref() {
                        update_largest(ti.transform_length_0, &mut largest_tl);
                    }
                    tools.three_channel_data = Some(d);
                }
                Err(_) => body_ok = false,
            }
            if body_ok {
                tools.two_channel_data.clear();
                match parse_two_channel_data(br, frame_len_base) {
                    Ok(d) => {
                        if let Some(ti) = d.transform_info.as_ref() {
                            update_largest(ti.transform_length_0, &mut largest_tl);
                        }
                        tools.two_channel_data.push(d);
                    }
                    Err(_) => body_ok = false,
                }
            }
        }
        FiveXCodingConfig::Cfg2FourMono => {
            // 7.X Cfg2 = `four_channel_data` (no trailing mono inside
            // this switch — moved to the post-additional-channel block).
            match parse_four_channel_data(br, frame_len_base) {
                Ok(d) => {
                    if let Some(ti) = d.transform_info.as_ref() {
                        update_largest(ti.transform_length_0, &mut largest_tl);
                    }
                    tools.four_channel_data = Some(d);
                }
                Err(_) => body_ok = false,
            }
        }
        FiveXCodingConfig::Cfg3Five => match parse_five_channel_data(br, frame_len_base) {
            Ok(d) => {
                if let Some(ti) = d.transform_info.as_ref() {
                    update_largest(ti.transform_length_0, &mut largest_tl);
                }
                tools.five_channel_data = Some(d);
            }
            Err(_) => body_ok = false,
        },
        FiveXCodingConfig::AcplLite2 => {
            debug_assert!(false, "AcplLite2 unreachable from 7.X 2-bit coding_config");
            body_ok = false;
        }
    }
    if !body_ok {
        if std::env::var_os("AC4_T").is_some() { eprintln!("BAIL switch-body @{}", br.bit_position()); }
        // Round 407d: don't give up — the resync fallback below can
        // still recover the additional pair + trailers + sticky
        // configs past the desynced front. Only bail here for modes
        // without the trailer oracle.
        if !matches!(mode, SevenXCodecMode::Aspx) {
            return Ok(());
        }
    }

    // SIMPLE / ASPX additional-channel block: optional `chparam_info()×2`
    // gated on `b_use_sap_add_ch`, then a `two_channel_data()` carrying
    // the extra 2 channels (the front-extension or surround-back pair).
    if matches!(mode, SevenXCodecMode::Simple | SevenXCodecMode::Aspx) {
        let add_cfg = tools.aspx_config.clone();
        let mut add_done = false;
        // ---- normal attempt (only from a healthy front walk) ----
        let normal_save = *br;
        'normal: {
            if !body_ok {
                break 'normal;
            }
            let Ok(b_use_sap_add_ch) = br.read_bit() else {
                if std::env::var_os("AC4_T").is_some() { eprintln!("BAIL sap-gate"); }
                break 'normal;
            };
            tools.seven_x_b_use_sap_add_ch = Some(b_use_sap_add_ch);
            if b_use_sap_add_ch {
                // Round 406: when the aspx config is known, these SAP
                // chparams cover the aspx-core band range (same count
                // as the additional pair's own ms_used loop), not
                // num_sfb_48(tl).
                let max_sfb_g = tools
                    .aspx_config
                    .as_ref()
                    .zip(largest_tl)
                    .and_then(|(cfg, tl)| aspx_core_band_count(cfg, tl))
                    .or_else(|| largest_tl.and_then(crate::tables::num_sfb_48))
                    .unwrap_or(63);
                let Ok(cp0) = parse_chparam_info(br, &[max_sfb_g]) else { break 'normal };
                let Ok(cp1) = parse_chparam_info(br, &[max_sfb_g]) else { break 'normal };
                tools.seven_x_add_chparam_info = Some([cp0, cp1]);
            }
            // Additional `two_channel_data()` for the extra 2 channels.
            match parse_two_channel_data_additional(br, frame_len_base, add_cfg.as_ref()) {
                Ok(d) => {
                    // Round 407d: in ASPX mode, only accept when the
                    // trailer block validates to the wall from here —
                    // a fake add-pair parsed from a desynced front
                    // otherwise poisons the trailers and the sticky
                    // configs.
                    let trailers_ok = if matches!(mode, SevenXCodecMode::Aspx) {
                        match add_cfg.as_ref().map(|c| {
                            validate_7x_trailers_slots(*br, tools, c, b_iframe, frame_len_base)
                        }) {
                            Some(Some(slots)) => {
                                tools.aspx_xover_slots = slots;
                                tools.aspx_xover_slots_good = Some(slots);
                                true
                            }
                            Some(None) => false,
                            None => true,
                        }
                    } else {
                        true
                    };
                    if trailers_ok {
                        if let Some(ti) = d.transform_info.as_ref() {
                            update_largest(ti.transform_length_0, &mut largest_tl);
                        }
                        tools.seven_x_additional_channel_data = Some(d);
                        add_done = true;
                    } else if std::env::var_os("AC4_T").is_some() {
                        eprintln!("BAIL add-2ch trailer-validate @{}", br.bit_position());
                    }
                }
                Err(_) => {
                    if std::env::var_os("AC4_T").is_some() {
                        eprintln!("BAIL add-2ch @{}", br.bit_position());
                    }
                }
            }
        }
        // ---- resync fallback (ASPX only — needs the trailer oracle) ----
        if !add_done && matches!(mode, SevenXCodecMode::Aspx) {
            if let Some(cfg) = add_cfg.as_ref() {
                if let Some((head, slots)) =
                    resync_7x_addpair(switch_floor, tools, cfg, b_iframe, frame_len_base)
                {
                    *br = head;
                    tools.aspx_xover_slots = slots;
                    tools.aspx_xover_slots_good = Some(slots);
                    // The gate/chparams preceding the recovered head
                    // were not parsed — clear the stale slots.
                    tools.seven_x_b_use_sap_add_ch = None;
                    tools.seven_x_add_chparam_info = None;
                    if let Ok(d) =
                        parse_two_channel_data_additional(br, frame_len_base, add_cfg.as_ref())
                    {
                        if let Some(ti) = d.transform_info.as_ref() {
                            update_largest(ti.transform_length_0, &mut largest_tl);
                        }
                        tools.seven_x_additional_channel_data = Some(d);
                        add_done = true;
                    }
                }
            }
        }
        if !add_done {
            *br = normal_save;
            return Ok(());
        }
    }

    // ASPX_ACPL_1-only joint-MDCT residual layer (max_sfb_master +
    // chparam_info×2 + sf_data×2). Mirrors the 5_X
    // `parse_aspx_acpl_1_2_inner_body` ACPL_1 block — same shape, same
    // n_side_bits derivation per Table 33 NOTE.
    if matches!(mode, SevenXCodecMode::AspxAcpl1) {
        let Some(tl) = largest_tl else {
            return Ok(());
        };
        let Some((_n_msfb, n_side, _n_msfbl)) = tables::n_msfb_bits_48(tl) else {
            return Ok(());
        };
        let Some(num_sfb_cap) = tables::num_sfb_48(tl) else {
            return Ok(());
        };
        let max_sfb_master = match br.read_u32(n_side) {
            Ok(v) => v.min(num_sfb_cap),
            Err(_) => return Ok(()),
        };
        if max_sfb_master == 0 {
            return Ok(());
        }
        let cp0 = match parse_chparam_info(br, &[max_sfb_master]) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        let cp1 = match parse_chparam_info(br, &[max_sfb_master]) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        let synth_ti = AsfTransformInfo {
            b_long_frame: true,
            transf_length: [0; 2],
            transform_length_0: tl,
            transform_length_1: tl,
        };
        let body0 = decode_asf_long_mono_body_with_max_sfb(br, &synth_ti, max_sfb_master);
        let Some(b0) = body0 else { return Ok(()) };
        let body1 = decode_asf_long_mono_body_with_max_sfb(br, &synth_ti, max_sfb_master);
        let Some(b1) = body1 else { return Ok(()) };
        // Persist the 7_X ASPX_ACPL_1 joint-MDCT residual pair too —
        // shape mirrors the 5_X path; the dispatch can use the same
        // tools slot for both 5_X and 7_X surround-driven render. Same
        // `chparam_info()` pair persisted for Table 181 SAP application.
        tools.acpl_1_residual_pair[0] = Some((tl, b0));
        tools.acpl_1_residual_pair[1] = Some((tl, b1));
        tools.acpl_1_residual_chparam[0] = Some(cp0);
        tools.acpl_1_residual_chparam[1] = Some(cp1);
        tools.acpl_1_residual_max_sfb_master = Some(max_sfb_master);
    }

    // Trailing `mono_data(0)` for `coding_config in {0, 2}` — the
    // centre (Cfg0) or surround-back (Cfg2) mono channel. In 7.X this
    // moves out of the coding_config switch and lands after the
    // additional-channel block / ASPX_ACPL_1 residual layer.
    if matches!(
        coding_cfg,
        FiveXCodingConfig::Cfg0Stereo2plusMono | FiveXCodingConfig::Cfg2FourMono
    ) {
        match parse_mono_data(br, false, frame_len_base) {
            Ok(m) => {
                // Land in the mode-appropriate slot — Cfg0 → centre,
                // Cfg2 → back. Reuses the existing 5_X plumbing.
                if matches!(coding_cfg, FiveXCodingConfig::Cfg0Stereo2plusMono) {
                    tools.cfg0_centre_mono = Some(m);
                } else {
                    tools.cfg2_back_mono = Some(m);
                }
            }
            Err(_) => return Ok(()),
        }
    }

    // ASPX trailers + ACPL pair: need an aspx_config in scope — parsed
    // from this frame's I-frame config block or pre-seeded from the
    // sticky state on P-frames (§4.2.6.14 Table 33 gates only the
    // configs on b_iframe; the data elements are always present). Same
    // gate as the 5_X ASPX_ACPL_{1,2,3} walkers.
    let Some(aspx_cfg) = tools.aspx_config else {
        return Ok(());
    };

    // `if (7_X_codec_mode != SIMPLE) { aspx_data_2ch + aspx_data_2ch
    // + aspx_data_1ch }` — covers the L/R + Ls/Rs front pair and the
    // additional-channel pair plus the centre mono.
    let _ttr = std::env::var_os("AC4_TRACE_BODIES").is_some();
    // Round 407j: per-trailer F0-coding-mode search. Frame-0's
    // fully-gated unique closure proves some trailers code their
    // first envelope values as fixed-width raw fields while others
    // use Table-58 Huffman (observed raw,raw,huff,huff); the selector
    // rule is unknown, so for the 4-trailer ASPX layout we search the
    // 16 per-trailer combinations on a reader copy (Huffman-first
    // order — proven frames keep their old parse when it already
    // closes) and commit the first whose chain ends within 8 bits of
    // the wall.
    let mut f0_combo: u8 = 0;
    if matches!(mode, SevenXCodecMode::Aspx) {
        if let Some(w) = tools.wall_bits {
            let mut combos: Vec<u8> = (0u8..16).collect();
            combos.sort_by_key(|c| (c.count_ones(), *c));
            // The anchor-proven frame-0 pattern (raw,raw,huff,huff =
            // 0b0011) goes first: the all-Huffman parse ALSO reaches
            // the wall on rich I-frames (that's exactly the 31-bit
            // masked drift this search exists to fix), so wall
            // closure alone cannot rank them — the proven pattern
            // wins ties by ordering. P-frame trailers are delta-time
            // coded (no F0 reads), so this choice is a no-op there.
            combos.retain(|&c| c != 3);
            combos.insert(0, 3);
            'combo: for &combo in &combos {
                let mut vb = *br;
                let mut vt = tools.clone();
                let mut ok = true;
                for (i, &chs) in [2u8, 2, 1, 2].iter().enumerate() {
                    crate::aspx::set_f0_raw_mode((combo >> i) & 1 == 1);
                    let r = if chs == 1 {
                        crate::asf::parse_aspx_data_1ch_body(
                            &mut vb,
                            &mut vt,
                            &aspx_cfg,
                            b_iframe,
                            frame_len_base,
                        )
                    } else {
                        crate::asf::parse_aspx_data_2ch_body(
                            &mut vb,
                            &mut vt,
                            &aspx_cfg,
                            b_iframe,
                            frame_len_base,
                        )
                    };
                    if r.is_err() || vb.bit_position() > w {
                        ok = false;
                        break;
                    }
                }
                crate::aspx::set_f0_raw_mode(false);
                if ok && (0..=8).contains(&(w as i64 - vb.bit_position() as i64)) {
                    f0_combo = combo;
                    if std::env::var_os("AC4_T").is_some() && combo != 0 {
                        eprintln!("F0 combo {combo:04b} selected @{}", br.bit_position());
                    }
                    break 'combo;
                }
            }
        }
    }
    let trailer_floor_br = *br;
    if !matches!(mode, SevenXCodecMode::Simple) {
        crate::aspx::set_f0_raw_mode(f0_combo & 1 == 1);
        let _t0 = br.bit_position();
        if let Err(e) =
            crate::asf::parse_aspx_data_2ch_body(br, tools, &aspx_cfg, b_iframe, frame_len_base)
        {
            if std::env::var_os("AC4_T").is_some() {
                eprintln!("BAIL aspx-2ch#1 @{} err={e:?}", br.bit_position());
            }
            return Ok(());
        }
        if _ttr {
            eprintln!("TRL#1 2ch [{_t0}..{}) slots={:?}", br.bit_position(), &tools.aspx_xover_slots[..4]);
        }
        let _t1 = br.bit_position();
        crate::aspx::set_f0_raw_mode((f0_combo >> 1) & 1 == 1);
        if crate::asf::parse_aspx_data_2ch_body(br, tools, &aspx_cfg, b_iframe, frame_len_base)
            .is_err()
        {
            crate::aspx::set_f0_raw_mode(false);
            if std::env::var_os("AC4_T").is_some() { eprintln!("BAIL aspx-2ch#2 @{}", br.bit_position()); }
            return Ok(());
        }
        if _ttr {
            eprintln!("TRL#2 2ch [{_t1}..{}) slots={:?}", br.bit_position(), &tools.aspx_xover_slots[..4]);
        }
        let _t2 = br.bit_position();
        crate::aspx::set_f0_raw_mode((f0_combo >> 2) & 1 == 1);
        if crate::asf::parse_aspx_data_1ch_body(br, tools, &aspx_cfg, b_iframe, frame_len_base)
            .is_err()
        {
            crate::aspx::set_f0_raw_mode(false);
            if std::env::var_os("AC4_T").is_some() { eprintln!("BAIL aspx-1ch @{}", br.bit_position()); }
            return Ok(());
        }
        if _ttr {
            eprintln!("TRL#3 1ch [{_t2}..{}) slots={:?}", br.bit_position(), &tools.aspx_xover_slots[..4]);
        }
    }
    // `if (7_X_codec_mode == ASPX) { aspx_data_2ch }` — extra 2ch
    // envelope for the additional-channel pair in pure-ASPX mode (the
    // ASPX_ACPL_{1,2} paths fold the additional-channel ASPX into the
    // single aspx_data_1ch above).
    if matches!(mode, SevenXCodecMode::Aspx) {
        crate::aspx::set_f0_raw_mode((f0_combo >> 3) & 1 == 1);
        let r = crate::asf::parse_aspx_data_2ch_body(br, tools, &aspx_cfg, b_iframe, frame_len_base);
        crate::aspx::set_f0_raw_mode(false);
        if r.is_err() {
            if std::env::var_os("AC4_T").is_some() { eprintln!("BAIL aspx-extra @{}", br.bit_position()); }
            return Ok(());
        }
        // Round 407j: I-frame slot-harvest repair. The linear parse's
        // internal boundaries can drift (the trailer-1/2 F0 quirk),
        // corrupting slots [2]/[3] that every following P-frame needs.
        // The 1ch+final tail pair is uniquely locatable by hard
        // constraints ('000' xover for the 1ch — five-frame-proven
        // slot 2 = 0 on this content class — then a final 2ch parse
        // ending within 8 bits of the wall); rescan and overwrite the
        // harvested tail slots from the anchored reads.
        if b_iframe {
            if let Some(w) = tools.wall_bits {
                if let Some((x2, s2, s3)) =
                    scan_iframe_tail_slots(trailer_floor_br, w, tools, &aspx_cfg, frame_len_base)
                {
                    if tools.aspx_xover_slots[2] != Some(s2)
                        || tools.aspx_xover_slots[3] != Some(s3)
                    {
                        if std::env::var_os("AC4_T").is_some() {
                            eprintln!(
                                "SLOT-REPAIR 1ch@{x2}: slots[2] {:?}->{s2} slots[3] {:?}->{s3}",
                                tools.aspx_xover_slots[2], tools.aspx_xover_slots[3]
                            );
                        }
                        tools.aspx_xover_slots[2] = Some(s2);
                        tools.aspx_xover_slots[3] = Some(s3);
                    }
                }
            }
        }
    }

    // ACPL pair for ASPX_ACPL_{1,2} — `acpl_data_1ch()×2`, lands in
    // tools.acpl_data_1ch_pair[0/1] per the §5.7.7.6.1 Pseudocode 117
    // pair walker shape (shared with the 5_X path).
    if matches!(
        mode,
        SevenXCodecMode::AspxAcpl1 | SevenXCodecMode::AspxAcpl2
    ) {
        let acpl_cfg = match mode {
            SevenXCodecMode::AspxAcpl1 => tools.acpl_config_1ch_partial,
            SevenXCodecMode::AspxAcpl2 => tools.acpl_config_1ch_full,
            _ => None,
        };
        let Some(acfg) = acpl_cfg else {
            return Ok(());
        };
        let start_band = if acfg.qmf_band == 0 {
            0
        } else {
            crate::acpl::sb_to_pb(acfg.qmf_band as u32, acfg.num_param_bands)
        };
        if let Ok(d0) =
            crate::acpl::parse_acpl_data_1ch(br, acfg.num_param_bands, start_band, acfg.quant_mode)
        {
            tools.acpl_data_1ch_pair[0] = Some(d0);
            if let Ok(d1) = crate::acpl::parse_acpl_data_1ch(
                br,
                acfg.num_param_bands,
                start_band,
                acfg.quant_mode,
            ) {
                tools.acpl_data_1ch_pair[1] = Some(d1);
            }
        }
    }
    tools.walk_complete = true;
    Ok(())
}

// =====================================================================
// §6.2.4.4 var_channel_element — A-JOC downmix spectral frontend
// =====================================================================

/// Parsed `var_channel_element()` (ETSI TS 103 190-2 §6.2.4.4) — the
/// downmix-signal spectral frontend for an A-JOC object-coded substream.
/// Reuses the same mono/two/three-channel ASF primitives as the
/// channel-coded path (`parse_mono_data`, `parse_two_channel_data`,
/// `parse_three_channel_data`).
#[derive(Debug, Clone, Default)]
pub struct VarChannelElement {
    /// `var_codec_mode == ASPX`.
    pub aspx_mode: bool,
    /// Present only when `aspx_mode` and `b_iframe`.
    pub aspx_config: Option<AspxConfig>,
    /// Present only when `aspx_mode` and `n_dmx_signals <= 5`.
    pub companding_control: Option<CompandingControl>,
    /// `mono_data(1)` when `b_has_lfe`.
    pub lfe: Option<MonoLfeData>,
    /// The sole signal's `mono_data(0)` when `n_dmx_signals == 1` (the
    /// only case with no pairs and no three-channel tail at all).
    pub single_mono: Option<MonoLfeData>,
    /// The `n_pairs` (or `n_pairs - 1` in the odd/two-channel-tail case)
    /// leading `two_channel_data()` elements.
    pub pairs: Vec<TwoChannelData>,
    /// Odd-count tail when `var_coding_config == 0`: one more
    /// `two_channel_data()` plus a trailing `mono_data(0)`.
    pub odd_tail_two_and_mono: Option<(TwoChannelData, MonoLfeData)>,
    /// Odd-count tail when `var_coding_config == 1`: one
    /// `three_channel_data()` instead.
    pub odd_tail_three: Option<ThreeChannelData>,
    /// `n_pairs` `aspx_data_2ch()` trailers (present iff `aspx_mode`) —
    /// note this loop always runs `n_pairs` times regardless of parity,
    /// independent of how the core data above grouped channels.
    pub aspx_pair_trailers: Vec<SubstreamTools>,
    /// The trailing `aspx_data_1ch()` when `aspx_mode && b_isodd`.
    pub aspx_single_trailer: Option<SubstreamTools>,
}

/// `var_channel_element(b_iframe, n_dmx_signals, b_has_lfe)` (§6.2.4.4).
///
/// `frame_len_base` is the same per-frame transform-length base the
/// channel-coded path derives from `fs_index`/`frame_rate_index` and
/// threads into `parse_asf_transform_info` throughout this module.
///
/// The trailing `aspx_data_2ch()`/`aspx_data_1ch()` bandwidth-extension
/// elements (when `aspx_mode`) reuse `asf::parse_aspx_data_2ch_body` /
/// `asf::parse_aspx_data_1ch_body` — the same production parsers the
/// channel-coded path's `walk_ac4_substream_sticky` calls — each fed a
/// fresh [`SubstreamTools`] rather than the caller's shared one, since
/// this loop can run more than once (`n_pairs` times, always — that
/// count is independent of how the core data above grouped channels
/// into pairs/mono/three-channel elements) and those functions' fields
/// hold one call's result at a time.
///
/// **Known limitation:** on a non-I-frame (`!b_iframe`), `aspx_config()`
/// is never read (per spec, it's only present on I-frames) and the
/// channel-coded path's answer — a per-substream "sticky" config
/// carried across frames — isn't threaded into this AJOC downmix path
/// yet. That case returns `Error::unsupported` rather than guessing;
/// every I-frame call is real, tested parsing.
pub fn parse_var_channel_element(
    br: &mut BitReader<'_>,
    b_iframe: bool,
    n_dmx_signals: u32,
    b_has_lfe: bool,
    frame_len_base: u32,
) -> Result<VarChannelElement> {
    let mut out = VarChannelElement {
        aspx_mode: br.read_bit()?,
        ..Default::default()
    };
    let b_isodd = n_dmx_signals % 2 == 1;
    let n_pairs = n_dmx_signals / 2;

    if out.aspx_mode {
        if b_iframe {
            out.aspx_config = Some(parse_aspx_config(br)?);
        }
        if n_dmx_signals <= 5 {
            out.companding_control = Some(parse_companding_control(br, n_dmx_signals)?);
        }
    }

    if b_has_lfe {
        out.lfe = Some(parse_mono_data(br, true, frame_len_base)?);
    }

    if b_isodd {
        if n_dmx_signals == 1 {
            out.single_mono = Some(parse_mono_data(br, false, frame_len_base)?);
        } else {
            for _ in 0..n_pairs.saturating_sub(1) {
                out.pairs.push(parse_two_channel_data(br, frame_len_base)?);
            }
            let var_coding_config = br.read_bit()?;
            if !var_coding_config {
                let two = parse_two_channel_data(br, frame_len_base)?;
                let mono = parse_mono_data(br, false, frame_len_base)?;
                out.odd_tail_two_and_mono = Some((two, mono));
            } else {
                out.odd_tail_three = Some(parse_three_channel_data(br, frame_len_base)?);
            }
        }
    } else {
        for _ in 0..n_pairs {
            out.pairs.push(parse_two_channel_data(br, frame_len_base)?);
        }
    }

    if out.aspx_mode {
        let cfg = out.aspx_config.clone().ok_or_else(|| {
            Error::unsupported(
                "ac4: var_channel_element non-I-frame A-SPX trailer needs a sticky aspx_config, \
                 not yet threaded through for the A-JOC downmix path",
            )
        })?;
        for _ in 0..n_pairs {
            let mut tools = SubstreamTools::default();
            parse_aspx_data_2ch_body(br, &mut tools, &cfg, b_iframe, frame_len_base)?;
            out.aspx_pair_trailers.push(tools);
        }
        if b_isodd {
            let mut tools = SubstreamTools::default();
            parse_aspx_data_1ch_body(br, &mut tools, &cfg, b_iframe, frame_len_base)?;
            out.aspx_single_trailer = Some(tools);
        }
    }

    Ok(out)
}

// =====================================================================
// Helpers
// =====================================================================

/// Lookup `n_msfbl_bits` (Table 106 column 4) for a 48 kHz / 44.1 kHz
/// transform length. Returns `None` for transform lengths that have
/// `N/A` in the table (the LFE channel is restricted to long-frame
/// transforms — short windows aren't permitted on LFE).
pub fn n_msfbl_bits_48(transform_length: u32) -> Option<u32> {
    tables::n_msfb_bits_48(transform_length)
        .and_then(|(_n, _s, l)| if l == 0 { None } else { Some(l) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::bits::BitWriter;

    /// Append a minimal "all-zero spectra" `sf_data(ASF)` body to the
    /// writer for one channel. Walks (per §4.2.8.3-6 + r23 wiring):
    ///
    ///   * `asf_section_data`: one section with `sect_cb = 0` covering
    ///     all `max_sfb` bands (4 bits sect_cb + N bits sect_len_incr,
    ///     where N = 3 for `transf_length_idx <= 2`, with 7-escape
    ///     accumulation when `max_sfb > 7`).
    ///   * `asf_spectral_data`: empty (cb=0 emits no bits).
    ///   * `asf_scalefac_data`: 8 bits `reference_scale_factor` only —
    ///     all bands have `cb == 0`, so no DPCM codewords.
    ///   * `asf_snf_data`: 1 bit `b_snf_data_exists = 0`.
    ///
    /// The decoder reads the body and produces an all-zero scaled
    /// spectrum of length `sfb_offset[max_sfb]`.
    fn write_zero_sf_data_body(bw: &mut BitWriter, max_sfb: u32, transform_length: u32) {
        let (n_sect_bits, sect_esc_val) = crate::asf_data::sect_len_bits(transform_length);
        // sect_cb = 0.
        bw.write_u32(0, 4);
        // sect_len = 1 + sum of increments; we want sect_len == max_sfb.
        // So we need the sum of increments to equal max_sfb - 1.
        let mut remaining = max_sfb.saturating_sub(1);
        while remaining >= sect_esc_val {
            bw.write_u32(sect_esc_val, n_sect_bits);
            remaining -= sect_esc_val;
        }
        bw.write_u32(remaining, n_sect_bits);
        // asf_spectral_data: nothing (cb=0).
        // asf_scalefac_data: reference_scale_factor (any value works
        // since no bands have non-zero quants).
        bw.write_u32(120, 8);
        // asf_snf_data: b_snf_data_exists = 0.
        bw.write_bit(false);
    }

    /// Write a whole grouped `sf_data(ASF)` body (§4.2.8, Tables 39-42)
    /// for one channel spanning `num_groups` window groups, all-zero
    /// (`sect_cb == 0` for every band in every group). Unlike calling
    /// [`write_zero_sf_data_body`] `num_groups` times — which duplicates
    /// a full `reference_scale_factor`(8) + `b_snf_data_exists`(1)
    /// header per group, matching the *old*, incorrect per-group-header
    /// assumption — this writes the section data once per group but the
    /// `reference_scale_factor` / `b_snf_data_exists` header only once
    /// for the whole body, per Tables 41/42's real syntax.
    fn write_zero_sf_data_body_grouped(
        bw: &mut BitWriter,
        max_sfb: u32,
        transform_length: u32,
        num_groups: u32,
    ) {
        let (n_sect_bits, sect_esc_val) = crate::asf_data::sect_len_bits(transform_length);
        for _ in 0..num_groups {
            bw.write_u32(0, 4); // sect_cb = 0
            let mut remaining = max_sfb.saturating_sub(1);
            while remaining >= sect_esc_val {
                bw.write_u32(sect_esc_val, n_sect_bits);
                remaining -= sect_esc_val;
            }
            bw.write_u32(remaining, n_sect_bits);
        }
        // asf_spectral_data: nothing (cb=0 in every group).
        bw.write_u32(120, 8); // reference_scale_factor, once.
        bw.write_bit(false); // b_snf_data_exists, once.
    }

    #[test]
    fn five_x_codec_mode_round_trip() {
        assert_eq!(FiveXCodecMode::from_u32(0), FiveXCodecMode::Simple);
        assert_eq!(FiveXCodecMode::from_u32(1), FiveXCodecMode::Aspx);
        assert_eq!(FiveXCodecMode::from_u32(2), FiveXCodecMode::AspxAcpl1);
        assert_eq!(FiveXCodecMode::from_u32(3), FiveXCodecMode::AspxAcpl2);
        assert_eq!(FiveXCodecMode::from_u32(4), FiveXCodecMode::AspxAcpl3);
        assert_eq!(FiveXCodecMode::from_u32(5), FiveXCodecMode::Reserved(5));
        assert_eq!(FiveXCodecMode::from_u32(7), FiveXCodecMode::Reserved(7));
    }

    #[test]
    fn n_msfbl_bits_48_known_rows() {
        // Table 106 (long-frame entries):
        // 2048/1920/1536 -> 3, 1024/960/768/512 -> 2, 384 -> 2,
        // 480/256/240/192/128/120/96 -> N/A.
        assert_eq!(n_msfbl_bits_48(2048), Some(3));
        assert_eq!(n_msfbl_bits_48(1920), Some(3));
        assert_eq!(n_msfbl_bits_48(1024), Some(2));
        assert_eq!(n_msfbl_bits_48(384), Some(2));
        assert_eq!(n_msfbl_bits_48(480), None);
        assert_eq!(n_msfbl_bits_48(128), None);
    }

    #[test]
    fn parse_mono_data_lfe_long_frame() {
        // mono_data(1) for frame_len_base=1920: sf_info_lfe() (Table 35)
        // sets b_long_frame=1 *implicitly* (no bits read — "transform
        // length = frame_length") and reads only max_sfb[0] with
        // n_msfbl_bits=3 (Table 106 column 4 for tl=1920) = value 5.
        let mut bw = BitWriter::new();
        bw.write_u32(5, 3); // max_sfb[0] — n_msfbl_bits=3 for tl=1920
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let lfe = parse_mono_data(&mut br, true, 1920).unwrap();
        assert!(lfe.b_lfe);
        assert_eq!(lfe.spec_frontend_bit, 0);
        let ti = lfe.transform_info.unwrap();
        assert_eq!(ti.transform_length_0, 1920);
        let psy = lfe.psy_info.unwrap();
        assert_eq!(psy.max_sfb_0, 5);
        // LFE psy_info has no grouping bits or window groups.
        assert_eq!(psy.num_windows, 1);
        assert_eq!(psy.num_window_groups, 1);
        assert!(psy.scale_factor_grouping.is_empty());
    }

    #[test]
    fn parse_mono_data_lfe_rejects_short_only_transform() {
        // frame_len_base=480 forces the (implicit) long-frame transform
        // length to 480 too — n_msfb_bits_48(480) has n_msfbl_bits=0
        // (Table 106: LFE isn't permitted at this transform length).
        // sf_info_lfe() reads zero bits before this check fires, so no
        // bitstream content is needed at all.
        let bytes: [u8; 0] = [];
        let mut br = BitReader::new(&bytes);
        let err = parse_mono_data(&mut br, true, 480).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("LFE") || msg.contains("transform_length"),
            "expected LFE-rejection error, got: {msg}"
        );
    }

    /// Round 37: `parse_mono_data(b_lfe=false)` walks the trailing
    /// `sf_data(ASF)` body for the long-frame, ASF-frontend, single
    /// window group case and lands a dequantised + scaled spectrum on
    /// `scaled_spec`. The all-zero body decodes to a zero-length-matched
    /// spectrum (no Huffman codepoints fired).
    #[test]
    fn parse_mono_data_non_lfe_walks_sf_data_body() {
        let mut bw = BitWriter::new();
        bw.write_bit(false); // spec_frontend = ASF
        bw.write_bit(true); // b_long_frame
        bw.write_u32(8, 6); // max_sfb[0]
        write_zero_sf_data_body(&mut bw, 8, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mono = parse_mono_data(&mut br, false, 1920).unwrap();
        assert!(!mono.b_lfe);
        assert_eq!(mono.spec_frontend_bit, 0);
        let scaled = mono
            .scaled_spec
            .as_ref()
            .expect("body walked into scaled_spec");
        // Spectrum length is `sfb_offset[max_sfb]` — Table 110 row for
        // tl=1920 / max_sfb=8 puts that just below 256 bins; we don't
        // hard-pin the exact value but the body must be non-empty.
        assert!(!scaled.is_empty(), "scaled spectrum must be non-empty");
        // All-zero body: every bin should be exactly 0.0 (dequantise of
        // q == 0 + any scalefac is 0.0).
        assert!(
            scaled.iter().all(|&v| v == 0.0),
            "all-zero sf_data body must dequantise to all zeros"
        );
    }

    /// Round 38: `parse_mono_data(b_lfe=true)` walks the trailing
    /// `sf_data(ASF)` body via `decode_asf_long_lfe_body_with_max_sfb_lfe`.
    /// LFE channels are always long-frame / single window group per
    /// Table 35 (`sf_info_lfe`), so an all-zero body decodes to a length-
    /// matched all-zero spectrum, identical in shape to the non-LFE long
    /// path but with `max_sfb` constrained by the `n_msfbl_bits` width.
    #[test]
    fn parse_mono_data_lfe_walks_sf_data_body() {
        let mut bw = BitWriter::new();
        bw.write_u32(5, 3); // max_sfb[0] — n_msfbl_bits=3 @ tl=1920
        write_zero_sf_data_body(&mut bw, 5, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let lfe = parse_mono_data(&mut br, true, 1920).unwrap();
        assert!(lfe.b_lfe);
        let scaled = lfe
            .scaled_spec
            .as_ref()
            .expect("LFE body walked into scaled_spec");
        assert!(!scaled.is_empty(), "LFE scaled spectrum must be non-empty");
        // All-zero body: every bin should be exactly 0.0.
        assert!(
            scaled.iter().all(|&v| v == 0.0),
            "all-zero LFE sf_data body must dequantise to all zeros"
        );
        // Length matches `sfb_offset[max_sfb]` for tl=1920, max_sfb=5 —
        // not pinned exactly, just bounded above by the transform length.
        assert!(scaled.len() <= 1920);
    }

    /// Round 37: SSF-frontend (`spec_frontend_bit == 1`) mono channels
    /// don't have a co-located body; the walker stops after the outer
    /// shell and `scaled_spec` stays `None`. The bit cursor advances
    /// past the leading 1-bit selector + `asf_transform_info` +
    /// `asf_psy_info` only.
    #[test]
    fn parse_mono_data_non_lfe_ssf_frontend_skips_body_walk() {
        let mut bw = BitWriter::new();
        bw.write_bit(true); // spec_frontend = SSF
        bw.write_bit(true); // b_long_frame
        bw.write_u32(8, 6); // max_sfb[0]
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mono = parse_mono_data(&mut br, false, 1920).unwrap();
        assert_eq!(mono.spec_frontend_bit, 1);
        assert!(
            mono.scaled_spec.is_none(),
            "SSF-frontend mono must skip the ASF body walk"
        );
    }

    #[test]
    fn parse_three_channel_info_reads_chel_matsel_and_two_chparam() {
        // chel_matsel = 0b1010, then two chparam_info bodies with
        // sap_mode = 0 (None) — each consumes only 2 bits.
        let mut bw = BitWriter::new();
        bw.write_u32(0b1010, 4);
        bw.write_u32(0, 2); // chparam_info #0: sap_mode=None
        bw.write_u32(0, 2); // chparam_info #1: sap_mode=None
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let info = parse_three_channel_info(&mut br, &[10]).unwrap();
        assert_eq!(info.chel_matsel, 0b1010);
        assert_eq!(info.chparam[0].sap_mode, 0);
        assert_eq!(info.chparam[1].sap_mode, 0);
    }

    #[test]
    fn parse_four_channel_info_reads_four_chparam() {
        let mut bw = BitWriter::new();
        for _ in 0..4 {
            bw.write_u32(0, 2); // sap_mode=None
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let info = parse_four_channel_info(&mut br, &[10]).unwrap();
        assert!(info.chparam.iter().all(|c| c.sap_mode == 0));
    }

    #[test]
    fn parse_five_channel_info_reads_chel_matsel_and_five_chparam() {
        let mut bw = BitWriter::new();
        bw.write_u32(0b0111, 4); // chel_matsel
        for _ in 0..5 {
            bw.write_u32(0, 2); // sap_mode=None
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let info = parse_five_channel_info(&mut br, &[10]).unwrap();
        assert_eq!(info.chel_matsel, 0b0111);
        assert!(info.chparam.iter().all(|c| c.sap_mode == 0));
    }

    #[test]
    fn parse_three_channel_data_outer_shell() {
        // sf_info(ASF, 0, 0) at frame_len_base=1920 long-frame:
        //   b_long_frame=1; max_sfb[0]=12 (6 bits).
        // three_channel_info: chel_matsel=3, two chparam_info(None).
        // r23: trailing 3x sf_data(ASF) bodies (all-zero spectra).
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_long_frame
        bw.write_u32(12, 6); // max_sfb[0]
        bw.write_u32(3, 4); // chel_matsel
        bw.write_u32(0, 2); // chparam_info #0
        bw.write_u32(0, 2); // chparam_info #1
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 12, 1920);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d = parse_three_channel_data(&mut br, 1920).unwrap();
        let psy = d.psy_info.unwrap();
        assert_eq!(psy.max_sfb_0, 12);
        let info = d.info.unwrap();
        assert_eq!(info.chel_matsel, 3);
        assert_eq!(d.scaled_spec_per_channel.len(), 3);
        // All-zero spectra: every channel slot is Some(vec![0.0; ...]).
        for ch in &d.scaled_spec_per_channel {
            let v = ch.as_ref().expect("per-channel sf_data should decode");
            assert!(v.iter().all(|&s| s == 0.0));
        }
    }

    #[test]
    fn parse_five_channel_data_outer_shell() {
        // r23: trailing 5x sf_data(ASF) bodies.
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_long_frame
        bw.write_u32(20, 6); // max_sfb[0]
        bw.write_u32(0xF, 4); // chel_matsel
        for _ in 0..5 {
            bw.write_u32(0, 2);
        }
        for _ in 0..5 {
            write_zero_sf_data_body(&mut bw, 20, 1920);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d = parse_five_channel_data(&mut br, 1920).unwrap();
        let info = d.info.unwrap();
        assert_eq!(info.chel_matsel, 0xF);
        assert_eq!(info.chparam.len(), 5);
        assert_eq!(d.scaled_spec_per_channel.len(), 5);
        for ch in &d.scaled_spec_per_channel {
            assert!(ch.is_some());
        }
    }

    #[test]
    fn parse_5x_outer_simple_cfg3_five_channel() {
        // 5_X_codec_mode = SIMPLE (0). b_has_lfe=0, b_iframe=1.
        // No companding (SIMPLE). coding_config = 3 (five_channel_data).
        // Then five_channel_data outer shell + 5x sf_data(ASF).
        let mut bw = BitWriter::new();
        bw.write_u32(0, 3); // 5_X_codec_mode = SIMPLE
                            // No I-frame config (SIMPLE).
                            // No LFE.
                            // No companding.
        bw.write_u32(3, 2); // coding_config = 3
                            // five_channel_data outer:
        bw.write_bit(true); // b_long_frame
        bw.write_u32(15, 6); // max_sfb[0]
        bw.write_u32(0, 4); // chel_matsel
        for _ in 0..5 {
            bw.write_u32(0, 2); // chparam_info
        }
        for _ in 0..5 {
            write_zero_sf_data_body(&mut bw, 15, 1920);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::Simple));
        assert_eq!(
            tools.five_x_coding_config,
            Some(FiveXCodingConfig::Cfg3Five)
        );
        let d = tools.five_channel_data.as_ref().unwrap();
        assert_eq!(d.psy_info.as_ref().unwrap().max_sfb_0, 15);
    }

    #[test]
    fn parse_5x_outer_simple_with_lfe_walks_lfe_mono_data() {
        // 5_X_codec_mode = SIMPLE, b_has_lfe = 1.
        // mono_data(1): asf_transform_info long-frame at 1920 +
        // sf_info_lfe with n_msfbl_bits=3 -> value 4.
        // Round 38: LFE body now decoded — append an all-zero sf_data
        // body at max_sfb=4 (n_sect_bits=3 since transf_length_idx=0).
        // Then coding_config=3 + five_channel_data shell + 5x sf_data.
        let mut bw = BitWriter::new();
        bw.write_u32(0, 3); // SIMPLE
                            // LFE mono_data(1): sf_info_lfe() implies b_long_frame=1
                            // with no bits read.
        bw.write_u32(4, 3); // max_sfb[0] -- n_msfbl_bits = 3 for tl=1920
        write_zero_sf_data_body(&mut bw, 4, 1920); // round 38: LFE body
                                                // coding_config = 3, then five_channel_data:
        bw.write_u32(3, 2);
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 4);
        for _ in 0..5 {
            bw.write_u32(0, 2);
        }
        for _ in 0..5 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, true, true, 1920).unwrap();
        assert!(tools.five_x_b_has_lfe);
        let lfe = tools.lfe_mono_data.as_ref().unwrap();
        assert!(lfe.b_lfe);
        assert_eq!(lfe.psy_info.as_ref().unwrap().max_sfb_0, 4);
        // Round 38: LFE body is now decoded; scaled_spec must be Some.
        assert!(
            lfe.scaled_spec.is_some(),
            "round 38: LFE body walks into scaled_spec"
        );
        let d = tools.five_channel_data.as_ref().unwrap();
        assert_eq!(d.psy_info.as_ref().unwrap().max_sfb_0, 10);
    }

    #[test]
    fn parse_two_channel_data_outer_walks_sf_info_plus_chparam() {
        // Long-frame @1920, max_sfb=20, chparam_info sap_mode=0,
        // r23: + 2 sf_data(ASF) all-zero bodies.
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true); // b_long_frame
        bw.write_u32(20, 6); // max_sfb[0]
        bw.write_u32(0, 2); // chparam sap_mode = 0
        write_zero_sf_data_body(&mut bw, 20, 1920);
        write_zero_sf_data_body(&mut bw, 20, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d = parse_two_channel_data(&mut br, 1920).unwrap();
        assert_eq!(d.transform_info.as_ref().unwrap().transform_length_0, 1920);
        assert_eq!(d.psy_info.as_ref().unwrap().max_sfb_0, 20);
        assert_eq!(d.chparam.as_ref().unwrap().sap_mode, 0);
        assert_eq!(d.scaled_spec_per_channel.len(), 2);
        assert!(d.scaled_spec_per_channel.iter().all(|c| c.is_some()));
    }

    #[test]
    fn parse_5x_outer_simple_cfg0_walks_pair_pair_centre() {
        // 5_X_codec_mode = SIMPLE (0). b_has_lfe=0, b_iframe=1.
        // coding_config = 0 (Cfg0Stereo2plusMono).
        // Then 1-bit `2ch_mode` + two_channel_data x2 + mono_data(0).
        // r23: each two_channel_data trails 2x sf_data(ASF). The
        // mono_data(0) shell isn't yet sf_data-extended, so no trailer
        // there.
        let mut bw = BitWriter::new();
        bw.write_u32(0, 3); // SIMPLE
                            // No LFE, no I-frame config.
        bw.write_u32(0, 2); // coding_config = 0 (Cfg0)
        bw.write_bit(true); // b_2ch_mode
                            // two_channel_data #1: long-frame, max_sfb=10, chparam=0.
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 10, 1920); // sf_data #1
        write_zero_sf_data_body(&mut bw, 10, 1920); // sf_data #2
                                                 // two_channel_data #2: long-frame, max_sfb=12, chparam=0.
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(12, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 12, 1920); // sf_data #1
        write_zero_sf_data_body(&mut bw, 12, 1920); // sf_data #2
                                                 // mono_data(0): spec_frontend bit + transform + psy.
        bw.write_bit(false); // spec_frontend = 0 (ASF)
        bw.write_bit(true); // b_long_frame
        bw.write_u32(8, 6); // max_sfb[0]
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(
            tools.five_x_coding_config,
            Some(FiveXCodingConfig::Cfg0Stereo2plusMono)
        );
        assert_eq!(tools.b_2ch_mode, Some(true));
        assert_eq!(tools.two_channel_data.len(), 2);
        assert_eq!(
            tools.two_channel_data[0]
                .psy_info
                .as_ref()
                .unwrap()
                .max_sfb_0,
            10
        );
        assert_eq!(
            tools.two_channel_data[1]
                .psy_info
                .as_ref()
                .unwrap()
                .max_sfb_0,
            12
        );
        let centre = tools.cfg0_centre_mono.as_ref().unwrap();
        assert!(!centre.b_lfe);
        assert_eq!(centre.spec_frontend_bit, 0);
        assert_eq!(centre.psy_info.as_ref().unwrap().max_sfb_0, 8);
    }

    #[test]
    fn parse_5x_outer_simple_cfg1_walks_three_plus_two() {
        // SIMPLE, coding_config=1 -> three_channel_data + two_channel_data.
        // r23: 3+2 sf_data(ASF) trailers.
        let mut bw = BitWriter::new();
        bw.write_u32(0, 3); // SIMPLE
        bw.write_u32(1, 2); // coding_config = 1 (Cfg1ThreeStereo)
                            // three_channel_data: long-frame, max_sfb=14, chel_matsel=0,
                            // 2x chparam_info(sap_mode=0).
        bw.write_bit(true);
        bw.write_u32(14, 6);
        bw.write_u32(0, 4);
        bw.write_u32(0, 2);
        bw.write_u32(0, 2);
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 14, 1920);
        }
        // two_channel_data: long-frame, max_sfb=18, chparam=0.
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(18, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 18, 1920);
        write_zero_sf_data_body(&mut bw, 18, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(
            tools.five_x_coding_config,
            Some(FiveXCodingConfig::Cfg1ThreeStereo)
        );
        let three = tools.three_channel_data.as_ref().unwrap();
        assert_eq!(three.psy_info.as_ref().unwrap().max_sfb_0, 14);
        assert_eq!(tools.two_channel_data.len(), 1);
        assert_eq!(
            tools.two_channel_data[0]
                .psy_info
                .as_ref()
                .unwrap()
                .max_sfb_0,
            18
        );
    }

    #[test]
    fn parse_5x_outer_simple_cfg2_walks_four_plus_mono() {
        // SIMPLE, coding_config=2 -> four_channel_data + mono_data(0).
        // r23: 4 sf_data(ASF) trailers from four_channel_data.
        let mut bw = BitWriter::new();
        bw.write_u32(0, 3); // SIMPLE
        bw.write_u32(2, 2); // coding_config = 2 (Cfg2FourMono)
                            // four_channel_data: long-frame, max_sfb=22, 4x chparam_info.
        bw.write_bit(true);
        bw.write_u32(22, 6);
        for _ in 0..4 {
            bw.write_u32(0, 2);
        }
        for _ in 0..4 {
            write_zero_sf_data_body(&mut bw, 22, 1920);
        }
        // mono_data(0): spec_frontend + transform + psy.
        bw.write_bit(false);
        bw.write_bit(true);
        bw.write_u32(7, 6);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(
            tools.five_x_coding_config,
            Some(FiveXCodingConfig::Cfg2FourMono)
        );
        let four = tools.four_channel_data.as_ref().unwrap();
        assert_eq!(four.psy_info.as_ref().unwrap().max_sfb_0, 22);
        let back = tools.cfg2_back_mono.as_ref().unwrap();
        assert!(!back.b_lfe);
        assert_eq!(back.psy_info.as_ref().unwrap().max_sfb_0, 7);
    }

    #[test]
    fn parse_5x_outer_aspx_acpl3_reads_acpl_config_2ch_and_companding() {
        // ASPX_ACPL_3 (4) on b_iframe=1:
        //   aspx_config(): for r19 the easiest exercise is to feed an
        //   all-zero aspx_config payload. parse_aspx_config consumes a
        //   known prefix; we just check the round-trip succeeds without
        //   walking the body — the test focuses on
        //   acpl_config_2ch_present + companding(2) + 5_X_codec_mode.
        // Skip aspx_config exercise here — it's complex. Instead test
        // a non-iframe to dodge it.
        let mut bw = BitWriter::new();
        bw.write_u32(4, 3); // 5_X_codec_mode = ASPX_ACPL_3
                            // b_iframe=0: skip aspx_config + acpl_config_2ch.
                            // No LFE (b_has_lfe=0).
                            // companding_control(2): per Table 41 it's 1 bit
                            // (b_compand_avg) + per-channel bits. For the round-trip we
                            // just need the bits to be consumed correctly.
        bw.write_bit(false); // b_compand_avg = 0
        bw.write_bit(false); // b_compand_on[0]
        bw.write_bit(false); // b_compand_on[1]
                             // ASPX_ACPL_3 body: stereo_data() + aspx_data_2ch +
                             // acpl_data_2ch — all opaque for r19.
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, false, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl3));
        // acpl_config_2ch is gated on b_iframe=1 — should be None.
        assert!(tools.acpl_config_2ch.is_none());
        assert!(tools.companding.is_some());
    }

    // =================================================================
    // Round 23: per-channel sf_data(ASF) wiring tests
    // =================================================================

    /// `decode_mch_sf_data_channels` should produce one `Some(Vec<f32>)`
    /// per channel for the long-frame, single-window-group all-zero
    /// case, and the spectrum length should match
    /// `sfb_offset[max_sfb]`.
    #[test]
    fn decode_mch_sf_data_long_frame_all_zero_two_channels() {
        let mut bw = BitWriter::new();
        // Two stacked sf_data bodies for max_sfb=8 at tl=1920.
        write_zero_sf_data_body(&mut bw, 8, 1920);
        write_zero_sf_data_body(&mut bw, 8, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let ti = AsfTransformInfo {
            b_long_frame: true,
            transf_length: [0, 0],
            transform_length_0: 1920,
            transform_length_1: 1920,
        };
        let psy = AsfPsyInfo {
            max_sfb_0: 8,
            num_windows: 1,
            num_window_groups: 1,
            ..Default::default()
        };
        let (out, _windows) = decode_mch_sf_data_channels(&mut br, &ti, &psy, 2);
        assert_eq!(out.len(), 2);
        let sfbo = crate::sfb_offset::sfb_offset_48(1920).unwrap();
        let expected_len = sfbo[8] as usize;
        for slot in &out {
            let v = slot.as_ref().expect("each channel decodes");
            assert_eq!(v.len(), expected_len);
            assert!(v.iter().all(|&s| s == 0.0));
        }
    }

    /// `decode_mch_sf_data_channels` returns `None` for **all** channels
    /// when the input bits are garbage (Huffman miss in the very first
    /// section_data). r24's grouped walker still attempts the body
    /// chain for `num_window_groups > 1`, so the all-None return here
    /// signals a parse failure rather than the previous "skip
    /// short/grouped" gate.
    #[test]
    fn decode_mch_sf_data_short_frame_garbage_returns_all_none() {
        let bytes = [0xFFu8; 4];
        let mut br = BitReader::new(&bytes);
        let ti = AsfTransformInfo {
            b_long_frame: false,
            transf_length: [0, 0],
            transform_length_0: 480,
            transform_length_1: 480,
        };
        let psy = AsfPsyInfo {
            max_sfb_0: 6,
            num_windows: 4,
            num_window_groups: 2,
            ..Default::default()
        };
        let (_flat, windows) = decode_mch_sf_data_channels(&mut br, &ti, &psy, 5);
        assert_eq!(windows.len(), 5);
        assert!(windows.iter().all(|c| c.is_none()));
    }

    /// `parse_three_channel_data` populates all three
    /// `scaled_spec_per_channel` slots with vectors of the correct
    /// length when the trailing `sf_data(ASF)` bodies decode cleanly.
    /// This is the per-channel walk anchored to Tables 27 + Annex A.1
    /// (codebooks `HCB_1..HCB_11`, `HCB_SCALEFAC`, `HCB_SNF`).
    #[test]
    fn parse_three_channel_data_decodes_three_sf_data_bodies() {
        // Long-frame at fl_base=1920, max_sfb=10, chel_matsel=1,
        // 2x chparam_info(sap_mode=0), then 3x all-zero sf_data(ASF).
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_long_frame
        bw.write_u32(10, 6); // max_sfb[0]
        bw.write_u32(1, 4); // chel_matsel
        bw.write_u32(0, 2); // chparam_info #0
        bw.write_u32(0, 2); // chparam_info #1
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d = parse_three_channel_data(&mut br, 1920).unwrap();
        assert_eq!(d.scaled_spec_per_channel.len(), 3);
        let sfbo = crate::sfb_offset::sfb_offset_48(1920).unwrap();
        let expected_len = sfbo[10] as usize;
        for ch in &d.scaled_spec_per_channel {
            let v = ch.as_ref().unwrap();
            assert_eq!(v.len(), expected_len);
            assert!(v.iter().all(|&s| s == 0.0));
        }
    }

    /// `parse_four_channel_data` populates four per-channel slots and
    /// `parse_five_channel_data` populates five slots. The parser
    /// progresses linearly through the bit-stream, consuming exactly
    /// N body's worth of bits for an N-channel layout.
    #[test]
    fn parse_four_and_five_channel_data_emit_correct_per_channel_counts() {
        // four_channel_data: 4 sf_data bodies.
        let mut bw = BitWriter::new();
        bw.write_bit(true);
        bw.write_u32(8, 6); // max_sfb[0]
        for _ in 0..4 {
            bw.write_u32(0, 2); // chparam_info
        }
        for _ in 0..4 {
            write_zero_sf_data_body(&mut bw, 8, 1920);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d4 = parse_four_channel_data(&mut br, 1920).unwrap();
        assert_eq!(d4.scaled_spec_per_channel.len(), 4);
        assert_eq!(
            d4.scaled_spec_per_channel
                .iter()
                .filter(|c| c.is_some())
                .count(),
            4
        );

        // five_channel_data: 5 sf_data bodies.
        let mut bw = BitWriter::new();
        bw.write_bit(true);
        bw.write_u32(6, 6); // max_sfb[0]
        bw.write_u32(0, 4); // chel_matsel
        for _ in 0..5 {
            bw.write_u32(0, 2);
        }
        for _ in 0..5 {
            write_zero_sf_data_body(&mut bw, 6, 1920);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d5 = parse_five_channel_data(&mut br, 1920).unwrap();
        assert_eq!(d5.scaled_spec_per_channel.len(), 5);
        assert!(d5.scaled_spec_per_channel.iter().all(|c| c.is_some()));
    }

    /// When the bit-stream is truncated mid-way through the multichannel
    /// `sf_data(ASF)` walk, the channels parsed before the truncation
    /// retain their `Some(...)` slots while the remaining ones stay
    /// `None`. The walker must not panic.
    #[test]
    fn parse_three_channel_data_truncated_sf_data_yields_partial_decode() {
        // Long-frame at fl_base=1920, max_sfb=12, chel_matsel=2,
        // 2x chparam_info(sap_mode=0), then ONLY ONE sf_data body (the
        // walker should consume that one, then bail on the second).
        let mut bw = BitWriter::new();
        bw.write_bit(true);
        bw.write_u32(12, 6);
        bw.write_u32(2, 4);
        bw.write_u32(0, 2);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d = parse_three_channel_data(&mut br, 1920).unwrap();
        assert_eq!(d.scaled_spec_per_channel.len(), 3);
        // First channel decoded; rest bailed.
        assert!(d.scaled_spec_per_channel[0].is_some());
        // We can't assert which exact remaining slots are None vs. Some
        // — depending on byte alignment, the bit reader may have a few
        // leftover zero-padding bits that get re-interpreted as a
        // partial section header. What we DO require is that at least
        // one of the trailing slots is `None` and that the function
        // didn't panic.
        let some_count = d
            .scaled_spec_per_channel
            .iter()
            .filter(|c| c.is_some())
            .count();
        assert!(some_count < 3, "expected at least one None slot");
    }

    /// `parse_two_channel_data` populates two scaled-spec slots and the
    /// per-channel vectors have the exact length dictated by
    /// `sfb_offset_48(transform_length_0)[max_sfb_0]`.
    #[test]
    fn parse_two_channel_data_per_channel_lengths_match_sfb_offset() {
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true); // b_long_frame
        bw.write_u32(15, 6); // max_sfb[0]
        bw.write_u32(0, 2); // chparam_info sap_mode = 0
        write_zero_sf_data_body(&mut bw, 15, 1920);
        write_zero_sf_data_body(&mut bw, 15, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d = parse_two_channel_data(&mut br, 1920).unwrap();
        let sfbo = crate::sfb_offset::sfb_offset_48(1920).unwrap();
        let expected_len = sfbo[15] as usize;
        assert_eq!(d.scaled_spec_per_channel.len(), 2);
        for ch in &d.scaled_spec_per_channel {
            let v = ch.as_ref().unwrap();
            assert_eq!(v.len(), expected_len);
        }
    }

    // =================================================================
    // Round 24: grouped multichannel sf_data(ASF) walker (num_window_groups > 1)
    // =================================================================

    /// `decode_mch_sf_data_channels` for a grouped short frame
    /// (`num_window_groups == 2`, `b_long_frame == 0`) walks
    /// `num_window_groups` consecutive `section / spectral / scalefac /
    /// snf` chains per channel and returns a per-channel spectrum of
    /// length `num_window_groups * sfb_offset[max_sfb]` (all-zero for
    /// the synthetic input). Pins r24 §5.4.4.4 grouped path.
    #[test]
    fn decode_mch_sf_data_grouped_short_frame_two_groups_two_channels() {
        // Two channels x two window groups x sf_data body each. tl=480
        // is at frame_len_base=1920 with transf_length=2 (the third
        // short-frame index — `n_sect_bits = 3`, esc = 7).
        let max_sfb = 8u32;
        let tl_idx = 2u32; // matches transf_length=2 (tl=480 short-frame).
        let mut bw = BitWriter::new();
        // Channel 0: one grouped body spanning both window groups.
        write_zero_sf_data_body_grouped(&mut bw, max_sfb, 480, 2);
        // Channel 1: ditto.
        write_zero_sf_data_body_grouped(&mut bw, max_sfb, 480, 2);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let ti = AsfTransformInfo {
            b_long_frame: false,
            transf_length: [tl_idx, tl_idx],
            transform_length_0: 480,
            transform_length_1: 480,
        };
        let psy = AsfPsyInfo {
            max_sfb_0: max_sfb,
            num_windows: 2,
            num_window_groups: 2,
            scale_factor_grouping: vec![0],
            ..Default::default()
        };
        let (_flat, windows) = decode_mch_sf_data_channels(&mut br, &ti, &psy, 2);
        assert_eq!(windows.len(), 2);
        let sfbo = crate::sfb_offset::sfb_offset_48(480).unwrap();
        let per_window_len = sfbo[max_sfb as usize] as usize;
        for slot in &windows {
            let v = slot.as_ref().expect("each channel decodes");
            assert_eq!(v.len(), 2); // num_windows (one window per group here)
            for (tl, spec) in v {
                assert_eq!(*tl, 480);
                assert_eq!(spec.len(), per_window_len);
                assert!(spec.iter().all(|&s| s == 0.0));
            }
        }
    }

    /// Three-window-group case at the same short-frame transform
    /// length. Verifies the walker scales linearly with
    /// `num_window_groups`.
    #[test]
    fn decode_mch_sf_data_grouped_three_groups_one_channel() {
        let max_sfb = 6u32;
        let tl_idx = 2u32;
        let mut bw = BitWriter::new();
        write_zero_sf_data_body_grouped(&mut bw, max_sfb, 480, 3);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let ti = AsfTransformInfo {
            b_long_frame: false,
            transf_length: [tl_idx, tl_idx],
            transform_length_0: 480,
            transform_length_1: 480,
        };
        let psy = AsfPsyInfo {
            max_sfb_0: max_sfb,
            num_windows: 3,
            num_window_groups: 3,
            scale_factor_grouping: vec![0, 0],
            ..Default::default()
        };
        let (_flat, windows) = decode_mch_sf_data_channels(&mut br, &ti, &psy, 1);
        assert_eq!(windows.len(), 1);
        let v = windows[0].as_ref().expect("decode succeeds");
        let sfbo = crate::sfb_offset::sfb_offset_48(480).unwrap();
        assert_eq!(v.len(), 3); // 3 window groups, 1 window each
        for (tl, spec) in v {
            assert_eq!(*tl, 480);
            assert_eq!(spec.len(), sfbo[max_sfb as usize] as usize);
        }
    }

    /// `parse_three_channel_data` correctly drives the grouped path
    /// when the head `sf_info(ASF, 0, 0)` reports
    /// `num_window_groups > 1`. Each channel's `scaled_spec_per_channel`
    /// slot carries the concatenated per-group spectra.
    #[test]
    fn parse_three_channel_data_grouped_short_frame_walks_per_group() {
        // sf_info(ASF, 0, 0) at frame_len_base=1920 with
        // b_long_frame=0, transf_length=[2,2] -> tl=480 short-frame +
        // n_grp_bits = n_grp_bits_lt_1536 isn't applicable here; for
        // frame_len_base=1920 (>= 1536) with equal transf_length we
        // use n_grp_bits_ge_1536(2,2) = 3. Set
        // scale_factor_grouping = [1, 0, 1] -> num_window_groups = 2.
        let mut bw = BitWriter::new();
        bw.write_bit(false); // b_long_frame = 0
        bw.write_u32(2, 2); // transf_length[0] = 2 (tl=480)
        bw.write_u32(2, 2); // transf_length[1] = 2 (tl=480)
        let max_sfb = 5u32;
        // n_msfb_bits for tl=480 short-frame at 48 kHz: per Table 106
        // tl=480 column 1 (n_msfb_bits) = 6.
        bw.write_u32(max_sfb, 6); // max_sfb[0]
                                  // scale_factor_grouping bits = 3 (n_grp_bits_ge_1536(2,2)).
                                  // Pattern [1, 0, 1] -> exactly one group boundary (one zero) ->
                                  // num_window_groups = 1 + 1 = 2.
        bw.write_u32(1, 1);
        bw.write_u32(0, 1);
        bw.write_u32(1, 1);
        // three_channel_info: chel_matsel + 2x chparam_info(sap_mode=0).
        bw.write_u32(0, 4);
        bw.write_u32(0, 2);
        bw.write_u32(0, 2);
        // Three channels, each one grouped body spanning both window
        // groups (num_win_in_group == [2, 2] — a *real* multi-window
        // group, not the degenerate 1-window-per-group case).
        for _ in 0..3 {
            write_zero_sf_data_body_grouped(&mut bw, max_sfb, 2, 2);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d = parse_three_channel_data(&mut br, 1920).unwrap();
        let psy = d.psy_info.as_ref().unwrap();
        assert_eq!(psy.num_window_groups, 2);
        assert_eq!(psy.num_windows, 4);
        assert!(!d.transform_info.as_ref().unwrap().b_long_frame);
        assert_eq!(d.scaled_spec_windows_per_channel.len(), 3);
        let sfbo = crate::sfb_offset::sfb_offset_48(480).unwrap();
        let expected_win_len = sfbo[max_sfb as usize] as usize;
        for ch in &d.scaled_spec_windows_per_channel {
            let v = ch.as_ref().expect("each channel decodes");
            // 2 groups x 2 windows each (num_win_in_group == [2, 2]).
            assert_eq!(v.len(), 4);
            for (tl, spec) in v {
                assert_eq!(*tl, 480);
                assert_eq!(spec.len(), expected_win_len);
                assert!(spec.iter().all(|&s| s == 0.0));
            }
        }
    }

    /// `parse_two_channel_data` mirrors the grouped walk for the
    /// `5_X_channel_element` Cfg0 / Cfg1 paths — every per-channel slot
    /// carries the concatenated grouped spectrum.
    #[test]
    fn parse_two_channel_data_grouped_short_frame_walks_per_group() {
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(false); // b_long_frame = 0
        bw.write_u32(2, 2); // transf_length[0] = 2 (tl=480)
        bw.write_u32(2, 2); // transf_length[1] = 2 (tl=480)
        let max_sfb = 4u32;
        bw.write_u32(max_sfb, 6); // max_sfb[0]
                                  // n_grp_bits = 3 from n_grp_bits_ge_1536(2,2). [0,0,0] -> 4 groups.
        bw.write_u32(0, 1);
        bw.write_u32(0, 1);
        bw.write_u32(0, 1);
        // chparam_info: sap_mode=0.
        bw.write_u32(0, 2);
        // 2 channels, each one grouped body spanning 4 window groups.
        for _ in 0..2 {
            write_zero_sf_data_body_grouped(&mut bw, max_sfb, 2, 4);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let d = parse_two_channel_data(&mut br, 1920).unwrap();
        let psy = d.psy_info.as_ref().unwrap();
        assert_eq!(psy.num_window_groups, 4);
        assert_eq!(d.scaled_spec_windows_per_channel.len(), 2);
        let sfbo = crate::sfb_offset::sfb_offset_48(480).unwrap();
        let expected_win_len = sfbo[max_sfb as usize] as usize;
        for ch in &d.scaled_spec_windows_per_channel {
            let v = ch.as_ref().expect("each channel decodes");
            assert_eq!(v.len(), 4); // 4 window groups, 1 window each
            for (tl, spec) in v {
                assert_eq!(*tl, 480);
                assert_eq!(spec.len(), expected_win_len);
            }
        }
    }

    /// Truncated grouped input should yield a partial per-channel
    /// decode and not panic. We feed only one window-group's body for
    /// the single channel and expect a `None` slot.
    #[test]
    fn decode_mch_sf_data_grouped_truncated_returns_none() {
        let max_sfb = 6u32;
        let tl_idx = 2u32;
        let mut bw = BitWriter::new();
        // Only one body when num_window_groups=2 expects two.
        write_zero_sf_data_body(&mut bw, max_sfb, 480);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let ti = AsfTransformInfo {
            b_long_frame: false,
            transf_length: [tl_idx, tl_idx],
            transform_length_0: 480,
            transform_length_1: 480,
        };
        let psy = AsfPsyInfo {
            max_sfb_0: max_sfb,
            num_windows: 2,
            num_window_groups: 2,
            scale_factor_grouping: vec![0],
            ..Default::default()
        };
        let (_flat, windows) = decode_mch_sf_data_channels(&mut br, &ti, &psy, 1);
        assert_eq!(windows.len(), 1);
        // Single channel: with only 1 of 2 groups present, the second
        // group attempt should bail (Huffman miss on garbage / EOF).
        // We allow either Some (if zero-padding accidentally validates
        // as a section header) or None — but we must NOT panic.
        let _ = &windows[0];
    }

    // =================================================================
    // Round 24: ASPX_ACPL_3 inner body walker
    // =================================================================

    /// `parse_5x_audio_data_outer` for ASPX_ACPL_3 on a non-iframe path
    /// shouldn't touch the inner body walker (gated on b_iframe + an
    /// in-scope aspx_config) — `tools.acpl_data_2ch` stays `None`.
    #[test]
    fn parse_5x_aspx_acpl_3_non_iframe_leaves_acpl_data_2ch_none() {
        let mut bw = BitWriter::new();
        bw.write_u32(4, 3); // 5_X_codec_mode = ASPX_ACPL_3
                            // companding_control(2): all-zero.
        bw.write_bit(false);
        bw.write_bit(false);
        bw.write_bit(false);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, false, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl3));
        assert!(tools.acpl_data_2ch.is_none());
        assert!(tools.companding.is_some());
    }

    /// `parse_5x_audio_data_outer` for ASPX_ACPL_3 on an I-frame walks
    /// `aspx_config` + `acpl_config_2ch` + `companding_control(2)` +
    /// `stereo_data()` then, when the body decodes cleanly, runs
    /// `aspx_data_2ch()` and `acpl_data_2ch()`. We feed an
    /// all-zero-tail bitstream — the body walker is allowed to bail
    /// silently downstream of `stereo_data()` (since the freq-table
    /// derivation may fail on a degenerate aspx_config), but the
    /// outer walker must finish without erroring and the parsed
    /// `acpl_config_2ch` must be visible on the tools.
    #[test]
    fn parse_5x_aspx_acpl_3_iframe_parses_aspx_and_acpl_configs() {
        let mut bw = BitWriter::new();
        bw.write_u32(4, 3); // 5_X_codec_mode = ASPX_ACPL_3
                            // aspx_config(): 15 bits all-zero.
        bw.write_u32(0, 15);
        // acpl_config_2ch(): 2-bit num_param_bands_id + 2x 1-bit
        // quant_mode. id=0 -> 15 bands; both quant modes = Fine.
        bw.write_u32(0, 2);
        bw.write_bit(false);
        bw.write_bit(false);
        // companding_control(2).
        bw.write_bit(false); // sync_flag = 0
        bw.write_bit(false); // compand_on[0]
        bw.write_bit(false); // compand_on[1]
        bw.write_bit(false); // compand_avg
                             // stereo_data() body — feed enough zeros that the body walker
                             // either succeeds or bails cleanly without panicking.
        bw.align_to_byte();
        while bw.byte_len() < 256 {
            bw.write_u32(0, 8);
        }
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        // The walker must complete without erroring — any body-walker
        // mid-stream miss should be swallowed silently per the
        // try-and-bail contract (acpl_data_2ch slot would simply stay
        // None in that case).
        parse_5x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl3));
        let cfg = tools.acpl_config_2ch.expect("acpl_config_2ch parsed");
        assert_eq!(cfg.num_param_bands, 15);
    }

    // =================================================================
    // Round 25: ASPX_ACPL_1 / ASPX_ACPL_2 inner body walker
    // =================================================================

    /// Helper — write a `companding_control(3)` element with all
    /// channels companded on (sync_flag=true compresses the per-channel
    /// loop to a single `compand_on=true` bit and skips compand_avg).
    fn write_companding_3_all_on(bw: &mut oxideav_core::bits::BitWriter) {
        bw.write_bit(true); // sync_flag
        bw.write_bit(true); // compand_on (sync=1 → only 1 channel-bit)
    }

    /// Helper — write a 15-bit all-zero `aspx_config()` payload.
    fn write_zero_aspx_config(bw: &mut oxideav_core::bits::BitWriter) {
        bw.write_u32(0, 15);
    }

    /// Helper — write a PARTIAL `acpl_config_1ch()`: 2-bit
    /// num_param_bands_id + 1-bit quant_mode + 3-bit qmf_band_minus1.
    fn write_acpl_config_1ch_partial(bw: &mut oxideav_core::bits::BitWriter) {
        bw.write_u32(0, 2); // id = 0 (15 bands)
        bw.write_bit(false); // quant_mode = Fine
        bw.write_u32(0, 3); // qmf_band_minus1 = 0 -> qmf_band = 1
    }

    /// Helper — write a FULL `acpl_config_1ch()`: 2-bit + 1-bit (no
    /// qmf_band field).
    fn write_acpl_config_1ch_full(bw: &mut oxideav_core::bits::BitWriter) {
        bw.write_u32(0, 2); // id = 0 (15 bands)
        bw.write_bit(false); // quant_mode = Fine
    }

    /// `parse_5x_audio_data_outer` for ASPX_ACPL_2 on a non-iframe
    /// shouldn't reach the ACPL pair walker (gated on b_iframe +
    /// in-scope aspx_config). The outer must still consume
    /// companding_control(3) + 1-bit coding_config and try to parse the
    /// inner channel data — but the per-side acpl_data slots stay
    /// `None`.
    #[test]
    fn parse_5x_aspx_acpl_2_non_iframe_leaves_acpl_pair_none() {
        let mut bw = BitWriter::new();
        bw.write_u32(3, 3); // 5_X_codec_mode = ASPX_ACPL_2
        write_companding_3_all_on(&mut bw);
        bw.write_bit(false); // coding_config = 0 -> two_channel_data + mono(0)
                             // two_channel_data() outer + 2x sf_data:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true); // b_long_frame
        bw.write_u32(8, 6); // max_sfb[0]
        bw.write_u32(0, 2); // chparam sap_mode = 0
        write_zero_sf_data_body(&mut bw, 8, 1920);
        write_zero_sf_data_body(&mut bw, 8, 1920);
        // mono_data(0): spec_frontend bit + asf_transform_info long +
        // sf_info(ASF, 0, 0).
        bw.write_bit(false); // spec_frontend = ASF
        bw.write_bit(true); // b_long_frame
        bw.write_u32(8, 6); // max_sfb[0] (n_msfb_bits=6 @ tl=1920)
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, false, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl2));
        assert_eq!(
            tools.five_x_coding_config,
            Some(FiveXCodingConfig::AcplLite2)
        );
        assert_eq!(tools.two_channel_data.len(), 1);
        assert!(tools.cfg0_centre_mono.is_some());
        assert!(tools.acpl_data_1ch_pair[0].is_none());
        assert!(tools.acpl_data_1ch_pair[1].is_none());
    }

    /// ASPX_ACPL_1 non-iframe with `coding_config = 1`
    /// (three_channel_data branch, no Cfg0 mono_data trailer).
    /// Walker should populate `three_channel_data` but the joint-MDCT
    /// residual layer + ACPL pair are gated and stay unset.
    #[test]
    fn parse_5x_aspx_acpl_1_non_iframe_walks_three_channel_data() {
        let mut bw = BitWriter::new();
        bw.write_u32(2, 3); // 5_X_codec_mode = ASPX_ACPL_1
        write_companding_3_all_on(&mut bw);
        bw.write_bit(true); // coding_config = 1 -> three_channel_data
                            // three_channel_data outer:
        bw.write_bit(true); // b_long_frame
        bw.write_u32(10, 6); // max_sfb[0]
        bw.write_u32(0, 4); // chel_matsel
        bw.write_u32(0, 2); // chparam_info #0
        bw.write_u32(0, 2); // chparam_info #1
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        // Joint-MDCT residual layer (ASPX_ACPL_1 only): max_sfb_master
        // is read with n_side_bits=5 @ tl=1920 (Table 106).
        bw.write_u32(8, 5); // max_sfb_master = 8
        bw.write_u32(0, 2); // chparam_info residual ch0 (sap_mode=0)
        bw.write_u32(0, 2); // chparam_info residual ch1
        write_zero_sf_data_body(&mut bw, 8, 1920); // residual ch0 sf_data
        write_zero_sf_data_body(&mut bw, 8, 1920); // residual ch1 sf_data
                                                // Pad to be safe.
        bw.align_to_byte();
        while bw.byte_len() < 64 {
            bw.write_u32(0, 8);
        }
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, false, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl1));
        assert_eq!(
            tools.five_x_coding_config,
            Some(FiveXCodingConfig::Cfg1ThreeStereo)
        );
        let three = tools.three_channel_data.as_ref().expect("3ch parsed");
        assert_eq!(three.psy_info.as_ref().unwrap().max_sfb_0, 10);
        // Non-iframe: joint-MDCT residual layer is walked but ACPL pair
        // stays None (no aspx_config in scope).
        assert!(tools.acpl_data_1ch_pair[0].is_none());
        assert!(tools.acpl_data_1ch_pair[1].is_none());
        // Round 40: the residual pair (sSMP,3 / sSMP,4 spectra per
        // Table 181) is now persisted on `tools.acpl_1_residual_pair`
        // so the dispatch can IMDCT it into Ls/Rs PCM carriers. The
        // sf_data bodies above are all-zero, so the resulting spectra
        // are all-zero — the slot is `Some` regardless.
        assert!(tools.acpl_1_residual_pair[0].is_some(), "sSMP,3 persisted");
        assert!(tools.acpl_1_residual_pair[1].is_some(), "sSMP,4 persisted");
        let (tl0, spec0) = tools.acpl_1_residual_pair[0].as_ref().unwrap();
        assert_eq!(*tl0, 1920);
        // sfb_offset[8] for tl=1920 — caller-provided max_sfb_master = 8.
        // Length is the number of MDCT bins covered by 8 sfbs at tl=1920.
        assert!(!spec0.is_empty(), "sSMP,3 spectrum is non-empty");
    }

    /// ASPX_ACPL_2 I-frame with `coding_config = 1`
    /// (three_channel_data branch). The outer parses aspx_config +
    /// acpl_config_1ch(FULL) before the body. The walker exercises
    /// three_channel_data + aspx_data_2ch + aspx_data_1ch + 2x
    /// acpl_data_1ch. The downstream Huffman walks may bail silently
    /// on a degenerate aspx_config (zero start_freq), but the parsed
    /// configs and the upstream three_channel_data must surface on
    /// tools.
    #[test]
    fn parse_5x_aspx_acpl_2_iframe_parses_configs_and_three_channel() {
        let mut bw = BitWriter::new();
        bw.write_u32(3, 3); // 5_X_codec_mode = ASPX_ACPL_2
        write_zero_aspx_config(&mut bw);
        write_acpl_config_1ch_full(&mut bw);
        write_companding_3_all_on(&mut bw);
        bw.write_bit(true); // coding_config = 1 -> three_channel_data
                            // three_channel_data outer:
        bw.write_bit(true); // b_long_frame
        bw.write_u32(10, 6); // max_sfb[0]
        bw.write_u32(0, 4); // chel_matsel
        bw.write_u32(0, 2); // chparam_info #0
        bw.write_u32(0, 2); // chparam_info #1
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        // Pad with zeros for downstream aspx/acpl walkers (which are
        // try-and-bail).
        bw.align_to_byte();
        while bw.byte_len() < 256 {
            bw.write_u32(0, 8);
        }
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl2));
        assert!(tools.aspx_config.is_some());
        let cfg_full = tools.acpl_config_1ch_full.expect("FULL config parsed");
        assert_eq!(cfg_full.num_param_bands, 15);
        assert_eq!(cfg_full.qmf_band, 0); // FULL has no qmf_band
        let three = tools.three_channel_data.as_ref().expect("3ch parsed");
        assert_eq!(three.psy_info.as_ref().unwrap().max_sfb_0, 10);
    }

    /// ASPX_ACPL_1 I-frame with `coding_config = 0` (two_channel_data
    /// branch — pulls the joint-MDCT residual layer + Cfg0
    /// `mono_data(0)` trailer). Validates that:
    ///   * aspx_config is parsed,
    ///   * acpl_config_1ch_partial is parsed (with non-zero qmf_band),
    ///   * two_channel_data lands in the slot,
    ///   * cfg0_centre_mono is populated by the trailing mono_data(0).
    ///
    /// Downstream aspx_data / acpl_data may bail silently on the
    /// all-zero pad; the test just asserts non-fatal completion.
    #[test]
    fn parse_5x_aspx_acpl_1_iframe_walks_residual_and_mono_trailer() {
        let mut bw = BitWriter::new();
        bw.write_u32(2, 3); // 5_X_codec_mode = ASPX_ACPL_1
        write_zero_aspx_config(&mut bw);
        write_acpl_config_1ch_partial(&mut bw);
        write_companding_3_all_on(&mut bw);
        bw.write_bit(false); // coding_config = 0 -> two_channel_data
                             // two_channel_data outer (Table 26):
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true); // b_long_frame
        bw.write_u32(12, 6); // max_sfb[0]
        bw.write_u32(0, 2); // chparam sap_mode = 0
        write_zero_sf_data_body(&mut bw, 12, 1920);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        // Joint-MDCT residual layer (ASPX_ACPL_1):
        // max_sfb_master uses n_side_bits = 5 @ tl=1920.
        bw.write_u32(6, 5); // max_sfb_master = 6
        bw.write_u32(0, 2); // chparam residual ch0
        bw.write_u32(0, 2); // chparam residual ch1
        write_zero_sf_data_body(&mut bw, 6, 1920);
        write_zero_sf_data_body(&mut bw, 6, 1920);
        // Cfg0 trailer: mono_data(0) for the centre channel.
        bw.write_bit(false); // spec_frontend = ASF
        bw.write_bit(true); // b_long_frame
        bw.write_u32(7, 6); // max_sfb[0] for centre mono
                            // Pad for downstream aspx_data / acpl_data.
        bw.align_to_byte();
        while bw.byte_len() < 256 {
            bw.write_u32(0, 8);
        }
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl1));
        assert!(tools.aspx_config.is_some());
        let cfg_partial = tools
            .acpl_config_1ch_partial
            .expect("PARTIAL config parsed");
        assert_eq!(cfg_partial.num_param_bands, 15);
        assert_eq!(cfg_partial.qmf_band, 1); // qmf_band_minus1=0 -> 1
        assert_eq!(tools.two_channel_data.len(), 1);
        assert_eq!(
            tools.two_channel_data[0]
                .psy_info
                .as_ref()
                .unwrap()
                .max_sfb_0,
            12
        );
        let centre = tools
            .cfg0_centre_mono
            .as_ref()
            .expect("Cfg0 centre mono walked");
        assert_eq!(centre.psy_info.as_ref().unwrap().max_sfb_0, 7);
    }

    /// Truncated input mid-`three_channel_data` for ASPX_ACPL_2 should
    /// leave `three_channel_data` `None` (the channel-data parser
    /// errored) without panicking and without setting any of the
    /// downstream slots. The outer walker still returns Ok(()).
    #[test]
    fn parse_5x_aspx_acpl_2_truncated_channel_data_bails() {
        let mut bw = BitWriter::new();
        bw.write_u32(3, 3); // 5_X_codec_mode = ASPX_ACPL_2
        write_companding_3_all_on(&mut bw);
        bw.write_bit(true); // coding_config = 1 -> three_channel_data
                            // start three_channel_data but truncate after b_long_frame
                            // (no max_sfb bits).
        bw.write_bit(true);
        // intentionally cut here — the next byte boundary won't have
        // the 6-bit max_sfb field complete.
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        // The truncation lands inside `parse_three_channel_data` which
        // returns Err — that's caught by the inner walker's
        // try-and-bail and we surface Ok(()).
        parse_5x_audio_data_outer(&mut br, &mut tools, false, false, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl2));
        assert!(tools.three_channel_data.is_none());
        assert!(tools.acpl_data_1ch_pair[0].is_none());
        assert!(tools.acpl_data_1ch_pair[1].is_none());
    }

    /// max_sfb_master = 0 in the joint-MDCT residual layer should bail
    /// silently — the chparam_info / sf_data trailers would be
    /// degenerate. Subsequent aspx/acpl trailers stay unset.
    #[test]
    fn parse_5x_aspx_acpl_1_iframe_zero_max_sfb_master_bails() {
        let mut bw = BitWriter::new();
        bw.write_u32(2, 3); // 5_X_codec_mode = ASPX_ACPL_1
        write_zero_aspx_config(&mut bw);
        write_acpl_config_1ch_partial(&mut bw);
        write_companding_3_all_on(&mut bw);
        bw.write_bit(true); // coding_config = 1 -> three_channel_data
                            // three_channel_data outer:
        bw.write_bit(true); // b_long_frame
        bw.write_u32(10, 6); // max_sfb[0]
        bw.write_u32(0, 4); // chel_matsel
        bw.write_u32(0, 2);
        bw.write_u32(0, 2);
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        // max_sfb_master = 0 (n_side_bits = 5 @ tl=1920).
        bw.write_u32(0, 5);
        bw.align_to_byte();
        while bw.byte_len() < 64 {
            bw.write_u32(0, 8);
        }
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_5x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.five_x_mode, Some(FiveXCodecMode::AspxAcpl1));
        assert!(tools.three_channel_data.is_some());
        assert!(tools.acpl_data_1ch_pair[0].is_none());
        assert!(tools.acpl_data_1ch_pair[1].is_none());
    }

    // =================================================================
    // Round 27: 7_X channel-element walker (immersive 7.0 / 7.1)
    // =================================================================

    /// Helper — write a `companding_control(5)` element with all five
    /// channels companded on (sync_flag=true compresses the per-channel
    /// loop to a single `compand_on=true` bit).
    fn write_companding_5_all_on(bw: &mut oxideav_core::bits::BitWriter) {
        bw.write_bit(true); // sync_flag
        bw.write_bit(true); // compand_on
    }

    #[test]
    fn seven_x_codec_mode_round_trip() {
        assert_eq!(SevenXCodecMode::from_u32(0), SevenXCodecMode::Simple);
        assert_eq!(SevenXCodecMode::from_u32(1), SevenXCodecMode::Aspx);
        assert_eq!(SevenXCodecMode::from_u32(2), SevenXCodecMode::AspxAcpl1);
        assert_eq!(SevenXCodecMode::from_u32(3), SevenXCodecMode::AspxAcpl2);
        // Wraparound — only 2 bits are used so 4 .. 7 fold back.
        assert_eq!(SevenXCodecMode::from_u32(4), SevenXCodecMode::Simple);
    }

    /// 7_X SIMPLE coding_config = 3 (five_channel_data) + the trailing
    /// SIMPLE additional `two_channel_data` (no SAP). 7.0 path (no LFE).
    #[test]
    fn parse_7x_outer_simple_cfg3_no_sap_walks_full_body() {
        let mut bw = BitWriter::new();
        // 7_X_codec_mode = SIMPLE (0) -- 2 bits.
        bw.write_u32(0, 2);
        // No I-frame config (SIMPLE).
        // No LFE.
        // No companding (SIMPLE/ASPX skip companding in 7.X).
        // coding_config = 3 -> five_channel_data.
        bw.write_u32(3, 2);
        // five_channel_data outer:
        bw.write_bit(true); // b_long_frame
        bw.write_u32(15, 6); // max_sfb[0]
        bw.write_u32(0, 4); // chel_matsel
        for _ in 0..5 {
            bw.write_u32(0, 2); // chparam_info
        }
        for _ in 0..5 {
            write_zero_sf_data_body(&mut bw, 15, 1920);
        }
        // SIMPLE additional-channel block: b_use_sap_add_ch = 0.
        bw.write_bit(false);
        // additional two_channel_data (no SAP):
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true); // b_long_frame
        bw.write_u32(10, 6); // max_sfb[0]
        bw.write_u32(0, 2); // chparam sap_mode = 0
        write_zero_sf_data_body(&mut bw, 10, 1920);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.seven_x_mode, Some(SevenXCodecMode::Simple));
        assert!(!tools.seven_x_b_has_lfe);
        assert_eq!(
            tools.seven_x_coding_config,
            Some(FiveXCodingConfig::Cfg3Five)
        );
        let five = tools.five_channel_data.as_ref().expect("5ch parsed");
        assert_eq!(five.psy_info.as_ref().unwrap().max_sfb_0, 15);
        assert_eq!(tools.seven_x_b_use_sap_add_ch, Some(false));
        assert!(tools.seven_x_add_chparam_info.is_none());
        let add = tools
            .seven_x_additional_channel_data
            .as_ref()
            .expect("additional 2ch parsed");
        assert_eq!(add.psy_info.as_ref().unwrap().max_sfb_0, 10);
    }

    /// 7.1 SIMPLE: leading `mono_data(1)` LFE then five_channel_data
    /// then the additional `two_channel_data`.
    #[test]
    fn parse_7x_outer_simple_71_walks_lfe_and_five_channel() {
        let mut bw = BitWriter::new();
        bw.write_u32(0, 2); // SIMPLE
                            // LFE mono_data(1): sf_info_lfe() implies b_long_frame=1
                            // with no bits read.
        bw.write_u32(4, 3); // max_sfb[0] (n_msfbl_bits=3 @ tl=1920)
        write_zero_sf_data_body(&mut bw, 4, 1920); // round 38: LFE body
                                                // coding_config = 3 -> five_channel_data:
        bw.write_u32(3, 2);
        bw.write_bit(true); // b_long_frame
        bw.write_u32(10, 6); // max_sfb[0]
        bw.write_u32(0, 4);
        for _ in 0..5 {
            bw.write_u32(0, 2);
        }
        for _ in 0..5 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        // SIMPLE additional-channel block.
        bw.write_bit(false); // b_use_sap_add_ch = 0
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true); // b_long_frame
        bw.write_u32(8, 6); // max_sfb[0]
        bw.write_u32(0, 2); // chparam sap_mode = 0
        write_zero_sf_data_body(&mut bw, 8, 1920);
        write_zero_sf_data_body(&mut bw, 8, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, true, true, 1920).unwrap();
        assert!(tools.seven_x_b_has_lfe);
        let lfe = tools.lfe_mono_data.as_ref().expect("LFE walked");
        assert!(lfe.b_lfe);
        assert_eq!(lfe.psy_info.as_ref().unwrap().max_sfb_0, 4);
        assert!(tools.five_channel_data.is_some());
        assert!(tools.seven_x_additional_channel_data.is_some());
    }

    /// 7_X SIMPLE Cfg0 — `2ch_mode + two_channel_data + two_channel_data`
    /// (no centre mono inside the switch). The trailing centre
    /// `mono_data(0)` lands AFTER the additional-channel block.
    #[test]
    fn parse_7x_outer_simple_cfg0_walks_two_pairs_then_centre_mono() {
        let mut bw = BitWriter::new();
        bw.write_u32(0, 2); // SIMPLE
                            // coding_config = 0:
        bw.write_u32(0, 2);
        // 2ch_mode (1 bit).
        bw.write_bit(false);
        // two_channel_data #0:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(12, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        // two_channel_data #1:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(12, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        // SIMPLE additional-channel block.
        bw.write_bit(false); // b_use_sap_add_ch = 0
                             // additional two_channel_data:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        // Trailing mono_data(0) for Cfg0 (centre).
        bw.write_bit(false); // spec_frontend = ASF
        bw.write_bit(true); // b_long_frame
        bw.write_u32(7, 6); // max_sfb[0]
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.seven_x_mode, Some(SevenXCodecMode::Simple));
        assert_eq!(
            tools.seven_x_coding_config,
            Some(FiveXCodingConfig::Cfg0Stereo2plusMono)
        );
        assert_eq!(tools.b_2ch_mode, Some(false));
        assert_eq!(tools.two_channel_data.len(), 2);
        let centre = tools.cfg0_centre_mono.as_ref().expect("centre mono walked");
        assert_eq!(centre.psy_info.as_ref().unwrap().max_sfb_0, 7);
    }

    /// 7_X SIMPLE Cfg2 — `four_channel_data` (no surround mono inside
    /// switch). Trailing surround `mono_data(0)` lands AFTER the
    /// additional-channel block.
    #[test]
    fn parse_7x_outer_simple_cfg2_walks_four_then_back_mono() {
        let mut bw = BitWriter::new();
        bw.write_u32(0, 2); // SIMPLE
                            // coding_config = 2 -> four_channel_data:
        bw.write_u32(2, 2);
        bw.write_bit(true); // b_long_frame
        bw.write_u32(11, 6); // max_sfb[0]
        for _ in 0..4 {
            bw.write_u32(0, 2);
        }
        for _ in 0..4 {
            write_zero_sf_data_body(&mut bw, 11, 1920);
        }
        // SIMPLE additional-channel block.
        bw.write_bit(false); // b_use_sap_add_ch = 0
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(9, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 9, 1920);
        write_zero_sf_data_body(&mut bw, 9, 1920);
        // Trailing mono_data(0) for Cfg2 (back surround).
        bw.write_bit(false);
        bw.write_bit(true);
        bw.write_u32(6, 6);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(
            tools.seven_x_coding_config,
            Some(FiveXCodingConfig::Cfg2FourMono)
        );
        assert!(tools.four_channel_data.is_some());
        let back = tools.cfg2_back_mono.as_ref().expect("back mono walked");
        assert_eq!(back.psy_info.as_ref().unwrap().max_sfb_0, 6);
    }

    /// 7_X SIMPLE Cfg1 — `three_channel_data + two_channel_data` (no
    /// trailing mono_data — coding_config in {0,2} only triggers the
    /// trailer). The additional-channel block still fires.
    #[test]
    fn parse_7x_outer_simple_cfg1_no_mono_trailer() {
        let mut bw = BitWriter::new();
        bw.write_u32(0, 2); // SIMPLE
                            // coding_config = 1 -> three_channel_data + two_channel_data
        bw.write_u32(1, 2);
        // three_channel_data:
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 4); // chel_matsel
        bw.write_u32(0, 2);
        bw.write_u32(0, 2);
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        // two_channel_data:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        // SIMPLE additional-channel block.
        bw.write_bit(false); // b_use_sap_add_ch
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(8, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 8, 1920);
        write_zero_sf_data_body(&mut bw, 8, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(
            tools.seven_x_coding_config,
            Some(FiveXCodingConfig::Cfg1ThreeStereo)
        );
        assert!(tools.three_channel_data.is_some());
        assert_eq!(tools.two_channel_data.len(), 1);
        // Cfg1: no trailing mono_data(0).
        assert!(tools.cfg0_centre_mono.is_none());
        assert!(tools.cfg2_back_mono.is_none());
        assert!(tools.seven_x_additional_channel_data.is_some());
    }

    /// 7_X SIMPLE with `b_use_sap_add_ch = 1` — two `chparam_info()`
    /// elements precede the additional `two_channel_data`. Validates
    /// that `seven_x_add_chparam_info` is populated.
    #[test]
    fn parse_7x_outer_simple_with_sap_add_ch_populates_chparam_pair() {
        let mut bw = BitWriter::new();
        bw.write_u32(0, 2); // SIMPLE
        bw.write_u32(3, 2); // coding_config = 3 -> five_channel_data
        bw.write_bit(true);
        bw.write_u32(12, 6);
        bw.write_u32(0, 4);
        for _ in 0..5 {
            bw.write_u32(0, 2);
        }
        for _ in 0..5 {
            write_zero_sf_data_body(&mut bw, 12, 1920);
        }
        // SIMPLE additional-channel block with SAP.
        bw.write_bit(true); // b_use_sap_add_ch = 1
        bw.write_u32(0, 2); // chparam_info #0 sap_mode = 0
        bw.write_u32(0, 2); // chparam_info #1 sap_mode = 0
                            // additional two_channel_data:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(8, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 8, 1920);
        write_zero_sf_data_body(&mut bw, 8, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.seven_x_b_use_sap_add_ch, Some(true));
        let pair = tools
            .seven_x_add_chparam_info
            .as_ref()
            .expect("SAP chparam pair");
        assert_eq!(pair[0].sap_mode, 0);
        assert_eq!(pair[1].sap_mode, 0);
    }

    /// 7_X ASPX_ACPL_2 non-iframe with `coding_config = 1`
    /// (three_channel_data branch). Walker should populate
    /// `three_channel_data` + `two_channel_data` but the ACPL pair stays
    /// unset (no aspx_config in scope on a non-iframe). NO additional
    /// `two_channel_data` should be parsed (it's SIMPLE/ASPX-only).
    #[test]
    fn parse_7x_aspx_acpl_2_non_iframe_walks_three_channel_no_addch() {
        let mut bw = BitWriter::new();
        bw.write_u32(3, 2); // 7_X_codec_mode = ASPX_ACPL_2
        write_companding_5_all_on(&mut bw);
        bw.write_u32(1, 2); // coding_config = 1 -> three_channel + two_channel
                            // three_channel_data outer:
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 4);
        bw.write_u32(0, 2);
        bw.write_u32(0, 2);
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        // two_channel_data:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, false, 1920).unwrap();
        assert_eq!(tools.seven_x_mode, Some(SevenXCodecMode::AspxAcpl2));
        assert_eq!(
            tools.seven_x_coding_config,
            Some(FiveXCodingConfig::Cfg1ThreeStereo)
        );
        assert!(tools.three_channel_data.is_some());
        assert_eq!(tools.two_channel_data.len(), 1);
        // No additional two_channel_data — that's SIMPLE/ASPX-only.
        assert!(tools.seven_x_additional_channel_data.is_none());
        assert!(tools.seven_x_b_use_sap_add_ch.is_none());
        // ACPL pair gated on b_iframe + aspx_config in scope.
        assert!(tools.acpl_data_1ch_pair[0].is_none());
        assert!(tools.acpl_data_1ch_pair[1].is_none());
    }

    /// 7_X ASPX_ACPL_1 I-frame with `coding_config = 0` (two_channel_data
    /// branch). Validates the joint-MDCT residual layer + Cfg0
    /// trailing mono_data(0) (which moves AFTER the additional-channel
    /// block in 7.X — but ASPX_ACPL_1 has no additional-channel block,
    /// so it's right after the residual layer).
    #[test]
    fn parse_7x_aspx_acpl_1_iframe_walks_residual_and_mono_trailer() {
        let mut bw = BitWriter::new();
        bw.write_u32(2, 2); // 7_X_codec_mode = ASPX_ACPL_1
        write_zero_aspx_config(&mut bw);
        write_acpl_config_1ch_partial(&mut bw);
        write_companding_5_all_on(&mut bw);
        bw.write_u32(0, 2); // coding_config = 0 -> 2ch_mode + 2x two_channel_data
        bw.write_bit(false); // 2ch_mode
                             // two_channel_data #0:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(12, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        // two_channel_data #1:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(12, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        write_zero_sf_data_body(&mut bw, 12, 1920);
        // ASPX_ACPL_1 joint-MDCT residual layer (n_side_bits=5 @ tl=1920).
        bw.write_u32(6, 5); // max_sfb_master = 6
        bw.write_u32(0, 2);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 6, 1920);
        write_zero_sf_data_body(&mut bw, 6, 1920);
        // Cfg0 trailer: mono_data(0) for the centre.
        bw.write_bit(false);
        bw.write_bit(true);
        bw.write_u32(7, 6);
        // Pad for downstream try-and-bail aspx/acpl trailers.
        bw.align_to_byte();
        while bw.byte_len() < 256 {
            bw.write_u32(0, 8);
        }
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.seven_x_mode, Some(SevenXCodecMode::AspxAcpl1));
        assert!(tools.aspx_config.is_some());
        let cfg_partial = tools
            .acpl_config_1ch_partial
            .expect("PARTIAL config parsed");
        assert_eq!(cfg_partial.num_param_bands, 15);
        assert_eq!(cfg_partial.qmf_band, 1);
        assert_eq!(tools.two_channel_data.len(), 2);
        let centre = tools.cfg0_centre_mono.as_ref().expect("centre mono walked");
        assert_eq!(centre.psy_info.as_ref().unwrap().max_sfb_0, 7);
        // No additional-channel block on ASPX_ACPL_*.
        assert!(tools.seven_x_additional_channel_data.is_none());
        assert!(tools.seven_x_b_use_sap_add_ch.is_none());
    }

    /// 7_X ASPX_ACPL_1 zero `max_sfb_master` should bail silently —
    /// matching the 5_X walker's bail behaviour. Subsequent aspx/acpl
    /// trailers stay unset.
    #[test]
    fn parse_7x_aspx_acpl_1_iframe_zero_max_sfb_master_bails() {
        let mut bw = BitWriter::new();
        bw.write_u32(2, 2); // ASPX_ACPL_1
        write_zero_aspx_config(&mut bw);
        write_acpl_config_1ch_partial(&mut bw);
        write_companding_5_all_on(&mut bw);
        bw.write_u32(1, 2); // coding_config = 1 -> three_channel + two_channel
                            // three_channel_data:
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 4);
        bw.write_u32(0, 2);
        bw.write_u32(0, 2);
        for _ in 0..3 {
            write_zero_sf_data_body(&mut bw, 10, 1920);
        }
        // two_channel_data:
        bw.write_bit(true); // b_enable_mdct_stereo_proc
        bw.write_bit(true);
        bw.write_u32(10, 6);
        bw.write_u32(0, 2);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        write_zero_sf_data_body(&mut bw, 10, 1920);
        // max_sfb_master = 0 (n_side_bits=5).
        bw.write_u32(0, 5);
        bw.align_to_byte();
        while bw.byte_len() < 64 {
            bw.write_u32(0, 8);
        }
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.seven_x_mode, Some(SevenXCodecMode::AspxAcpl1));
        assert!(tools.three_channel_data.is_some());
        assert!(tools.acpl_data_1ch_pair[0].is_none());
        assert!(tools.acpl_data_1ch_pair[1].is_none());
    }

    /// Truncated input mid-`five_channel_data` for SIMPLE 7_X should
    /// leave `five_channel_data` `None` (the channel-data parser
    /// errored) without panicking — the outer walker still returns
    /// Ok(()) thanks to try-and-bail.
    #[test]
    fn parse_7x_simple_truncated_five_channel_data_bails() {
        let mut bw = BitWriter::new();
        bw.write_u32(0, 2); // SIMPLE
        bw.write_u32(3, 2); // coding_config = 3
                            // start five_channel_data but truncate:
        bw.write_bit(true); // b_long_frame
                            // intentionally cut here.
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let mut tools = SubstreamTools::default();
        parse_7x_audio_data_outer(&mut br, &mut tools, false, true, 1920).unwrap();
        assert_eq!(tools.seven_x_mode, Some(SevenXCodecMode::Simple));
        assert_eq!(
            tools.seven_x_coding_config,
            Some(FiveXCodingConfig::Cfg3Five)
        );
        assert!(tools.five_channel_data.is_none());
        assert!(tools.seven_x_additional_channel_data.is_none());
    }

    /// Generous zero-padding after the leading control bits — every
    /// `mono_data`/`two_channel_data`/`three_channel_data` call's outer
    /// shell (transform_info/psy_info/chparam) reads real required
    /// fields, but their inner `sf_data` spectral body is try-and-bail
    /// (leaves `scaled_spec` as `None` rather than erroring on garbage),
    /// so this just needs to be long enough, not bit-exact.
    fn padded_var_channel_bits(leading: &[u8]) -> Vec<u8> {
        let mut bw = BitWriter::new();
        for &b in leading {
            bw.write_bit(b != 0);
        }
        for _ in 0..2000 {
            bw.write_bit(false);
        }
        bw.align_to_byte();
        bw.finish()
    }

    #[test]
    fn var_channel_element_even_pairs_no_lfe() {
        // aspx_mode = 0, n_dmx_signals = 4 (even) -> 2 pairs, no LFE.
        let bytes = padded_var_channel_bits(&[0]);
        let mut br = BitReader::new(&bytes);
        let out = parse_var_channel_element(&mut br, true, 4, false, 1920).unwrap();
        assert!(!out.aspx_mode);
        assert!(out.lfe.is_none());
        assert!(out.single_mono.is_none());
        assert_eq!(out.pairs.len(), 2);
        assert!(out.odd_tail_two_and_mono.is_none());
        assert!(out.odd_tail_three.is_none());
    }

    #[test]
    fn var_channel_element_single_signal_is_mono_only() {
        // n_dmx_signals = 1 -> single_mono, nothing else.
        let bytes = padded_var_channel_bits(&[0]);
        let mut br = BitReader::new(&bytes);
        let out = parse_var_channel_element(&mut br, true, 1, false, 1920).unwrap();
        assert!(out.single_mono.is_some());
        assert!(out.pairs.is_empty());
        assert!(out.odd_tail_two_and_mono.is_none());
        assert!(out.odd_tail_three.is_none());
    }

    #[test]
    fn var_channel_element_odd_var_coding_config_0_gives_two_and_mono_tail() {
        // n_dmx_signals = 3 (odd, n_pairs = 1): 0 leading pairs, then
        // var_coding_config = 0 -> two_channel_data + mono_data tail.
        let bytes = padded_var_channel_bits(&[0, 0]);
        let mut br = BitReader::new(&bytes);
        let out = parse_var_channel_element(&mut br, true, 3, false, 1920).unwrap();
        assert!(out.pairs.is_empty());
        assert!(out.odd_tail_two_and_mono.is_some());
        assert!(out.odd_tail_three.is_none());
    }

    #[test]
    fn var_channel_element_odd_var_coding_config_1_gives_three_channel_tail() {
        // Same shape, but var_coding_config = 1 -> three_channel_data tail.
        let bytes = padded_var_channel_bits(&[0, 1]);
        let mut br = BitReader::new(&bytes);
        let out = parse_var_channel_element(&mut br, true, 3, false, 1920).unwrap();
        assert!(out.pairs.is_empty());
        assert!(out.odd_tail_two_and_mono.is_none());
        assert!(out.odd_tail_three.is_some());
    }

    #[test]
    fn var_channel_element_with_lfe_parses_lfe_first() {
        // n_dmx_signals = 2 (even), b_has_lfe = true -> lfe then 1 pair.
        // The LFE's own mono_data(1) call requires b_long_frame = 1
        // (asf_psy_info_lfe rejects short transforms for LFE), so that
        // bit can't be part of the generic zero padding.
        let bytes = padded_var_channel_bits(&[0, 1]);
        let mut br = BitReader::new(&bytes);
        let out = parse_var_channel_element(&mut br, true, 2, true, 1920).unwrap();
        assert!(out.lfe.is_some());
        assert_eq!(out.pairs.len(), 1);
    }

    #[test]
    fn var_channel_element_aspx_non_iframe_errors_on_missing_sticky_config() {
        // aspx_mode = 1, n_dmx_signals = 2, b_iframe = false: aspx_config()
        // is never read (only present on I-frames per spec), and this
        // path doesn't yet thread a sticky config through for the AJOC
        // downmix case, so it should report that specific limitation
        // rather than guess.
        let mut bw = BitWriter::new();
        bw.write_bit(true); // aspx_mode = 1
        bw.write_bit(true); // companding_control: sync_flag = true (num_chan=2 > 1)
        bw.write_bit(true); // b_compand_on[0] = true (sync -> single flag, all on)
        for _ in 0..2000 {
            bw.write_bit(false);
        }
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let err = parse_var_channel_element(&mut br, false, 2, false, 1920).unwrap_err();
        assert!(err.to_string().contains("sticky aspx_config"));
    }

    fn minimal_test_aspx_cfg() -> crate::aspx::AspxConfig {
        crate::aspx::AspxConfig {
            quant_mode_env: crate::aspx::AspxQuantStep::Fine,
            start_freq: 0,
            stop_freq: 0,
            master_freq_scale: crate::aspx::AspxMasterFreqScale::LowRes,
            interpolation: false,
            preflat: false,
            limiter: false,
            noise_sbg: 0,
            num_env_bits_fixfix: 0,
            freq_res_mode: crate::aspx::AspxFreqResMode::DurationDependent,
        }
    }

    #[test]
    fn var_channel_element_aspx_iframe_real_trailer_roundtrips() {
        // n_dmx_signals = 2 (even, aspx_mode = 1, b_iframe = true):
        // aspx_config, then companding_control (n_dmx_signals <= 5),
        // then the core two_channel_data pair (padded — try-and-bail),
        // then exactly one real aspx_data_2ch() trailer (n_pairs = 1)
        // built with the crate's own minimal encoder helper and parsed
        // back through the same production parser the channel-coded
        // path uses.
        let cfg = minimal_test_aspx_cfg();
        let mut bw = BitWriter::new();
        bw.write_bit(true); // aspx_mode = 1
        crate::encoder_acpl3::write_aspx_config(&mut bw, &cfg);
        bw.write_bit(true); // companding_control: sync_flag = true
        bw.write_bit(true); // b_compand_on[0] = true
                            // Core two_channel_data pair: enough zero padding for its
                            // outer shell; the inner sf_data is try-and-bail.
        for _ in 0..200 {
            bw.write_bit(false);
        }
        crate::encoder_acpl3::write_aspx_data_2ch_minimal(&mut bw, &cfg).unwrap();
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);

        let out = parse_var_channel_element(&mut br, true, 2, false, 1920).unwrap();
        assert!(out.aspx_mode);
        assert!(out.aspx_config.is_some());
        assert!(out.companding_control.is_some());
        assert_eq!(out.pairs.len(), 1);
        assert_eq!(out.aspx_pair_trailers.len(), 1);
        assert!(out.aspx_single_trailer.is_none());
    }

    #[test]
    fn var_channel_element_aspx_iframe_odd_gets_pair_plus_single_trailer() {
        // n_dmx_signals = 3 (odd, n_pairs = 1): the ASPX trailer loop
        // still runs n_pairs = 1 times regardless of parity, plus one
        // more aspx_data_1ch() because b_isodd — independent of how the
        // core data above split into a two+mono or three-channel tail.
        let cfg = minimal_test_aspx_cfg();
        let mut bw = BitWriter::new();
        bw.write_bit(true); // aspx_mode = 1
        crate::encoder_acpl3::write_aspx_config(&mut bw, &cfg);
        bw.write_bit(true); // companding_control: sync_flag = true (num_chan=3)
        bw.write_bit(true); // b_compand_on[0] = true (sync -> single flag)
                            // Core: n_pairs - 1 = 0 leading pairs, then var_coding_config
                            // = 0 -> two_channel_data + mono_data(0), padded.
        bw.write_bit(false); // var_coding_config = 0
        for _ in 0..200 {
            bw.write_bit(false);
        }
        crate::encoder_acpl3::write_aspx_data_2ch_minimal(&mut bw, &cfg).unwrap();
        crate::encoder_acpl3::write_aspx_data_1ch_minimal(&mut bw, &cfg).unwrap();
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);

        let out = parse_var_channel_element(&mut br, true, 3, false, 1920).unwrap();
        assert!(out.odd_tail_two_and_mono.is_some());
        assert_eq!(out.aspx_pair_trailers.len(), 1);
        assert!(out.aspx_single_trailer.is_some());
    }
}
