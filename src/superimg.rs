//! Android dynamic partitions: `super.img` LP metadata.
//!
//! Rewritten from AOSP `system/extras/partition_tools`
//! (`liblp/metadata_format.h`, `lpunpack.cc`, `lpmake.cc`, `lpdump.cc`).
//!
//! Layout on disk (all little endian, 512-byte sectors):
//! ```text
//! 0        geometry (4 KiB, sha256 checked)
//! 4096     geometry backup
//! 8192     metadata slot 0 .. slot N-1   (metadata_max_size each)
//! ...      metadata backup copies
//! first_logical_sector  partition extents (dm-linear targets)
//! ```

use crate::sparse;
use crate::util::*;
use anyhow::{bail, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const GEOMETRY_MAGIC: u32 = 0x616c_4467;
pub const HEADER_MAGIC: u32 = 0x414c_5030;
pub const GEOMETRY_SIZE: u64 = 4096;
pub const SECTOR: u64 = 512;
pub const LP_PARTITION_RESERVED_BYTES: u64 = 4096;

pub const ATTR_READONLY: u32 = 1 << 0;
pub const ATTR_SLOT_SUFFIXED: u32 = 1 << 1;
pub const ATTR_UPDATED: u32 = 1 << 2;
pub const ATTR_DISABLED: u32 = 1 << 3;

pub const TARGET_LINEAR: u32 = 0;
pub const TARGET_ZERO: u32 = 1;

const PART_ENTRY_SIZE: usize = 52;
const EXTENT_ENTRY_SIZE: usize = 24;
const GROUP_ENTRY_SIZE: usize = 48;
const DEVICE_ENTRY_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Geometry {
    pub metadata_max_size: u32,
    pub metadata_slot_count: u32,
    pub logical_block_size: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Partition {
    pub name: String,
    pub attributes: u32,
    pub first_extent_index: u32,
    pub num_extents: u32,
    pub group_index: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Extent {
    pub num_sectors: u64,
    pub target_type: u32,
    pub target_data: u64,
    pub target_source: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub name: String,
    pub flags: u32,
    pub maximum_size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockDevice {
    pub first_logical_sector: u64,
    pub alignment: u32,
    pub alignment_offset: u32,
    pub size: u64,
    pub partition_name: String,
    pub flags: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LpMetadata {
    pub major: u16,
    pub minor: u16,
    pub header_flags: u32,
    pub geometry: Geometry,
    pub partitions: Vec<Partition>,
    pub extents: Vec<Extent>,
    pub groups: Vec<Group>,
    pub block_devices: Vec<BlockDevice>,
}

fn u16at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o],
        b[o + 1],
        b[o + 2],
        b[o + 3],
        b[o + 4],
        b[o + 5],
        b[o + 6],
        b[o + 7],
    ])
}

fn sha256_zeroed(buf: &[u8], csum_off: usize, csum_len: usize) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(&buf[..csum_off]);
    h.update(vec![0u8; csum_len]);
    h.update(&buf[csum_off + csum_len..]);
    h.finalize().into()
}

/// Open a path as a raw image. If it is a sparse image it is expanded into a
/// temp file first (returned alongside so the caller keeps it alive).
pub fn open_maybe_sparse(p: &Path) -> Result<(File, Option<tempfile::NamedTempFile>)> {
    if sparse::file_is_sparse(p)? {
        let tmp = tempfile::NamedTempFile::new()?;
        sparse::to_raw(&[p.to_path_buf()], tmp.path())?;
        let f = File::open(tmp.path())?;
        Ok((f, Some(tmp)))
    } else {
        Ok((File::open(p)?, None))
    }
}

