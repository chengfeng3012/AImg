//! On-disk ext2/3/4 structures (little endian, pure userspace).

use anyhow::{bail, Result};

pub const EXT4_MAGIC: u16 = 0xEF53;
pub const ROOT_INO: u32 = 2;

pub const INCOMPAT_FILETYPE: u32 = 0x2;
pub const INCOMPAT_EXTENTS: u32 = 0x40;
pub const INCOMPAT_64BIT: u32 = 0x80;
pub const INCOMPAT_FLEX_BG: u32 = 0x200;

pub const RO_COMPAT_SPARSE_SUPER: u32 = 0x1;
pub const RO_COMPAT_LARGE_FILE: u32 = 0x2;

pub const COMPAT_EXT_ATTR: u32 = 0x8;

pub const EXT4_EXTENTS_FL: u32 = 0x0008_0000;
pub const EXT4_INLINE_DATA_FL: u32 = 0x1000_0000;

pub const FT_REG_FILE: u8 = 1;
pub const FT_DIR: u8 = 2;
pub const FT_SYMLINK: u8 = 7;

pub const S_IFREG: u16 = 0o100000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFLNK: u16 = 0o120000;

pub const EXT4_XATTR_MAGIC: u32 = 0xEA02_0000;
pub const XATTR_INDEX_USER: u8 = 1;
pub const XATTR_INDEX_TRUSTED: u8 = 4;
pub const XATTR_INDEX_SECURITY: u8 = 6;
pub const XATTR_INDEX_SYSTEM: u8 = 7;
pub const XATTR_HEADER_LEN: usize = 32;

fn u16at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[derive(Debug, Clone, Default)]
pub struct Superblock {
    pub inodes_count: u32,
    pub blocks_count: u64,
    pub free_blocks: u64,
    pub reserved_blocks: u64,
    pub free_inodes: u32,
    pub first_data_block: u32,
    pub block_size: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub inode_size: u16,
    pub desc_size: u16,
    pub rev_level: u32,
    pub first_ino: u32,
    pub feature_compat: u32,
    pub feature_incompat: u32,
    pub feature_ro_compat: u32,
    pub uuid: [u8; 16],
    pub volume_name: String,
    pub mtime: u32,
    pub wtime: u32,
    pub lastcheck: u32,
}

