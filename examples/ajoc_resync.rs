//! Round 415: A-JOC P-frame resync probe. For each frame, searches
//! gap0 (after the LFE) x gap1 (after the pair / three element) with
//! STRICT body validation (every channel body must fully decode) and
//! the full audio_data_ajoc tail parse, accepting candidates whose
//! walk closes at the audio_size wall. Prints every hit.
//!
//! Usage: ajoc_resync <mp4> <max_frames> [max_gap]
use oxideav_ac4::ajoc_data::new_ajoc_diff_state;
use oxideav_ac4::ajoc_substream::{
    parse_audio_data_ajoc_sticky, parse_audio_data_ajoc_tail, AjocBodyParams, VarChannelElement,
};
use oxideav_ac4::asf::{parse_aspx_data_1ch_body, parse_aspx_data_2ch_body, SubstreamTools};
use oxideav_ac4::aspx::{parse_aspx_config, parse_companding_control, AspxConfig};
use oxideav_ac4::mch::{parse_mono_data, parse_three_channel_data, parse_two_channel_data};
use oxideav_ac4::oamd::ObjType;
use oxideav_ac4::toc;
use oxideav_core::bits::BitReader;
use std::{env, fs};

include!("mp4_helper.rs");

const TL: u32 = 2048;

fn pair_ok(p: &oxideav_ac4::mch::TwoChannelData) -> bool {
    (0..2).all(|c| {
        p.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
            || p.scaled_spec_windows_per_channel
                .get(c)
                .map_or(false, |s| s.is_some())
    })
}

fn mono_ok(m: &oxideav_ac4::mch::MonoLfeData) -> bool {
    m.scaled_spec.is_some() || m.scaled_spec_windows.is_some()
}

fn three_ok(t: &oxideav_ac4::mch::ThreeChannelData) -> bool {
    (0..3).all(|c| {
        t.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
            || t.scaled_spec_windows_per_channel
                .get(c)
                .map_or(false, |s| s.is_some())
    })
}

#[allow(clippy::too_many_arguments)]
fn try_tail(
    mut b: BitReader<'_>,
    params: &AjocBodyParams,
    cfg: &AspxConfig,
    xover: u8,
    state: &oxideav_ac4::ajoc::AjocDiffState,
    wall_bits: u64,
) -> Option<(u64, i64)> {
    // aspx_data_2ch + aspx_data_1ch (P-frame, sticky config).
    let mut t2 = Box::<SubstreamTools>::default();
    t2.aspx_xover_subband_offset = Some(xover);
    parse_aspx_data_2ch_body(&mut b, &mut t2, cfg, false, TL).ok()?;
    let mut t1 = Box::<SubstreamTools>::default();
    t1.aspx_xover_subband_offset = Some(xover);
    parse_aspx_data_1ch_body(&mut b, &mut t1, cfg, false, TL).ok()?;
    let aspx_end = b.bit_position();
    let mut st = state.clone();
    parse_audio_data_ajoc_tail(
        &mut b,
        params,
        false,
        false,
        &mut st,
        None,
        None,
        Some(VarChannelElement::default()),
    )
    .ok()?;
    let residue = wall_bits as i64 - b.bit_position() as i64;
    Some((aspx_end, residue))
}