/// Some OEM/vendor dumps place the geometry at offset 4096 instead of 0
/// (a 4 KiB prefix). Detect which one it is.
pub fn geometry_base(f: &mut File) -> Result<u64> {
    let len = f.metadata()?.len();
    let mut b = [0u8; 4];
    // standard LP layout: 4 KiB reserved, then primary geometry at 4096
    if len >= GEOMETRY_SIZE + 4 {
        read_at(f, GEOMETRY_SIZE, &mut b)?;
        if u32::from_le_bytes(b) == GEOMETRY_MAGIC {
            return Ok(GEOMETRY_SIZE);
        }
    }
    // legacy images produced by our first writer had geometry at offset 0
    read_at(f, 0, &mut b)?;
    if u32::from_le_bytes(b) == GEOMETRY_MAGIC {
        return Ok(0);
    }
    bail!("not an LP super image (bad geometry magic)")
}

pub fn read_geometry_at(f: &mut File, base: u64) -> Result<Geometry> {
    let mut b = [0u8; 52];
    read_at(f, base, &mut b)?;
    if u32at(&b, 0) != GEOMETRY_MAGIC {
        bail!("not an LP super image (bad geometry magic)");
    }
    let struct_size = u32at(&b, 4) as usize;
    if struct_size < 52 || struct_size > GEOMETRY_SIZE as usize {
        bail!("bad geometry struct size {}", struct_size);
    }
    let mut full = vec![0u8; struct_size];
    read_at(f, base, &mut full)?;
    let expect = sha256_zeroed(&full, 8, 32);
    if full[8..40] != expect {
        bail!("geometry checksum mismatch");
    }
    Ok(Geometry {
        metadata_max_size: u32at(&b, 40),
        metadata_slot_count: u32at(&b, 44),
        logical_block_size: u32at(&b, 48),
    })
}

/// Read the primary geometry, auto-detecting the standard 4 KiB prefix.
pub fn read_geometry(f: &mut File) -> Result<Geometry> {
    let base = geometry_base(f)?;
    read_geometry_at(f, base)
}

pub fn metadata_offset(slot: u32, backup: bool, geo: &Geometry, base_off: u64) -> u64 {
    let base = base_off + GEOMETRY_SIZE * 2 + (geo.metadata_max_size as u64) * slot as u64;
    let mut off = base;
    if backup {
        off += geo.metadata_max_size as u64 * geo.metadata_slot_count as u64;
    }
    off
}

pub fn parse_slot(f: &mut File, slot: u32) -> Result<LpMetadata> {
    let base = geometry_base(f)?;
    let geo = read_geometry_at(f, base)?;
    if slot >= geo.metadata_slot_count {
        bail!(
            "slot {} out of range (metadata_slot_count={})",
            slot,
            geo.metadata_slot_count
        );
    }
    let off = metadata_offset(slot, false, &geo, base);
    parse_header_at(f, off, geo)
}

