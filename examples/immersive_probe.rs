//! Round 415: parse the audio substream as part-2 §6.2.4.1
//! immersive_channel_element(b_lfe=1, b_5fronts=0, b_iframe) — the
//! 7.1.4 container — and meter every frame against the audio_size
//! wall. The A-JOC (3+LFE) reading and this one converge at the LFE
//! on I-frames, which is why the LFE always decoded; the SCPL/A-CPL
//! tail here is the candidate for the 0.7-13 kbit per-frame deficit
//! the round-415 wall meter exposed.
use oxideav_ac4::acpl::{parse_acpl_config_1ch, parse_acpl_data_1ch, Acpl1chMode, AcplConfig1ch};
use oxideav_ac4::asf::{parse_aspx_data_1ch_body, parse_aspx_data_2ch_body, SubstreamTools};
use oxideav_ac4::aspx::{parse_aspx_config, parse_companding_control, AspxConfig};
use oxideav_ac4::asf::parse_chparam_info;
use oxideav_ac4::mch::{
    parse_five_channel_data, parse_four_channel_data, parse_mono_data, parse_three_channel_data,
    parse_two_channel_data,
};
use oxideav_ac4::toc;
use oxideav_core::bits::BitReader;
use std::{env, fs};

include!("mp4_helper.rs");

const TL: u32 = 2048;

#[derive(Default, Clone)]
struct Sticky {
    aspx: Option<AspxConfig>,
    xover: Option<u8>,
    acpl: Option<AcplConfig1ch>,
}

fn aspx2(
    br: &mut BitReader<'_>,
    cfg: &AspxConfig,
    b_iframe: bool,
    st: &mut Sticky,
) -> Result<(), oxideav_core::Error> {
    let mut tools = Box::<SubstreamTools>::default();
    if let Some(x) = st.xover {
        tools.aspx_xover_subband_offset = Some(x);
    }
    parse_aspx_data_2ch_body(br, &mut tools, cfg, b_iframe, TL)?;
    if b_iframe {
        if let Some(x) = tools.aspx_xover_subband_offset {
            st.xover = Some(x);
        }
    }
    Ok(())
}

fn aspx1(
    br: &mut BitReader<'_>,
    cfg: &AspxConfig,
    b_iframe: bool,
    st: &mut Sticky,
) -> Result<(), oxideav_core::Error> {
    let mut tools = Box::<SubstreamTools>::default();
    if let Some(x) = st.xover {
        tools.aspx_xover_subband_offset = Some(x);
    }
    parse_aspx_data_1ch_body(br, &mut tools, cfg, b_iframe, TL)?;
    if b_iframe {
        if let Some(x) = tools.aspx_xover_subband_offset {
            st.xover = Some(x);
        }
    }
    Ok(())
}

