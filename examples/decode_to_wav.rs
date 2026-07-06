//! `decode_to_wav` — decode a real AC-4 MP4 end-to-end through
//! `Ac4Decoder` and dump the result to WAV so it can actually be
//! listened to, rather than just inspected via per-frame diagnostics.
//!
//! Writes two files next to the input (or to `--out <prefix>`):
//!   `<prefix>.8ch.wav`    — full 8-channel (7.1, channel_mode 6:
//!                            L,R,C,Ls,Rs,Lb,Rb,LFE) interleaved PCM.
//!   `<prefix>.stereo.wav` — a quick-and-dirty stereo downmix
//!                            (L+0.707*C+0.707*Ls+0.707*Lb, mirrored for
//!                            R) purely for easy listening — NOT the
//!                            eventual "creative stereo renderer" this
//!                            repo still owes; just enough to sanity-
//!                            check real audio came out the other end.
//!
//! Usage: `decode_to_wav <file.mp4> [--out <prefix>] [--frames N]`

use std::env;
use std::fs;
use std::io::{self, Write};

use oxideav_ac4::toc;
use oxideav_core::{CodecId, CodecParameters, Decoder, Frame, Packet, TimeBase};

fn main() {
    let mut args = env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: decode_to_wav <file.mp4> [--out <prefix>] [--frames N]");
            std::process::exit(2);
        }
    };
    let mut out_prefix = path.trim_end_matches(".mp4").trim_end_matches(".m4a").to_string();
    let mut max_frames = usize::MAX;
    let rest: Vec<String> = args.collect();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--out" => {
                i += 1;
                if let Some(v) = rest.get(i) {
                    out_prefix = v.clone();
                }
            }
            "--frames" => {
                i += 1;
                if let Some(v) = rest.get(i) {
                    max_frames = v.parse().unwrap_or(usize::MAX);
                }
            }
            _ => {}
        }
        i += 1;
    }

    let data = fs::read(&path).unwrap_or_else(|e| {
        eprintln!("error reading {path}: {e}");
        std::process::exit(1);
    });
    let frames = mp4::extract_ac4_samples(&data).unwrap_or_else(|e| {
        eprintln!("error extracting AC-4 samples from {path}: {e}");
        std::process::exit(1);
    });
    println!("{}: {} AC-4 samples found", path, frames.len());

    let params = CodecParameters::audio(CodecId::new("ac4"));
    let mut dec = oxideav_ac4::decoder::Ac4Decoder::new(&params);

    let mut multich_pcm: Vec<u8> = Vec::new();
    let mut stereo_pcm: Vec<u8> = Vec::new();
    let mut n_channels = 8usize;
    let mut sample_rate = 48_000u32;
    let mut decoded = 0usize;
    let mut silent = 0usize;

    for (i, payload) in frames.iter().enumerate() {
        if i >= max_frames {
            break;
        }
        if let Ok(info) = toc::parse_ac4_toc(payload) {
            n_channels = info.channels.max(1) as usize;
            sample_rate = info.sample_rate.max(1);
        }
        let pkt = Packet::new(0, TimeBase::new(1, sample_rate.into()), payload.clone());
        if dec.send_packet(&pkt).is_err() {
            continue;
        }
        match dec.receive_frame() {
            Ok(Frame::Audio(af)) => {
                let interleaved = &af.data[0];
                let mut any_nonzero = false;
                for c in interleaved.chunks_exact(2) {
                    if c[0] != 0 || c[1] != 0 {
                        any_nonzero = true;
                        break;
                    }
                }
                if any_nonzero {
                    decoded += 1;
                } else {
                    silent += 1;
                }
                multich_pcm.extend_from_slice(interleaved);
                downmix_stereo_append(interleaved, n_channels, &mut stereo_pcm);
            }
            _ => continue,
        }
    }

    println!(
        "decoded {decoded} frames with audio, {silent} fully-silent frames ({} channels @ {sample_rate} Hz)",
        n_channels
    );

    let multich_path = format!("{out_prefix}.8ch.wav");
    let stereo_path = format!("{out_prefix}.stereo.wav");
    write_wav(&multich_path, &multich_pcm, n_channels as u16, sample_rate).unwrap();
    write_wav(&stereo_path, &stereo_pcm, 2, sample_rate).unwrap();
    println!("wrote {multich_path}");
    println!("wrote {stereo_path}");
}