pub fn parse_header_at(f: &mut File, off: u64, geo: Geometry) -> Result<LpMetadata> {
    let mut head = [0u8; 128];
    read_at(f, off, &mut head)?;
    if u32at(&head, 0) != HEADER_MAGIC {
        bail!("bad LP metadata header magic at {:#x}", off);
    }
    let major = u16at(&head, 4);
    let minor = u16at(&head, 6);
    if major != 10 {
        bail!("unsupported LP metadata major version {}", major);
    }
    let header_size = u32at(&head, 8) as usize;
    if header_size < 128 || header_size > 4096 {
        bail!("bad header size {}", header_size);
    }
    let mut hdr = vec![0u8; header_size];
    read_at(f, off, &mut hdr)?;
    let expect = sha256_zeroed(&hdr, 12, 32);
    if hdr[12..44] != expect {
        bail!("metadata header checksum mismatch");
    }
    let tables_size = u32at(&hdr, 44) as usize;
    if tables_size > (1 << 20) {
        bail!("absurd LP tables size {}", tables_size);
    }
    let mut tables = vec![0u8; tables_size];
    read_at(f, off + header_size as u64, &mut tables)?;
    let expect_t = sha256(&tables);
    if hdr[48..80] != expect_t {
        bail!("metadata tables checksum mismatch");
    }
    let d = |o: usize| (u32at(&hdr, o), u32at(&hdr, o + 4), u32at(&hdr, o + 8));
    let pd = d(80);
    let ed = d(92);
    let gd = d(104);
    let bd = d(116);
    let header_flags = if header_size >= 132 { u32at(&hdr, 128) } else { 0 };
    if (pd.2 as usize) < PART_ENTRY_SIZE
        || (ed.2 as usize) < EXTENT_ENTRY_SIZE
        || (gd.2 as usize) < GROUP_ENTRY_SIZE
        || (bd.2 as usize) < 60
    {
        bail!(
            "LP table entry size too small: partition={} extent={} group={} device={}",
            pd.2,
            ed.2,
            gd.2,
            bd.2
        );
    }

    let mut partitions = Vec::new();
    for i in 0..pd.1 as usize {
        let o = pd.0 as usize + i * pd.2 as usize;
        if o + PART_ENTRY_SIZE > tables.len() {
            bail!("partition table truncated");
        }
        partitions.push(Partition {
            name: cstr(&tables[o..o + 36]),
            attributes: u32at(&tables, o + 36),
            first_extent_index: u32at(&tables, o + 40),
            num_extents: u32at(&tables, o + 44),
            group_index: u32at(&tables, o + 48),
        });
    }
    let mut extents = Vec::new();
    for i in 0..ed.1 as usize {
        let o = ed.0 as usize + i * ed.2 as usize;
        if o + EXTENT_ENTRY_SIZE > tables.len() {
            bail!("extent table truncated");
        }
        extents.push(Extent {
            num_sectors: u64at(&tables, o),
            target_type: u32at(&tables, o + 8),
            target_data: u64at(&tables, o + 12),
            target_source: u32at(&tables, o + 20),
        });
    }
    let mut groups = Vec::new();
    for i in 0..gd.1 as usize {
        let o = gd.0 as usize + i * gd.2 as usize;
        if o + GROUP_ENTRY_SIZE > tables.len() {
            bail!("group table truncated");
        }
        groups.push(Group {
            name: cstr(&tables[o..o + 36]),
            flags: u32at(&tables, o + 36),
            maximum_size: u64at(&tables, o + 40),
        });
    }
    // block device entry is 64 bytes; older metadata uses 60 (no flags)
    let mut block_devices = Vec::new();
    for i in 0..bd.1 as usize {
        let o = bd.0 as usize + i * bd.2 as usize;
        if o + 60 > tables.len() {
            bail!("block device table truncated");
        }
        let flags = if o + 64 <= tables.len() {
            u32at(&tables, o + 60)
        } else {
            0
        };
        block_devices.push(BlockDevice {
            first_logical_sector: u64at(&tables, o),
            alignment: u32at(&tables, o + 8),
            alignment_offset: u32at(&tables, o + 12),
            size: u64at(&tables, o + 16),
            partition_name: cstr(&tables[o + 24..o + 60]),
            flags,
        });
    }

    Ok(LpMetadata {
        major,
        minor,
        header_flags,
        geometry: geo,
        partitions,
        extents,
        groups,
        block_devices,
    })
}

impl LpMetadata {
    pub fn slot_label(&self, p: &Partition) -> String {
        p.name.clone()
    }

    pub fn partition_size(&self, p: &Partition) -> u64 {
        let mut total = 0u64;
        for i in 0..p.num_extents as usize {
            if let Some(e) = self.extents.get(p.first_extent_index as usize + i) {
                total += e.num_sectors;
            }
        }
        total * SECTOR
    }

