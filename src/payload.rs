//! `payload.bin` (A/B OTA) reader and writer.
//!
//! Rewritten from AOSP `system/update_engine` (`update_metadata.proto`,
//! `payload_generator/generate_delta_main.cc`) and cross-checked against
//! `ssut/payload-dumper-go` + `vm03/payload_dumper`.
//!
//! Supports extracting full and delta payloads (REPLACE, REPLACE_BZ,
//! REPLACE_XZ, ZERO, DISCARD, SOURCE_COPY, SOURCE_BSDIFF) and generating a
//! full payload from raw partition images, optionally RSA signed.

use anyhow::{bail, Context, Result};
use bzip2::read::BzDecoder;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const PAYLOAD_MAGIC: &[u8; 4] = b"CrAU";
pub const OP_REPLACE: u32 = 0;
pub const OP_REPLACE_BZ: u32 = 1;
pub const OP_MOVE: u32 = 2;
pub const OP_BSDIFF: u32 = 3;
pub const OP_SOURCE_COPY: u32 = 4;
pub const OP_SOURCE_BSDIFF: u32 = 5;
pub const OP_ZERO: u32 = 6;
pub const OP_DISCARD: u32 = 7;
pub const OP_REPLACE_XZ: u32 = 8;
pub const OP_BROTLI_BSDIFF: u32 = 10;

#[derive(Default, Clone, Debug, Serialize)]
pub struct Extent {
    pub start_block: u64,
    pub num_blocks: u64,
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct InstallOp {
    pub ty: u32,
    pub data_offset: u64,
    pub data_length: u64,
    pub src_extents: Vec<Extent>,
    pub src_length: u64,
    pub dst_extents: Vec<Extent>,
    pub dst_length: u64,
    pub data_sha256: Vec<u8>,
    pub src_sha256: Vec<u8>,
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct PartitionInfo {
    pub size: u64,
    pub hash: Vec<u8>,
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct PartitionUpdate {
    pub name: String,
    pub ops: Vec<InstallOp>,
    pub old_info: Option<PartitionInfo>,
    pub new_info: Option<PartitionInfo>,
    pub filesystem_type: String,
    pub run_postinstall: bool,
    pub postinstall_path: String,
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct Manifest {
    pub block_size: u64,
    pub minor_version: u32,
    pub max_timestamp: i64,
    pub partitions: Vec<PartitionUpdate>,
    pub partial_update: bool,
    pub security_patch_level: String,
    pub signatures_offset: u64,
    pub signatures_size: u64,
}

// ---------------------------------------------------------------------------
// minimal protobuf
// ---------------------------------------------------------------------------

struct Pb<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Pb<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, p: 0 }
    }
    fn eof(&self) -> bool {
        self.p >= self.d.len()
    }
    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        let mut s = 0;
        loop {
            if self.p >= self.d.len() || s > 63 {
                bail!("bad varint");
            }
            let b = self.d[self.p];
            self.p += 1;
            v |= ((b & 0x7f) as u64) << s;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            s += 7;
        }
    }
    fn tag(&mut self) -> Result<Option<(u32, u8)>> {
        if self.eof() {
            return Ok(None);
        }
        let k = self.varint()?;
        Ok(Some(((k >> 3) as u32, (k & 7) as u8)))
    }
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.varint()? as usize;
        let end = self.p.checked_add(n).context("protobuf length overflow")?;
        if end > self.d.len() {
            bail!("truncated protobuf field");
        }
        let s = &self.d[self.p..end];
        self.p = end;
        Ok(s)
    }
    fn skip(&mut self, wire: u8) -> Result<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.p += 8,
            2 => {
                self.bytes()?;
            }
            5 => self.p += 4,
            _ => bail!("unsupported wire type {}", wire),
        }
        Ok(())
    }
}

fn parse_extent(d: &[u8]) -> Result<Extent> {
    let mut r = Pb::new(d);
    let mut e = Extent::default();
    while let Some((f, w)) = r.tag()? {
        match (f, w) {
            (1, 0) => e.start_block = r.varint()?,
            (2, 0) => e.num_blocks = r.varint()?,
            _ => r.skip(w)?,
        }
    }
    Ok(e)
}

