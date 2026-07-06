//! Object Audio Metadata (OAMD) dynamic data — ETSI TS 103 190-2 §6.2.8.
//!
//! `oamd_common_data()` (§6.2.8.1) lives in `toc.rs` alongside the other
//! substream-descriptor parsers it's called from
//! (`ac4_substream_info_ajoc`). This module covers the *dynamic*,
//! per-frame OAMD data: `oamd_timing_data()` (§6.2.8.2),
//! `oamd_dyndata_single()` (§6.2.8.3) and its `object_info_block()` /
//! `object_basic_info()` / `object_render_info()` tree (§6.2.8.5-.7).
//!
//! **Scope note:** two rare, deeply-nested optional sub-elements are
//! *not* implemented and return `Error::unsupported` if actually
//! reached, rather than guessing: `add_per_object_md()`
//! (`object_info_block`'s `b_add_table_data` branch) and
//! `ext_prec_alt_pos()` (`oamd_dyndata_single`'s `b_alternative`
//! additional-data branch). Both gate genuinely obscure extension data
//! (extended-precision alternate positions; additional per-object
//! metadata table entries) that — unlike `oamd_common_data`'s
//! `bed_render_info()`/`headphone()` skip — can't be skipped as an
//! opaque block, because the spec computes the skip length *from* how
//! many bits the (unimplemented) function itself would consume.

use oxideav_core::bits::BitReader;
use oxideav_core::{Error, Result};

/// `oamd_timing_data()` (§6.2.8.2).
#[derive(Debug, Clone, Default)]
pub struct OamdTimingData {
    pub num_obj_info_blocks: u32,
}

pub fn parse_oamd_timing_data(br: &mut BitReader<'_>) -> Result<OamdTimingData> {
    // oa_sample_offset_type is 1 or 2 bits: read the first bit; if set,
    // read a second to disambiguate 0b10 vs 0b11 (Table-style short
    // escape, matching the "1/2" bit-count convention used throughout
    // this spec for other 1-or-2-bit fields).
    let first = br.read_bit()?;
    if first {
        let second = br.read_bit()?;
        if !second {
            // 0b10
            let _oa_sample_offset_code = br.read_bit()?;
        } else {
            // 0b11
            let _oa_sample_offset = br.read_u32(5)?;
        }
    }
    let num_obj_info_blocks = br.read_u32(3)?;
    for _ in 0..num_obj_info_blocks {
        let _block_offset_factor = br.read_u32(6)?;
        let ramp_duration_code = br.read_u32(2)?;
        if ramp_duration_code == 0b11 {
            let b_use_ramp_table = br.read_bit()?;
            if b_use_ramp_table {
                let _ramp_duration_table = br.read_u32(4)?;
            } else {
                let _ramp_duration = br.read_u32(11)?;
            }
        }
    }
    Ok(OamdTimingData {
        num_obj_info_blocks,
    })
}

/// `object_basic_info()` (§6.2.8.6).
#[derive(Debug, Clone, Copy, Default)]
pub struct ObjectBasicInfo {
    pub gain: Option<u32>,
    pub priority: Option<u32>,
}

fn parse_object_basic_info(br: &mut BitReader<'_>) -> Result<ObjectBasicInfo> {
    let mut out = ObjectBasicInfo::default();
    let b_default_basic_info_md = br.read_bit()?;
    if !b_default_basic_info_md {
        let bit0 = br.read_bit()?;
        let basic_info_md = if bit0 {
            // "0b10"/"0b11" escape: one more bit disambiguates.
            let bit1 = br.read_bit()?;
            if bit1 {
                0b11u32
            } else {
                0b10u32
            }
        } else {
            0b0u32
        };
        if basic_info_md == 0b0 || basic_info_md == 0b10 {
            let object_gain_code = br.read_bit()?;
            if !object_gain_code {
                out.gain = Some(br.read_u32(6)?);
            }
        }
        if basic_info_md == 0b10 || basic_info_md == 0b11 {
            out.priority = Some(br.read_u32(5)?);
        }
    }
    Ok(out)
}