    pub fn dump(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "Metadata version: {}.{}\nMetadata geometry: max_size={} slots={} logical_block_size={}\n",
            self.major, self.minor, self.geometry.metadata_max_size,
            self.geometry.metadata_slot_count, self.geometry.logical_block_size
        ));
        for (i, g) in self.groups.iter().enumerate() {
            s.push_str(&format!(
                "  Group {}: {} (max {})\n",
                i,
                g.name,
                if g.maximum_size == 0 {
                    "unlimited".to_string()
                } else {
                    human(g.maximum_size)
                }
            ));
        }
        for (i, b) in self.block_devices.iter().enumerate() {
            s.push_str(&format!(
                "  Block device {}: {} size={} first_logical_sector={} alignment={}\n",
                i,
                b.partition_name,
                human(b.size),
                b.first_logical_sector,
                b.alignment
            ));
        }
        for p in &self.partitions {
            let mut extras = Vec::new();
            if p.attributes & ATTR_READONLY != 0 {
                extras.push("readonly");
            }
            if p.attributes & ATTR_SLOT_SUFFIXED != 0 {
                extras.push("slot_suffixed");
            }
            if p.attributes & ATTR_UPDATED != 0 {
                extras.push("updated");
            }
            if p.attributes & ATTR_DISABLED != 0 {
                extras.push("disabled");
            }
            s.push_str(&format!(
                "  Partition {}: size={} group={} extents={} attrs=[{}]\n",
                p.name,
                human(self.partition_size(p)),
                self.groups
                    .get(p.group_index as usize)
                    .map(|g| g.name.clone())
                    .unwrap_or_default(),
                p.num_extents,
                extras.join(",")
            ));
            for i in 0..p.num_extents as usize {
                if let Some(e) = self.extents.get(p.first_extent_index as usize + i) {
                    s.push_str(&format!(
                        "      extent {} sectors @ sector {} (device {})\n",
                        e.num_sectors, e.target_data, e.target_source
                    ));
                }
            }
        }
        s
    }
}

// ---------------------------------------------------------------------------
// unpack (lpunpack)
// ---------------------------------------------------------------------------

pub struct UnpackOptions {
    pub slot: Option<u32>,
    /// Optional filter on logical partition names (base names, without _a/_b).
    pub partitions: Option<Vec<String>>,
    /// Keep the original names including the _a/_b suffix.
    pub keep_suffix: bool,
    pub sparse_out: bool,
    pub sparse_block: u32,
}