fn parse_info(d: &[u8]) -> Result<PartitionInfo> {
    let mut r = Pb::new(d);
    let mut i = PartitionInfo::default();
    while let Some((f, w)) = r.tag()? {
        match (f, w) {
            (1, 0) => i.size = r.varint()?,
            (2, 2) => i.hash = r.bytes()?.to_vec(),
            _ => r.skip(w)?,
        }
    }
    Ok(i)
}

fn parse_op(d: &[u8]) -> Result<InstallOp> {
    let mut r = Pb::new(d);
    let mut o = InstallOp::default();
    while let Some((f, w)) = r.tag()? {
        match (f, w) {
            (1, 0) => o.ty = r.varint()? as u32,
            (2, 0) => o.data_offset = r.varint()?,
            (3, 0) => o.data_length = r.varint()?,
            (4, 2) => o.src_extents.push(parse_extent(r.bytes()?)?),
            (5, 0) => o.src_length = r.varint()?,
            (6, 2) => o.dst_extents.push(parse_extent(r.bytes()?)?),
            (7, 0) => o.dst_length = r.varint()?,
            (8, 2) => o.data_sha256 = r.bytes()?.to_vec(),
            (9, 2) => o.src_sha256 = r.bytes()?.to_vec(),
            _ => r.skip(w)?,
        }
    }
    Ok(o)
}

fn parse_partition(d: &[u8]) -> Result<PartitionUpdate> {
    let mut r = Pb::new(d);
    let mut p = PartitionUpdate::default();
    while let Some((f, w)) = r.tag()? {
        match (f, w) {
            (1, 2) => p.name = String::from_utf8_lossy(r.bytes()?).to_string(),
            (2, 0) => p.run_postinstall = r.varint()? != 0,
            (3, 2) => p.postinstall_path = String::from_utf8_lossy(r.bytes()?).to_string(),
            (4, 2) => p.filesystem_type = String::from_utf8_lossy(r.bytes()?).to_string(),
            (6, 2) => p.old_info = Some(parse_info(r.bytes()?)?),
            (7, 2) => p.new_info = Some(parse_info(r.bytes()?)?),
            (8, 2) => p.ops.push(parse_op(r.bytes()?)?),
            _ => r.skip(w)?,
        }
    }
    Ok(p)
}

fn parse_manifest(d: &[u8]) -> Result<Manifest> {
    let mut r = Pb::new(d);
    let mut m = Manifest {
        block_size: 4096,
        ..Default::default()
    };
    while let Some((f, w)) = r.tag()? {
        match (f, w) {
            (3, 0) => m.block_size = r.varint()?,
            (4, 0) => m.signatures_offset = r.varint()?,
            (5, 0) => m.signatures_size = r.varint()?,
            (12, 0) => m.minor_version = r.varint()? as u32,
            (13, 2) => m.partitions.push(parse_partition(r.bytes()?)?),
            (14, 0) => m.max_timestamp = r.varint()? as i64,
            (16, 0) => m.partial_update = r.varint()? != 0,
            (18, 2) => m.security_patch_level = String::from_utf8_lossy(r.bytes()?).to_string(),
            _ => r.skip(w)?,
        }
    }
    Ok(m)
}

#[derive(Default)]
struct Pbw {
    out: Vec<u8>,
}

impl Pbw {
    fn varint(&mut self, mut v: u64) {
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            self.out.push(b);
            if v == 0 {
                break;
            }
        }
    }
    fn tag(&mut self, f: u32, w: u8) {
        self.varint(((f as u64) << 3) | w as u64);
    }
    fn uint(&mut self, f: u32, v: u64) {
        if v == 0 {
            return;
        }
        self.tag(f, 0);
        self.varint(v);
    }
    fn bytes(&mut self, f: u32, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        self.tag(f, 2);
        self.varint(b.len() as u64);
        self.out.extend_from_slice(b);
    }
    fn string(&mut self, f: u32, s: &str) {
        self.bytes(f, s.as_bytes());
    }
    fn msg(&mut self, f: u32, m: Pbw) {
        self.bytes(f, &m.out);
    }
}

