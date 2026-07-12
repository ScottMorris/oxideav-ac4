use oxideav_ac4::ajoc::AjocDiffState;
use oxideav_ac4::ajoc_substream::{parse_audio_data_ajoc, AjocBodyParams};
use oxideav_ac4::oamd::ObjType;
use oxideav_core::bits::BitReader;
use std::{env, fs};

fn main() {
    let path = env::args().nth(1).expect("substream .bin");
    let b_iframe: bool = env::args().nth(2).map(|v| v == "1").unwrap_or(true);
    let n_umx: u32 = env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(1);
    let static_dmx: bool = env::args().nth(4).map(|v| v == "1").unwrap_or(true);
    let n_dmx: u32 = env::args().nth(5).and_then(|v| v.parse().ok()).unwrap_or(5);
    let data = fs::read(&path).expect("read");
    // header: audio_size(15) + b_more_bits(1) [+ variable_bits], byte-align
    let mut hr = BitReader::new(&data);
    let _short = hr.read_u32(15).unwrap();
    let b_more = hr.read_bit().unwrap();
    if b_more {
        // variable_bits(7): read chunks
        loop {
            let _c = hr.read_u32(7).unwrap();
            if hr.read_bit().unwrap() == false { break; }
        }
    }
    hr.align_to_byte();
    let body_off = hr.byte_position();
    // Params from upstream's TOC read of frame 0, first ajoc descriptor:
    // b_lfe=true, b_static_dmx=true, n_dmx=5, n_umx=1 (isf_config=4).
    let params = AjocBodyParams {
        b_lfe: true,
        b_static_dmx: static_dmx,
        n_fullband_dmx_signals: n_dmx,
        n_fullband_upmix_signals: n_umx,
        obj_type_dmx: std::iter::once(ObjType::Dyn).chain(std::iter::repeat(ObjType::Bed).take(n_dmx as usize)).collect(),
        obj_type_umx: std::iter::once(ObjType::Dyn).chain(std::iter::repeat(ObjType::Isf).take(n_umx as usize)).collect(),
    };
    let mut state = AjocDiffState::new((n_umx as usize)+2, (n_dmx as usize)+2, 4, 16);
    let mut br = BitReader::with_position(&data, body_off);
    let total_bits = data.len() as u64 * 8;
    match parse_audio_data_ajoc(&mut br, &params, b_iframe, false, 2048, &mut state) {
        Ok(_) => {
            let end = br.bit_position();
            println!("AJOC-OK body[{}..{}] of {} bits (slack {} bits)",
                body_off*8, end, total_bits, total_bits as i64 - end as i64);
        }
        Err(e) => println!("AJOC-ERR {:?} @ bit {} of {}", e, br.bit_position(), total_bits),
    }
}