pub fn unpack(super_path: &Path, outdir: &Path, opt: &UnpackOptions) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(outdir)?;
    let (mut f, _tmp) = open_maybe_sparse(super_path)?;
    read_geometry(&mut f)?;
    let slot = opt.slot.unwrap_or(0);
    let meta = parse_slot(&mut f, slot)?;
    let mut written = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for p in &meta.partitions {
        if p.attributes & ATTR_DISABLED != 0 {
            continue;
        }
        if let Some(sel) = &opt.partitions {
            let base = p
                .name
                .strip_suffix("_a")
                .or_else(|| p.name.strip_suffix("_b"))
                .unwrap_or(&p.name);
            if !sel.iter().any(|s| s == base || s == &p.name) {
                continue;
            }
        }
        crate::util::safe_component(&p.name)?;
        let slot_excluded = |name: &str| -> bool {
            match opt.slot {
                Some(sl) => {
                    let other = if sl == 0 { "_b" } else { "_a" };
                    name.ends_with(other)
                }
                None => false,
            }
        };
        if slot_excluded(&p.name) {
            continue;
        }
        let mut name = p.name.clone();
        if !opt.keep_suffix {
            if let Some(base) = name.strip_suffix("_a").or_else(|| name.strip_suffix("_b")) {
                let base = base.to_string();
                // Only strip the slot suffix when that base name is unique
                // among the partitions we are actually exporting; otherwise
                // system_a/system_b would silently overwrite each other.
                let collides = meta.partitions.iter().any(|q| {
                    q.name != p.name
                        && !slot_excluded(&q.name)
                        && (q.name == format!("{}_a", base)
                            || q.name == format!("{}_b", base))
                });
                if !collides {
                    name = base;
                }
            }
        }
        let size = meta.partition_size(p);
        let raw_path = if opt.sparse_out {
            outdir.join(format!("{}.raw", name))
        } else {
            outdir.join(format!("{}.img", name))
        };
        if !seen.insert(raw_path.clone()) {
            bail!(
                "duplicate output name {} (use --keep-suffix or --partitions)",
                raw_path.display()
            );
        }
        let mut out = create_sized(&raw_path, size)?;
        let mut pos = 0u64;
        for i in 0..p.num_extents as usize {
            let idx = p.first_extent_index as usize + i;
            let e = meta.extents.get(idx).ok_or_else(|| {
                anyhow::anyhow!("partition {} extent {} out of range", p.name, idx)
            })?;
            if e.target_type == TARGET_ZERO {
                pos += e.num_sectors * SECTOR;
                continue;
            }
            let src_off = e.target_data * SECTOR;
            let len = e.num_sectors * SECTOR;
            out.seek(SeekFrom::Start(pos))?;
            copy_range(&mut f, &mut out, src_off, len)?;
            pos += len;
        }
        out.sync_all()?;
        drop(out);
        if opt.sparse_out {
            let sp = outdir.join(format!("{}.img", name));
            sparse::raw_to_sparse(&raw_path, &sp, opt.sparse_block)?;
            std::fs::remove_file(&raw_path)?;
            written.push(sp);
        } else {
            written.push(raw_path);
        }
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// build (lpmake)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BuildPartition {
    pub name: String,
    pub image: PathBuf,
    pub attributes: u32,
    pub group: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub super_size: Option<u64>,
    pub metadata_max_size: u32,
    pub metadata_slots: u32,
    pub logical_block_size: u32,
    pub device_alignment: u32,
    pub group_max: Option<u64>,
    pub minor_version: u16,
    pub sparse: bool,
    pub sparse_block: u32,
    pub block_device_name: String,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            super_size: None,
            metadata_max_size: 65536,
            metadata_slots: 2,
            logical_block_size: 4096,
            device_alignment: 1024 * 1024,
            group_max: None,
            minor_version: 2,
            sparse: false,
            sparse_block: 4096,
            block_device_name: "super".to_string(),
        }
    }
}

fn write_geometry(f: &mut File, geo: &Geometry) -> Result<()> {
    let mut b = vec![0u8; GEOMETRY_SIZE as usize];
    b[0..4].copy_from_slice(&GEOMETRY_MAGIC.to_le_bytes());
    b[4..8].copy_from_slice(&52u32.to_le_bytes());
    b[40..44].copy_from_slice(&geo.metadata_max_size.to_le_bytes());
    b[44..48].copy_from_slice(&geo.metadata_slot_count.to_le_bytes());
    b[48..52].copy_from_slice(&geo.logical_block_size.to_le_bytes());
    let cs = sha256_zeroed(&b[..52], 8, 32);
    b[8..40].copy_from_slice(&cs);
    write_at(f, GEOMETRY_SIZE, &b)?;
    write_at(f, GEOMETRY_SIZE * 2, &b)?;
    Ok(())
}