fn enc_extent(e: &Extent) -> Pbw {
    let mut w = Pbw::default();
    w.uint(1, e.start_block);
    w.uint(2, e.num_blocks);
    w
}

fn enc_info(i: &PartitionInfo) -> Pbw {
    let mut w = Pbw::default();
    w.uint(1, i.size);
    w.bytes(2, &i.hash);
    w
}

fn enc_op(o: &InstallOp) -> Pbw {
    let mut w = Pbw::default();
    w.uint(1, o.ty as u64);
    w.uint(2, o.data_offset);
    w.uint(3, o.data_length);
    for e in &o.src_extents {
        w.msg(4, enc_extent(e));
    }
    w.uint(5, o.src_length);
    for e in &o.dst_extents {
        w.msg(6, enc_extent(e));
    }
    w.uint(7, o.dst_length);
    w.bytes(8, &o.data_sha256);
    w.bytes(9, &o.src_sha256);
    w
}

fn enc_partition(p: &PartitionUpdate) -> Pbw {
    let mut w = Pbw::default();
    w.string(1, &p.name);
    if p.run_postinstall {
        w.uint(2, 1);
    }
    w.string(3, &p.postinstall_path);
    w.string(4, &p.filesystem_type);
    if let Some(i) = &p.old_info {
        w.msg(6, enc_info(i));
    }
    if let Some(i) = &p.new_info {
        w.msg(7, enc_info(i));
    }
    for o in &p.ops {
        w.msg(8, enc_op(o));
    }
    w
}

fn enc_manifest(m: &Manifest) -> Vec<u8> {
    let mut w = Pbw::default();
    w.uint(3, m.block_size);
    w.uint(4, m.signatures_offset);
    w.uint(5, m.signatures_size);
    w.uint(12, m.minor_version as u64);
    for p in &m.partitions {
        w.msg(13, enc_partition(p));
    }
    if m.max_timestamp != 0 {
        w.tag(14, 0);
        w.varint(m.max_timestamp as u64);
    }
    if m.partial_update {
        w.uint(16, 1);
    }
    w.string(18, &m.security_patch_level);
    w.out
}

// ---------------------------------------------------------------------------
// payload file
// ---------------------------------------------------------------------------

pub struct Payload {
    pub version: u64,
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub metadata_signature: Vec<u8>,
    /// Offset of the blob area inside the payload file.
    pub data_start: u64,
}

impl Payload {
    pub fn open(p: &Path) -> Result<Self> {
        let mut f = File::open(p).with_context(|| format!("open {}", p.display()))?;
        let mut head = [0u8; 20];
        f.read_exact(&mut head)?;
        if &head[0..4] != PAYLOAD_MAGIC {
            bail!("not a payload.bin (bad magic)");
        }
        let version = u64::from_be_bytes(head[4..12].try_into().unwrap());
        let manifest_size = u64::from_be_bytes(head[12..20].try_into().unwrap());
        // v1 has a 20-byte header; v2 added metadata_signature_size.
        let mut meta_sig_size = 0u64;
        let header_len: u64 = if version >= 2 {
            let mut b = [0u8; 4];
            f.read_exact(&mut b)?;
            meta_sig_size = u32::from_be_bytes(b) as u64;
            24
        } else {
            20
        };
        let file_len = f.metadata()?.len();
        if manifest_size > (64u64 << 20)
            || header_len
                .saturating_add(manifest_size)
                .saturating_add(meta_sig_size)
                > file_len
        {
            bail!("payload manifest size {} does not fit file", manifest_size);
        }
        if meta_sig_size > (1 << 20) {
            bail!("absurd metadata signature size {}", meta_sig_size);
        }
        let mut mb = vec![0u8; manifest_size as usize];
        f.read_exact(&mut mb)?;
        let mut sig = vec![0u8; meta_sig_size as usize];
        if meta_sig_size > 0 {
            f.read_exact(&mut sig)?;
        }
        let manifest = parse_manifest(&mb)?;
        Ok(Self {
            version,
            manifest,
            manifest_bytes: mb,
            metadata_signature: sig,
            data_start: header_len + manifest_size + meta_sig_size,
        })
    }