impl Superblock {
    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < 1024 {
            bail!("superblock truncated");
        }
        if u16at(b, 56) != EXT4_MAGIC {
            bail!("bad ext4 magic 0x{:x}", u16at(b, 56));
        }
        let log_bs = u32at(b, 24);
        if log_bs > 6 {
            bail!("unsupported block size (log {})", log_bs);
        }
        let block_size = 1024u32 << log_bs;
        let blocks_lo = u32at(b, 4) as u64;
        let blocks_hi = u32at(b, 336) as u64;
        let bpg = u32at(b, 32);
        let ipg = u32at(b, 40);
        if bpg == 0 || ipg == 0 {
            bail!(
                "invalid ext4 superblock: blocks_per_group={} inodes_per_group={}",
                bpg,
                ipg
            );
        }
        Ok(Self {
            inodes_count: u32at(b, 0),
            blocks_count: blocks_lo | (blocks_hi << 32),
            free_blocks: u32at(b, 12) as u64 | ((u32at(b, 344) as u64) << 32),
            reserved_blocks: u32at(b, 8) as u64 | ((u32at(b, 340) as u64) << 32),
            free_inodes: u32at(b, 16),
            first_data_block: u32at(b, 20),
            block_size,
            blocks_per_group: bpg,
            inodes_per_group: ipg,
            inode_size: u16at(b, 88),
            desc_size: u16at(b, 254).max(32),
            rev_level: u32at(b, 76),
            first_ino: u32at(b, 84),
            feature_compat: u32at(b, 92),
            feature_incompat: u32at(b, 96),
            feature_ro_compat: u32at(b, 100),
            uuid: b[104..120].try_into().unwrap(),
            volume_name: crate::util::cstr(&b[120..136]),
            mtime: u32at(b, 44),
            wtime: u32at(b, 48),
            lastcheck: u32at(b, 64),
        })
    }

    pub fn block_count(&self) -> u64 {
        self.blocks_count
    }

    pub fn group_count(&self) -> u32 {
        let first = self.first_data_block as u64;
        let n = (self.blocks_count - first).div_ceil(self.blocks_per_group as u64);
        n.max(1) as u32
    }

    pub fn itable_blocks(&self) -> u32 {
        let bytes = self.inodes_per_group as u64 * self.inode_size as u64;
        bytes.div_ceil(self.block_size as u64) as u32
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut b = vec![0u8; 1024];
        put32(&mut b, 0, self.inodes_count);
        put32(&mut b, 4, self.blocks_count as u32);
        put32(&mut b, 8, self.reserved_blocks as u32);
        put32(&mut b, 12, self.free_blocks as u32);
        put32(&mut b, 16, self.free_inodes);
        put32(&mut b, 20, self.first_data_block);
        let log_bs = (self.block_size / 1024).trailing_zeros();
        put32(&mut b, 24, log_bs);
        put32(&mut b, 28, log_bs);
        put32(&mut b, 32, self.blocks_per_group);
        put32(&mut b, 36, self.blocks_per_group);
        put32(&mut b, 40, self.inodes_per_group);
        put32(&mut b, 44, self.mtime);
        put32(&mut b, 48, self.wtime);
        put16(&mut b, 52, 0);
        put16(&mut b, 54, 0xFFFF);
        put16(&mut b, 56, EXT4_MAGIC);
        put16(&mut b, 58, 1); // clean
        put16(&mut b, 60, 1); // continue on error
        put16(&mut b, 62, 0);
        put32(&mut b, 64, self.lastcheck);
        put32(&mut b, 68, 0);
        put32(&mut b, 72, 0); // linux
        put32(&mut b, 76, self.rev_level.max(1));
        put16(&mut b, 80, 0);
        put16(&mut b, 82, 0);
        put32(&mut b, 84, self.first_ino.max(11));
        put16(&mut b, 88, self.inode_size);
        put16(&mut b, 90, 0);
        put32(&mut b, 92, self.feature_compat);
        put32(&mut b, 96, self.feature_incompat);
        put32(&mut b, 100, self.feature_ro_compat);
        b[104..120].copy_from_slice(&self.uuid);
        crate::util::put_cstr(&mut b[120..136], &self.volume_name);
        put32(&mut b, 252, 1); // def_hash_version = half_md4
        put16(&mut b, 254, self.desc_size);
        put32(&mut b, 336, (self.blocks_count >> 32) as u32);
        put32(&mut b, 340, (self.reserved_blocks >> 32) as u32);
        put32(&mut b, 344, (self.free_blocks >> 32) as u32);
        put16(&mut b, 348, 32); // min_extra_isize
        put16(&mut b, 350, 32); // want_extra_isize
        b
    }
}

#[derive(Debug, Clone, Default)]
pub struct GroupDesc {
    pub block_bitmap: u64,
    pub inode_bitmap: u64,
    pub inode_table: u64,
    pub free_blocks: u16,
    pub free_inodes: u16,
    pub used_dirs: u16,
    pub flags: u16,
    pub itable_unused: u16,
}

impl GroupDesc {
    pub fn parse(b: &[u8], desc_size: u16) -> Self {
        let mut d = Self::default();
        d.block_bitmap = u32at(b, 0) as u64;
        d.inode_bitmap = u32at(b, 4) as u64;
        d.inode_table = u32at(b, 8) as u64;
        d.free_blocks = u16at(b, 12);
        d.free_inodes = u16at(b, 14);
        d.used_dirs = u16at(b, 16);
        d.flags = u16at(b, 18);
        d.itable_unused = u16at(b, 28);
        if desc_size >= 64 {
            d.block_bitmap |= (u32at(b, 32) as u64) << 32;
            d.inode_bitmap |= (u32at(b, 36) as u64) << 32;
            d.inode_table |= (u32at(b, 40) as u64) << 32;
        }
        d
    }