pub fn serialize_metadata(meta: &LpMetadata) -> Result<Vec<u8>> {
    let header_size: usize = if meta.minor >= 2 { 256 } else { 128 };
    let mut parts = Vec::new();
    let mut extents = Vec::new();
    let mut groups = Vec::new();
    let mut devices = Vec::new();

    for p in &meta.partitions {
        let mut e = vec![0u8; PART_ENTRY_SIZE];
        put_cstr(&mut e[0..36], &p.name);
        e[36..40].copy_from_slice(&p.attributes.to_le_bytes());
        e[40..44].copy_from_slice(&p.first_extent_index.to_le_bytes());
        e[44..48].copy_from_slice(&p.num_extents.to_le_bytes());
        e[48..52].copy_from_slice(&p.group_index.to_le_bytes());
        parts.extend_from_slice(&e);
    }
    for p in &meta.extents {
        let mut e = vec![0u8; EXTENT_ENTRY_SIZE];
        e[0..8].copy_from_slice(&p.num_sectors.to_le_bytes());
        e[8..12].copy_from_slice(&p.target_type.to_le_bytes());
        e[12..20].copy_from_slice(&p.target_data.to_le_bytes());
        e[20..24].copy_from_slice(&p.target_source.to_le_bytes());
        extents.extend_from_slice(&e);
    }
    for g in &meta.groups {
        let mut e = vec![0u8; GROUP_ENTRY_SIZE];
        put_cstr(&mut e[0..36], &g.name);
        e[36..40].copy_from_slice(&g.flags.to_le_bytes());
        e[40..48].copy_from_slice(&g.maximum_size.to_le_bytes());
        groups.extend_from_slice(&e);
    }
    for d in &meta.block_devices {
        let mut e = vec![0u8; DEVICE_ENTRY_SIZE];
        e[0..8].copy_from_slice(&d.first_logical_sector.to_le_bytes());
        e[8..12].copy_from_slice(&d.alignment.to_le_bytes());
        e[12..16].copy_from_slice(&d.alignment_offset.to_le_bytes());
        e[16..24].copy_from_slice(&d.size.to_le_bytes());
        put_cstr(&mut e[24..60], &d.partition_name);
        e[60..64].copy_from_slice(&d.flags.to_le_bytes());
        devices.extend_from_slice(&e);
    }

    let mut tables = Vec::new();
    let poff = 0u32;
    tables.extend_from_slice(&parts);
    let eoff = tables.len() as u32;
    tables.extend_from_slice(&extents);
    let goff = tables.len() as u32;
    tables.extend_from_slice(&groups);
    let boff = tables.len() as u32;
    tables.extend_from_slice(&devices);

    let mut hdr = vec![0u8; header_size];
    hdr[0..4].copy_from_slice(&HEADER_MAGIC.to_le_bytes());
    hdr[4..6].copy_from_slice(&meta.major.to_le_bytes());
    hdr[6..8].copy_from_slice(&meta.minor.to_le_bytes());
    hdr[8..12].copy_from_slice(&(header_size as u32).to_le_bytes());
    hdr[44..48].copy_from_slice(&(tables.len() as u32).to_le_bytes());
    let tcs = sha256(&tables);
    hdr[48..80].copy_from_slice(&tcs);
    let mut put_desc = |o: usize, off: u32, num: u32, size: u32| {
        hdr[o..o + 4].copy_from_slice(&off.to_le_bytes());
        hdr[o + 4..o + 8].copy_from_slice(&num.to_le_bytes());
        hdr[o + 8..o + 12].copy_from_slice(&size.to_le_bytes());
    };
    put_desc(80, poff, (parts.len() / PART_ENTRY_SIZE) as u32, PART_ENTRY_SIZE as u32);
    put_desc(92, eoff, (extents.len() / EXTENT_ENTRY_SIZE) as u32, EXTENT_ENTRY_SIZE as u32);
    put_desc(104, goff, (groups.len() / GROUP_ENTRY_SIZE) as u32, GROUP_ENTRY_SIZE as u32);
    put_desc(116, boff, (devices.len() / DEVICE_ENTRY_SIZE) as u32, DEVICE_ENTRY_SIZE as u32);
    if header_size >= 132 {
        hdr[128..132].copy_from_slice(&meta.header_flags.to_le_bytes());
    }
    let hcs = sha256_zeroed(&hdr, 12, 32);
    hdr[12..44].copy_from_slice(&hcs);

    let mut out = hdr;
    out.extend_from_slice(&tables);
    Ok(out)
}