fn main() {
    let path = env::args().nth(1).expect("mp4");
    let max_frames: usize = env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(30);
    let max_gap: u32 = env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(64);
    let data = fs::read(&path).expect("read");
    let frames = mp4::extract_ac4_samples(&data).expect("mp4");
    let params = AjocBodyParams {
        b_lfe: true,
        b_static_dmx: false,
        n_fullband_dmx_signals: 3,
        n_fullband_upmix_signals: 1,
        obj_type_dmx: vec![ObjType::Dyn, ObjType::Bed, ObjType::Bed, ObjType::Bed],
        obj_type_umx: vec![ObjType::Dyn, ObjType::Isf],
    };
    let mut ajoc_state = new_ajoc_diff_state(1, 3, 7);
    let mut sticky: Option<(AspxConfig, u8)> = None;

    for (i, payload) in frames.iter().enumerate() {
        if i >= max_frames {
            break;
        }
        let Ok(info) = toc::parse_ac4_toc(payload) else { continue };
        let b_iframe = info
            .presentations
            .first()
            .map(|p| p.b_iframe)
            .unwrap_or(info.b_iframe_global);
        let base = (info.toc_size + info.payload_base) as usize;
        let idx = info.substream_index.unwrap_or(0) as usize;
        let start = base
            + info
                .substream_sizes
                .iter()
                .take(idx)
                .map(|&s| s as usize)
                .sum::<usize>();
        let end = info
            .substream_sizes
            .get(idx)
            .map(|&s| (start + s as usize).min(payload.len()))
            .unwrap_or(payload.len());
        if start >= payload.len() {
            continue;
        }
        let sb = &payload[start..end];

        if b_iframe {
            // Establish sticky A-SPX + ajoc diff state from I-frames
            // via the normal parse.
            let mut br = BitReader::new(sb);
            let mut audio_size = br.read_u32(15).unwrap();
            if br.read_bit().unwrap() {
                audio_size += oxideav_ac4::toc::variable_bits(&mut br, 7).unwrap() << 15;
            }
            br.align_to_byte();
            match parse_audio_data_ajoc_sticky(
                &mut br, &params, true, false, TL, &mut ajoc_state, None,
            ) {
                Ok(ajoc) => {
                    if let Some(ve) = ajoc.var_element.as_ref() {
                        let xo = ve
                            .aspx_pair_tools
                            .first()
                            .and_then(|t| t.aspx_xover_subband_offset)
                            .or_else(|| {
                                ve.aspx_odd_tools
                                    .as_ref()
                                    .and_then(|t| t.aspx_xover_subband_offset)
                            });
                        if let (Some(cfg), Some(x)) = (ve.aspx_config.clone(), xo) {
                            sticky = Some((cfg, x));
                        }
                    }
                    println!("frame {i} IFRAME parsed ok");
                }
                Err(e) => println!("frame {i} IFRAME parse error: {e:?}"),
            }
            continue;
        }
        let Some((cfg, xover)) = sticky.clone() else {
            println!("frame {i} skipped (no sticky yet)");
            continue;
        };

        let mut br = BitReader::new(sb);
        let mut audio_size = br.read_u32(15).unwrap();
        if br.read_bit().unwrap() {
            audio_size += oxideav_ac4::toc::variable_bits(&mut br, 7).unwrap() << 15;
        }
        br.align_to_byte();
        let audio_start_bits = br.bit_position();
        let wall_bits = audio_start_bits + audio_size as u64 * 8;

        // Head: b_some_signals_inactive (+mask), aspx_mode, companding,
        // LFE.
        if br.read_bit().unwrap_or(true) {
            let _ = br.read_u32(params.n_fullband_dmx_signals);
        }
        let Ok(aspx_mode) = br.read_bit() else { continue };
        if !aspx_mode {
            println!("frame {i}: aspx_mode=0 (unexpected)");
            continue;
        }
        if parse_companding_control(&mut br, params.n_fullband_dmx_signals).is_err() {
            continue;
        }
        let Ok(lfe) = parse_mono_data(&mut br, true, TL) else { continue };
        let lfe_decoded = mono_ok(&lfe);
        let br_lfe = br;

        let mut hits = 0u32;
        for gap0 in 0..=max_gap {
            let mut b = br_lfe;
            if gap0 > 0 && b.skip(gap0).is_err() {
                break;
            }
            let Ok(vcc) = b.read_bit() else { continue };
            if !vcc {
                let Ok(pair) = parse_two_channel_data(&mut b, TL) else { continue };
                if !pair_ok(&pair) {
                    continue;
                }
                let b_pair = b;
                for gap1 in 0..=max_gap {
                    let mut c = b_pair;
                    if gap1 > 0 && c.skip(gap1).is_err() {
                        break;
                    }
                    let Ok(mono) = parse_mono_data(&mut c, false, TL) else { continue };
                    if !mono_ok(&mono) {
                        continue;
                    }
                    if let Some((_ae, res)) =
                        try_tail(c, &params, &cfg, xover, &ajoc_state, wall_bits)
                    {
                        if (0..=200).contains(&res) {
                            println!(
                                "frame {i} HIT vcc=0 gap0={gap0} gap1={gap1} residue={res} lfe_ok={lfe_decoded}"
                            );
                            hits += 1;
                        }
                    }
                }
            } else {
                let Ok(three) = parse_three_channel_data(&mut b, TL) else { continue };
                if !three_ok(&three) {
                    continue;
                }
                let b_three = b;
                for gap1 in 0..=max_gap {
                    let mut c = b_three;
                    if gap1 > 0 && c.skip(gap1).is_err() {
                        break;
                    }
                    if let Some((_ae, res)) =
                        try_tail(c, &params, &cfg, xover, &ajoc_state, wall_bits)
                    {
                        if (0..=200).contains(&res) {
                            println!(
                                "frame {i} HIT vcc=1 gap0={gap0} gap1={gap1} residue={res} lfe_ok={lfe_decoded}"
                            );
                            hits += 1;
                        }
                    }
                }
            }
        }
        if hits == 0 {
            println!("frame {i}: no hits (lfe_ok={lfe_decoded})");
        }
    }
}