/// Quick-and-dirty stereo downmix for listening only. Channel order per
/// channel_mode 6 (7.1, "3/4/0.1"): 0=L 1=R 2=C 3=Ls 4=Rs 5=Lb 6=Rb 7=LFE.
/// `L_out = L + 0.707*(C + Ls + Lb)`, mirrored for R. Clips to i16.
fn downmix_stereo_append(interleaved: &[u8], n_channels: usize, out: &mut Vec<u8>) {
    if n_channels < 5 {
        // Not enough channels for the mapping below — pass the first
        // one or two channels through unchanged.
        out.extend_from_slice(interleaved);
        return;
    }
    let frame_samples = interleaved.len() / 2 / n_channels;
    for i in 0..frame_samples {
        let get = |ch: usize| -> f32 {
            let off = (i * n_channels + ch) * 2;
            i16::from_le_bytes([interleaved[off], interleaved[off + 1]]) as f32
        };
        let l = get(0);
        let r = get(1);
        let c = get(2);
        let ls = get(3);
        let rs = get(4);
        let lb = if n_channels > 5 { get(5) } else { 0.0 };
        let rb = if n_channels > 6 { get(6) } else { 0.0 };
        const K: f32 = 0.707;
        let l_out = (l + K * (c + ls + lb)).clamp(-32768.0, 32767.0) as i16;
        let r_out = (r + K * (c + rs + rb)).clamp(-32768.0, 32767.0) as i16;
        out.extend_from_slice(&l_out.to_le_bytes());
        out.extend_from_slice(&r_out.to_le_bytes());
    }
}

fn write_wav(path: &str, pcm: &[u8], channels: u16, sample_rate: u32) -> io::Result<()> {
    let mut f = fs::File::create(path)?;
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?; // bits per sample
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    f.write_all(pcm)?;
    Ok(())
}

mod mp4 {
    use oxideav_core::{Error, Result};

    struct Box_ {
        typ: [u8; 4],
        end: usize,
        body_start: usize,
    }