pub fn build(parts: &[BuildPartition], out: &Path, opt: &BuildOptions) -> Result<LpMetadata> {
    for p in parts {
        if crate::sparse::file_is_sparse(&p.image)? {
            bail!(
                "partition {} image {} is sparse; run `aimg sparse to-raw` first",
                p.name,
                p.image.display()
            );
        }
    }
    if parts.is_empty() {
        bail!("no partitions given");
    }
    let lbs = opt.logical_block_size as u64;
    if lbs == 0 || lbs % SECTOR != 0 {
        bail!("logical_block_size must be a multiple of 512");
    }
    let align = opt.device_alignment.max(1) as u64;

    // Metadata region occupies 2 geometry blocks + slots*2 copies.
    // metadata region ends after the two backup slots, which start at 12288
    let meta_end = LP_PARTITION_RESERVED_BYTES
        + GEOMETRY_SIZE * 2
        + opt.metadata_max_size as u64 * opt.metadata_slots as u64 * 2;
    let first_logical = align_up(meta_end, align) / SECTOR;

    let mut cursor = first_logical * SECTOR;
    let mut extents: Vec<Extent> = Vec::new();
    let mut partitions: Vec<Partition> = Vec::new();
    let mut total_needed = cursor;

    let mut group_names: Vec<String> = vec!["default".to_string()];
    for p in parts {
        if !group_names.iter().any(|g| g == &p.group) {
            group_names.push(p.group.clone());
        }
    }

    for p in parts {
        if p.name.len() > 35 {
            bail!("partition name too long: {}", p.name);
        }
        let len = match p.size {
            Some(s) => s,
            None => file_len(&p.image)?,
        };
        let size = align_up(len.max(lbs), lbs);
        cursor = align_up(cursor, align);
        let first_extent_index = extents.len() as u32;
        extents.push(Extent {
            num_sectors: size / SECTOR,
            target_type: TARGET_LINEAR,
            target_data: cursor / SECTOR,
            target_source: 0,
        });
        partitions.push(Partition {
            name: p.name.clone(),
            attributes: p.attributes,
            first_extent_index,
            num_extents: 1,
            group_index: group_names
                .iter()
                .position(|g| g == &p.group)
                .unwrap_or(0) as u32,
        });
        cursor += size;
        total_needed = cursor;
    }

    let super_size = match opt.super_size {
        Some(s) => s,
        None => align_up(total_needed, align),
    };
    if super_size < total_needed {
        bail!(
            "super size {} too small, need at least {}",
            human(super_size),
            human(total_needed)
        );
    }

    let groups: Vec<Group> = group_names
        .iter()
        .map(|n| Group {
            name: n.clone(),
            flags: 0,
            maximum_size: if n == "default" {
                opt.group_max.unwrap_or(0)
            } else {
                opt.group_max.unwrap_or(super_size - first_logical * SECTOR)
            },
        })
        .collect();

    let geo = Geometry {
        metadata_max_size: opt.metadata_max_size,
        metadata_slot_count: opt.metadata_slots,
        logical_block_size: opt.logical_block_size,
    };
    let meta = LpMetadata {
        major: 10,
        minor: opt.minor_version,
        header_flags: 0,
        geometry: geo,
        partitions,
        extents,
        groups,
        block_devices: vec![BlockDevice {
            first_logical_sector: first_logical,
            alignment: opt.device_alignment,
            alignment_offset: 0,
            size: super_size,
            partition_name: opt.block_device_name.clone(),
            flags: 0,
        }],
    };

    let blob = serialize_metadata(&meta)?;
    if blob.len() as u32 > opt.metadata_max_size {
        bail!(
            "metadata ({} bytes) exceeds metadata_max_size ({})",
            blob.len(),
            opt.metadata_max_size
        );
    }

    let mut f = create_sized(out, super_size)?;
    write_geometry(&mut f, &geo)?;
    for slot in 0..opt.metadata_slots {
        let off = metadata_offset(slot, false, &geo, GEOMETRY_SIZE);
        write_at(&mut f, off, &blob)?;
        let boff = metadata_offset(slot, true, &geo, GEOMETRY_SIZE);
        write_at(&mut f, boff, &blob)?;
    }
    // write partition payloads
    for (i, p) in parts.iter().enumerate() {
        let e = &meta.extents[meta.partitions[i].first_extent_index as usize];
        let dst_off = e.target_data * SECTOR;
        let mut src = File::open(&p.image)?;
        let len = src.metadata()?.len();
        f.seek(SeekFrom::Start(dst_off))?;
        copy_range(&mut src, &mut f, 0, len)?;
    }
    f.sync_all()?;
    drop(f);

    if opt.sparse {
        let sp = out.with_extension("simg");
        sparse::raw_to_sparse(out, &sp, opt.sparse_block)?;
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn build_and_unpack_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("system_a.img");
        let b = d.path().join("vendor_a.img");
        File::create(&a).unwrap().write_all(&vec![0x11u8; 300_000]).unwrap();
        File::create(&b).unwrap().write_all(&vec![0x22u8; 100_000]).unwrap();

        let parts = vec![
            BuildPartition {
                name: "system_a".into(),
                image: a.clone(),
                attributes: ATTR_READONLY,
                group: "main".into(),
                size: None,
            },
            BuildPartition {
                name: "vendor_a".into(),
                image: b.clone(),
                attributes: ATTR_READONLY,
                group: "main".into(),
                size: None,
            },
        ];
        let opt = BuildOptions {
            metadata_max_size: 65536,
            metadata_slots: 2,
            super_size: Some(8 * 1024 * 1024),
            ..Default::default()
        };
        let sup = d.path().join("super.img");
        build(&parts, &sup, &opt).unwrap();

        let (mut f, _t) = open_maybe_sparse(&sup).unwrap();
        let geo = read_geometry(&mut f).unwrap();
        assert_eq!(geo.metadata_slot_count, 2);
        let meta = parse_slot(&mut f, 0).unwrap();
        assert_eq!(meta.partitions.len(), 2);
        assert_eq!(meta.partition_size(&meta.partitions[0]), 300_000u64.div_ceil(4096) * 4096);

        let out = d.path().join("out");
        let files = unpack(&sup, &out, &UnpackOptions {
            slot: Some(0),
            partitions: None,
            keep_suffix: false,
            sparse_out: false,
            sparse_block: 4096,
        })
        .unwrap();
        assert_eq!(files.len(), 2);
        let got_a = std::fs::read(&files[0]).unwrap();
        assert_eq!(&got_a[..300_000], &vec![0x11u8; 300_000][..]);
        let got_b = std::fs::read(&files[1]).unwrap();
        assert_eq!(&got_b[..100_000], &vec![0x22u8; 100_000][..]);
    }

    #[test]
    fn sparse_super_input() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("system_a.img");
        File::create(&a).unwrap().write_all(&vec![1u8; 8192]).unwrap();
        let parts = vec![BuildPartition {
            name: "system_a".into(),
            image: a,
            attributes: ATTR_READONLY,
            group: "main".into(),
            size: None,
        }];
        let opt = BuildOptions {
            super_size: Some(4 * 1024 * 1024),
            ..Default::default()
        };
        let sup = d.path().join("super.img");
        build(&parts, &sup, &opt).unwrap();
        let sp = d.path().join("super.simg");
        sparse::raw_to_sparse(&sup, &sp, 4096).unwrap();
        let files = unpack(&sp, &d.path().join("o"), &UnpackOptions {
            slot: Some(0),
            partitions: None,
            keep_suffix: false,
            sparse_out: true,
            sparse_block: 4096,
        })
        .unwrap();
        assert_eq!(files.len(), 1);
        assert!(sparse::file_is_sparse(&files[0]).unwrap());
    }
}
