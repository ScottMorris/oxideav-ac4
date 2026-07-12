//! Round 414/415: A-JOC route decoder — the pinned geometry (3+LFE
//! dmx, 1 umx) through the full object pipeline, with I-frame-sticky
//! A-SPX state. Outputs interleaved f32: dmx0, dmx1, dmx2, LFE, obj0
//! (5 channels).
use oxideav_ac4::ajoc_substream::{AjocBodyParams, AjocSubstreamDecoder};
use oxideav_ac4::oamd::ObjType;
use oxideav_ac4::toc;
use std::collections::BTreeMap;
use std::{env, fs, io::Write};

include!("mp4_helper.rs");

const CH_OUT: usize = 5;

fn main() {
    let path = env::args().nth(1).expect("mp4");
    let out = env::args().nth(2).expect("out.f32");
    let max_frames: usize = env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let n_umx: u32 = env::args().nth(4).and_then(|v| v.parse().ok()).unwrap_or(1);
    let data = fs::read(&path).expect("read");
    let frames = mp4::extract_ac4_samples(&data).expect("mp4");
    let params = AjocBodyParams {
        b_lfe: true,
        b_static_dmx: false,
        n_fullband_dmx_signals: 3,
        n_fullband_upmix_signals: n_umx,
        obj_type_dmx: vec![ObjType::Dyn, ObjType::Bed, ObjType::Bed, ObjType::Bed],
        obj_type_umx: std::iter::once(ObjType::Dyn)
            .chain(std::iter::repeat(ObjType::Isf).take(n_umx as usize))
            .collect(),
    };
    let mut dec = AjocSubstreamDecoder::new(3, n_umx as usize);
    let mut f = fs::File::create(&out).unwrap();
    let (mut ok, mut fail) = (0usize, 0usize);
    let mut classes: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, payload) in frames.iter().enumerate() {
        if i >= max_frames { break; }
        let Ok(info) = toc::parse_ac4_toc(payload) else {
            fail += 1;
            classes.entry("toc parse".into()).or_default().push(i);
            continue;
        };
        let b_iframe = info.presentations.first().map(|p| p.b_iframe).unwrap_or(info.b_iframe_global);
        let base = (info.toc_size + info.payload_base) as usize;
        let idx = info.substream_index.unwrap_or(0) as usize;
        let start = base + info.substream_sizes.iter().take(idx).map(|&s| s as usize).sum::<usize>();
        let end = info.substream_sizes.get(idx).map(|&s| (start + s as usize).min(payload.len())).unwrap_or(payload.len());
        if start >= payload.len() {
            fail += 1;
            classes.entry("substream bounds".into()).or_default().push(i);
            continue;
        }
        let sb = &payload[start..end];
        let n = info.frame_length as usize;
        if std::env::var_os("AC4_AJOC_TRACE").is_some() {
            eprintln!("FRAME {i} iframe={} sub_len={}", u8::from(b_iframe), sb.len());
        }
        if let Some(dir) = std::env::var_os("AC4_DUMP_SUB_DIR") {
            let p = std::path::Path::new(&dir).join(format!("sub{i:03}.bin"));
            let _ = fs::write(p, sb);
        }
        match dec.decode_substream_pcm(sb, &params, b_iframe, std::env::var_os("AC4_B_ALT").is_some(), info.frame_length) {
            Ok((objs, dmx, lfe, _ajoc, _meta)) => {
                ok += 1;
                let zero = vec![0f32; n];
                let o0 = objs.first().map(|v| v.as_slice()).unwrap_or(&zero);
                let lf = lfe.as_deref().unwrap_or(&zero);
                let mut chans: Vec<&[f32]> = Vec::with_capacity(CH_OUT);
                for c in 0..3 {
                    chans.push(dmx.get(c).map(|v| v.as_slice()).unwrap_or(&zero));
                }
                chans.push(lf);
                chans.push(o0);
                let mut buf = Vec::with_capacity(n * CH_OUT * 4);
                for k in 0..n {
                    for ch in &chans {
                        buf.extend_from_slice(&ch.get(k).copied().unwrap_or(0.0).to_le_bytes());
                    }
                }
                f.write_all(&buf).unwrap();
            }
            Err(e) => {
                fail += 1;
                classes.entry(format!("{e:?}")).or_default().push(i);
                let buf = vec![0u8; n * CH_OUT * 4];
                f.write_all(&buf).unwrap();
            }
        }
    }
    println!("AJOC ROUTE: ok {ok} fail {fail}");
    for (cls, frames) in &classes {
        let head: Vec<String> = frames.iter().take(12).map(|v| v.to_string()).collect();
        println!("  {:4} x {} [{}{}]", frames.len(), cls, head.join(","),
            if frames.len() > 12 { ",..." } else { "" });
    }
}