/// Strict post-region walk for the region-extent scanner:
/// [grp(2)][core bodies — ALL must decode][sap + chparams][add pair —
/// both channels must decode][aspx2 x3 + aspx1][acpl x4] → residue.
#[allow(clippy::too_many_arguments)]
fn strict_walk(
    mut br: BitReader<'_>,
    cfg: &AspxConfig,
    ac: &AcplConfig1ch,
    st: &Sticky,
    wall: u64,
) -> Option<(i64, u32, u64)> {
    let grp = br.read_u32(2).ok()?;
    let mut core_m0 = 1u32;
    let mut all = true;
    match grp {
        0 => {
            let _ = br.read_bit().ok()?;
            for _ in 0..2 {
                let p = parse_two_channel_data(&mut br, TL).ok()?;
                core_m0 = p.psy_info.as_ref().map(|x| x.max_sfb_0).unwrap_or(core_m0);
                for c in 0..2 {
                    all &= p.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                        || p.scaled_spec_windows_per_channel.get(c).map_or(false, |s| s.is_some());
                }
            }
            let m = parse_mono_data(&mut br, false, TL).ok()?;
            all &= m.scaled_spec.is_some() || m.scaled_spec_windows.is_some();
        }
        1 => {
            let t = parse_three_channel_data(&mut br, TL).ok()?;
            core_m0 = t.psy_info.as_ref().map(|x| x.max_sfb_0).unwrap_or(1);
            for c in 0..3 {
                all &= t.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                    || t.scaled_spec_windows_per_channel.get(c).map_or(false, |s| s.is_some());
            }
            let p = parse_two_channel_data(&mut br, TL).ok()?;
            for c in 0..2 {
                all &= p.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                    || p.scaled_spec_windows_per_channel.get(c).map_or(false, |s| s.is_some());
            }
        }
        2 => {
            let f = parse_four_channel_data(&mut br, TL).ok()?;
            core_m0 = f.psy_info.as_ref().map(|x| x.max_sfb_0).unwrap_or(1);
            for c in 0..4 {
                all &= f.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                    || f.scaled_spec_windows_per_channel.get(c).map_or(false, |s| s.is_some());
            }
            let m = parse_mono_data(&mut br, false, TL).ok()?;
            all &= m.scaled_spec.is_some() || m.scaled_spec_windows.is_some();
        }
        _ => {
            let f = parse_five_channel_data(&mut br, TL).ok()?;
            core_m0 = f.psy_info.as_ref().map(|x| x.max_sfb_0).unwrap_or(1);
            for c in 0..5 {
                all &= f.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                    || f.scaled_spec_windows_per_channel.get(c).map_or(false, |s| s.is_some());
            }
        }
    }
    if !all {
        return None;
    }
    let b_sap = br.read_bit().ok()?;
    if b_sap {
        let _ = parse_chparam_info(&mut br, &[core_m0.max(1)]).ok()?;
        let _ = parse_chparam_info(&mut br, &[core_m0.max(1)]).ok()?;
    }
    let p = parse_two_channel_data(&mut br, TL).ok()?;
    for c in 0..2 {
        if !(p.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
            || p.scaled_spec_windows_per_channel.get(c).map_or(false, |s| s.is_some()))
        {
            return None;
        }
    }
    let add_end = br.bit_position();
    let mut st2 = st.clone();
    for _ in 0..3 {
        aspx2(&mut br, cfg, false, &mut st2).ok()?;
    }
    aspx1(&mut br, cfg, false, &mut st2).ok()?;
    for _ in 0..4 {
        let _ = parse_acpl_data_1ch(&mut br, ac.num_param_bands, 0, ac.quant_mode).ok()?;
    }
    Some((wall as i64 - br.bit_position() as i64, grp, add_end))
}

