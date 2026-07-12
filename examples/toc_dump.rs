//! Dump the parsed TOC (presentations + ajoc descriptors) for the
//! first frames of an mp4.
use oxideav_ac4::toc;
use std::{env, fs};
include!("mp4_helper.rs");
fn main() {
    let path = env::args().nth(1).expect("mp4");
    let n: usize = env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(2);
    let data = fs::read(&path).expect("read");
    let frames = mp4::extract_ac4_samples(&data).expect("mp4");
    for (i, payload) in frames.iter().enumerate().take(n) {
        match toc::parse_ac4_toc(payload) {
            Ok(info) => {
                println!("=== frame {i} ===");
                println!("frame_length={} toc_size={} payload_base={} sizes={:?} sub_index={:?}",
                    info.frame_length, info.toc_size, info.payload_base, info.substream_sizes, info.substream_index);
                for (k, p) in info.presentations.iter().enumerate() {
                    println!("pres[{k}]: b_iframe={}", p.b_iframe);
                }
                for (k, d) in info.ajoc_substreams.iter().enumerate() {
                    println!("ajoc[{k}]: {:#?}", d);
                }
            }
            Err(e) => println!("frame {i}: TOC error {e:?}"),
        }
    }
}