    pub fn info(&self) -> String {
        let m = &self.manifest;
        let mut s = format!(
            "payload version: {}\nmanifest: {} bytes\nblock_size: {}\nminor_version: {}\nmax_timestamp: {}\npartitions: {}\n",
            self.version,
            self.manifest_bytes.len(),
            m.block_size,
            m.minor_version,
            m.max_timestamp,
            m.partitions.len()
        );
        for p in &m.partitions {
            let size = p.new_info.as_ref().map(|i| i.size).unwrap_or(0);
            s.push_str(&format!(
                "  {:<20} size={:<12} ops={} fs={}\n",
                p.name,
                crate::util::human(size),
                p.ops.len(),
                p.filesystem_type
            ));
        }
        s
    }
}

fn xz_compress(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write as _;
    let mut enc = xz2::write::XzEncoder::new(Vec::new(), 6);
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

fn xz_decompress(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut dec = xz2::read::XzDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)?;
    Ok(out)
}

fn read_blob(f: &mut File, payload: &Payload, off: u64, len: u64) -> Result<Vec<u8>> {
    let start = payload.data_start + off;
    let file_len = f.metadata()?.len();
    if start.saturating_add(len) > file_len {
        bail!("blob [{}, +{}) is outside the payload", start, len);
    }
    if len > (1u64 << 31) {
        bail!("absurd blob length {}", len);
    }
    let mut b = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(start))?;
    f.read_exact(&mut b)?;
    Ok(b)
}

fn write_zero_extents(f: &mut File, exts: &[Extent], block: u64) -> Result<()> {
    let zero = vec![0u8; 1 << 20];
    for e in exts {
        f.seek(SeekFrom::Start(e.start_block * block))?;
        let mut left = e.num_blocks * block;
        while left > 0 {
            let n = left.min(zero.len() as u64) as usize;
            f.write_all(&zero[..n])?;
            left -= n as u64;
        }
    }
    Ok(())
}

fn read_extents(f: &mut File, exts: &[Extent], block: u64) -> Result<Vec<u8>> {
    let total: u64 = exts.iter().map(|e| e.num_blocks * block).sum();
    if total > (4u64 << 30) {
        bail!("absurd extent span {} bytes", total);
    }
    let mut out = Vec::with_capacity(total as usize);
    for e in exts {
        let mut buf = vec![0u8; (e.num_blocks * block) as usize];
        f.seek(SeekFrom::Start(e.start_block * block))?;
        f.read_exact(&mut buf)?;
        out.extend_from_slice(&buf);
    }
    Ok(out)
}

fn write_extents(f: &mut File, exts: &[Extent], block: u64, data: &[u8]) -> Result<()> {
    let mut off = 0usize;
    for e in exts {
        let n = (e.num_blocks * block) as usize;
        let n = n.min(data.len() - off);
        f.seek(SeekFrom::Start(e.start_block * block))?;
        f.write_all(&data[off..off + n])?;
        off += n;
        if off >= data.len() {
            break;
        }
    }
    Ok(())
}

pub struct ExtractOptions {
    pub partitions: Option<Vec<String>>,
    pub old_dir: Option<PathBuf>,
    pub out_dir: PathBuf,
    pub verify: bool,
}

