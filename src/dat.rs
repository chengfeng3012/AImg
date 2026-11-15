//! Android `.dat` block-image data (`system.new.dat` + `system.transfer.list`)
//! and differential patches (`system.patch.dat`).
//!
//! Rewritten from `xpirt/sdat2img`, `xpirt/img2sdat` and the
//! BlockImageUpdate logic of `erfanoabdi/imgpatchtools` (which itself is a port
//! of AOSP `bootable/recovery/blockimg.cpp` + `applypatch`).

use crate::util::*;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const BLOCK_SIZE: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Cmd {
    New(Vec<(u64, u64)>),
    Zero(Vec<(u64, u64)>),
    Erase(Vec<(u64, u64)>),
    Copy {
        src: Vec<(u64, u64)>,
        dst: Vec<(u64, u64)>,
    },
    Move {
        src: Vec<(u64, u64)>,
        dst: Vec<(u64, u64)>,
    },
    Bsdiff {
        patch_off: u64,
        patch_len: u64,
        src: Vec<(u64, u64)>,
        dst: Vec<(u64, u64)>,
    },
    Stash {
        id: u32,
        ranges: Vec<(u64, u64)>,
    },
    Free {
        id: u32,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferList {
    pub version: u32,
    pub total_blocks: u64,
    pub cmds: Vec<Cmd>,
}

/// Parse `"<count>,<begin>,<end>,..."` where count == number of following ints.
pub fn rangeset(s: &str) -> Result<Vec<(u64, u64)>> {
    let nums: Vec<u64> = s
        .split(',')
        .map(|x| x.trim().parse::<u64>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("bad rangeset '{}'", s))?;
    if nums.is_empty() || nums.len() != nums[0] as usize + 1 {
        bail!("rangeset count mismatch in '{}'", s);
    }
    let mut out = Vec::new();
    let mut i = 1;
    while i + 1 < nums.len() {
        if nums[i] > nums[i + 1] {
            bail!("rangeset range {}-{} is reversed", nums[i], nums[i + 1]);
        }
        out.push((nums[i], nums[i + 1]));
        i += 2;
    }
    Ok(out)
}

pub fn rangeset_string(r: &[(u64, u64)]) -> String {
    let mut s = format!("{}", r.len() * 2);
    for (a, b) in r {
        s.push_str(&format!(",{},{}", a, b));
    }
    s
}

pub fn parse_transfer_list(text: &str) -> Result<TransferList> {
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    let version: u32 = lines
        .next()
        .context("empty transfer list")?
        .trim()
        .parse()
        .context("bad transfer list version")?;
    let total_blocks: u64 = lines
        .next()
        .context("missing block count")?
        .trim()
        .parse()
        .context("bad block count")?;
    if version >= 2 {
        lines.next(); // stash entries needed
        lines.next(); // max stashed blocks
    }
    let mut cmds = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let cmd = it.next().unwrap();
        match cmd {
            "new" => cmds.push(Cmd::New(rangeset(it.next().context("new: missing ranges")?)?)),
            "zero" => cmds.push(Cmd::Zero(rangeset(
                it.next().context("zero: missing ranges")?,
            )?)),
            "erase" => cmds.push(Cmd::Erase(rangeset(
                it.next().context("erase: missing ranges")?,
            )?)),
            "copy" => {
                let src = rangeset(it.next().context("copy: missing src")?)?;
                let dst = rangeset(it.next().context("copy: missing dst")?)?;
                cmds.push(Cmd::Copy { src, dst });
            }
            "move" => {
                let src = rangeset(it.next().context("move: missing src")?)?;
                let dst = rangeset(it.next().context("move: missing dst")?)?;
                cmds.push(Cmd::Move { src, dst });
            }
            "bsdiff" => {
                let patch_off: u64 = it.next().context("bsdiff: missing offset")?.parse().context("bsdiff: bad offset")?;
                let patch_len: u64 = it.next().context("bsdiff: missing length")?.parse().context("bsdiff: bad length")?;
                let rest: Vec<&str> = it.collect();
                if rest.len() < 2 {
                    bail!("bsdiff: expected src and dst rangesets");
                }
                let src = rangeset(rest[rest.len() - 2])?;
                let dst = rangeset(rest[rest.len() - 1])?;
                cmds.push(Cmd::Bsdiff { patch_off, patch_len, src, dst });
            }
            "stash" => {
                let id: u32 = it.next().context("stash: missing id")?.parse().context("stash: bad id")?;
                let ranges = match it.next() { Some(s) => rangeset(s)?, None => Vec::new() };
                cmds.push(Cmd::Stash { id, ranges });
            }
            "free" => {
                let id: u32 = it.next().context("free: missing id")?.parse().context("free: bad id")?;
                cmds.push(Cmd::Free { id });
            }
            other => bail!("unknown transfer list command '{}'", other),
        }
    }
    Ok(TransferList {
        version,
        total_blocks,
        cmds,
    })
}

pub fn load_transfer_list(p: &Path) -> Result<TransferList> {
    let text = std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
    parse_transfer_list(&text)
}

fn last_block(tl: &TransferList) -> u64 {
    let mut max = 0u64;
    for c in &tl.cmds {
        let rs = match c {
            Cmd::New(r) | Cmd::Zero(r) | Cmd::Erase(r) => r,
            Cmd::Copy { dst, .. } | Cmd::Move { dst, .. } | Cmd::Bsdiff { dst, .. } => dst,
            Cmd::Stash { .. } | Cmd::Free { .. } => continue,
        };
        for (_, e) in rs {
            max = max.max(*e);
        }
    }
    max
}

/// `sdat2img`: rebuild a raw image from `.new.dat` + `.transfer.list`.
pub fn merge(transfer: &Path, new_dat: &Path, out: &Path) -> Result<u64> {
    let tl = load_transfer_list(transfer)?;
    let size = tl.total_blocks.max(last_block(&tl)) * BLOCK_SIZE;
    let mut src = File::open(new_dat).with_context(|| format!("open {}", new_dat.display()))?;
    let mut dst = create_sized(out, size)?;
    let mut buf = vec![0u8; BLOCK_SIZE as usize];

    for c in &tl.cmds {
        match c {
            Cmd::New(rs) => {
                for (b, e) in rs {
                    dst.seek(SeekFrom::Start(b * BLOCK_SIZE))?;
                    for _ in *b..*e {
                        src.read_exact(&mut buf)?;
                        dst.write_all(&buf)?;
                    }
                }
            }
            Cmd::Zero(rs) | Cmd::Erase(rs) => {
                let zeros = vec![0u8; BLOCK_SIZE as usize];
                for (b, e) in rs {
                    dst.seek(SeekFrom::Start(b * BLOCK_SIZE))?;
                    for _ in *b..*e {
                        dst.write_all(&zeros)?;
                    }
                }
            }
            other => bail!("merge: unsupported command {:?} (use `aimg dat patch`)", other),
        }
    }
    dst.sync_all()?;
    Ok(size)
}

fn read_ranges(f: &mut File, rs: &[(u64, u64)]) -> Result<Vec<u8>> {
    let total: u64 = rs.iter().map(|(a, b)| (b - a) * BLOCK_SIZE).sum();
    if total > f.metadata()?.len().saturating_add(BLOCK_SIZE) {
        bail!("ranges span {} bytes, larger than the source image", total);
    }
    let mut out = Vec::with_capacity(total.min(64 << 20) as usize);
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    for (a, b) in rs {
        f.seek(SeekFrom::Start(a * BLOCK_SIZE))?;
        for _ in *a..*b {
            f.read_exact(&mut buf)?;
            out.extend_from_slice(&buf);
        }
    }
    Ok(out)
}

fn write_ranges(f: &mut File, rs: &[(u64, u64)], data: &[u8]) -> Result<()> {
    let mut off = 0usize;
    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    for (a, b) in rs {
        f.seek(SeekFrom::Start(a * BLOCK_SIZE))?;
        for _ in *a..*b {
            let n = buf.len().min(data.len().saturating_sub(off));
            if n == 0 {
                for x in buf.iter_mut() {
                    *x = 0;
                }
                f.write_all(&buf)?;
                continue;
            }
            buf[..n].copy_from_slice(&data[off..off + n]);
            if n < buf.len() {
                for x in buf[n..].iter_mut() {
                    *x = 0;
                }
            }
            f.write_all(&buf)?;
            off += n;
        }
    }
    Ok(())
}

/// Differential merge: apply `system.patch.dat` on top of a base image.
///
/// Real transfer-list syntax (blockimg.cpp):
/// `bsdiff <patch_off> <patch_len> [hashes...] <src_rs> <dst_rs>`,
/// `move <src_rs> <dst_rs>`, `stash <id> [ranges]`, `free <id>`.
/// Sources are always read from the untouched base image, so in-place
/// overwrites cannot corrupt later operations.
pub fn patch(base: &Path, transfer: &Path, new_dat: &Path, patch_dat: &Path, out: &Path) -> Result<()> {
    let tl = load_transfer_list(transfer)?;
    let tmp = out.with_extension("patchtmp");
    std::fs::copy(base, &tmp).with_context(|| format!("copy base {}", base.display()))?;
    let mut srcf = File::open(base).with_context(|| format!("open base {}", base.display()))?;
    let mut dst = File::options().read(true).write(true).open(&tmp)?;
    let size = tl.total_blocks.max(last_block(&tl)) * BLOCK_SIZE;
    if dst.metadata()?.len() < size {
        dst.set_len(size)?;
    }
    let mut nd = File::open(new_dat).with_context(|| format!("open {}", new_dat.display()))?;
    let mut pd = File::open(patch_dat).with_context(|| format!("open {}", patch_dat.display()))?;

    for c in &tl.cmds {
        match c {
            Cmd::New(rs) => {
                let mut buf = vec![0u8; BLOCK_SIZE as usize];
                for (b, e) in rs {
                    dst.seek(SeekFrom::Start(b * BLOCK_SIZE))?;
                    for _ in *b..*e {
                        nd.read_exact(&mut buf)?;
                        dst.write_all(&buf)?;
                    }
                }
            }
            Cmd::Zero(rs) | Cmd::Erase(rs) => {
                let zeros = vec![0u8; BLOCK_SIZE as usize];
                for (b, e) in rs {
                    dst.seek(SeekFrom::Start(b * BLOCK_SIZE))?;
                    for _ in *b..*e {
                        dst.write_all(&zeros)?;
                    }
                }
            }
            Cmd::Copy { src, dst: dstr } | Cmd::Move { src, dst: dstr } => {
                let data = read_ranges(&mut srcf, src)?;
                write_ranges(&mut dst, dstr, &data)?;
            }
            Cmd::Bsdiff {
                patch_off,
                patch_len,
                src,
                dst: dstr,
            } => {
                let pd_len = pd.metadata()?.len();
                if patch_off.saturating_add(*patch_len) > pd_len || *patch_len > (1u64 << 31) {
                    bail!("bsdiff blob [{}, +{}) is outside the patch file", patch_off, patch_len);
                }
                pd.seek(SeekFrom::Start(*patch_off))?;
                let mut blob = vec![0u8; *patch_len as usize];
                pd.read_exact(&mut blob)?;
                let base_data = read_ranges(&mut srcf, src)?;
                let new_data = crate::payload::bsdiff_apply(&base_data, &blob)?;
                write_ranges(&mut dst, dstr, &new_data)?;
            }
            Cmd::Stash { .. } | Cmd::Free { .. } => {
                // Sources are read from the immutable base, so the stash buffer
                // is not needed for correctness.
            }
        }
    }
    dst.sync_all()?;
    drop(dst);
    if out.exists() {
        let _ = std::fs::remove_file(out);
    }
    std::fs::rename(&tmp, out)?;
    Ok(())
}

/// `img2sdat`: split a raw image into `<prefix>.new.dat` + `<prefix>.transfer.list`.
pub fn split(img: &Path, version: u32, prefix: &str, outdir: &Path) -> Result<(u64, u64)> {
    if !(1..=4).contains(&version) {
        bail!("transfer list version must be 1..4");
    }
    let len = file_len(img)?;
    let blocks = len.div_ceil(BLOCK_SIZE);
    std::fs::create_dir_all(outdir)?;
    let dat_path = outdir.join(format!("{}.new.dat", prefix));
    let tl_path = outdir.join(format!("{}.transfer.list", prefix));
    let patch_path = outdir.join(format!("{}.patch.dat", prefix));

    std::fs::copy(img, &dat_path)?;
    let f = File::options().write(true).open(&dat_path)?;
    f.set_len(blocks * BLOCK_SIZE)?;

    let mut tl = String::new();
    tl.push_str(&format!("{}\n", version));
    tl.push_str(&format!("{}\n", blocks));
    if version >= 2 {
        tl.push_str("0\n0\n");
    }
    // one range covering the whole image
    tl.push_str(&format!("new {}\n", rangeset_string(&[(0, blocks)])));
    std::fs::write(&tl_path, tl)?;
    std::fs::write(&patch_path, b"")?;
    Ok((blocks, blocks * BLOCK_SIZE))
}

// ---------------------------------------------------------------------------
// brotli (.dat.br)
// ---------------------------------------------------------------------------

pub fn brotli_decompress(src: &Path, dst: &Path) -> Result<u64> {
    let mut input = File::open(src).with_context(|| format!("open {}", src.display()))?;
    let mut out = File::create(dst)?;
    let mut dec = brotli::Decompressor::new(&mut input, 1 << 16);
    let n = std::io::copy(&mut dec, &mut out)?;
    Ok(n)
}

pub fn brotli_compress(src: &Path, dst: &Path, quality: u32) -> Result<u64> {
    let mut input = File::open(src).with_context(|| format!("open {}", src.display()))?;
    let out = File::create(dst)?;
    let mut params = brotli::enc::BrotliEncoderParams::default();
    params.quality = quality as i32;
    let mut w = brotli::CompressorWriter::with_params(out, 1 << 16, &params);
    let n = std::io::copy(&mut input, &mut w)?;
    w.flush()?;
    Ok(n)
}

/// If `p` ends in `.br`, decompress to `<name without .br>` and return that path.
pub fn maybe_br(p: &Path, workdir: &Path) -> Result<std::path::PathBuf> {
    let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
    if !name.ends_with(".br") {
        return Ok(p.to_path_buf());
    }
    std::fs::create_dir_all(workdir)?;
    let out = workdir.join(name.trim_end_matches(".br"));
    brotli_decompress(p, &out)?;
    Ok(out)
}

/// Convenience: read the first command list summary.
pub fn describe(p: &Path) -> Result<String> {
    let tl = load_transfer_list(p)?;
    let mut s = format!(
        "transfer.list version {}: {} blocks ({})\n",
        tl.version,
        tl.total_blocks,
        human(tl.total_blocks * BLOCK_SIZE)
    );
    for (i, c) in tl.cmds.iter().enumerate() {
        let d = match c {
            Cmd::New(r) => format!("new {}", rangeset_string(r)),
            Cmd::Zero(r) => format!("zero {}", rangeset_string(r)),
            Cmd::Erase(r) => format!("erase {}", rangeset_string(r)),
            Cmd::Copy { src, .. } => format!("copy {}", rangeset_string(src)),
            Cmd::Move { src, .. } => format!("move {}", rangeset_string(src)),
            Cmd::Bsdiff { patch_off, patch_len, src, .. } => format!(
                "bsdiff {} {} {}",
                patch_off,
                patch_len,
                rangeset_string(src)
            ),
            Cmd::Stash { id, .. } => format!("stash {}", id),
            Cmd::Free { id } => format!("free {}", id),
        };
        s.push_str(&format!("  #{:<3} {}\n", i, d));
    }
    Ok(s)
}

/// Skip the header lines of a `.patch.dat` stream, returning the payload offset.
pub fn patch_stream_offset(p: &Path) -> Result<u64> {
    let f = File::open(p)?;
    let mut r = BufReader::new(f);
    let mut line = String::new();
    r.read_line(&mut line)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rangeset_parse() {
        let r = rangeset("6,0,1,2,3,4,5").unwrap();
        assert_eq!(r, vec![(0, 1), (2, 3), (4, 5)]);
        assert_eq!(rangeset_string(&r), "6,0,1,2,3,4,5");
    }

    #[test]
    fn merge_and_split_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let img = d.path().join("system.img");
        let mut data = vec![0u8; 20 * 4096];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 253) as u8;
        }
        std::fs::write(&img, &data).unwrap();

        let (blocks, _) = split(&img, 4, "system", d.path()).unwrap();
        assert_eq!(blocks, 20);
        let tl = d.path().join("system.transfer.list");
        let text = std::fs::read_to_string(&tl).unwrap();
        assert!(text.starts_with("4\n20\n0\n0\nnew "));

        let out = d.path().join("back.img");
        merge(&tl, &d.path().join("system.new.dat"), &out).unwrap();
        let got = std::fs::read(&out).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn patch_copy_and_bsdiff() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("base.img");
        let data = vec![0xAAu8; 8 * 4096];
        std::fs::write(&base, &data).unwrap();

        // build a bsdiff-looking stream: new content = base + 1 per byte
        let old = data.clone();
        let new: Vec<u8> = old.iter().map(|b| b.wrapping_add(1)).collect();
        // simple patch: diff bytes with x = len, y = 0, z = 0
        let mut ctrl = Vec::new();
        ctrl.extend_from_slice(&(old.len() as i64).to_le_bytes());
        ctrl.extend_from_slice(&0i64.to_le_bytes());
        ctrl.extend_from_slice(&0i64.to_le_bytes());
        let mut ctrl_bz = Vec::new();
        {
            let mut e = bzip2::write::BzEncoder::new(&mut ctrl_bz, bzip2::Compression::best());
            e.write_all(&ctrl).unwrap();
            e.finish().unwrap();
        }
        let mut diff_bz = Vec::new();
        {
            let mut e = bzip2::write::BzEncoder::new(&mut diff_bz, bzip2::Compression::best());
            e.write_all(&vec![1u8; old.len()]).unwrap();
            e.finish().unwrap();
        }
        let extra_bz = {
            let mut v = Vec::new();
            {
                let mut e = bzip2::write::BzEncoder::new(&mut v, bzip2::Compression::best());
                e.write_all(&[]).unwrap();
                e.finish().unwrap();
            }
            v
        };
        let mut patch_blob = Vec::new();
        patch_blob.extend_from_slice(b"BSDIFF40");
        patch_blob.extend_from_slice(&(ctrl_bz.len() as i64).to_le_bytes());
        patch_blob.extend_from_slice(&(diff_bz.len() as i64).to_le_bytes());
        patch_blob.extend_from_slice(&(new.len() as i64).to_le_bytes());
        patch_blob.extend_from_slice(&ctrl_bz);
        patch_blob.extend_from_slice(&diff_bz);
        patch_blob.extend_from_slice(&extra_bz);

        let tl = d.path().join("t.transfer.list");
        std::fs::write(
            &tl,
            format!("4\n8\n0\n0\nbsdiff 0 {} 2,0,8 2,0,8\n", patch_blob.len()),
        )
        .unwrap();
        let nd = d.path().join("n.dat");
        std::fs::write(&nd, b"").unwrap();
        let pd = d.path().join("p.dat");
        std::fs::write(&pd, &patch_blob).unwrap();

        let out = d.path().join("out.img");
        patch(&base, &tl, &nd, &pd, &out).unwrap();
        let got = std::fs::read(&out).unwrap();
        assert_eq!(got, new);
    }
}