fn main() {
    let path = env::args().nth(1).expect("mp4");
    let max_frames: usize = env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(30);
    let force_mode: Option<u32> = env::args().nth(3).and_then(|v| v.parse().ok());
    let data = fs::read(&path).expect("read");
    let frames = mp4::extract_ac4_samples(&data).expect("mp4");
    let mut sticky = Sticky::default();

    for (i, payload) in frames.iter().enumerate().take(max_frames) {
        let Ok(info) = toc::parse_ac4_toc(payload) else { continue };
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
            continue;
        }
        let sb = &payload[start..end];
        let mut br = BitReader::new(sb);
        let mut audio_size = br.read_u32(15).unwrap();
        if br.read_bit().unwrap() {
            audio_size += oxideav_ac4::toc::variable_bits(&mut br, 7).unwrap() << 15;
        }
        br.align_to_byte();
        let wall = br.bit_position() + audio_size as u64 * 8;

        // AC4_IMM_TAILSCAN=1: instead of a forward walk, scan backward
        // from the wall for [aspx2ch x3][aspx1ch][acpl x4] tail chains
        // that close at the wall — pins the true aspx start per frame.
        if std::env::var_os("AC4_IMM_TAILSCAN").is_some()
            && sticky.aspx.is_some()
            && sticky.acpl.is_some()
        {
            let cfg = sticky.aspx.clone().unwrap();
            let ac = sticky.acpl.clone().unwrap();
            let mut hits: Vec<(u64, i64)> = Vec::new();
            let lo = wall.saturating_sub(4000).max(br.bit_position());
            for cand in lo..wall.saturating_sub(150) {
                let mut b2 = BitReader::new(sb);
                if b2.skip(cand as u32).is_err() {
                    break;
                }
                let mut st2 = sticky.clone();
                let ok = (|| -> Result<(), oxideav_core::Error> {
                    aspx2(&mut b2, &cfg, b_iframe, &mut st2)?;
                    aspx2(&mut b2, &cfg, b_iframe, &mut st2)?;
                    aspx2(&mut b2, &cfg, b_iframe, &mut st2)?;
                    aspx1(&mut b2, &cfg, b_iframe, &mut st2)?;
                    for _ in 0..4 {
                        let _ = parse_acpl_data_1ch(&mut b2, ac.num_param_bands, 0, ac.quant_mode)?;
                    }
                    Ok(())
                })();
                if ok.is_ok() {
                    let res = wall as i64 - b2.bit_position() as i64;
                    if (-8..=64).contains(&res) {
                        hits.push((cand, res));
                    }
                }
            }
            let show: Vec<String> = hits
                .iter()
                .take(12)
                .map(|(c, r)| format!("{c}(r{r})"))
                .collect();
            println!(
                "frame {i} ifr={} wall@{wall} tail_hits={} [{}]",
                u8::from(b_iframe),
                hits.len(),
                show.join(" ")
            );
            if b_iframe {
                // Refresh sticky configs via a quiet forward walk.
            } else {
                continue;
            }
        }
        let r = (|| -> Result<String, oxideav_core::Error> {
            // immersive_codec_mode_code: 1 bit, 0 -> 2 more.
            // AC4_IMM_P_NOCODE=1: P-frames do NOT carry the code —
            // reuse the I-frame's mode and consume nothing.
            let p_nocode = std::env::var_os("AC4_IMM_P_NOCODE").is_some();
            // AC4_IMM_P_SKIP=<bits>: P-frames carry a fixed-size prefix
            // (observed constant '010111' on this stream) — skip it and
            // reuse the sticky I-frame mode.
            let p_skip: Option<u32> = std::env::var("AC4_IMM_P_SKIP")
                .ok()
                .and_then(|v| v.parse().ok());
            static mut STICKY_MODE: u32 = 3;
            // AC4_IMM_MODE2=1: deviation-#14 head model — the mode code
            // is a flat 2-bit field on EVERY frame ('01' on this
            // stream), the aspx_config sits at bit 18 on I-frames (the
            // sba=40 master-table alignment), and a 4-bit sticky field
            // ('0111') follows the config on I-frames / the mode on
            // P-frames. Treated as ACPL_2-equivalent structure.
            let mode2 = std::env::var_os("AC4_IMM_MODE2").is_some();
            let mode = if mode2 {
                let _code = br.read_u32(2)?;
                if b_iframe {
                    sticky.aspx = Some(parse_aspx_config(&mut br)?);
                }
                let _field4 = br.read_u32(4)?;
                if b_iframe && sticky.acpl.is_none() {
                    // No separately coded acpl cfg under this model —
                    // synthesize from the 4-bit field later; default
                    // 7 bands / coarse for now via a fake parse below.
                }
                3
            } else if !b_iframe && p_skip.is_some() {
                br.skip(p_skip.unwrap())?;
                unsafe { STICKY_MODE }
            } else if !b_iframe && p_nocode {
                unsafe { STICKY_MODE }
            } else if let Some(m) = force_mode {
                // Still consume the real code bits.
                if !br.read_bit()? {
                    let _ = br.read_u32(2)?;
                }
                m
            } else if br.read_bit()? {
                4
            } else {
                br.read_u32(2)?
            };
            if b_iframe {
                unsafe { STICKY_MODE = mode };
            }
            let seven_ch_static = mode <= 3;
            let mut log = format!("mode={mode} ");
            if mode2 {
                if sticky.acpl.is_none() {
                    sticky.acpl = Some(AcplConfig1ch {
                        num_param_bands_id: 3,
                        num_param_bands: 7,
                        quant_mode: oxideav_ac4::acpl::AcplQuantMode::Fine,
                        qmf_band: 0,
                    });
                }
            } else if b_iframe {
                if mode != 0 {
                    sticky.aspx = Some(parse_aspx_config(&mut br)?);
                }
                if mode == 2 {
                    sticky.acpl = Some(parse_acpl_config_1ch(&mut br, Acpl1chMode::Partial)?);
                }
                if mode == 3 {
                    sticky.acpl = Some(parse_acpl_config_1ch(&mut br, Acpl1chMode::Full)?);
                }
            }
            // LFE.
            let lfe = parse_mono_data(&mut br, true, TL)?;
            log += &format!(
                "lfe_end@{} ok={} ",
                br.bit_position(),
                lfe.scaled_spec.is_some()
            );
            if mode == 4 {
                let _ = parse_companding_control(&mut br, 5)?;
            }
            // AC4_IMM_RL=<bits>: force a fixed post-LFE region skip on
            // P-frames (content-validation runs at a scanner hit).
            if !b_iframe {
                if let Some(rl) = std::env::var("AC4_IMM_RL")
                    .ok()
                    .and_then(|v| v.parse::<u32>().ok())
                {
                    br.skip(rl)?;
                }
            }
            // AC4_IMM_REGION_SCAN=1 (P-frames): sweep the region length
            // after the LFE; accept only strict full-chain walks that
            // land on the wall. Prints exact region extents.
            let scan_from: usize = std::env::var("AC4_IMM_SCAN_FROM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let scan_max: u32 = std::env::var("AC4_IMM_SCAN_MAX")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(9000);
            if std::env::var_os("AC4_IMM_REGION_SCAN").is_some() && !b_iframe && i >= scan_from {
                let cfg = sticky
                    .aspx
                    .clone()
                    .ok_or_else(|| oxideav_core::Error::invalid("no sticky aspx"))?;
                let ac = sticky
                    .acpl
                    .clone()
                    .ok_or_else(|| oxideav_core::Error::invalid("no sticky acpl"))?;
                let lfe_end = br.bit_position();
                let max_len = (wall.saturating_sub(lfe_end + 600)) as u32;
                let mut hits = 0u32;
                for rl in 0..=max_len.min(scan_max) {
                    let mut b2 = br;
                    if rl > 0 && b2.skip(rl).is_err() {
                        break;
                    }
                    if let Some((res, grp, add_end)) =
                        strict_walk(b2, &cfg, &ac, &sticky, wall)
                    {
                        if (-8..=80).contains(&res) {
                            println!(
                                "REGION f={i} lfe_end={lfe_end} rl={rl} grp={grp} add_end={add_end} res={res}"
                            );
                            hits += 1;
                        }
                    }
                }
                println!("frame {i}: region scan done, {hits} hits (lfe_end={lfe_end} wall={wall})");
                return Ok(String::new());
            }
            // AC4_IMM_ASPX_FIRST=1: unified-layout hypothesis — the
            // four A-SPX elements sit right after the LFE (the war's
            // P-frame "region"), not after the add pair. Frame 0 is
            // blind to the order (its aspx is ~empty).
            let aspx_first = std::env::var_os("AC4_IMM_ASPX_FIRST").is_some();
            if aspx_first && mode >= 2 && mode != 4 {
                let cfg = sticky
                    .aspx
                    .clone()
                    .ok_or_else(|| oxideav_core::Error::invalid("no sticky aspx cfg"))?;
                aspx2(&mut br, &cfg, b_iframe, &mut sticky)?;
                aspx2(&mut br, &cfg, b_iframe, &mut sticky)?;
                if mode <= 3 {
                    aspx2(&mut br, &cfg, b_iframe, &mut sticky)?;
                }
                aspx1(&mut br, &cfg, b_iframe, &mut sticky)?;
                log += &format!("aspxF_end@{} ", br.bit_position());
            }
            let grouping = br.read_u32(2)?;
            log += &format!("grp={grouping} ");
            let mut core_m0 = 0u32;
            let mut bodies_ok = 0usize;
            let mut bodies_n = 0usize;
            let mut count = |ok: bool, n: &mut usize, k: &mut usize| {
                *n += 1;
                if ok {
                    *k += 1;
                }
            };
            match grouping {
                0 => {
                    let _two_ch_mode = br.read_bit()?;
                    for _ in 0..2 {
                        let p = parse_two_channel_data(&mut br, TL)?;
                        core_m0 = p.psy_info.as_ref().map(|x| x.max_sfb_0).unwrap_or(core_m0);
                        for c in 0..2 {
                            count(
                                p.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                                    || p.scaled_spec_windows_per_channel
                                        .get(c)
                                        .map_or(false, |s| s.is_some()),
                                &mut bodies_n,
                                &mut bodies_ok,
                            );
                        }
                    }
                    let m = parse_mono_data(&mut br, false, TL)?;
                    count(
                        m.scaled_spec.is_some() || m.scaled_spec_windows.is_some(),
                        &mut bodies_n,
                        &mut bodies_ok,
                    );
                }
                1 => {
                    let t = parse_three_channel_data(&mut br, TL)?;
                    core_m0 = t.psy_info.as_ref().map(|x| x.max_sfb_0).unwrap_or(0);
                    for c in 0..3 {
                        count(
                            t.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                                || t.scaled_spec_windows_per_channel
                                    .get(c)
                                    .map_or(false, |s| s.is_some()),
                            &mut bodies_n,
                            &mut bodies_ok,
                        );
                    }
                    let p = parse_two_channel_data(&mut br, TL)?;
                    for c in 0..2 {
                        count(
                            p.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                                || p.scaled_spec_windows_per_channel
                                    .get(c)
                                    .map_or(false, |s| s.is_some()),
                            &mut bodies_n,
                            &mut bodies_ok,
                        );
                    }
                }
                2 => {
                    let f = parse_four_channel_data(&mut br, TL)?;
                    core_m0 = f.psy_info.as_ref().map(|x| x.max_sfb_0).unwrap_or(0);
                    for c in 0..4 {
                        count(
                            f.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some()),
                            &mut bodies_n,
                            &mut bodies_ok,
                        );
                    }
                    let m = parse_mono_data(&mut br, false, TL)?;
                    count(
                        m.scaled_spec.is_some() || m.scaled_spec_windows.is_some(),
                        &mut bodies_n,
                        &mut bodies_ok,
                    );
                }
                _ => {
                    let f = parse_five_channel_data(&mut br, TL)?;
                    core_m0 = f.psy_info.as_ref().map(|x| x.max_sfb_0).unwrap_or(0);
                    for c in 0..5 {
                        count(
                            f.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some()),
                            &mut bodies_n,
                            &mut bodies_ok,
                        );
                    }
                }
            }
            log += &format!("core_end@{} bodies={bodies_ok}/{bodies_n} ", br.bit_position());
            if seven_ch_static {
                let b_use_sap_add_ch = br.read_bit()?;
                if b_use_sap_add_ch {
                    // Round-406 war rule: add-channel chparam ms loops
                    // run over the A-SPX core band count, not max_sfb.
                    // AC4_IMM_CHPARAM=core|msfb selects.
                    let use_core = std::env::var("AC4_IMM_CHPARAM")
                        .map(|v| v != "msfb")
                        .unwrap_or(true);
                    let m = if use_core {
                        sticky
                            .aspx
                            .as_ref()
                            .and_then(|c| {
                                oxideav_ac4::mch::aspx_core_band_count(c, TL)
                            })
                            .unwrap_or(core_m0.max(1))
                    } else {
                        core_m0.max(1)
                    };
                    let _ = parse_chparam_info(&mut br, &[m])?;
                    let _ = parse_chparam_info(&mut br, &[m])?;
                }
                let p = parse_two_channel_data(&mut br, TL)?;
                let addok = (0..2)
                    .filter(|&c| {
                        p.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                            || p.scaled_spec_windows_per_channel
                                .get(c)
                                .map_or(false, |s| s.is_some())
                    })
                    .count();
                log += &format!(
                    "sap={} addpair_end@{} addok={addok} ",
                    u8::from(b_use_sap_add_ch),
                    br.bit_position()
                );
            }
            let cfg = sticky
                .aspx
                .clone()
                .ok_or_else(|| oxideav_core::Error::invalid("no sticky aspx cfg"))?;
            if mode == 1 {
                for _ in 0..3 {
                    aspx2(&mut br, &cfg, b_iframe, &mut sticky)?;
                }
                aspx1(&mut br, &cfg, b_iframe, &mut sticky)?;
                // reading Table literally: [2ch 2ch 1ch] (b_5fronts=0 ->
                // one more 2ch) then 2ch 2ch — total 6x 2ch + 1x 1ch.
                for _ in 0..2 {
                    aspx2(&mut br, &cfg, b_iframe, &mut sticky)?;
                }
            } else if mode >= 2 && !aspx_first {
                aspx2(&mut br, &cfg, b_iframe, &mut sticky)?;
                aspx2(&mut br, &cfg, b_iframe, &mut sticky)?;
                if seven_ch_static {
                    aspx2(&mut br, &cfg, b_iframe, &mut sticky)?;
                }
                aspx1(&mut br, &cfg, b_iframe, &mut sticky)?;
            }
            log += &format!("aspx_end@{} ", br.bit_position());
            if mode == 4 {
                let _ = oxideav_ac4::ajcc::parse_ajcc_data(&mut br, false)?;
                log += &format!("ajcc_end@{} ", br.bit_position());
            }
            if mode <= 2 {
                // SCPL pairs (Tfl/Tfr, Tbl/Tbr) + 4x chparam.
                let mut scplok = 0;
                for _ in 0..2 {
                    let p = parse_two_channel_data(&mut br, TL)?;
                    scplok += (0..2)
                        .filter(|&c| {
                            p.scaled_spec_per_channel.get(c).map_or(false, |s| s.is_some())
                                || p.scaled_spec_windows_per_channel
                                    .get(c)
                                    .map_or(false, |s| s.is_some())
                        })
                        .count();
                }
                let m = core_m0.max(1);
                for _ in 0..4 {
                    let _ = parse_chparam_info(&mut br, &[m])?;
                }
                log += &format!("scpl_end@{} scplok={scplok} ", br.bit_position());
            }
            if mode == 2 || mode == 3 {
                let ac = sticky
                    .acpl
                    .clone()
                    .ok_or_else(|| oxideav_core::Error::invalid("no sticky acpl cfg"))?;
                let nb: u32 = std::env::var("AC4_IMM_ACPL_BANDS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(ac.num_param_bands);
                let n_acpl: u32 = std::env::var("AC4_IMM_ACPL_N")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(4);
                let verbose = std::env::var_os("AC4_IMM_ACPL_TRACE").is_some();
                for a in 0..n_acpl {
                    let p0 = br.bit_position();
                    let d = parse_acpl_data_1ch(&mut br, nb, 0, ac.quant_mode)?;
                    if verbose {
                        log += &format!(
                            "acpl{a}[{}..{} nps={}] ",
                            p0,
                            br.bit_position(),
                            d.framing.num_param_sets
                        );
                    }
                }
                log += &format!("acpl_end@{} nb={nb} ", br.bit_position());
            }
            log += &format!(
                "END@{} residue={}",
                br.bit_position(),
                wall as i64 - br.bit_position() as i64
            );
            Ok(log)
        })();
        match r {
            Ok(log) => println!("frame {i} ifr={} wall@{wall} {log}", u8::from(b_iframe)),
            Err(e) => println!("frame {i} ifr={} ERR {e:?}", u8::from(b_iframe)),
        }
    }
}