pub fn extract(payload_path: &Path, opt: &ExtractOptions) -> Result<Vec<PathBuf>> {
    let payload = Payload::open(payload_path)?;
    fs::create_dir_all(&opt.out_dir)?;
    let mut f = File::open(payload_path)?;
    let block = payload.manifest.block_size;
    let mut written = Vec::new();

    for p in &payload.manifest.partitions {
        if let Some(sel) = &opt.partitions {
            if !sel.iter().any(|s| s == &p.name) {
                continue;
            }
        }
        let size = p.new_info.as_ref().map(|i| i.size).unwrap_or(0);
        crate::util::safe_component(&p.name)?;
        let out_path = opt.out_dir.join(format!("{}.img", p.name));
        let mut out = crate::util::create_sized(&out_path, size)?;

        // Base image is only needed when an op actually reads the old partition.
        let needs_old = p
            .ops
            .iter()
            .any(|op| op.ty == OP_SOURCE_COPY || op.ty == OP_SOURCE_BSDIFF);
        let mut old: Option<File> = None;
        if needs_old {
            let dir = opt
                .old_dir
                .as_ref()
                .with_context(|| format!("delta payload: -old required for {}", p.name))?;
            crate::util::safe_component(&p.name)?;
            let op = dir.join(format!("{}.img", p.name));
            old = Some(
                File::open(&op).with_context(|| format!("open base image {}", op.display()))?,
            );
        }

        for (i, op) in p.ops.iter().enumerate() {
            match op.ty {
                OP_REPLACE => {
                    let blob = read_blob(&mut f, &payload, op.data_offset, op.data_length)?;
                    if opt.verify && !op.data_sha256.is_empty() {
                        let h = Sha256::digest(&blob);
                        if h[..] != op.data_sha256[..] {
                            bail!("{}: op {} data sha256 mismatch", p.name, i);
                        }
                    }
                    write_extents(&mut out, &op.dst_extents, block, &blob)?;
                }
                OP_REPLACE_BZ => {
                    let blob = read_blob(&mut f, &payload, op.data_offset, op.data_length)?;
                    if opt.verify && !op.data_sha256.is_empty() {
                        let h = Sha256::digest(&blob);
                        if h[..] != op.data_sha256[..] {
                            bail!("{}: op {} data sha256 mismatch", p.name, i);
                        }
                    }
                    let mut dec = Vec::new();
                    BzDecoder::new(&blob[..]).read_to_end(&mut dec)?;
                    write_extents(&mut out, &op.dst_extents, block, &dec)?;
                }
                OP_REPLACE_XZ => {
                    let blob = read_blob(&mut f, &payload, op.data_offset, op.data_length)?;
                    if opt.verify && !op.data_sha256.is_empty() {
                        let h = Sha256::digest(&blob);
                        if h[..] != op.data_sha256[..] {
                            bail!("{}: op {} data sha256 mismatch", p.name, i);
                        }
                    }
                    let dec = xz_decompress(&blob)?;
                    write_extents(&mut out, &op.dst_extents, block, &dec)?;
                }
                OP_ZERO | OP_DISCARD => {
                    write_zero_extents(&mut out, &op.dst_extents, block)?;
                }
                OP_SOURCE_COPY => {
                    let src = old.as_mut().context("SOURCE_COPY without base image")?;
                    let data = read_extents(src, &op.src_extents, block)?;
                    write_extents(&mut out, &op.dst_extents, block, &data)?;
                }
                OP_SOURCE_BSDIFF | OP_BSDIFF => {
                    let src = old.as_mut().context("SOURCE_BSDIFF without base image")?;
                    let base = read_extents(src, &op.src_extents, block)?;
                    let patch = read_blob(&mut f, &payload, op.data_offset, op.data_length)?;
                    let data = bsdiff_apply(&base, &patch)?;
                    write_extents(&mut out, &op.dst_extents, block, &data)?;
                }
                other => bail!(
                    "{}: op {} unsupported type {} (delta packages may use PUFFDIFF/ZUCCHINI; only REPLACE/SOURCE_COPY/SOURCE_BSDIFF are supported)",
                    p.name,
                    i,
                    other
                ),
            }
        }
        // block-padded ops may extend the file past the real partition size
        out.set_len(size)?;
        out.sync_all()?;
        drop(out);
        if opt.verify {
            if let Some(info) = &p.new_info {
                if !info.hash.is_empty() {
                    let mut h = Sha256::new();
                    let mut bf = File::open(&out_path)?;
                    let mut buf = vec![0u8; 1 << 20];
                    loop {
                        let n = bf.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        h.update(&buf[..n]);
                    }
                    if h.finalize()[..] != info.hash[..] {
                        bail!("{}: partition sha256 mismatch", p.name);
                    }
                }
            }
        }
        written.push(out_path);
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// bsdiff
// ---------------------------------------------------------------------------

fn offtin(b: &[u8]) -> i64 {
    let mut y = (b[7] & 0x7f) as i64;
    for i in (0..7).rev() {
        y = y * 256 + b[i] as i64;
    }
    if b[7] & 0x80 != 0 {
        y = -y;
    }
    y
}

pub fn bsdiff_apply(old: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    if patch.len() >= 4 && &patch[0..4] == b"BSDF2" {
        bail!("BSDF2 (compressed bsdiff) patches are not supported");
    }
    if patch.len() < 32 || &patch[0..8] != b"BSDIFF40" {
        bail!("bad bsdiff patch");
    }
    let ctrl_len = offtin(&patch[8..16]);
    let diff_len = offtin(&patch[16..24]);
    let new_size = offtin(&patch[24..32]);
    if ctrl_len < 0 || diff_len < 0 || new_size < 0 {
        bail!("bad bsdiff header");
    }
    if new_size > (16i64 << 30) {
        bail!("absurd bsdiff output size {}", new_size);
    }
    let ctrl_end = 32 + ctrl_len as usize;
    let diff_end = ctrl_end + diff_len as usize;
    if diff_end > patch.len() {
        bail!("truncated bsdiff patch");
    }
    let mut ctrl = Vec::new();
    BzDecoder::new(&patch[32..ctrl_end]).read_to_end(&mut ctrl)?;
    let mut diff = Vec::new();
    BzDecoder::new(&patch[ctrl_end..diff_end]).read_to_end(&mut diff)?;
    let mut extra = Vec::new();
    BzDecoder::new(&patch[diff_end..]).read_to_end(&mut extra)?;

    let mut new = vec![0u8; new_size as usize];
    let (mut oldpos, mut newpos) = (0i64, 0i64);
    let (mut cp, mut dp, mut ep) = (0usize, 0usize, 0usize);
    while newpos < new_size {
        if cp + 24 > ctrl.len() {
            bail!("bsdiff ctrl truncated");
        }
        let x = offtin(&ctrl[cp..cp + 8]);
        let y = offtin(&ctrl[cp + 8..cp + 16]);
        let z = offtin(&ctrl[cp + 16..cp + 24]);
        cp += 24;
        if x < 0 || y < 0 || z < 0 {
            bail!("negative bsdiff control value");
        }
        for i in 0..x {
            let di = dp + i as usize;
            let ni = newpos + i;
            let oi = oldpos + i;
            if ni < 0 || ni as usize >= new.len() || di >= diff.len() {
                break;
            }
            let ov = if oi >= 0 && (oi as usize) < old.len() {
                old[oi as usize]
            } else {
                0
            };
            new[ni as usize] = ov.wrapping_add(diff[di]);
        }
        newpos += x;
        dp += x as usize;
        for i in 0..y {
            let ni = newpos + i;
            let ei = ep + i as usize;
            if ni < 0 || ni as usize >= new.len() || ei >= extra.len() {
                break;
            }
            new[ni as usize] = extra[ei];
        }
        newpos += y;
        ep += y as usize;
        oldpos += x + z;
    }
    Ok(new)
}

// ---------------------------------------------------------------------------
// repack
// ---------------------------------------------------------------------------

pub struct BuildPartition {
    pub name: String,
    pub image: PathBuf,
}

pub struct BuildOptions {
    pub chunk_size: u64,
    pub max_timestamp: i64,
    pub compress: bool,
    pub sign_key: Option<PathBuf>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            chunk_size: 2 * 1024 * 1024,
            max_timestamp: 0,
            compress: false,
            sign_key: None,
        }
    }
}