    pub fn serialize(&self, desc_size: u16) -> Vec<u8> {
        let mut b = vec![0u8; desc_size as usize];
        put32(&mut b, 0, self.block_bitmap as u32);
        put32(&mut b, 4, self.inode_bitmap as u32);
        put32(&mut b, 8, self.inode_table as u32);
        put16(&mut b, 12, self.free_blocks);
        put16(&mut b, 14, self.free_inodes);
        put16(&mut b, 16, self.used_dirs);
        put16(&mut b, 18, self.flags);
        put16(&mut b, 28, self.itable_unused);
        if desc_size >= 64 {
            put32(&mut b, 32, (self.block_bitmap >> 32) as u32);
            put32(&mut b, 36, (self.inode_bitmap >> 32) as u32);
            put32(&mut b, 40, (self.inode_table >> 32) as u32);
        }
        b
    }
}

#[derive(Debug, Clone)]
pub struct Inode {
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub dtime: u32,
    pub links: u16,
    pub blocks: u64,
    pub flags: u32,
    pub block: [u32; 15],
    pub file_acl: u64,
    pub extra_isize: u16,
    pub generation: u32,
    /// Raw inode bytes (needed to read in-inode xattrs).
    pub raw: Vec<u8>,
}

impl Default for Inode {
    fn default() -> Self {
        Self {
            mode: 0,
            uid: 0,
            gid: 0,
            size: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            links: 0,
            blocks: 0,
            flags: 0,
            block: [0; 15],
            file_acl: 0,
            extra_isize: 32,
            generation: 0,
            raw: Vec::new(),
        }
    }
}

impl Inode {
    pub fn parse(b: &[u8]) -> Self {
        let mut i = Self::default();
        i.mode = u16at(b, 0);
        i.uid = u16at(b, 2) as u32 | ((u16at(b, 120) as u32) << 16);
        i.size = u32at(b, 4) as u64 | ((u32at(b, 108) as u64) << 32);
        i.atime = u32at(b, 8);
        i.ctime = u32at(b, 12);
        i.mtime = u32at(b, 16);
        i.dtime = u32at(b, 20);
        i.gid = u16at(b, 24) as u32 | ((u16at(b, 122) as u32) << 16);
        i.links = u16at(b, 26);
        i.blocks = u32at(b, 28) as u64 | ((u16at(b, 116) as u64) << 32);
        i.flags = u32at(b, 32);
        for k in 0..15 {
            i.block[k] = u32at(b, 40 + k * 4);
        }
        i.generation = u32at(b, 100);
        i.file_acl = u32at(b, 104) as u64 | ((u16at(b, 118) as u64) << 32);
        if b.len() > 130 {
            i.extra_isize = u16at(b, 128);
        }
        i.raw = b.to_vec();
        i
    }

    pub fn serialize(&self, inode_size: usize) -> Vec<u8> {
        let mut b = vec![0u8; inode_size];
        put16(&mut b, 0, self.mode);
        put16(&mut b, 2, self.uid as u16);
        put32(&mut b, 4, self.size as u32);
        put32(&mut b, 8, self.atime);
        put32(&mut b, 12, self.ctime);
        put32(&mut b, 16, self.mtime);
        put32(&mut b, 20, self.dtime);
        put16(&mut b, 24, self.gid as u16);
        put16(&mut b, 26, self.links);
        put32(&mut b, 28, self.blocks as u32);
        put32(&mut b, 32, self.flags);
        for k in 0..15 {
            put32(&mut b, 40 + k * 4, self.block[k]);
        }
        put32(&mut b, 100, self.generation);
        put32(&mut b, 104, self.file_acl as u32);
        put32(&mut b, 108, (self.size >> 32) as u32);
        put16(&mut b, 116, (self.blocks >> 32) as u16);
        put16(&mut b, 118, (self.file_acl >> 32) as u16);
        put16(&mut b, 120, (self.uid >> 16) as u16);
        put16(&mut b, 122, (self.gid >> 16) as u16);
        if inode_size > 128 {
            put16(&mut b, 128, 32);
            put32(&mut b, 144, self.ctime); // crtime
        }
        b
    }

