//! Dump per-frame substream payloads (16-bit audio_size header intact)
//! as subNNNN.bin for offline bitstream analysis.
//! Usage: dump_subs <in.mp4> <outdir> [max_frames]
use oxideav_ac4::toc;
use std::{env, fs};

include!("mp4_helper.rs");

fn main() {
    let path = env::args().nth(1).expect("mp4");
    let out = env::args().nth(2).expect("outdir");
    let max_frames: usize = env::args()
        .nth(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let data = fs::read(&path).expect("read");
    let frames = mp4::extract_ac4_samples(&data).expect("mp4");
    fs::create_dir_all(&out).unwrap();
    let mut n = 0usize;
    for (i, payload) in frames.iter().enumerate() {
        if i >= max_frames {
            break;
        }
        let Ok(info) = toc::parse_ac4_toc(payload) else { continue };
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
        fs::write(format!("{out}/sub{i:04}.bin"), &payload[start..end]).unwrap();
        n += 1;
    }
    println!("wrote {n} substreams of {} frames", frames.len());
}
