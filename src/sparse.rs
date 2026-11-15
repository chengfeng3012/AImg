//! Android sparse image format (libsparse compatible).
//!
//! Rewritten from AOSP `system/core/libsparse` (`sparse_format.h`,
//! `simg2img.cpp`, `img2simg.cpp`). Supports RAW / FILL / DONT_CARE / CRC32
//! chunks, multi-file merge (system.img_sparsechunk.*) and raw -> sparse.

use crate::util::*;
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const SPARSE_MAGIC: u32 = 0xed26_ff3a;
pub const CHUNK_TYPE_RAW: u16 = 0xcac1;
pub const CHUNK_TYPE_FILL: u16 = 0xcac2;
pub const CHUNK_TYPE_DONT_CARE: u16 = 0xcac3;
pub const CHUNK_TYPE_CRC32: u16 = 0xcac4;

#[derive(Debug, Clone, Copy)]
pub struct SparseHeader {
    pub major: u16,
    pub minor: u16,
    pub file_hdr_sz: u16,
    pub chunk_hdr_sz: u16,
    pub blk_sz: u32,
    pub total_blks: u32,
    pub total_chunks: u32,
    pub checksum: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkDesc {
    pub ty: u16,
    pub chunk_sz: u32,
    pub total_sz: u32,
    pub data_off: u64,
}

#[derive(Debug, Clone)]
pub struct SparseInfo {
    pub header: SparseHeader,
    pub chunks: Vec<ChunkDesc>,
}

pub fn is_sparse(head: &[u8]) -> bool {
    head.len() >= 4 && u32::from_le_bytes([head[0], head[1], head[2], head[3]]) == SPARSE_MAGIC
}

pub fn file_is_sparse(p: &Path) -> Result<bool> {
    let mut f = File::open(p)?;
    let mut b = [0u8; 4];
    if f.read(&mut b)? < 4 {
        return Ok(false);
    }
    Ok(is_sparse(&b))
}

pub fn read_header(f: &mut File) -> Result<SparseHeader> {
    let mut b = [0u8; 28];
    read_at(f, 0, &mut b)?;
    if !is_sparse(&b) {
        bail!("not an Android sparse image");
    }
    let h = SparseHeader {
        major: u16::from_le_bytes([b[4], b[5]]),
        minor: u16::from_le_bytes([b[6], b[7]]),
        file_hdr_sz: u16::from_le_bytes([b[8], b[9]]),
        chunk_hdr_sz: u16::from_le_bytes([b[10], b[11]]),
        blk_sz: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        total_blks: u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
        total_chunks: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
        checksum: u32::from_le_bytes([b[24], b[25], b[26], b[27]]),
    };
    if h.major != 1 {
        bail!("unsupported sparse major version {}", h.major);
    }
    if h.blk_sz == 0 || h.blk_sz % 4 != 0 {
        bail!("bad sparse block size {}", h.blk_sz);
    }
    Ok(h)
}

pub fn parse(f: &mut File) -> Result<SparseInfo> {
    let header = read_header(f)?;
    if header.total_chunks > (1 << 24) {
        bail!("absurd sparse chunk count {}", header.total_chunks);
    }
    let mut chunks = Vec::new();
    let mut off = header.file_hdr_sz as u64;
    let mut blocks: u64 = 0;
    let len = f.metadata()?.len();
    for _ in 0..header.total_chunks {
        if off + header.chunk_hdr_sz as u64 > len {
            bail!("truncated sparse chunk header at {}", off);
        }
        let mut hb = [0u8; 12];
        read_at(f, off, &mut hb)?;
        let ty = u16::from_le_bytes([hb[0], hb[1]]);
        let chunk_sz = u32::from_le_bytes([hb[4], hb[5], hb[6], hb[7]]);
        let total_sz = u32::from_le_bytes([hb[8], hb[9], hb[10], hb[11]]);
        match ty {
            CHUNK_TYPE_RAW => {
                if total_sz as u64 != header.chunk_hdr_sz as u64 + (chunk_sz as u64 * header.blk_sz as u64) {
                    bail!("RAW chunk size mismatch");
                }
            }
            CHUNK_TYPE_FILL | CHUNK_TYPE_CRC32 => {
                if total_sz != header.chunk_hdr_sz as u32 + 4 {
                    bail!("FILL/CRC32 chunk size mismatch");
                }
            }
            CHUNK_TYPE_DONT_CARE => {}
            _ => bail!("unknown sparse chunk type {:#x}", ty),
        }
        blocks += chunk_sz as u64;
        if blocks > header.total_blks as u64 {
            bail!("sparse chunks claim {} blocks, header says {}", blocks, header.total_blks);
        }
        chunks.push(ChunkDesc {
            ty,
            chunk_sz,
            total_sz,
            data_off: off + header.chunk_hdr_sz as u64,
        });
        off += total_sz as u64;
    }
    Ok(SparseInfo { header, chunks })
}

pub fn info(p: &Path) -> Result<SparseInfo> {
    let mut f = File::open(p)?;
    parse(&mut f)
}

/// Expand (a) sparse image(s) into one raw image. Multiple inputs are
/// concatenated in the given order (system.img_sparsechunk.* use case).
pub fn to_raw(inputs: &[PathBuf], out: &Path) -> Result<u64> {
    let mut dst = create_sized(out, 0)?;
    let mut written = 0u64;
    for p in inputs {
        let mut src = File::open(p).with_context(|| format!("open {}", p.display()))?;
        let header = read_header(&mut src)?;
        let mut buf = vec![0u8; 1 << 20];
        let mut pos = header.file_hdr_sz as u64;
        let mut blocks = 0u64;
        let len = src.metadata()?.len();
        while pos + header.chunk_hdr_sz as u64 <= len {
            let mut hb = [0u8; 12];
            read_at(&mut src, pos, &mut hb)?;
            let ty = u16::from_le_bytes([hb[0], hb[1]]);
            let chunk_sz = u32::from_le_bytes([hb[4], hb[5], hb[6], hb[7]]) as u64;
            let total_sz = u32::from_le_bytes([hb[8], hb[9], hb[10], hb[11]]) as u64;
            let data_off = pos + header.chunk_hdr_sz as u64;
            // A malformed chunk must not stall the loop (total_sz==0) or make
            // us write an absurd amount of data.
            if total_sz < header.chunk_hdr_sz as u64 {
                bail!("sparse chunk at {} has total_sz {}", pos, total_sz);
            }
            if chunk_sz.saturating_mul(header.blk_sz as u64) > (1u64 << 42) {
                bail!("sparse chunk at {} claims {} blocks", pos, chunk_sz);
            }
            if blocks + chunk_sz as u64 > header.total_blks as u64 {
                bail!(
                    "sparse chunk at {} claims blocks {}+{} > header total {}",
                    pos,
                    blocks,
                    chunk_sz,
                    header.total_blks
                );
            }
            match ty {
                CHUNK_TYPE_RAW => {
                    let mut left = chunk_sz * header.blk_sz as u64;
                    let mut o = data_off;
                    while left > 0 {
                        let n = left.min(buf.len() as u64) as usize;
                        read_at(&mut src, o, &mut buf[..n])?;
                        dst.write_all(&buf[..n])?;
                        o += n as u64;
                        left -= n as u64;
                    }
                }
                CHUNK_TYPE_FILL => {
                    let mut pb = [0u8; 4];
                    read_at(&mut src, data_off, &mut pb)?;
                    let pattern: Vec<u8> = pb.iter().cycle().take(1 << 20).cloned().collect();
                    let mut left = chunk_sz * header.blk_sz as u64;
                    while left > 0 {
                        let n = left.min(pattern.len() as u64) as usize;
                        dst.write_all(&pattern[..n])?;
                        left -= n as u64;
                    }
                }
                CHUNK_TYPE_DONT_CARE => {
                    let n = chunk_sz * header.blk_sz as u64;
                    let zeros = vec![0u8; (1 << 20).min(n as usize)];
                    let mut left = n;
                    while left > 0 {
                        let k = left.min(zeros.len() as u64) as usize;
                        dst.write_all(&zeros[..k])?;
                        left -= k as u64;
                    }
                }
                CHUNK_TYPE_CRC32 => {}
                _ => bail!("unknown sparse chunk type {:#x}", ty),
            }
            blocks += chunk_sz;
            pos += total_sz;
        }
        if blocks != header.total_blks as u64 {
            bail!(
                "{}: chunk block count {} != header {}",
                p.display(),
                blocks,
                header.total_blks
            );
        }
        written += blocks * header.blk_sz as u64;
    }
    dst.set_len(written)?;
    Ok(written)
}

fn write_sparse_header(f: &mut File, blk_sz: u32, total_blks: u32, total_chunks: u32) -> Result<()> {
    let mut b = [0u8; 28];
    b[0..4].copy_from_slice(&SPARSE_MAGIC.to_le_bytes());
    b[4..6].copy_from_slice(&1u16.to_le_bytes());
    b[6..8].copy_from_slice(&0u16.to_le_bytes());
    b[8..10].copy_from_slice(&28u16.to_le_bytes());
    b[10..12].copy_from_slice(&12u16.to_le_bytes());
    b[12..16].copy_from_slice(&blk_sz.to_le_bytes());
    b[16..20].copy_from_slice(&total_blks.to_le_bytes());
    b[20..24].copy_from_slice(&total_chunks.to_le_bytes());
    b[24..28].copy_from_slice(&0u32.to_le_bytes());
    write_at(f, 0, &b)
}

struct ChunkWriter<'a> {
    f: &'a mut File,
    pending: Vec<u8>,
    pending_blocks: u32,
    dont_care: u32,
    chunks: u32,
    total_blocks: u32,
}

