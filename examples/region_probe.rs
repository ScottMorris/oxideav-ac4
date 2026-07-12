//! Round 416: parse the P-frame "region" ([lfe_end .. war-proven 3ch
//! start]) as candidate A-CPL / A-SPX payload sequences. Frame 1's
//! target end is 1808-1811 (exact-end war anchor).
use oxideav_ac4::acpl::{parse_acpl_data_1ch, AcplQuantMode};
use oxideav_ac4::asf::{parse_aspx_data_1ch_body, parse_aspx_data_2ch_body, SubstreamTools};
use oxideav_ac4::aspx::{parse_aspx_config, AspxConfig};
use oxideav_ac4::mch::parse_mono_data;
use oxideav_ac4::toc;
use oxideav_core::bits::BitReader;
use std::{env, fs};

include!("mp4_helper.rs");

const TL: u32 = 2048;

fn main() {
    let path = env::args().nth(1).expect("mp4");
    let frames_wanted: Vec<usize> = env::args()
        .nth(2)
        .map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_else(|| vec![1, 2, 5]);
    let data = fs::read(&path).expect("read");
    let frames = mp4::extract_ac4_samples(&data).expect("mp4");

    // Harvest the aspx config from frame 0 (mode2 head: 2-bit code,
    // cfg@18).
    let mut cfg: Option<AspxConfig> = None;
    {
        let payload = &frames[0];
        let info = toc::parse_ac4_toc(payload).unwrap();
        let base = (info.toc_size + info.payload_base) as usize;
        let idx = info.substream_index.unwrap_or(0) as usize;
        let start = base
            + info.substream_sizes.iter().take(idx).map(|&s| s as usize).sum::<usize>();
        let sb = &payload[start..];
        let mut br = BitReader::new(sb);
        let _ = br.read_u32(16);
        let _ = br.read_u32(2);
        cfg = parse_aspx_config(&mut br).ok();
    }
    let cfg = cfg.expect("cfg");
    eprintln!("cfg: {cfg:?}");

    for &fi in &frames_wanted {
        let payload = &frames[fi];
        let Ok(info) = toc::parse_ac4_toc(payload) else { continue };
        let base = (info.toc_size + info.payload_base) as usize;
        let idx = info.substream_index.unwrap_or(0) as usize;
        let start = base
            + info.substream_sizes.iter().take(idx).map(|&s| s as usize).sum::<usize>();
        let end = info
            .substream_sizes
            .get(idx)
            .map(|&s| (start + s as usize).min(payload.len()))
            .unwrap_or(payload.len());
        let sb = &payload[start..end];
        let mut br = BitReader::new(sb);
        let mut audio_size = br.read_u32(15).unwrap();
        if br.read_bit().unwrap() {
            audio_size += oxideav_ac4::toc::variable_bits(&mut br, 7).unwrap() << 15;
        }
        br.align_to_byte();
        // P head: 2-bit mode + 4-bit field.
        let _ = br.read_u32(6);
        let _lfe = parse_mono_data(&mut br, true, TL).unwrap();
        let lfe_end = br.bit_position();
        println!("frame {fi}: lfe_end={lfe_end} wall={}", br.bit_position() + 0);

        // Candidate sequences from lfe_end.
        let quants = [AcplQuantMode::Fine, AcplQuantMode::Coarse];
        let bandss = [7u32, 9, 12, 15];
        // A) acpl x k
        for &q in &quants {
            for &nb in &bandss {
                for k in 1..=8u32 {
                    let mut b = br;
                    let mut ok = true;
                    for _ in 0..k {
                        if parse_acpl_data_1ch(&mut b, nb, 0, q).is_err() {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        println!(
                            "  ACPLx{k} nb={nb} q={q:?} -> end={}",
                            b.bit_position()
                        );
                    }
                }
            }
        }
        // C) ajcc_data — the A-JCC joint-channel payload (both
        // b_5fronts values), with a small leading-offset sweep.
        for b5 in [false, true] {
            for k in 0..10u32 {
                let mut b = br;
                if k > 0 && b.skip(k).is_err() {
                    break;
                }
                if let Ok(_a) = oxideav_ac4::ajcc::parse_ajcc_data(&mut b, b5) {
                    println!("  AJCC+{k} b5={b5} -> end={}", b.bit_position());
                }
            }
            let mut b = br;
            if oxideav_ac4::aspx::parse_companding_control(&mut b, 5).is_ok() {
                if let Ok(_a) = oxideav_ac4::ajcc::parse_ajcc_data(&mut b, b5) {
                    println!("  COMP5+AJCC b5={b5} -> end={}", b.bit_position());
                }
            }
        }
        // B) aspx P-form sequences: orders (2,2,2,1) and (2,2,1,2).
        for (name, order) in [("aspx2221", [2u8, 2, 2, 1]), ("aspx2212", [2, 2, 1, 2])] {
            for xover in 0..7u8 {
                let mut b = br;
                let mut ok = true;
                for &o in &order {
                    let mut t = Box::<SubstreamTools>::default();
                    t.aspx_xover_subband_offset = Some(xover);
                    let r = if o == 2 {
                        parse_aspx_data_2ch_body(&mut b, &mut t, &cfg, false, TL)
                    } else {
                        parse_aspx_data_1ch_body(&mut b, &mut t, &cfg, false, TL)
                    };
                    if r.is_err() {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    println!("  {name} xover={xover} -> end={}", b.bit_position());
                }
            }
        }
    }
}
