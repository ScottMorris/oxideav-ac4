//! Dump the first N bytes of an AC-4 frame payload as bits.
use std::{env, fs};
include!("mp4_helper.rs");
fn main() {
    let path = env::args().nth(1).expect("mp4");
    let idx: usize = env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(0);
    let nbytes: usize = env::args().nth(3).and_then(|v| v.parse().ok()).unwrap_or(32);
    let data = fs::read(&path).expect("read");
    let frames = mp4::extract_ac4_samples(&data).expect("mp4");
    let p = &frames[idx];
    println!("frame {idx} len {}", p.len());
    let n = nbytes.min(p.len());
    let bits: String = p[..n].iter().map(|b| format!("{b:08b}")).collect();
    for (k, chunk) in bits.as_bytes().chunks(64).enumerate() {
        println!("{:4} {}", k * 64, std::str::from_utf8(chunk).unwrap());
    }
}
