//! Pure userspace ext2/3/4 reader: no mount, no root, no loop device.

use crate::ext4::disk::*;
use crate::perm::{Entry, FileContexts, Kind, MetaDb};
use crate::util::*;
use anyhow::{bail, Context, Result};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub struct Ext4 {
    f: File,
    pub sb: Superblock,
    pub gdt: Vec<GroupDesc>,
}

impl Ext4 {
    pub fn open(p: &Path) -> Result<Self> {
        let mut f = File::open(p).with_context(|| format!("open {}", p.display()))?;
        let mut sb_buf = vec![0u8; 1024];
        read_at(&mut f, 1024, &mut sb_buf)?;
        let sb = Superblock::parse(&sb_buf)?;
        let gdt_off = (sb.first_data_block as u64 + 1) * sb.block_size as u64;
        let mut gdt = Vec::new();
        for i in 0..sb.group_count() {
            let mut b = vec![0u8; sb.desc_size as usize];
            read_at(
                &mut f,
                gdt_off + i as u64 * sb.desc_size as u64,
                &mut b,
            )?;
            gdt.push(GroupDesc::parse(&b, sb.desc_size));
        }
        Ok(Self { f, sb, gdt })
    }

    pub fn read_block(&mut self, blk: u64) -> Result<Vec<u8>> {
        let bs = self.sb.block_size as u64;
        if blk >= self.sb.blocks_count {
            bail!("block {} out of range", blk);
        }
        let mut b = vec![0u8; bs as usize];
        read_at(&mut self.f, blk * bs, &mut b)?;
        Ok(b)
    }

    fn inode_location(&self, ino: u32) -> Result<(u32, u32, u64, usize)> {
        if ino == 0 || ino > self.sb.inodes_count {
            bail!("inode {} out of range", ino);
        }
        let idx = ino - 1;
        let group = idx / self.sb.inodes_per_group;
        let within = idx % self.sb.inodes_per_group;
        let gd = self
            .gdt
            .get(group as usize)
            .with_context(|| format!("group {} missing", group))?;
        let byte_off = within as u64 * self.sb.inode_size as u64;
        let blk = gd.inode_table + byte_off / self.sb.block_size as u64;
        Ok((group, within, blk, (byte_off % self.sb.block_size as u64) as usize))
    }

    pub fn inode(&mut self, ino: u32) -> Result<Inode> {
        let (_, _, blk, off) = self.inode_location(ino)?;
        let b = self.read_block(blk)?;
        let sz = self.sb.inode_size as usize;
        if off + sz > b.len() {
            bail!("inode {} crosses block boundary", ino);
        }
        Ok(Inode::parse(&b[off..off + sz]))
    }