/// `pos3D_X`/`Y`/`Z` from `object_render_info()` (§6.2.8.7), in raw
/// bitstream units (`X`/`Y`: 0..63; `Z`: signed magnitude, `z_sign` +
/// 0..15).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectPosition {
    pub x: u32,
    pub y: u32,
    pub z_sign: bool,
    pub z: u32,
}

/// `object_render_info(object_render_info_status, b_no_delta)`
/// (§6.2.8.7). The "otherprops" (width/screen-factor/distance/div-mode)
/// and "zone" fields are consumed for bit-accuracy but not surfaced —
/// they're renderer tuning hints we don't need for a basic per-object
/// position + gain render.
#[derive(Debug, Clone, Copy, Default)]
pub struct ObjectRenderInfo {
    pub position: Option<ObjectPosition>,
}

#[allow(clippy::if_same_then_else)]
fn parse_object_render_info(
    br: &mut BitReader<'_>,
    all_new: bool,
    b_no_delta: bool,
    prev_position: Option<ObjectPosition>,
) -> Result<ObjectRenderInfo> {
    let (b_position_present, b_zone_present, b_otherprops_present) = if all_new {
        (true, true, true)
    } else {
        (br.read_bit()?, br.read_bit()?, br.read_bit()?)
    };

    let mut out = ObjectRenderInfo::default();
    if b_position_present {
        let b_diff_pos_coding = if b_no_delta { false } else { br.read_bit()? };
        if b_diff_pos_coding {
            // diff_pos3D_{X,Y,Z}: 3 bits each. Not explicitly spelled out
            // as signed/unsigned in the syntax table; treated as an
            // offset-binary delta (range -4..=3) added to the previous
            // absolute position, matching this spec family's usual
            // small-delta convention elsewhere (e.g. A-JOC's differential
            // coding). Flagged here as a best-effort reading pending
            // real-content validation.
            let dx = br.read_u32(3)? as i32 - 4;
            let dy = br.read_u32(3)? as i32 - 4;
            let dz = br.read_u32(3)? as i32 - 4;
            let base = prev_position.unwrap_or_default();
            out.position = Some(ObjectPosition {
                x: (base.x as i32 + dx).clamp(0, 63) as u32,
                y: (base.y as i32 + dy).clamp(0, 63) as u32,
                z_sign: base.z_sign,
                z: (base.z as i32 + dz).clamp(0, 15) as u32,
            });
        } else {
            let x = br.read_u32(6)?;
            let y = br.read_u32(6)?;
            let z_sign = br.read_bit()?;
            let z = br.read_u32(4)?;
            out.position = Some(ObjectPosition { x, y, z_sign, z });
        }
    }

    if b_zone_present {
        let b_grouped_zone_defaults = br.read_bit()?;
        if !b_grouped_zone_defaults {
            let group_zone_flag = br.read_u32(3)?;
            if group_zone_flag & 0b100 != 0 {
                let _zone_mask = br.read_u32(3)?;
            }
            // group_zone_flag & 0b010 / 0b001 just set derived defaults
            // (b_enable_elevation / b_object_snap) with no further bits.
        }
    }

    if b_otherprops_present {
        let b_grouped_other_defaults = br.read_bit()?;
        if !b_grouped_other_defaults {
            let group_other_mask = br.read_u32(4)?;
            if group_other_mask & 0b0001 != 0 {
                let object_width_mode = br.read_bit()?;
                if !object_width_mode {
                    let _object_width_code = br.read_u32(5)?;
                } else {
                    let _x = br.read_u32(5)?;
                    let _y = br.read_u32(5)?;
                    let _z = br.read_u32(5)?;
                }
            }
            if group_other_mask & 0b0010 != 0 {
                let _object_screen_factor_code = br.read_u32(3)?;
                let _object_depth_factor = br.read_u32(2)?;
            }
            if group_other_mask & 0b0100 != 0 {
                let b_obj_at_infinity = br.read_bit()?;
                if !b_obj_at_infinity {
                    let _obj_distance_factor_code = br.read_u32(4)?;
                }
            }
            if group_other_mask & 0b1000 != 0 {
                let object_div_mode = br.read_u32(2)?;
                if object_div_mode == 0b00 {
                    let _object_div_table = br.read_u32(2)?;
                } else if object_div_mode & 0b10 != 0 {
                    let _object_div_code = br.read_u32(6)?;
                }
            }
        }
    }

    Ok(out)
}