    pub fn is_dir(&self) -> bool {
        self.mode & 0o170000 == S_IFDIR
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & 0o170000 == S_IFLNK
    }
    pub fn is_file(&self) -> bool {
        self.mode & 0o170000 == S_IFREG
    }
    pub fn has_extents(&self) -> bool {
        self.flags & EXT4_EXTENTS_FL != 0
    }
}

// ---------------------------------------------------------------------------
// extents
// ---------------------------------------------------------------------------

pub const EXTENT_MAGIC: u16 = 0xF30A;

#[derive(Debug, Clone, Copy, Default)]
pub struct ExtentLeaf {
    pub logical: u32,
    pub len: u16,
    pub start: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ExtentNode {
    pub depth: u16,
    pub leaves: Vec<ExtentLeaf>,
    /// (logical, physical block of the child node)
    pub index: Vec<(u32, u64)>,
}

pub fn parse_extent_node(b: &[u8]) -> Result<ExtentNode> {
    if b.len() < 12 {
        bail!("extent node too small");
    }
    if u16at(b, 0) != EXTENT_MAGIC {
        bail!("bad extent magic 0x{:x}", u16at(b, 0));
    }
    let entries = u16at(b, 2) as usize;
    let depth = u16at(b, 6);
    let mut n = ExtentNode {
        depth,
        ..Default::default()
    };
    if depth == 0 {
        for i in 0..entries {
            let o = 12 + i * 12;
            if o + 12 > b.len() {
                break;
            }
            let ee_block = u32at(b, o);
            let ee_len = u16at(b, o + 4);
            let ee_start_hi = u16at(b, o + 6) as u64;
            let ee_start_lo = u32at(b, o + 8) as u64;
            n.leaves.push(ExtentLeaf {
                logical: ee_block,
                // raw value: > 32768 means an unwritten extent, decoded by the reader
                len: ee_len,
                start: (ee_start_hi << 32) | ee_start_lo,
            });
        }
    } else {
        for i in 0..entries {
            let o = 12 + i * 12;
            if o + 12 > b.len() {
                break;
            }
            let ei_block = u32at(b, o);
            let leaf_lo = u32at(b, o + 4) as u64;
            let leaf_hi = u16at(b, o + 8) as u64;
            n.index.push((ei_block, (leaf_hi << 32) | leaf_lo));
        }
    }
    Ok(n)
}

/// Build an extent tree for `extents`. Returns the inode-resident root
/// (60 bytes as 15 u32) plus every external block (leaf and index nodes).
pub fn build_extent_tree(
    extents: &[ExtentLeaf],
    block_size: u32,
    alloc_block: &mut dyn FnMut() -> Result<u64>,
) -> Result<([u32; 15], Vec<(u64, Vec<u8>)>)> {
    let mut extra = Vec::new();
    if extents.len() <= 4 {
        let mut b = [0u8; 60];
        write_leaf_node_raw(&mut b, extents, 4);
        return Ok((node_to_iblock(&b), extra));
    }
    let per_node = ((block_size as usize - 12) / 12).max(1);
    // level entries: (first logical block, physical block, depth of that node)
    let mut level: Vec<(u32, u64, u16)> = Vec::new();
    for chunk in extents.chunks(per_node) {
        let blk = alloc_block()?;
        let mut node = vec![0u8; block_size as usize];
        write_leaf_node_raw(&mut node, chunk, per_node as u16);
        extra.push((blk, node));
        level.push((chunk[0].logical, blk, 0));
    }
    let mut child_depth = 0u16;
    // collapse until the root's 4 slots can hold everything (supports depth 2+)
    while level.len() > 4 {
        let mut next = Vec::new();
        for chunk in level.chunks(per_node) {
            let blk = alloc_block()?;
            let mut node = vec![0u8; block_size as usize];
            write_index_node_raw(&mut node, chunk, per_node as u16, child_depth + 1);
            extra.push((blk, node));
            next.push((chunk[0].0, blk, child_depth + 1));
        }
        level = next;
        child_depth += 1;
    }
    let mut root = [0u8; 60];
    put16(&mut root, 0, EXTENT_MAGIC);
    put16(&mut root, 2, level.len() as u16);
    put16(&mut root, 4, 4);
    put16(&mut root, 6, child_depth + 1);
    put32(&mut root, 8, 0);
    for (i, (logical, blk, _)) in level.iter().enumerate() {
        let o = 12 + i * 12;
        put32(&mut root, o, *logical);
        put32(&mut root, o + 4, *blk as u32);
        put16(&mut root, o + 8, (*blk >> 32) as u16);
    }
    Ok((node_to_iblock(&root), extra))
}

fn write_index_node_raw(b: &mut [u8], children: &[(u32, u64, u16)], max: u16, depth: u16) {
    for x in b.iter_mut() {
        *x = 0;
    }
    put16(b, 0, EXTENT_MAGIC);
    put16(b, 2, children.len() as u16);
    put16(b, 4, max);
    put16(b, 6, depth);
    put32(b, 8, 0);
    for (i, (logical, blk, _)) in children.iter().enumerate() {
        let o = 12 + i * 12;
        put32(b, o, *logical);
        put32(b, o + 4, *blk as u32);
        put16(b, o + 8, (*blk >> 32) as u16);
    }
}

fn node_to_iblock(b: &[u8; 60]) -> [u32; 15] {
    let mut out = [0u32; 15];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
    }
    out
}

