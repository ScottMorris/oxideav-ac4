//! `ac4info` — a small mediainfo-style CLI for AC-4 content.
//!
//! Stock `ffprobe`/`mediainfo` don't understand AC-4's internal TOC at
//! all (they report whatever the container's `stsd` box says, which for
//! AC-4 IMS content can be a completely different channel count than
//! what the bitstream itself carries — see README "Decoder" section).
//! This tool reads the real per-frame TOC via [`oxideav_ac4::toc`] and
//! reports what's actually in the bitstream: bitstream version, real
//! sample rate / frame rate, per-substream-group channel layout
//! (channel-coded vs A-JOC, and which of the three same-channel-count
//! 7.1 layouts), and basic I/P-frame statistics.
//!
//! Usage: `ac4info <file.mp4|file.ac4> [--frames]`
//!
//! Accepts either a raw AC-4 elementary stream (bare `raw_ac4_frame()`s
//! back to back) or an MP4/M4A file containing an `ac-4` sample entry —
//! the MP4 path is auto-detected by the leading `ftyp` box.
//!
//! `--frames` prints one line per frame instead of just the summary.

use std::env;
use std::fs;

use oxideav_ac4::toc::{self, Ac4FrameInfo};

fn main() {
    let mut args = env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: ac4info <file.mp4|file.ac4> [--frames]");
            std::process::exit(2);
        }
    };
    let per_frame = args.any(|a| a == "--frames");

    let data = fs::read(&path).unwrap_or_else(|e| {
        eprintln!("error reading {path}: {e}");
        std::process::exit(1);
    });

    let frames: Vec<Vec<u8>> = if data.len() >= 8 && &data[4..8] == b"ftyp" {
        match mp4::extract_ac4_samples(&data) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error extracting AC-4 samples from {path}: {e}");
                std::process::exit(1);
            }
        }
    } else {
        match sync::split_raw_frames(&data) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error splitting raw AC-4 stream {path}: {e}");
                std::process::exit(1);
            }
        }
    };

    if frames.is_empty() {
        eprintln!("no AC-4 frames found in {path}");
        std::process::exit(1);
    }

    let mut parsed: Vec<Ac4FrameInfo> = Vec::with_capacity(frames.len());
    let mut parse_failures = 0usize;
    for f in &frames {
        match toc::parse_ac4_toc(f) {
            Ok(info) => parsed.push(info),
            Err(_) => parse_failures += 1,
        }
    }

    println!("{path}");
    println!("  container:       {}", if is_mp4(&data) { "MP4/ISO-BMFF (ac-4 track)" } else { "raw AC-4 elementary stream" });
    println!("  frames found:    {}", frames.len());
    if parse_failures > 0 {
        println!("  TOC parse fails: {parse_failures} (of {})", frames.len());
    }
    let Some(first) = parsed.first() else {
        eprintln!("no frame's TOC parsed successfully");
        std::process::exit(1);
    };

    println!("  bitstream ver:   {}", first.bitstream_version);
    println!("  sample rate:     {} Hz", first.sample_rate);
    println!("  frame rate:      {:.3} fps", first.frame_rate_milli as f64 / 1000.0);
    println!("  samples/frame:   {}", first.frame_length);
    let duration_s = frames.len() as f64 * (first.frame_length as f64) / (first.sample_rate.max(1) as f64);
    println!("  duration:        {duration_s:.1} s (approx, {} frames)", frames.len());
    println!("  presentations:   {}", first.n_presentations);

    println!("  substream groups:");
    if first.substream_groups.is_empty() {
        // bitstream_version <= 1: only the top-level summary fields are
        // populated.
        println!(
            "    [0] channels={} channel_coded={} channel_mode={}",
            first.channels,
            first.channel_coded,
            channel_mode_name(first.channel_mode)
        );
    } else {
        for (i, g) in first.substream_groups.iter().enumerate() {
            if g.channel_coded {
                println!(
                    "    [{i}] channel-coded  channels={} layout={} substream_index={}",
                    g.channels,
                    channel_mode_name(g.channel_mode),
                    g.substream_index.map(|x| x.to_string()).unwrap_or_else(|| "implicit".into()),
                );
            } else if let Some(ajoc) = g.ajoc_info.as_ref() {
                println!(
                    "    [{i}] A-JOC object-coded  upmix_signals={} dmx_signals={} lfe={} substream_index={}",
                    ajoc.n_fullband_upmix_signals,
                    ajoc.n_fullband_dmx_signals,
                    ajoc.b_lfe,
                    ajoc.substream_index.map(|x| x.to_string()).unwrap_or_else(|| "implicit".into()),
                );
            } else {
                println!("    [{i}] object-coded (non-A-JOC, not decoded)");
            }
        }
    }

    // I/P-frame + channel_mode consistency stats across the whole file
    // — real content sometimes carries the same channel_mode throughout
    // (the common case) but this flags it if it doesn't.
    let mut iframes = 0usize;
    let mut modes: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
    for info in &parsed {
        let is_i = info
            .presentations
            .first()
            .map(|p| p.b_iframe)
            .unwrap_or(info.b_iframe_global);
        if is_i {
            iframes += 1;
        }
        *modes.entry(info.channels).or_insert(0) += 1;
    }
    println!(
        "  I-frames:        {iframes} / {} ({:.1}%)",
        parsed.len(),
        100.0 * iframes as f64 / parsed.len().max(1) as f64
    );
    if modes.len() > 1 {
        println!("  channel count varies across frames: {modes:?}");
    }

    if per_frame {
        println!();
        println!("per-frame detail:");
        for (i, info) in parsed.iter().enumerate() {
            let is_i = info
                .presentations
                .first()
                .map(|p| p.b_iframe)
                .unwrap_or(info.b_iframe_global);
            println!(
                "  frame {i}: {}  channels={} sample_rate={} frame_len={}",
                if is_i { "I" } else { "P" },
                info.channels,
                info.sample_rate,
                info.frame_length,
            );
        }
    }
}