/// `object_info_block(b_no_delta, b_dynamic_object)` (§6.2.8.5).
#[derive(Debug, Clone, Copy, Default)]
pub struct ObjectInfoBlock {
    pub active: bool,
    pub basic_info: Option<ObjectBasicInfo>,
    pub render_info: Option<ObjectRenderInfo>,
}

fn parse_object_info_block(
    br: &mut BitReader<'_>,
    b_no_delta: bool,
    b_dynamic_object: bool,
    prev_position: Option<ObjectPosition>,
) -> Result<ObjectInfoBlock> {
    let mut out = ObjectInfoBlock::default();
    let b_object_not_active = br.read_bit()?;
    out.active = !b_object_not_active;

    let basic_all_new = if b_object_not_active {
        false
    } else if b_no_delta {
        true
    } else {
        !br.read_bit()? // b_basic_info_reuse
    };
    if basic_all_new {
        out.basic_info = Some(parse_object_basic_info(br)?);
    }

    let (render_all_new, render_part_reuse) = if b_object_not_active {
        (false, false)
    } else if b_dynamic_object {
        if b_no_delta {
            (true, false)
        } else {
            let b_render_info_reuse = br.read_bit()?;
            if b_render_info_reuse {
                (false, false)
            } else {
                let b_render_info_partial_reuse = br.read_bit()?;
                (!b_render_info_partial_reuse, b_render_info_partial_reuse)
            }
        }
    } else {
        (false, false)
    };
    if render_all_new || render_part_reuse {
        out.render_info = Some(parse_object_render_info(
            br,
            render_all_new,
            b_no_delta,
            prev_position,
        )?);
    }

    let b_add_table_data = br.read_bit()?;
    if b_add_table_data {
        return Err(Error::unsupported(
            "ac4: object_info_block add_table_data (add_per_object_md) not implemented",
        ));
    }

    Ok(out)
}

/// `oamd_dyndata_single(n_objs, n_blocks, b_iframe, b_alternative, ...)`
/// (§6.2.8.3) for one signal set (either the A-JOC downmix or upmix
/// side — the caller supplies `obj_type`/`b_lfe` per signal, matching
/// the spec's `obj_type[n_objs]`/`b_lfe[n_objs]` parameters).
#[derive(Debug, Clone, Default)]
pub struct OamdDyndataSingle {
    /// Per-signal, per-block info (`[signal][block]`).
    pub blocks: Vec<Vec<ObjectInfoBlock>>,
}