fn write_leaf_node_raw(b: &mut [u8], extents: &[ExtentLeaf], max: u16) {
    for x in b.iter_mut() {
        *x = 0;
    }
    put16(b, 0, EXTENT_MAGIC);
    put16(b, 2, extents.len() as u16);
    put16(b, 4, max);
    put16(b, 6, 0);
    put32(b, 8, 0);
    for (i, e) in extents.iter().enumerate() {
        let o = 12 + i * 12;
        put32(b, o, e.logical);
        put16(b, o + 4, e.len);
        put16(b, o + 6, (e.start >> 32) as u16);
        put32(b, o + 8, e.start as u32);
    }
}

// ---------------------------------------------------------------------------
// directory entries
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub ino: u32,
    pub name: String,
    pub file_type: u8,
}

/// Parse linear directory entries out of one filesystem block.
pub fn parse_dir_block(b: &[u8]) -> Vec<DirEntry> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 8 <= b.len() {
        let ino = u32at(b, off);
        let rec_len = match u16at(b, off + 4) as usize {
            0 => 65536,
            v => v,
        };
        let name_len = b[off + 6] as usize;
        let file_type = b[off + 7];
        if rec_len < 8 || off + rec_len > b.len() {
            break;
        }
        if ino != 0 && name_len > 0 && name_len <= rec_len - 8 {
            let name = String::from_utf8_lossy(&b[off + 8..off + 8 + name_len]).to_string();
            out.push(DirEntry {
                ino,
                name,
                file_type,
            });
        }
        off += rec_len;
    }
    out
}