impl<'a> ChunkWriter<'a> {
    fn new(f: &'a mut File, _blk_sz: u32) -> Self {
        Self {
            f,
            pending: Vec::new(),
            pending_blocks: 0,
            dont_care: 0,
            chunks: 0,
            total_blocks: 0,
        }
    }

    fn flush_dont_care(&mut self) -> Result<()> {
        if self.dont_care == 0 {
            return Ok(());
        }
        let mut hb = [0u8; 12];
        hb[0..2].copy_from_slice(&CHUNK_TYPE_DONT_CARE.to_le_bytes());
        hb[4..8].copy_from_slice(&self.dont_care.to_le_bytes());
        hb[8..12].copy_from_slice(&12u32.to_le_bytes());
        self.f.write_all(&hb)?;
        self.total_blocks += self.dont_care;
        self.chunks += 1;
        self.dont_care = 0;
        Ok(())
    }

    fn flush_raw(&mut self) -> Result<()> {
        if self.pending_blocks == 0 {
            return Ok(());
        }
        let data_len = self.pending.len() as u32;
        let mut hb = [0u8; 12];
        hb[0..2].copy_from_slice(&CHUNK_TYPE_RAW.to_le_bytes());
        hb[4..8].copy_from_slice(&self.pending_blocks.to_le_bytes());
        hb[8..12].copy_from_slice(&(12 + data_len).to_le_bytes());
        self.f.write_all(&hb)?;
        self.f.write_all(&self.pending)?;
        self.total_blocks += self.pending_blocks;
        self.chunks += 1;
        self.pending.clear();
        self.pending_blocks = 0;
        Ok(())
    }