    fn read_boxes(data: &[u8], start: usize, end: usize) -> Result<Vec<Box_>> {
        let mut boxes = Vec::new();
        let mut pos = start;
        while pos + 8 <= end {
            let size32 = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            let typ: [u8; 4] = data[pos + 4..pos + 8].try_into().unwrap();
            let (size, body_start) = if size32 == 1 {
                if pos + 16 > end {
                    return Err(Error::invalid("mp4: truncated 64-bit box size"));
                }
                let size64 = u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap());
                (size64 as usize, pos + 16)
            } else if size32 == 0 {
                (end - pos, pos + 8)
            } else {
                (size32, pos + 8)
            };
            if size < 8 || pos + size > end {
                return Err(Error::invalid("mp4: box size out of bounds"));
            }
            boxes.push(Box_ {
                typ,
                end: pos + size,
                body_start,
            });
            pos += size;
        }
        Ok(boxes)
    }

    fn find<'a>(boxes: &'a [Box_], typ: &[u8; 4]) -> Option<&'a Box_> {
        boxes.iter().find(|b| &b.typ == typ)
    }

    pub fn extract_ac4_samples(data: &[u8]) -> Result<Vec<Vec<u8>>> {
        let top = read_boxes(data, 0, data.len())?;
        let moov = find(&top, b"moov").ok_or_else(|| Error::invalid("mp4: no moov box"))?;
        let mdat = find(&top, b"mdat").ok_or_else(|| Error::invalid("mp4: no mdat box"))?;

        let moov_children = read_boxes(data, moov.body_start, moov.end)?;
        for trak in moov_children.iter().filter(|b| &b.typ == b"trak") {
            let trak_children = read_boxes(data, trak.body_start, trak.end)?;
            let Some(mdia) = find(&trak_children, b"mdia") else {
                continue;
            };
            let mdia_children = read_boxes(data, mdia.body_start, mdia.end)?;
            let Some(minf) = find(&mdia_children, b"minf") else {
                continue;
            };
            let minf_children = read_boxes(data, minf.body_start, minf.end)?;
            let Some(stbl) = find(&minf_children, b"stbl") else {
                continue;
            };
            let stbl_children = read_boxes(data, stbl.body_start, stbl.end)?;
            let Some(stsd) = find(&stbl_children, b"stsd") else {
                continue;
            };
            if stsd.body_start + 16 > stsd.end {
                continue;
            }
            let fourcc = &data[stsd.body_start + 12..stsd.body_start + 16];
            if fourcc != b"ac-4" {
                continue;
            }

            let Some(stsz) = find(&stbl_children, b"stsz") else {
                continue;
            };
            let sizes = read_stsz(data, stsz)?;

            let content_start = mdat.body_start;
            let content_len = mdat.end - mdat.body_start;
            let total: usize = sizes.iter().sum();

            let offsets: Vec<usize> = if total == content_len {
                let mut offs = Vec::with_capacity(sizes.len());
                let mut cur = content_start;
                for &sz in &sizes {
                    offs.push(cur);
                    cur += sz;
                }
                offs
            } else if let Some(stco) = find(&stbl_children, b"stco") {
                read_stco(data, stco)?
            } else if let Some(co64) = find(&stbl_children, b"co64") {
                read_co64(data, co64)?
            } else {
                return Err(Error::invalid(
                    "mp4: stsz total doesn't match mdat length and no chunk offset table found",
                ));
            };

            let mut samples = Vec::with_capacity(sizes.len());
            for (i, &sz) in sizes.iter().enumerate() {
                let Some(&off) = offsets.get(i) else { break };
                if off + sz > data.len() {
                    break;
                }
                samples.push(data[off..off + sz].to_vec());
            }
            return Ok(samples);
        }
        Err(Error::invalid("mp4: no ac-4 track found"))
    }

    fn read_stsz(data: &[u8], stsz: &Box_) -> Result<Vec<usize>> {
        let p = stsz.body_start;
        if p + 12 > stsz.end {
            return Err(Error::invalid("mp4: truncated stsz"));
        }
        let sample_size = u32::from_be_bytes(data[p + 4..p + 8].try_into().unwrap());
        let count = u32::from_be_bytes(data[p + 8..p + 12].try_into().unwrap()) as usize;
        if sample_size != 0 {
            return Ok(vec![sample_size as usize; count]);
        }
        let mut sizes = Vec::with_capacity(count);
        let mut off = p + 12;
        for _ in 0..count {
            if off + 4 > stsz.end {
                break;
            }
            sizes.push(u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize);
            off += 4;
        }
        Ok(sizes)
    }

    fn read_stco(data: &[u8], stco: &Box_) -> Result<Vec<usize>> {
        let p = stco.body_start;
        if p + 8 > stco.end {
            return Err(Error::invalid("mp4: truncated stco"));
        }
        let count = u32::from_be_bytes(data[p + 4..p + 8].try_into().unwrap()) as usize;
        let mut out = Vec::with_capacity(count);
        let mut off = p + 8;
        for _ in 0..count {
            if off + 4 > stco.end {
                break;
            }
            out.push(u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize);
            off += 4;
        }
        Ok(out)
    }

    fn read_co64(data: &[u8], co64: &Box_) -> Result<Vec<usize>> {
        let p = co64.body_start;
        if p + 8 > co64.end {
            return Err(Error::invalid("mp4: truncated co64"));
        }
        let count = u32::from_be_bytes(data[p + 4..p + 8].try_into().unwrap()) as usize;
        let mut out = Vec::with_capacity(count);
        let mut off = p + 8;
        for _ in 0..count {
            if off + 8 > co64.end {
                break;
            }
            out.push(u64::from_be_bytes(data[off..off + 8].try_into().unwrap()) as usize);
            off += 8;
        }
        Ok(out)
    }
}