/// Build a full (minor_version 0) payload from raw partition images.
pub fn build(parts: &[BuildPartition], out: &Path, opt: &BuildOptions) -> Result<()> {
    if parts.is_empty() {
        bail!("no partitions given");
    }
    const BLOCK: u64 = 4096;
    let mut manifest = Manifest {
        block_size: BLOCK,
        minor_version: 0,
        max_timestamp: opt.max_timestamp,
        ..Default::default()
    };
    // Stream partition data into a temporary blob file so multi-GB images do
    // not have to be held in memory.
    let tmp_blobs = out.with_extension("blobs.tmp");
    let mut blob_out = crate::util::create_sized(&tmp_blobs, 0)?;
    let mut blob_total = 0u64;

    for p in parts {
        let mut src = File::open(&p.image)
            .with_context(|| format!("open {}", p.image.display()))?;
        let flen = src.metadata()?.len();
        let mut ops = Vec::new();
        let mut hasher = Sha256::new();
        // operations must be block aligned: chunk by whole blocks
        let chunk_bytes = (opt.chunk_size / BLOCK).max(1) * BLOCK;
        let mut buf = vec![0u8; chunk_bytes as usize];
        let mut b = 0u64;
        while b < flen {
            let want = (flen - b).min(chunk_bytes) as usize;
            src.read_exact(&mut buf[..want])?;
            hasher.update(&buf[..want]);
            // the operation payload covers whole blocks; pad the tail
            let mut chunk = buf[..want].to_vec();
            let pad_to = chunk.len().div_ceil(BLOCK as usize) * BLOCK as usize;
            if chunk.len() < pad_to {
                chunk.resize(pad_to, 0);
            }
            let mut op = InstallOp {
                ty: OP_REPLACE,
                dst_length: pad_to as u64,
                dst_extents: vec![Extent {
                    start_block: b / BLOCK,
                    num_blocks: (pad_to as u64) / BLOCK,
                }],
                ..Default::default()
            };
            let stored: Vec<u8> = if opt.compress {
                let enc = xz_compress(&chunk)?;
                if enc.len() < chunk.len() {
                    op.ty = OP_REPLACE_XZ;
                    enc
                } else {
                    chunk
                }
            } else {
                chunk
            };
            // data_sha256 covers the stored blob (after compression), which is
            // what payload-dumper / update_engine verify before decompressing.
            op.data_length = stored.len() as u64;
            op.data_sha256 = Sha256::digest(&stored).to_vec();
            op.data_offset = blob_total;
            blob_out.write_all(&stored)?;
            blob_total += op.data_length;
            ops.push(op);
            b += want as u64;
        }
        manifest.partitions.push(PartitionUpdate {
            name: p.name.clone(),
            ops,
            new_info: Some(PartitionInfo {
                size: flen,
                hash: hasher.finalize().to_vec(),
            }),
            ..Default::default()
        });
    }

    let mut mb = enc_manifest(&manifest);
    let meta_size = mb.len() as u64;
    // Sign the manifest if a key was supplied (metadata signature, PKCS#1 v1.5).
    let mut sig: Vec<u8> = Vec::new();
    if let Some(key) = &opt.sign_key {
        sig = crate::payload::sign::sign_sha256(key, &mb)?;
    }
    let meta_hash = Sha256::digest(&mb);
    let mut head = Vec::new();
    head.extend_from_slice(PAYLOAD_MAGIC);
    head.extend_from_slice(&2u64.to_be_bytes());
    head.extend_from_slice(&(mb.len() as u64).to_be_bytes());
    head.extend_from_slice(&(sig.len() as u32).to_be_bytes());
    head.append(&mut mb);
    head.extend_from_slice(&sig);

    let mut f = crate::util::create_sized(out, 0)?;
    f.write_all(&head)?;
    blob_out.sync_all()?;
    drop(blob_out);
    let mut tf = File::open(&tmp_blobs)?;
    std::io::copy(&mut tf, &mut f)?;
    f.sync_all()?;
    let file_len = f.metadata()?.len();
    drop(f);
    let _ = fs::remove_file(&tmp_blobs);

    let mut hf = File::open(out)?;
    let mut fh = Sha256::new();
    let mut copy_buf = vec![0u8; 1 << 20];
    loop {
        let n = hf.read(&mut copy_buf)?;
        if n == 0 {
            break;
        }
        fh.update(&copy_buf[..n]);
    }
    let props = format!(
        "FILE_HASH={}\nFILE_SIZE={}\nMETADATA_HASH={}\nMETADATA_SIZE={}\n",
        hex::encode(fh.finalize()),
        file_len,
        hex::encode(meta_hash),
        meta_size
    );
    let mut props_path = out.as_os_str().to_os_string();
    props_path.push(".properties");
    fs::write(std::path::PathBuf::from(props_path), props)?;
    Ok(())
}