    fn block(&mut self, block: &[u8]) -> Result<()> {
        if block.iter().all(|&b| b == 0) {
            self.flush_raw()?;
            self.dont_care += 1;
            if self.dont_care >= 4096 {
                self.flush_dont_care()?;
            }
            return Ok(());
        }
        self.flush_dont_care()?;
        self.pending.extend_from_slice(block);
        self.pending_blocks += 1;
        if self.pending_blocks >= 1024 {
            self.flush_raw()?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(u32, u32)> {
        self.flush_raw()?;
        self.flush_dont_care()?;
        Ok((self.total_blocks, self.chunks))
    }
}

/// Convert a raw image into a sparse image. `blk_sz` is normally 4096.
pub fn raw_to_sparse(input: &Path, output: &Path, blk_sz: u32) -> Result<()> {
    if blk_sz == 0 || blk_sz % 4 != 0 {
        bail!("invalid block size");
    }
    let mut src = File::open(input)?;
    let len = src.metadata()?.len();
    let total_blocks = ((len + blk_sz as u64 - 1) / blk_sz as u64) as u32;

    let mut dst = create_sized(output, 0)?;
    write_sparse_header(&mut dst, blk_sz, 0, 0)?;

    let mut cw = ChunkWriter::new(&mut dst, blk_sz);
    let mut buf = vec![0u8; blk_sz as usize];
    let mut left = total_blocks as u64;
    while left > 0 {
        let mut filled = 0usize;
        while filled < buf.len() {
            let n = src.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            bail!("unexpected EOF while reading {}", input.display());
        }
        if filled < buf.len() {
            for b in buf[filled..].iter_mut() {
                *b = 0;
            }
        }
        cw.block(&buf)?;
        left -= 1;
    }
    // pad tail partial block with zero blocks
    if left > 0 {
        let zero = vec![0u8; blk_sz as usize];
        while left > 0 {
            cw.block(&zero)?;
            left -= 1;
        }
    }
    let (blocks, chunks) = cw.finish()?;
    write_sparse_header(&mut dst, blk_sz, blocks, chunks)?;
    dst.sync_all()?;
    Ok(())
}

/// Rewrite a sparse image with a different block size (simg2simg).
pub fn resparsify(input: &Path, output: &Path, blk_sz: u32) -> Result<()> {
    let tmp = tempfile::NamedTempFile::new()?;
    let tmp_path = tmp.path().to_path_buf();
    to_raw(&[input.to_path_buf()], &tmp_path)?;
    let tmp_file = File::open(&tmp_path)?;
    drop(tmp_file);
    raw_to_sparse(&tmp_path, output, blk_sz)
}

/// Human readable summary used by `aimg sparse info`.
pub fn describe(p: &Path) -> Result<String> {
    if !file_is_sparse(p)? {
        let len = std::fs::metadata(p)?.len();
        return Ok(format!("{}: raw image, {} bytes", p.display(), len));
    }
    let i = info(p)?;
    let mut s = String::new();
    s.push_str(&format!(
        "{}: Android sparse image v{}.{}\n  block size: {}\n  total blocks: {} ({})\n  chunks: {}\n",
        p.display(),
        i.header.major,
        i.header.minor,
        i.header.blk_sz,
        i.header.total_blks,
        human(i.header.total_blks as u64 * i.header.blk_sz as u64),
        i.header.total_chunks
    ));
    for (n, c) in i.chunks.iter().enumerate() {
        let kind = match c.ty {
            CHUNK_TYPE_RAW => "RAW",
            CHUNK_TYPE_FILL => "FILL",
            CHUNK_TYPE_DONT_CARE => "DONT_CARE",
            CHUNK_TYPE_CRC32 => "CRC32",
            _ => "?",
        };
        s.push_str(&format!(
            "  #{:<3} {} blocks={} bytes={}\n",
            n, kind, c.chunk_sz, c.total_sz
        ));
    }
    Ok(s)
}

/// Detach helper: read whole raw image (used by tests/tools).
pub fn read_raw(p: &Path) -> Result<Vec<u8>> {
    let mut f = File::open(p)?;
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(v)
}

pub fn seek_to_end(f: &mut File) -> Result<u64> {
    Ok(f.seek(SeekFrom::End(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn roundtrip_raw_sparse_raw() {
        let d = tmpdir();
        let raw = d.path().join("a.raw");
        let sp = d.path().join("a.simg");
        let back = d.path().join("b.raw");
        let mut f = File::create(&raw).unwrap();
        let blk = 4096usize;
        // 8 blocks: zeros, data, zeros, data
        f.write_all(&vec![0u8; blk * 2]).unwrap();
        f.write_all(&vec![0xAAu8; blk * 3]).unwrap();
        f.write_all(&vec![0u8; blk * 2]).unwrap();
        f.write_all(&[1u8, 2, 3, 4].repeat(blk / 4).as_slice())
            .unwrap();
        f.sync_all().unwrap();
        drop(f);
        raw_to_sparse(&raw, &sp, 4096).unwrap();
        assert!(file_is_sparse(&sp).unwrap());
        to_raw(&[sp], &back).unwrap();
        assert_eq!(read_raw(&raw).unwrap(), read_raw(&back).unwrap());
    }

    #[test]
    fn multi_input_merge() {
        let d = tmpdir();
        let a = d.path().join("a.simg");
        let b = d.path().join("b.simg");
        let raw_a = d.path().join("a.raw");
        let raw_b = d.path().join("b.raw");
        File::create(&raw_a)
            .unwrap()
            .write_all(&vec![7u8; 8192])
            .unwrap();
        File::create(&raw_b)
            .unwrap()
            .write_all(&vec![9u8; 4096])
            .unwrap();
        raw_to_sparse(&raw_a, &a, 4096).unwrap();
        raw_to_sparse(&raw_b, &b, 4096).unwrap();
        let out = d.path().join("merged.raw");
        to_raw(&[a, b], &out).unwrap();
        let mut expect = vec![7u8; 8192];
        expect.extend(vec![9u8; 4096]);
        assert_eq!(read_raw(&out).unwrap(), expect);
    }
}