/// Serialize directory entries into a block, filling the last rec_len to the
/// end of the block (required by ext4).
pub fn build_dir_block(entries: &[(u32, String, u8)], block_size: usize) -> Result<Vec<u8>> {
    let mut b = vec![0u8; block_size];
    let mut off = 0usize;
    for (idx, (ino, name, ft)) in entries.iter().enumerate() {
        let name_b = name.as_bytes();
        if name_b.len() > 255 {
            bail!("file name longer than 255 bytes: {:?}", name);
        }
        let need = (8 + name_b.len() + 3) & !3;
        let last = idx + 1 == entries.len();
        let rec_len = if last { block_size - off } else { need };
        if rec_len < need || off + rec_len > block_size {
            bail!("directory block overflow");
        }
        put32(&mut b, off, *ino);
        // ext4 stores a 65536-byte record as 0 in the u16 field
        put16(&mut b, off + 4, if rec_len == 65536 { 0 } else { rec_len as u16 });
        b[off + 6] = name_b.len() as u8;
        b[off + 7] = *ft;
        b[off + 8..off + 8 + name_b.len()].copy_from_slice(name_b);
        off += rec_len;
    }
    Ok(b)
}

// ---------------------------------------------------------------------------
// xattrs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Xattr {
    pub name: String,
    pub value: Vec<u8>,
}

/// Entry hash from e2fsprogs `ext2fs_ext_attr_hash_entry`: rotate-xor over the
/// name bytes, then over the value in little-endian 32-bit words (padding
/// included, i.e. `(size + 3) >> 2` words).
fn xattr_entry_hash(name: &[u8], value: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &b in name {
        h = (h << 5) ^ (h >> 27) ^ (b as u32);
    }
    let mut i = 0usize;
    while i < value.len() {
        let mut w = [0u8; 4];
        let n = (value.len() - i).min(4);
        w[..n].copy_from_slice(&value[i..i + n]);
        h = (h << 16) ^ (h >> 16) ^ u32::from_le_bytes(w);
        i += 4;
    }
    h
}

pub fn encode_xattr_entries(xattrs: &[Xattr], block_size: usize) -> Result<Vec<u8>> {
    let mut block = vec![0u8; block_size];
    put32(&mut block, 0, EXT4_XATTR_MAGIC);
    put32(&mut block, 4, 1); // refcount
    put32(&mut block, 8, 1); // blocks
    let mut entry_off = XATTR_HEADER_LEN;
    let mut value_end = block_size;
    let mut block_hash: u32 = 0;
    for x in xattrs {
        let (index, name) = match x.name.split_once('.') {
            Some(("security", n)) => (XATTR_INDEX_SECURITY, n.to_string()),
            Some(("system", n)) => (XATTR_INDEX_SYSTEM, n.to_string()),
            Some(("user", n)) => (XATTR_INDEX_USER, n.to_string()),
            Some(("trusted", n)) => (XATTR_INDEX_TRUSTED, n.to_string()),
            _ => (0u8, x.name.clone()),
        };
        let name_b = name.as_bytes();
        if name_b.len() > 255 {
            bail!("xattr name too long: {:?}", name);
        }
        let vlen = x.value.len();
        // value is stored 4-byte aligned from the end of the block, so the
        // hash must cover the padded words as well
        let padded = (vlen + 3) & !3;
        if padded > value_end || value_end - padded < XATTR_HEADER_LEN {
            bail!("xattr block overflow");
        }
        value_end -= padded;
        if entry_off + 16 + ((name_b.len() + 3) & !3) > value_end {
            bail!("xattr entries collide with values");
        }
        if value_end > u16::MAX as usize {
            bail!("xattr value offset out of range");
        }
        block[value_end..value_end + vlen].copy_from_slice(&x.value);
        let e_hash = xattr_entry_hash(name_b, &block[value_end..value_end + padded]);
        block[entry_off] = name_b.len() as u8;
        block[entry_off + 1] = index;
        put16(&mut block, entry_off + 2, value_end as u16);
        put32(&mut block, entry_off + 4, 0);
        put32(&mut block, entry_off + 8, vlen as u32);
        put32(&mut block, entry_off + 12, e_hash);
        let nstart = entry_off + 16;
        block[nstart..nstart + name_b.len()].copy_from_slice(name_b);
        entry_off = nstart + ((name_b.len() + 3) & !3);
        // fold into the block hash the same way ext4_xattr_rehash() does
        block_hash = (block_hash << 16) ^ (block_hash >> 16) ^ e_hash;
    }
    put32(&mut block, 12, block_hash);
    Ok(block)
}