pub fn parse_oamd_dyndata_single(
    br: &mut BitReader<'_>,
    n_blocks: u32,
    b_iframe: bool,
    b_alternative: bool,
    obj_type: &[crate::toc::ObjType],
    b_lfe: &[bool],
) -> Result<OamdDyndataSingle> {
    let n_objs = obj_type.len();
    let mut out = OamdDyndataSingle {
        blocks: Vec::with_capacity(n_objs),
    };

    for i in 0..n_objs {
        let b_dynamic_object = obj_type[i] == crate::toc::ObjType::Dyn && !b_lfe[i];
        let mut per_block = Vec::with_capacity(n_blocks as usize);
        let mut prev_position = None;
        for b in 0..n_blocks {
            let b_no_delta = b_iframe && b == 0;
            let block = parse_object_info_block(br, b_no_delta, b_dynamic_object, prev_position)?;
            if let Some(render) = block.render_info.as_ref() {
                if let Some(pos) = render.position {
                    prev_position = Some(pos);
                }
            }
            per_block.push(block);
        }
        out.blocks.push(per_block);
    }

    if b_alternative {
        let _b_ducking_disabled = br.read_bit()?;
        let object_sound_category = br.read_u32(2)?;
        if object_sound_category == 3 {
            let _ = crate::toc::variable_bits(br, 2)?;
        }
        let mut n_alt_data_sets = br.read_u32(2)?;
        if n_alt_data_sets == 3 {
            n_alt_data_sets += crate::toc::variable_bits(br, 2)?;
        }
        for _ in 0..n_alt_data_sets {
            let b_keep = br.read_bit()?;
            if !b_keep {
                let is_isf = obj_type.first() == Some(&crate::toc::ObjType::Isf);
                let n_data_points = if is_isf {
                    1
                } else {
                    let b_common_data = br.read_bit()?;
                    if b_common_data {
                        1
                    } else {
                        n_objs as u32
                    }
                };
                for dp in 0..n_data_points as usize {
                    let ty = obj_type.get(dp).copied().unwrap_or(crate::toc::ObjType::Dyn);
                    match ty {
                        crate::toc::ObjType::Bed | crate::toc::ObjType::Isf => {
                            let b_alt_gain = br.read_bit()?;
                            if b_alt_gain {
                                let _alt_obj_gain = br.read_u32(6)?;
                            }
                        }
                        crate::toc::ObjType::Dyn => {
                            let b_alt_gain = br.read_bit()?;
                            if b_alt_gain {
                                let _alt_obj_gain = br.read_u32(6)?;
                            }
                            if !b_lfe.get(dp).copied().unwrap_or(false) {
                                let b_alt_position = br.read_bit()?;
                                if b_alt_position {
                                    let _x = br.read_u32(6)?;
                                    let _y = br.read_u32(6)?;
                                    let _z_sign = br.read_bit()?;
                                    let _z = br.read_u32(4)?;
                                }
                            }
                        }
                    }
                }
            }
            let b_additional_data = br.read_bit()?;
            if b_additional_data {
                return Err(Error::unsupported(
                    "ac4: oamd_dyndata_single alternative additional_data \
                     (ext_prec_alt_pos) not implemented",
                ));
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::bits::BitWriter;

    #[test]
    fn oamd_timing_data_minimal_no_blocks() {
        let mut bw = BitWriter::new();
        bw.write_bit(false); // oa_sample_offset_type first bit = 0
        bw.write_u32(0, 3); // num_obj_info_blocks = 0
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let out = parse_oamd_timing_data(&mut br).unwrap();
        assert_eq!(out.num_obj_info_blocks, 0);
    }

    #[test]
    fn oamd_timing_data_one_block_simple_ramp() {
        let mut bw = BitWriter::new();
        bw.write_bit(false); // oa_sample_offset_type = 0
        bw.write_u32(1, 3); // num_obj_info_blocks = 1
        bw.write_u32(5, 6); // block_offset_factor
        bw.write_u32(0b01, 2); // ramp_duration_code != 0b11 -> no extra bits
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let out = parse_oamd_timing_data(&mut br).unwrap();
        assert_eq!(out.num_obj_info_blocks, 1);
    }

    #[test]
    fn oamd_timing_data_escape_offset_type_11() {
        let mut bw = BitWriter::new();
        bw.write_bit(true); // oa_sample_offset_type first bit
        bw.write_bit(true); // second bit -> 0b11
        bw.write_u32(3, 5); // oa_sample_offset
        bw.write_u32(0, 3); // num_obj_info_blocks = 0
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let out = parse_oamd_timing_data(&mut br).unwrap();
        assert_eq!(out.num_obj_info_blocks, 0);
    }

    #[test]
    fn object_info_block_not_active_reads_only_two_flags() {
        let mut bw = BitWriter::new();
        bw.write_bit(true); // b_object_not_active = 1
        bw.write_bit(false); // b_add_table_data = 0
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let block = parse_object_info_block(&mut br, true, true, None).unwrap();
        assert!(!block.active);
        assert!(block.basic_info.is_none());
        assert!(block.render_info.is_none());
    }

    #[test]
    fn object_info_block_iframe_dynamic_all_new_reads_position() {
        // b_no_delta = true, b_dynamic_object = true -> both basic_info
        // and render_info are ALL_NEW (no reuse bits read).
        let mut bw = BitWriter::new();
        bw.write_bit(false); // b_object_not_active = 0
                            // object_basic_info(): b_default_basic_info_md = 1 -> nothing else.
        bw.write_bit(true);
        // object_render_info(ALL_NEW, ...): position/zone/otherprops all present.
        bw.write_u32(10, 6); // pos3D_X
        bw.write_u32(20, 6); // pos3D_Y
        bw.write_bit(false); // pos3D_Z_sign
        bw.write_u32(4, 4); // pos3D_Z
        bw.write_bit(true); // b_grouped_zone_defaults = 1 (no zone bits)
        bw.write_bit(true); // b_grouped_other_defaults = 1 (no otherprops bits)
        bw.write_bit(false); // b_add_table_data = 0
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let block = parse_object_info_block(&mut br, true, true, None).unwrap();
        assert!(block.active);
        assert!(block.basic_info.is_some());
        let render = block.render_info.unwrap();
        let pos = render.position.unwrap();
        assert_eq!(pos.x, 10);
        assert_eq!(pos.y, 20);
        assert!(!pos.z_sign);
        assert_eq!(pos.z, 4);
    }

    #[test]
    fn oamd_dyndata_single_one_dynamic_object_no_alternative() {
        use crate::toc::ObjType;
        let mut bw = BitWriter::new();
        // object_info_block for the sole DYN object, b_no_delta=true (iframe, block 0).
        bw.write_bit(false); // b_object_not_active = 0
        bw.write_bit(true); // b_default_basic_info_md = 1
        bw.write_u32(1, 6); // pos3D_X
        bw.write_u32(2, 6); // pos3D_Y
        bw.write_bit(false); // pos3D_Z_sign
        bw.write_u32(3, 4); // pos3D_Z
        bw.write_bit(true); // b_grouped_zone_defaults
        bw.write_bit(true); // b_grouped_other_defaults
        bw.write_bit(false); // b_add_table_data = 0
                            // b_alternative = false, so nothing else to write.
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let out = parse_oamd_dyndata_single(&mut br, 1, true, false, &[ObjType::Dyn], &[false])
            .unwrap();
        assert_eq!(out.blocks.len(), 1);
        assert_eq!(out.blocks[0].len(), 1);
        let pos = out.blocks[0][0].render_info.as_ref().unwrap().position.unwrap();
        assert_eq!((pos.x, pos.y, pos.z_sign, pos.z), (1, 2, false, 3));
    }

    #[test]
    fn oamd_dyndata_single_bed_object_skips_render_info() {
        use crate::toc::ObjType;
        // A BED object never has render_info (only DYN objects with
        // b_lfe == false do) -- object_info_block should just read
        // b_object_not_active, basic_info, and b_add_table_data.
        let mut bw = BitWriter::new();
        bw.write_bit(false); // b_object_not_active = 0
        bw.write_bit(true); // b_default_basic_info_md = 1
        bw.write_bit(false); // b_add_table_data = 0
        bw.align_to_byte();
        let bytes = bw.finish();
        let mut br = BitReader::new(&bytes);
        let out = parse_oamd_dyndata_single(&mut br, 1, true, false, &[ObjType::Bed], &[false])
            .unwrap();
        assert!(out.blocks[0][0].render_info.is_none());
    }
}