    /// Logical extents of a file, in logical-block order.
    pub fn extents(&mut self, inode: &Inode) -> Result<Vec<ExtentLeaf>> {
        let mut out = Vec::new();
        if inode.has_extents() {
            let mut root = [0u8; 60];
            for (i, v) in inode.block.iter().enumerate() {
                root[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            self.walk_extent(&root, &mut out)?;
        } else {
            // classic block map: 12 direct + single/double/triple indirect
            let bs = self.sb.block_size as u64;
            let ppb = bs / 4;
            let limit = inode.size.div_ceil(bs);
            let mut logical: u64 = 0;
            for &b in inode.block[..12].iter() {
                if logical >= limit {
                    break;
                }
                if b != 0 {
                    out.push(ExtentLeaf {
                        logical: logical as u32,
                        len: 1,
                        start: b as u64,
                    });
                }
                logical += 1;
            }
            self.walk_indirect(inode.block[12] as u64, 1, logical, limit, &mut out)?;
            logical += ppb;
            self.walk_indirect(inode.block[13] as u64, 2, logical, limit, &mut out)?;
            logical += ppb * ppb;
            self.walk_indirect(inode.block[14] as u64, 3, logical, limit, &mut out)?;
        }
        out.sort_by_key(|e| e.logical);
        Ok(out)
    }

    fn walk_indirect(
        &mut self,
        blk: u64,
        level: u32,
        base: u64,
        limit: u64,
        out: &mut Vec<ExtentLeaf>,
    ) -> Result<()> {
        if blk == 0 {
            return Ok(());
        }
        let bytes = self.read_block(blk)?;
        let ppb = bytes.len() / 4;
        let span = (ppb as u64).saturating_pow(level - 1);
        for i in 0..ppb {
            let logical = base + (i as u64) * span;
            if logical >= limit {
                break;
            }
            let o = i * 4;
            let child =
                u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) as u64;
            if child == 0 {
                continue;
            }
            if level == 1 {
                out.push(ExtentLeaf {
                    logical: logical as u32,
                    len: 1,
                    start: child,
                });
            } else {
                self.walk_indirect(child, level - 1, logical, limit, out)?;
            }
        }
        Ok(())
    }

    fn walk_extent(&mut self, node: &[u8], out: &mut Vec<ExtentLeaf>) -> Result<()> {
        self.walk_extent_at(node, out, 0)
    }

    fn walk_extent_at(
        &mut self,
        node: &[u8],
        out: &mut Vec<ExtentLeaf>,
        depth: u32,
    ) -> Result<()> {
        if depth > 5 {
            bail!("extent tree deeper than 5 levels");
        }
        let n = parse_extent_node(node)?;
        if n.depth == 0 {
            out.extend_from_slice(&n.leaves);
            return Ok(());
        }
        for (_, blk) in n.index {
            let b = self.read_block(blk)?;
            self.walk_extent_at(&b, out, depth + 1)?;
        }
        Ok(())
    }

    pub fn read_data(&mut self, inode: &Inode) -> Result<Vec<u8>> {
        let size = inode.size as usize;
        if inode.is_symlink() && size <= 60 && !inode.has_extents() {
            let mut raw = Vec::new();
            for v in inode.block.iter() {
                raw.extend_from_slice(&v.to_le_bytes());
            }
            raw.truncate(size);
            return Ok(raw);
        }
        let exts = self.extents(inode)?;
        let bs = self.sb.block_size as u64;
        let mut out: Vec<u8> = Vec::with_capacity(size.min(64 << 20));
        let mut buf = vec![0u8; bs as usize];
        let mut next_logical: u64 = 0;
        for e in &exts {
            let logical = e.logical as u64;
            // ee_len > 32768 marks an unwritten (preallocated) extent
            let (len, unwritten) = if e.len > 32768 {
                (e.len as u64 - 32768, true)
            } else {
                (e.len as u64, false)
            };
            // logical holes must be materialised as zeros, otherwise every
            // byte after the hole shifts and the file becomes corrupt
            if logical > next_logical {
                let gap = (logical - next_logical) * bs;
                let n = gap.min((size.saturating_sub(out.len())) as u64) as usize;
                out.resize(out.len() + n, 0);
                if out.len() >= size {
                    break;
                }
            }
            for i in 0..len {
                if out.len() >= size {
                    break;
                }
                let take = (size - out.len()).min(bs as usize);
                if unwritten {
                    out.resize(out.len() + take, 0);
                } else {
                    read_at(&mut self.f, (e.start + i) * bs, &mut buf)?;
                    out.extend_from_slice(&buf[..take]);
                }
            }
            next_logical = logical + len;
        }
        out.resize(size, 0);
        Ok(out)
    }

    /// Stream an inode's bytes to path without buffering the whole file.
    pub fn write_file_data(&mut self, inode: &Inode, path: &Path) -> Result<()> {
        let size = inode.size;
        let bs = self.sb.block_size as u64;
        let exts = self.extents(inode)?;
        let mut out = File::create(path)?;
        let mut buf = vec![0u8; bs as usize];
        let zero = vec![0u8; bs as usize];
        let mut next_logical: u64 = 0;
        let mut written: u64 = 0;
        for e in &exts {
            let logical = e.logical as u64;
            let (len, unwritten) = if e.len > 32768 {
                (e.len as u64 - 32768, true)
            } else {
                (e.len as u64, false)
            };
            if logical > next_logical {
                let mut gap = (logical - next_logical) * bs;
                while gap > 0 && written < size {
                    let n = gap.min(bs).min(size - written) as usize;
                    out.write_all(&zero[..n])?;
                    written += n as u64;
                    gap -= n as u64;
                }
            }
            for i in 0..len {
                if written >= size {
                    break;
                }
                let take = (size - written).min(bs) as usize;
                if unwritten {
                    out.write_all(&zero[..take])?;
                } else {
                    read_at(&mut self.f, (e.start + i) * bs, &mut buf)?;
                    out.write_all(&buf[..take])?;
                }
                written += take as u64;
            }
            next_logical = logical + len;
        }
        if written < size {
            out.set_len(size)?;
        }
        Ok(())
    }

    pub fn read_dir(&mut self, ino: u32) -> Result<Vec<DirEntry>> {
        let inode = self.inode(ino)?;
        if !inode.is_dir() {
            bail!("inode {} is not a directory", ino);
        }
        let data = self.read_data(&inode)?;
        let bs = self.sb.block_size as usize;
        let mut out = Vec::new();
        let mut off = 0;
        while off + bs <= data.len() {
            out.extend(parse_dir_block(&data[off..off + bs]));
            off += bs;
        }
        if off < data.len() {
            out.extend(parse_dir_block(&data[off..]));
        }
        Ok(out)
    }

    pub fn lookup(&mut self, path: &str) -> Result<u32> {
        let mut ino = ROOT_INO;
        for part in path.split('/').filter(|s| !s.is_empty()) {
            let ents = self.read_dir(ino)?;
            match ents.into_iter().find(|e| e.name == part) {
                Some(e) => ino = e.ino,
                None => bail!("path not found: {}", path),
            }
        }
        Ok(ino)
    }

    pub fn xattrs(&mut self, inode: &Inode) -> Result<Vec<Xattr>> {
        let mut out = Vec::new();
        // 1) in-inode (ibody) xattrs: 4-byte magic at 128 + i_extra_isize,
        //    entries immediately after it, values addressed relative to the
        //    entries start (same convention as e2fsprogs read_xattrs_from_buffer).
        let isz = inode.raw.len();
        let extra = inode.extra_isize as usize;
        if isz > 128 + extra + 4 && extra >= 4 && extra % 4 == 0 {
            let moff = 128 + extra;
            let magic = u32::from_le_bytes([
                inode.raw[moff],
                inode.raw[moff + 1],
                inode.raw[moff + 2],
                inode.raw[moff + 3],
            ]);
            if magic == EXT4_XATTR_MAGIC {
                let start = moff + 4;
                let base = &inode.raw[start..];
                let mut off = 0usize;
                while off + 16 <= base.len() && base[off] != 0 {
                    let name_len = base[off] as usize;
                    let index = base[off + 1];
                    let value_offs =
                        u16::from_le_bytes([base[off + 2], base[off + 3]]) as usize;
                    let value_size = u32::from_le_bytes([
                        base[off + 8],
                        base[off + 9],
                        base[off + 10],
                        base[off + 11],
                    ]) as usize;
                    let nend = off + 16 + name_len;
                    if nend > base.len() {
                        break;
                    }
                    let name = format!(
                        "{}{}",
                        xattr_prefix(index),
                        String::from_utf8_lossy(&base[off + 16..nend])
                    );
                    let value = if value_offs + value_size <= base.len() {
                        base[value_offs..value_offs + value_size].to_vec()
                    } else {
                        Vec::new()
                    };
                    out.push(Xattr { name, value });
                    off = nend + ((name_len + 3) & !3);
                }
            }
        }
        // 2) external xattr block
        if inode.file_acl != 0 {
            let b = self.read_block(inode.file_acl)?;
            out.extend(parse_xattr_block(&b));
        }
        Ok(out)
    }

    pub fn selinux_context(&mut self, inode: &Inode) -> Result<Option<String>> {
        for x in self.xattrs(inode)? {
            if x.name == "security.selinux" {
                return Ok(Some(cstr(&x.value)));
            }
        }
        Ok(None)
    }

    /// Extract the whole filesystem into `outdir` and build a `MetaDb` that
    /// carries uid/gid/mode/SELinux context (which NTFS cannot store).
    pub fn extract_tree(&mut self, outdir: &Path) -> Result<MetaDb> {
        fs::create_dir_all(outdir)?;
        let mut db = MetaDb::new();
        let mut queue: Vec<(u32, String)> = vec![(ROOT_INO, "/".to_string())];
        while let Some((ino, vpath)) = queue.pop() {
            let inode = self.inode(ino)?;
            let mode = (inode.mode & 0o7777) as u32;
            let ctx = self
                .selinux_context(&inode)?
                .unwrap_or_else(|| "u:object_r:unlabeled:s0".to_string());
            let kind = if inode.is_dir() {
                Kind::Dir
            } else if inode.is_symlink() {
                Kind::Link
            } else {
                Kind::File
            };
            let host = if vpath == "/" {
                outdir.to_path_buf()
            } else {
                outdir.join(vpath.trim_start_matches('/').replace('/', std::path::MAIN_SEPARATOR_STR))
            };
            let mut link = None;
            match kind {
                Kind::Dir => {
                    fs::create_dir_all(&host)?;
                }
                Kind::File => {
                    ensure_parent(&host)?;
                    self.write_file_data(&inode, &host)?;
                }
                Kind::Link => {
                    let target = cstr(&self.read_data(&inode)?);
                    link = Some(target.clone());
                    if let Some(parent) = host.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let is_dir_link = target.ends_with('/')
                        || outdir
                            .join(target.trim_start_matches('/').replace('/', std::path::MAIN_SEPARATOR_STR))
                            .is_dir();
                    match symlink(&target, &host, is_dir_link) {
                        Ok(()) => {}
                        Err(_) => {
                            // NTFS usually denies symlink creation without
                            // developer mode; keep the target in meta.txt.
                            fs::write(&host, target.as_bytes())?;
                        }
                    }
                }
            }
            {
                db.entries.push(Entry {
                    kind,
                    path: vpath.clone(),
                    uid: inode.uid,
                    gid: inode.gid,
                    mode,
                    ctx,
                    caps: 0,
                    size: if inode.is_file() { inode.size } else { 0 },
                    mtime: inode.mtime as i64,
                    link,
                    ino: if !inode.is_dir() && inode.links > 1 { Some(ino) } else { None },
                });
            }
            if inode.is_dir() {
                let mut ents = self.read_dir(ino)?;
                ents.sort_by(|a, b| a.name.cmp(&b.name));
                for e in ents {
                    if e.name == "." || e.name == ".." {
                        continue;
                    }
                    let cname = crate::util::safe_component(&e.name)?;
                    let child = if vpath == "/" {
                        format!("/{}", cname)
                    } else {
                        format!("{}/{}", vpath, cname)
                    };
                    queue.push((e.ino, child));
                }
            }
        }
        db.entries.sort_by(|a, b| a.path.cmp(&b.path));
        db.entries.dedup_by(|a, b| a.path == b.path);
        Ok(db)
    }

    /// Re-resolve metadata from text configs for a freshly extracted tree
    /// (unpack path that prefers policy over what is baked in the image).
    pub fn extract_tree_with_policy(
        &mut self,
        outdir: &Path,
        fc: &FileContexts,
    ) -> Result<MetaDb> {
        let mut db = self.extract_tree(outdir)?;
        for e in db.entries.iter_mut() {
            let kind = e.kind.clone();
            let ctx = fc.lookup(&e.path, &kind);
            if !fc.is_empty() {
                e.ctx = ctx;
            }
        }
        Ok(db)
    }

    pub fn describe(&self) -> String {
        let mut s = format!(
            "ext4: {} blocks of {} bytes ({}), inode size {}, {} inodes, {} groups\n",
            self.sb.blocks_count,
            self.sb.block_size,
            human(self.sb.blocks_count * self.sb.block_size as u64),
            self.sb.inode_size,
            self.sb.inodes_count,
            self.sb.group_count()
        );
        s.push_str(&format!(
            "features: compat={:#x} incompat={:#x} ro_compat={:#x}\n",
            self.sb.feature_compat, self.sb.feature_incompat, self.sb.feature_ro_compat
        ));
        s.push_str(&format!(
            "free: {} blocks, {} inodes\n",
            self.sb.free_blocks, self.sb.free_inodes
        ));
        s
    }

    /// `aimg ext4 ls` style listing.
    pub fn tree(&mut self, max_entries: usize) -> Result<String> {
        let mut out = String::new();
        let mut emitted = 0usize;
        self.tree_rec(ROOT_INO, "/", 0, &mut out, max_entries, &mut emitted)?;
        Ok(out)
    }

    fn tree_rec(
        &mut self,
        ino: u32,
        vpath: &str,
        depth: usize,
        out: &mut String,
        left: usize,
        emitted: &mut usize,
    ) -> Result<()> {
        if *emitted >= left {
            return Ok(());
        }
        let entries = self.read_dir(ino)?;
        let mut names: Vec<DirEntry> = entries
            .into_iter()
            .filter(|e| e.name != "." && e.name != "..")
            .collect();
        names.sort_by(|a, b| a.name.cmp(&b.name));
        for e in names {
            if *emitted >= left {
                break;
            }
            let child = if vpath == "/" {
                format!("/{}", e.name)
            } else {
                format!("{}/{}", vpath, e.name)
            };
            let i = self.inode(e.ino)?;
            let kind = if i.is_dir() {
                "/"
            } else if i.is_symlink() {
                "@"
            } else {
                ""
            };
            out.push_str(&format!(
                "{:indent$}{}{}\n",
                "",
                e.name,
                kind,
                indent = depth * 2
            ));
            *emitted += 1;
            if i.is_dir() {
                self.tree_rec(e.ino, &child, depth + 1, out, left, emitted)?;
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn symlink(target: &str, link: &Path, is_dir: bool) -> std::io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(not(windows))]
fn symlink(target: &str, link: &Path, _is_dir: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Hash helper used by the builder for directory ordering.
pub fn name_hash(name: &str) -> u32 {
    let mut h: u32 = 0;
    for b in name.as_bytes() {
        h = h.wrapping_mul(31).wrapping_add(*b as u32);
    }
    h
}