/// Encode xattrs into the inode body area: magic at byte 0, entries from
/// byte 4, values growing down from the end (offsets relative to byte 4).
/// Returns false when they do not fit.
pub fn encode_xattr_inode(area: &mut [u8], xattrs: &[Xattr]) -> Result<bool> {
    if area.len() < 4 + 16 + 8 {
        return Ok(false);
    }
    let entries_len = area.len() - 4;
    let mut entries = vec![0u8; entries_len];
    let mut entry_off = 0usize;
    let mut value_end = entries_len;
    for x in xattrs {
        let (index, name) = match x.name.split_once('.') {
            Some(("security", n)) => (XATTR_INDEX_SECURITY, n.to_string()),
            Some(("system", n)) => (XATTR_INDEX_SYSTEM, n.to_string()),
            Some(("user", n)) => (XATTR_INDEX_USER, n.to_string()),
            Some(("trusted", n)) => (XATTR_INDEX_TRUSTED, n.to_string()),
            _ => (0u8, x.name.clone()),
        };
        let name_b = name.as_bytes();
        let vlen = x.value.len();
        let padded = (vlen + 3) & !3;
        if padded > value_end {
            return Ok(false);
        }
        value_end -= padded;
        if value_end < entry_off + 16 + ((name_b.len() + 3) & !3) {
            return Ok(false);
        }
        if value_end > u16::MAX as usize {
            return Ok(false);
        }
        entries[value_end..value_end + vlen].copy_from_slice(&x.value);
        let e_hash = xattr_entry_hash(name_b, &entries[value_end..value_end + padded]);
        entries[entry_off] = name_b.len() as u8;
        entries[entry_off + 1] = index;
        put16(&mut entries, entry_off + 2, value_end as u16);
        put32(&mut entries, entry_off + 8, vlen as u32);
        put32(&mut entries, entry_off + 12, e_hash);
        let nstart = entry_off + 16;
        entries[nstart..nstart + name_b.len()].copy_from_slice(name_b);
        entry_off = nstart + ((name_b.len() + 3) & !3);
    }
    put32(area, 0, EXT4_XATTR_MAGIC);
    area[4..].copy_from_slice(&entries);
    Ok(true)
}
/// Name prefix for an ext4 xattr name index (security=6, system=7, ...).
pub fn xattr_prefix(index: u8) -> &'static str {
    match index {
        XATTR_INDEX_SECURITY => "security.",
        XATTR_INDEX_SYSTEM => "system.",
        XATTR_INDEX_TRUSTED => "trusted.",
        XATTR_INDEX_USER => "user.",
        _ => "",
    }
}

pub fn parse_xattr_block(b: &[u8]) -> Vec<Xattr> {
    let mut out = Vec::new();
    if b.len() < XATTR_HEADER_LEN || u32at(b, 0) != EXT4_XATTR_MAGIC {
        return out;
    }
    let mut off = XATTR_HEADER_LEN;
    while off + 16 <= b.len() {
        let name_len = b[off] as usize;
        let index = b[off + 1];
        let value_offs = u16at(b, off + 2) as usize;
        let value_size = u32at(b, off + 8) as usize;
        if name_len == 0 {
            break;
        }
        let prefix = xattr_prefix(index);
        let nend = off + 16 + name_len;
        if nend > b.len() {
            break;
        }
        let name = format!(
            "{}{}",
            prefix,
            String::from_utf8_lossy(&b[off + 16..nend])
        );
        let value = if value_offs + value_size <= b.len() {
            b[value_offs..value_offs + value_size].to_vec()
        } else {
            Vec::new()
        };
        out.push(Xattr { name, value });
        off = nend + ((name_len + 3) & !3);
    }
    out
}

pub fn checksum_u32(_b: &[u8]) -> u32 {
    0
}
