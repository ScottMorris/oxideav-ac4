//! Round 415 probe: after the (proven-music) first audio_data_ajoc
//! chain, try parsing the unread remainder as a SECOND element —
//! var_channel_element or full audio_data_ajoc — at every small bit
//! offset, and report ends vs the audio_size wall.
use oxideav_ac4::ajoc_data::new_ajoc_diff_state;
use oxideav_ac4::ajoc_substream::{parse_audio_data_ajoc_sticky, AjocBodyParams};
use oxideav_ac4::aspx::AspxConfig;
use oxideav_ac4::oamd::ObjType;
use oxideav_ac4::toc;
use oxideav_core::bits::BitReader;
use std::{env, fs};

include!("mp4_helper.rs");

const TL: u32 = 2048;

fn main() {
    let path = env::args().nth(1).expect("mp4");
    let max_frames: usize = env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(30);
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
    let mut state1 = new_ajoc_diff_state(1, 3, 7);
    let mut sticky: Option<(AspxConfig, u8)> = None;

    for (i, payload) in frames.iter().enumerate().take(max_frames) {
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

        let mut br = BitReader::new(sb);
        let mut audio_size = br.read_u32(15).unwrap();
        if br.read_bit().unwrap() {
            audio_size += oxideav_ac4::toc::variable_bits(&mut br, 7).unwrap() << 15;
        }
        br.align_to_byte();
        let wall = br.bit_position() + audio_size as u64 * 8;
        let sticky_ref = sticky.as_ref().map(|(c, x)| (c, *x));
        let Ok(first) = parse_audio_data_ajoc_sticky(
            &mut br, &params, b_iframe, false, TL, &mut state1, sticky_ref,
        ) else {
            println!("frame {i}: first chain error");
            continue;
        };
        if b_iframe {
            if let Some(ve) = first.var_element.as_ref() {
                let xo = ve
                    .aspx_pair_tools
                    .first()
                    .and_then(|t| t.aspx_xover_subband_offset);
                if let (Some(cfg), Some(x)) = (ve.aspx_config.clone(), xo) {
                    sticky = Some((cfg, x));
                }
            }
        }
        let end1 = br.bit_position();
        let deficit = wall as i64 - end1 as i64;
        if deficit < 300 {
            println!("frame {i}: first chain closes (deficit {deficit})");
            continue;
        }
        // Backchain the TAIL (b_dmx_timing .. umx_dyndata) from the
        // wall: find candidate start positions whose tail parse lands
        // within [wall-96, wall].
        let mut hits: Vec<(u64, i64)> = Vec::new();
        let scan_lo = wall.saturating_sub(6000).max(end1);
        for cand in scan_lo..wall.saturating_sub(16) {
            let mut b2 = BitReader::new(sb);
            let skip_total = cand as u32;
            if b2.skip(skip_total).is_err() {
                break;
            }
            let mut st2 = new_ajoc_diff_state(1, 3, 7);
            if let Ok(tail) = oxideav_ac4::ajoc_substream::parse_audio_data_ajoc_tail(
                &mut b2,
                &params,
                b_iframe,
                false,
                &mut st2,
                None,
                None,
                Some(Default::default()),
            ) {
                let res = wall as i64 - b2.bit_position() as i64;
                if (0..=96).contains(&res) {
                    let ctrl = &tail.ajoc_frame.ctrl;
                    let objpres: String = ctrl
                        .object_present
                        .iter()
                        .map(|&p| if p { '1' } else { '0' })
                        .collect();
                    let ndp = ctrl.data_point_info.num_dpoints;
                    let nbc: Vec<u8> = ctrl.num_bands_code.clone();
                    println!(
                        "TAIL f={i} cand={cand} len={} res={res} ndec={} ndp={ndp} objpres={objpres} nbc={:?} dmxT={} umxT={} oamdx={}",
                        wall - cand,
                        tail.ajoc_frame.num_decorr,
                        nbc,
                        u8::from(tail.dmx_timing.is_some()),
                        u8::from(tail.umx_timing.is_some()),
                        u8::from(tail.oamd_extension.is_some()),
                    );
                    hits.push((cand, res));
                }
            }
        }
        println!(
            "frame {i}: deficit {deficit} our_tail@{end1} wall@{wall} tail_hits={}",
            hits.len()
        );
    }
}
