//! Round 418: THE BED DECODER — per frame, decode the three elements
//! that are proven-parseable independently of the front war:
//!   [LFE]  (head-anchored, music-proven)
//!   [add pair L/R]  (production additional-pair grammar at the
//!                    resync_7x_addpair head — the primary music bed)
//! Output: interleaved f32 [bedL, bedR, LFE] per sample.
//! Decode law: set AC4_SF_REL=1 AC4_SF_GAIN_BITS=15 (the war's
//! winning scale-factor law) in the environment.
use oxideav_ac4::asf::SubstreamTools;
use oxideav_ac4::aspx::{parse_aspx_config, AspxConfig};
use oxideav_ac4::mch::{parse_mono_data, parse_two_channel_data_additional, WindowSpectrum};
use oxideav_ac4::mdct::{imdct, BlockSwitchOla};
use oxideav_ac4::toc;
use oxideav_core::bits::BitReader;
use std::{env, fs, io::Write};

include!("mp4_helper.rs");

const TL: u32 = 2048;
const N: usize = 2048;

struct Ola {
    banks: Vec<BlockSwitchOla>,
}

impl Ola {
    fn new(n: usize) -> Self {
        Ola {
            banks: (0..n).map(|_| BlockSwitchOla::new(N)).collect(),
        }
    }
    fn long(&mut self, ch: usize, spec: &[f32]) -> Vec<f32> {
        let mut x = vec![0.0f32; N];
        let c = spec.len().min(N);
        x[..c].copy_from_slice(&spec[..c]);
        self.banks[ch].process_block(&imdct(&x))
    }
    fn grouped(&mut self, ch: usize, windows: &[WindowSpectrum]) -> Vec<f32> {
        let mut out = Vec::with_capacity(N);
        for (tl, spec) in windows {
            let n = *tl as usize;
            let mut x = vec![0.0f32; n];
            let c: usize = spec.len().min(n);
            x[..c].copy_from_slice(&spec[..c]);
            out.extend_from_slice(&self.banks[ch].process_block(&imdct(&x)));
        }
        out.resize(N, 0.0);
        out
    }
    fn silent(&mut self, ch: usize) -> Vec<f32> {
        self.long(ch, &[])
    }
}

