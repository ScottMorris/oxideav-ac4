//! Round 414: A-JOC route decoder — the pinned geometry (3+LFE dmx,
//! 1 umx) through the full object pipeline, with I-frame-sticky
//! A-SPX state. Outputs interleaved f32 (dmx0,dmx1,dmx2,LFE,obj0).
use oxideav_ac4::ajoc_substream::{AjocBodyParams, AjocSubstreamDecoder};
use oxideav_ac4::oamd::ObjType;
use oxideav_ac4::toc;
use std::{env, fs, io::Write};

include!("mp4_helper.rs");

fn main() {
    let path = env::args().nth(1).expect("mp4");
    let out = env::args().nth(2).expect("out.f32");
    let max_frames: usize = env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
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
    let mut dec = AjocSubstreamDecoder::new(3, 1);
    let mut f = fs::File::create(&out).unwrap();
    let (mut ok, mut fail) = (0usize, 0usize);
    for (i, payload) in frames.iter().enumerate() {
        if i >= max_frames { break; }
        let Ok(info) = toc::parse_ac4_toc(payload) else { fail += 1; continue };
        let b_iframe = info.presentations.first().map(|p| p.b_iframe).unwrap_or(info.b_iframe_global);
        let base = (info.toc_size + info.payload_base) as usize;
        let idx = info.substream_index.unwrap_or(0) as usize;
        let start = base + info.substream_sizes.iter().take(idx).map(|&s| s as usize).sum::<usize>();
        let end = info.substream_sizes.get(idx).map(|&s| (start + s as usize).min(payload.len())).unwrap_or(payload.len());
        if start >= payload.len() { fail += 1; continue; }
        let sb = &payload[start..end];
        match dec.decode_substream_pcm(sb, &params, b_iframe, false, info.frame_length) {
            Ok((objs, lfe, ajoc, _meta)) => {
                ok += 1;
                // dmx spectra -> quick IMDCT via decoder? Objects PCM given.
                // Interleave: obj0 + LFE + flag frame ok. Also dmx via ajoc.var_element spectra len only.
                let n = info.frame_length as usize;
                let zero = vec![0f32; n];
                let o0 = objs.first().map(|v| v.as_slice()).unwrap_or(&zero);
                let lf = lfe.as_deref().unwrap_or(&zero);
                let mut buf = Vec::with_capacity(n * 2 * 4);
                for k in 0..n {
                    buf.extend_from_slice(&o0.get(k).copied().unwrap_or(0.0).to_le_bytes());
                    buf.extend_from_slice(&lf.get(k).copied().unwrap_or(0.0).to_le_bytes());
                }
                f.write_all(&buf).unwrap();
                let _ = ajoc;
            }
            Err(e) => {
                fail += 1;
                if fail <= 8 { eprintln!("frame {i}: {e:?}"); }
                let buf = vec![0u8; info.frame_length as usize * 2 * 4];
                f.write_all(&buf).unwrap();
            }
        }
    }
    println!("AJOC ROUTE: ok {ok} fail {fail}");
}