pub mod sign {
    use anyhow::{Context, Result};
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::DecodePrivateKey;
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::RsaPrivateKey;
    use sha2::Sha256;
    use std::fs;
    use std::path::Path;

    /// Sign with a PKCS#8 PEM RSA key (`openssl genrsa -out key.pem 2048`).
    pub fn sign_sha256(key_path: &Path, data: &[u8]) -> Result<Vec<u8>> {
        let pem = fs::read_to_string(key_path)
            .with_context(|| format!("read key {}", key_path.display()))?;
        let key = RsaPrivateKey::from_pkcs8_pem(&pem).context("parse PKCS#8 RSA key")?;
        let signing = SigningKey::<Sha256>::new(key);
        let sig = signing.sign(data);
        Ok(sig.to_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xz_compress_smaller() {
        let data = b"Android payload test line\n".repeat(12000);
        let enc = xz_compress(&data).unwrap();
        assert!(
            enc.len() < data.len(),
            "xz produced {} >= {}",
            enc.len(),
            data.len()
        );
    }

    #[test]
    fn manifest_roundtrip() {
        let mut m = Manifest {
            block_size: 4096,
            minor_version: 0,
            max_timestamp: 12345,
            ..Default::default()
        };
        m.partitions.push(PartitionUpdate {
            name: "system".into(),
            ops: vec![InstallOp {
                ty: OP_REPLACE,
                data_offset: 0,
                data_length: 100,
                dst_extents: vec![Extent {
                    start_block: 0,
                    num_blocks: 1,
                }],
                data_sha256: vec![7u8; 32],
                ..Default::default()
            }],
            new_info: Some(PartitionInfo {
                size: 4096,
                hash: vec![9u8; 32],
            }),
            ..Default::default()
        });
        let raw = enc_manifest(&m);
        let back = parse_manifest(&raw).unwrap();
        assert_eq!(back.block_size, 4096);
        assert_eq!(back.max_timestamp, 12345);
        assert_eq!(back.partitions.len(), 1);
        assert_eq!(back.partitions[0].name, "system");
        assert_eq!(back.partitions[0].ops[0].dst_extents[0].num_blocks, 1);
    }

    #[test]
    fn build_and_extract_full_payload() {
        let d = tempfile::tempdir().unwrap();
        let img = d.path().join("boot.img");
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&img, &data).unwrap();
        let out = d.path().join("payload.bin");
        build(
            &[BuildPartition {
                name: "boot".into(),
                image: img.clone(),
            }],
            &out,
            &BuildOptions {
                chunk_size: 30_000,
                ..Default::default()
            },
        )
        .unwrap();
        let p = Payload::open(&out).unwrap();
        assert_eq!(p.manifest.partitions.len(), 1);
        assert!(p.manifest.partitions[0].ops.len() >= 4);

        let ex = d.path().join("ex");
        extract(
            &out,
            &ExtractOptions {
                partitions: None,
                old_dir: None,
                out_dir: ex.clone(),
                verify: true,
            },
        )
        .unwrap();
        let got = fs::read(ex.join("boot.img")).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn bsdiff_apply_identity() {
        // patch that copies the old file verbatim: x=old_len, y=0, z=0
        let old = b"hello world".to_vec();
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
            e.write_all(&vec![0u8; old.len()]).unwrap();
            e.finish().unwrap();
        }
        let mut extra_bz = Vec::new();
        {
            let mut e = bzip2::write::BzEncoder::new(&mut extra_bz, bzip2::Compression::best());
            e.write_all(&[]).unwrap();
            e.finish().unwrap();
        }
        let mut patch = Vec::new();
        patch.extend_from_slice(b"BSDIFF40");
        patch.extend_from_slice(&(ctrl_bz.len() as i64).to_le_bytes());
        patch.extend_from_slice(&(diff_bz.len() as i64).to_le_bytes());
        patch.extend_from_slice(&(old.len() as i64).to_le_bytes());
        patch.extend_from_slice(&ctrl_bz);
        patch.extend_from_slice(&diff_bz);
        patch.extend_from_slice(&extra_bz);
        let out = bsdiff_apply(&old, &patch).unwrap();
        assert_eq!(out, old);
    }
}
