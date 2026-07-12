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