fn is_mp4(data: &[u8]) -> bool {
    data.len() >= 8 && &data[4..8] == b"ftyp"
}

/// Human-readable Table 85 layout name. `None` (A-JOC groups, or a
/// `bitstream_version <= 1` frame where we don't track the index) just
/// prints "n/a"; the escape/reserved region (channel_mode >= 15) prints
/// the raw index since we don't have named layouts for it yet.
fn channel_mode_name(mode: Option<u32>) -> &'static str {
    match mode {
        None => "n/a",
        Some(0) => "mono",
        Some(1) => "stereo",
        Some(2) => "3.0",
        Some(3) => "5.0",
        Some(4) => "5.1",
        Some(5) => "7.0 (3/4/0)",
        Some(6) => "7.1 (3/4/0.1)",
        Some(7) => "7.0 (5/2/0)",
        Some(8) => "7.1 (5/2/0.1)",
        Some(9) => "7.0 (3/2/2)",
        Some(10) => "7.1 (3/2/2.1)",
        Some(11) => "7.0.4",
        Some(12) => "7.1.4 (9.1)",
        Some(13) => "9.0.4",
        Some(14) => "9.1.4",
        _ => "reserved/escape",
    }
}

/// Bare `raw_ac4_frame()` splitting: sync-prefixed streams are handled
/// by [`oxideav_ac4::sync`]; anything else is treated as one frame
/// spanning the whole buffer (the common case for a single already-
/// extracted MP4 sample, e.g. the ones this tool's own MP4 path
/// produces).
mod sync {
    use oxideav_core::Result;

    pub fn split_raw_frames(data: &[u8]) -> Result<Vec<Vec<u8>>> {
        // Try sync-frame splitting first (0xAC40/0xAC41 prefixed
        // streams, e.g. a `.ac4`/`.ec3`-style elementary dump); fall
        // back to treating the whole buffer as one bare frame.
        if data.len() >= 2 && (data[0..2] == [0xAC, 0x40] || data[0..2] == [0xAC, 0x41]) {
            return Ok(split_sync_frames(data));
        }
        Ok(vec![data.to_vec()])
    }

    fn split_sync_frames(data: &[u8]) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        let mut pos = 0usize;
        while pos + 2 <= data.len() {
            if data[pos..pos + 2] != [0xAC, 0x40] && data[pos..pos + 2] != [0xAC, 0x41] {
                break;
            }
            // Find the next sync word (or EOF) to bound this frame —
            // best-effort; real `ac4_sync_frame()` framing carries its
            // own length fields, but for a simple CLI this linear scan
            // is sufficient.
            let next = data[pos + 2..]
                .windows(2)
                .position(|w| w == [0xAC, 0x40] || w == [0xAC, 0x41])
                .map(|p| pos + 2 + p);
            let end = next.unwrap_or(data.len());
            frames.push(data[pos..end].to_vec());
            pos = end;
        }
        frames
    }
}

/// Minimal MP4/ISO-BMFF box walker for extracting an `ac-4` track's
/// raw samples. Deliberately narrow — just enough to find `stsd`'s
/// codec fourcc, `stsz` sample sizes, and compute byte offsets.
///
/// Round 397's real-file investigation found that at least one real
/// riptide-downloaded file has a **stale `stco`** chunk-offset table
/// (off by exactly the size of a cover-art box embedded after the
/// original mux) — so this walker prefers computing offsets
/// sequentially from `mdat`'s content start using only `stsz` sizes,
/// falling back to `stco` only when the two disagree in total length
/// (a signal `stsz` itself might be untrustworthy, vs. cover-art
/// staleness which never affects `stsz`).
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
            // stsd: version(1)+flags(3)+entry_count(4) then the first
            // sample entry's size(4)+fourcc(4).
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
                // Trustworthy `stsz`; sequential offsets from mdat's
                // content start sidestep the known stale-`stco` bug.
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