fn main() {
    let path = env::args().nth(1).expect("mp4");
    let out = env::args().nth(2).expect("out.f32");
    let max_frames: usize = env::args()
        .nth(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let data = fs::read(&path).expect("read");
    let frames = mp4::extract_ac4_samples(&data).expect("mp4");

    let mut cfg: Option<AspxConfig> = None;
    let mut ola = Ola::new(3); // 0=bedL 1=bedR 2=LFE
    let mut f = fs::File::create(&out).unwrap();
    let (mut ok_lfe, mut ok_pair, mut fail) = (0usize, 0usize, 0usize);

    for (i, payload) in frames.iter().enumerate() {
        if i >= max_frames {
            break;
        }
        let Ok(info) = toc::parse_ac4_toc(payload) else {
            fail += 1;
            continue;
        };
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
            fail += 1;
            continue;
        }
        let sb = &payload[start..end];
        let mut br = BitReader::new(sb);
        let Ok(mut audio_size) = br.read_u32(15) else { fail += 1; continue };
        if br.read_bit().unwrap_or(false) {
            audio_size += oxideav_ac4::toc::variable_bits(&mut br, 7).unwrap_or(0) << 15;
        }
        br.align_to_byte();
        let wall = br.bit_position() + audio_size as u64 * 8;
        let elem_start = br;

        // Head: 2-bit mode; I-frames carry the aspx_config; 4-bit field.
        let _ = br.read_u32(2);
        if b_iframe {
            if let Ok(c) = parse_aspx_config(&mut br) {
                cfg = Some(c);
            }
        }
        let _ = br.read_u32(4);

        // LFE — round 421: position-scanned, strict, 3-bit section
        // widths (the rosetta grammar). AC4_BED_LFE_SCAN=1 enables;
        // otherwise the legacy fixed-position parse runs.
        let lfe_pcm = if std::env::var_os("AC4_BED_LFE_SCAN").is_some() {
            std::env::set_var("AC4_SECT_W3", "1");
            std::env::set_var("AC4_SECT_STRICT", "1");
            let head_base = br;
            let mut got: Option<(u32, Vec<f32>)> = None;
            // audio position: br currently sits right after the head
            // fields we consumed (mode+cfg+field4) — rewind logic:
            // scan absolute audio offsets 1..14 from the element start
            // instead. The element started at bit 16 of the substream;
            // we captured no absolute reader, so scan forward from the
            // current position minus nothing — use offsets 0..14
            // relative to the POST-audio_size, pre-head reader saved
            // below.
            for sb in 1..14u32 {
                let mut b2 = elem_start;
                if b2.skip(sb).is_err() {
                    break;
                }
                if let Ok(m) = parse_mono_data(&mut b2, true, TL) {
                    if let Some(s) = m.scaled_spec.as_deref() {
                        got = Some((sb, s.to_vec()));
                        if let Some(dir) = std::env::var_os("AC4_BED_SPEC_DIR") {
                            let p = std::path::Path::new(&dir)
                                .join(format!("lfe{i:05}.f32"));
                            let bytes: Vec<u8> =
                                s.iter().flat_map(|v| v.to_le_bytes()).collect();
                            let _ = fs::write(p, bytes);
                        }
                        break;
                    }
                }
            }
            std::env::remove_var("AC4_SECT_W3");
            std::env::remove_var("AC4_SECT_STRICT");
            let _ = head_base;
            match got {
                Some((_sb, s)) => {
                    ok_lfe += 1;
                    ola.long(2, &s)
                }
                None => ola.silent(2),
            }
        } else {
            if let Some(dir) = std::env::var_os("AC4_BED_SPEC_DIR") {
                let mut pk = br;
                if let Ok(m) = parse_mono_data(&mut pk, true, TL) {
                    if let Some(s) = m.scaled_spec.as_deref() {
                        let p = std::path::Path::new(&dir).join(format!("lfe{i:05}.f32"));
                        let bytes: Vec<u8> =
                            s.iter().flat_map(|v| v.to_le_bytes()).collect();
                        let _ = fs::write(p, bytes);
                    }
                }
            }
            let lfe_start = br.bit_position();
            match parse_mono_data(&mut br, true, TL) {
                Ok(m) => {
                    if std::env::var_os("AC4_BED_TRACE").is_some() {
                        eprintln!("LFE f={i} start={lfe_start} end={}", br.bit_position());
                    }
                    match (m.scaled_spec.as_deref(), m.scaled_spec_windows.as_deref()) {
                    (Some(s), _) => {
                        ok_lfe += 1;
                        ola.long(2, s)
                    }
                    (None, Some(w)) if !w.is_empty() => {
                        ok_lfe += 1;
                        ola.grouped(2, w)
                    }
                    _ => ola.silent(2),
                }}
                Err(_) => ola.silent(2),
            }
        };

        // Add-pair head via the war's production scanner.
        let (l_pcm, r_pcm) = (|| -> Option<(Vec<f32>, Vec<f32>)> {
            let c = cfg.as_ref()?;
            let mut tools = Box::<SubstreamTools>::default();
            tools.wall_bits = Some(wall);
            tools.aspx_xover_slots =
                [Some(0), Some(0), Some(0), Some(4), None, None, None, None];
            tools.aspx_xover_slots_good = Some(tools.aspx_xover_slots);
            let (mut head, _slots, _joint) =
                oxideav_ac4::mch::resync_7x_addpair(br, &tools, c, b_iframe, TL)?;
            let hpos = head.bit_position();
            let p = parse_two_channel_data_additional(&mut head, TL, Some(c)).ok()?;
            if std::env::var_os("AC4_BED_TRACE").is_some() {
                let long0 = p.scaled_spec_per_channel.first().map_or(false, |s| s.is_some());
                let long1 = p.scaled_spec_per_channel.get(1).map_or(false, |s| s.is_some());
                let grp0 = p
                    .scaled_spec_windows_per_channel
                    .first()
                    .map_or(false, |s| s.is_some());
                let grp1 = p
                    .scaled_spec_windows_per_channel
                    .get(1)
                    .map_or(false, |s| s.is_some());
                eprintln!(
                    "BED f={i} ifr={} H={hpos} end={} bmsp={} long=({},{}) grp=({},{}) wall={wall}",
                    u8::from(b_iframe),
                    head.bit_position(),
                    u8::from(p.b_enable_mdct_stereo_proc),
                    u8::from(long0),
                    u8::from(long1),
                    u8::from(grp0),
                    u8::from(grp1),
                );
            }
            if let Some(dir) = std::env::var_os("AC4_BED_SPEC_DIR") {
                for (ch, tag) in [(0usize, "pl"), (1, "pr")] {
                    if let Some(Some(s)) = p.scaled_spec_per_channel.get(ch) {
                        let pth = std::path::Path::new(&dir).join(format!("{tag}{i:05}.f32"));
                        let bytes: Vec<u8> = s.iter().flat_map(|v| v.to_le_bytes()).collect();
                        let _ = fs::write(pth, bytes);
                    }
                }
            }
            // §5.3.3.2 pair unmix (SAP a/b/c/d per band) for the
            // joint-MDCT-stereo long-frame case.
            if let (Some(Some(s0)), Some(Some(s1))) = (
                p.scaled_spec_per_channel.first(),
                p.scaled_spec_per_channel.get(1),
            ) {
                if p.b_enable_mdct_stereo_proc {
                    if let Some(cp) = p.chparam.as_ref() {
                        let mut m0 = s0.clone();
                        let mut m1 = s1.clone();
                        if let Some(sfbo) = oxideav_ac4::sfb_offset::sfb_offset_48(TL) {
                            let num_sfb = (sfbo.len() - 1) as u32;
                            let qv = oxideav_ac4::asf::extract_sap_abcd(cp, &[num_sfb]);
                            let n = m0.len().min(m1.len());
                            for sfb in 0..num_sfb as usize {
                                let lo = sfbo[sfb] as usize;
                                let hi = (sfbo[sfb + 1] as usize).min(n);
                                if lo >= hi {
                                    break;
                                }
                                let (a, b, cc, dd) = qv.abcd[0]
                                    .get(sfb)
                                    .copied()
                                    .unwrap_or((1.0, 0.0, 0.0, 1.0));
                                for k in lo..hi {
                                    let i0 = m0[k];
                                    let i1 = m1[k];
                                    m0[k] = a * i0 + b * i1;
                                    m1[k] = cc * i0 + dd * i1;
                                }
                            }
                        }
                        let l = ola.long(0, &m0);
                        let r = ola.long(1, &m1);
                        return Some((l, r));
                    }
                }
            }
            let get = |ch: usize, slot: usize, ola: &mut Ola| -> Option<Vec<f32>> {
                if let Some(Some(s)) = p.scaled_spec_per_channel.get(ch) {
                    return Some(ola.long(slot, s));
                }
                if let Some(Some(w)) = p.scaled_spec_windows_per_channel.get(ch) {
                    if !w.is_empty() {
                        return Some(ola.grouped(slot, w));
                    }
                }
                None
            };
            let l = get(0, 0, &mut ola);
            let r = get(1, 1, &mut ola);
            match (l, r) {
                (Some(l), Some(r)) => Some((l, r)),
                (Some(l), None) => Some((l, ola.silent(1))),
                (None, Some(r)) => Some((ola.silent(0), r)),
                _ => None,
            }
        })()
        .map(|(l, r)| {
            ok_pair += 1;
            (l, r)
        })
        .unwrap_or_else(|| (ola.silent(0), ola.silent(1)));

        let mut buf = Vec::with_capacity(N * 3 * 4);
        for k in 0..N {
            buf.extend_from_slice(&l_pcm.get(k).copied().unwrap_or(0.0).to_le_bytes());
            buf.extend_from_slice(&r_pcm.get(k).copied().unwrap_or(0.0).to_le_bytes());
            buf.extend_from_slice(&lfe_pcm.get(k).copied().unwrap_or(0.0).to_le_bytes());
        }
        f.write_all(&buf).unwrap();
    }
    println!("BED DECODE: lfe_ok {ok_lfe} pair_ok {ok_pair} fail {fail}");
}
