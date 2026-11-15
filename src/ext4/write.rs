//! Pure userspace ext4 builder: directory tree + text-config metadata ->
//! a mountable ext4 image, with uid/gid/mode and `security.selinux` xattrs
//! baked in. No mount, no root, no `mke2fs` binary, no NTFS ACLs involved.

use crate::ext4::disk::*;
use crate::perm::{Entry, Kind, MetaDb};
use crate::util::*;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub block_size: u32,
    pub size: u64,
    pub inode_size: u32,
    pub volume_name: String,
    pub uuid: Option<[u8; 16]>,
    pub mtime: u32,
    /// Blocks reserved for root (default 0 so small images stay usable).
    pub reserved_blocks: u64,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            block_size: 4096,
            size: 0,
            inode_size: 256,
            volume_name: "aimg".into(),
            uuid: None,
            mtime: 1_700_000_000,
            reserved_blocks: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct Node {
    path: String,
    name: String,
    kind: Kind,
    entry: Entry,
    parent: Option<usize>,
    children: Vec<usize>,
    ino: u32,
    secondary: bool,
    hardlink_count: u16,
}

/// Recursively materialise any missing ancestor directories so a deep path in
/// meta.txt is never flattened onto the root.
fn ensure_dir(
    path: &str,
    nodes: &mut Vec<Node>,
    index: &mut HashMap<String, usize>,
) -> Result<usize> {
    if let Some(&i) = index.get(path) {
        return Ok(i);
    }
    let (pp, nn) = match path.rfind('/') {
        Some(0) => ("/".to_string(), path[1..].to_string()),
        Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
        None => ("/".to_string(), path.to_string()),
    };
    let parent_idx = ensure_dir(&pp, nodes, index)?;
    let entry = crate::perm::default_entry(path, Kind::Dir, 0);
    let idx = nodes.len();
    nodes.push(Node {
        path: path.to_string(),
        name: nn,
        kind: Kind::Dir,
        entry,
        parent: Some(parent_idx),
        children: Vec::new(),
        ino: 0,        secondary: false,        hardlink_count: 0,
    });
    nodes[parent_idx].children.push(idx);
    index.insert(path.to_string(), idx);
    Ok(idx)
}

pub fn build_tree(root: &Path, meta: &MetaDb, out: &Path, opt: &BuildOptions) -> Result<()> {
    let bs = opt.block_size;
    if bs < 1024 || !bs.is_power_of_two() {
        bail!("block size must be a power of two >= 1024");
    }
    let inode_size = opt.inode_size.max(128).next_multiple_of(16);
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    // synthetic root
    let root_entry = meta.get("/").cloned().unwrap_or_else(|| {
        crate::perm::default_entry("/", Kind::Dir, 0)
    });
    nodes.push(Node {
        secondary: false,
        hardlink_count: 0,
        path: "/".into(),
        name: String::new(),
        kind: Kind::Dir,
        entry: root_entry,
        parent: None,
        children: Vec::new(),
        ino: ROOT_INO,
    });
    index.insert("/".into(), 0);

    let mut sorted: Vec<&Entry> = meta.entries.iter().filter(|e| e.path != "/").collect();
    sorted.sort_by_key(|e| (e.path.matches('/').count(), e.path.clone()));

    for e in sorted {
        let path = if e.path.starts_with('/') {
            e.path.clone()
        } else {
            format!("/{}", e.path)
        };
        crate::util::safe_image_path(&path)?;
        let (parent_path, name) = match path.rfind('/') {
            Some(0) => ("/".to_string(), path[1..].to_string()),
            Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
            None => continue,
        };
        if name.is_empty() {
            continue;
        }
        let parent_idx = ensure_dir(&parent_path, &mut nodes, &mut index)?;
        if let Some(existing) = index.get(&path) {
            if !nodes[*existing].children.is_empty() && e.kind != Kind::Dir {
                bail!("meta.txt makes {} a {:?} but it already has children", path, e.kind);
            }
            // metadata wins over implicit dir
            nodes[*existing].entry = e.clone();
            nodes[*existing].kind = e.kind.clone();
            continue;
        }
        let idx = nodes.len();
        nodes.push(Node {
            path: path.clone(),
            name,
            kind: e.kind.clone(),
            entry: e.clone(),
            parent: Some(parent_idx),
            children: Vec::new(),
            ino: 0,            secondary: false,            hardlink_count: 0,
        });
        nodes[parent_idx].children.push(idx);
        index.insert(path, idx);
    }

    // ---- geometry ----
    let data_bytes: u64 = nodes
        .iter()
        .filter(|n| n.kind == Kind::File)
        .map(|n| n.entry.size)
        .sum();
    let mut size = if opt.size > 0 {
        opt.size
    } else {
        let inode_bytes = (nodes.len() as u64 + 32) * inode_size as u64 * 2;
        // A group only carries 8192 (or 2048) inodes, so the image must be
        // large enough for ceil(nodes/ipg) groups or the table runs out.
        let ipg_guess: u32 = if bs >= 4096 { 8192 } else { 2048 };
        let bpg_guess: u64 =
            ((8 * bs as u64).min((u32::MAX as u64 / bs as u64) & !7u64)).max(8);
        let groups_needed = ((nodes.len() as u64).div_ceil(ipg_guess as u64)).max(1);
        let inode_min = groups_needed * bpg_guess * bs as u64;
        let raw = (data_bytes + data_bytes / 4 + inode_bytes + 32 * 1024 * 1024)
            .max(inode_min);
        align_up(raw.max(16 * 1024 * 1024), 1024 * 1024)
    };
    size = align_up(size, bs as u64);
    let blocks = size / bs as u64;
    let first_data_block: u32 = if bs == 1024 { 1 } else { 0 };
    // A block group must stay below 4 GiB: 8*bs blocks can exceed u32
    // bytes at 64 KiB blocks, which e2fsck rejects outright.
    let bpg: u32 = ((8 * bs as u64).min((u32::MAX as u64 / bs as u64) & !7u64)).max(8) as u32;
    let groups = (((blocks - first_data_block as u64).div_ceil(bpg as u64)).max(1)) as u32;
    // Size the per-group inode table from the real node count: a fixed 8192
    // wastes ~2 MiB per group on small partitions like vendor.
    let needed_inodes = (nodes.len() as u64).saturating_add(16);
    let max_ipg: u64 = if bs >= 4096 { 8192 } else { 2048 };
    let mut ipg: u32 = (needed_inodes
        .div_ceil(groups.max(1) as u64)
        .next_multiple_of(8))
    .clamp(16, max_ipg) as u32;
    let mut itable_blocks = ((ipg as u64 * inode_size as u64).div_ceil(bs as u64)).max(1) as u32;
    let desc_size: u16 = 32;
    let gdt_blocks = ((groups as u64 * desc_size as u64).div_ceil(bs as u64)).max(1) as u32;
    // The last group may not have room for a full inode table when the image
    // size lands just past a group boundary. mke2fs lowers the *global* inode
    // count in that case, which keeps s_inodes_count == groups * ipg.
    let last_g = (groups - 1) as u64;
    let last_start = first_data_block as u64 + last_g * bpg as u64;
    let last_hdr = if sparse_super_group(last_g) { 1 + gdt_blocks as u64 } else { 0 };
    let last_avail = (blocks - last_start).saturating_sub(last_hdr + 2);
    let max_ipg = ((last_avail * bs as u64) / inode_size as u64) as u32;
    if max_ipg < 16 {
        bail!("image too small: last group only has {} blocks", last_avail);
    }
    if max_ipg < ipg {
        ipg = max_ipg;
        itable_blocks = ((ipg as u64 * inode_size as u64) / bs as u64).max(1) as u32;
    }
    let total_inodes: u32 = ipg * groups;
    let g0_itable = itable_blocks as u64;

    // ---- layout ----
    let mut used = vec![false; blocks as usize];
    fn mark_block(used: &mut [bool], b: u64) -> Result<()> {
        if b as usize >= used.len() {
            bail!("layout overflow: block {} >= {}", b, used.len());
        }
        used[b as usize] = true;
        Ok(())
    }
    let mut gdt = vec![GroupDesc::default(); groups as usize];
    // Blocks reserved at the head of every group that carries a superblock
    // copy: one superblock block plus the backup GDT blocks. Bitmaps must
    // start *after* them, otherwise the backup SB/GDT overwrites the bitmaps
    // (this is what e2fsck reports as "bad block for block bitmap").
    let super_blocks = 1 + gdt_blocks as u64;
    let g0_start = first_data_block as u64;
    let g0_bb = g0_start + super_blocks;
    let g0_ib = g0_bb + 1;
    let g0_it = g0_ib + 1;
    for i in 0..super_blocks {
        mark_block(&mut used, g0_start + i)?;
    }
    // group 0 bitmaps must be marked too, otherwise e2fsck reports
    // "Block bitmap differences: +(2--3)"
    mark_block(&mut used, g0_bb)?;
    mark_block(&mut used, g0_ib)?;
    for i in 0..g0_itable {
        mark_block(&mut used, g0_it + i)?;
    }
    gdt[0].block_bitmap = g0_bb;
    gdt[0].inode_bitmap = g0_ib;
    gdt[0].inode_table = g0_it;
    for g in 1..groups as u64 {
        let start = first_data_block as u64 + g * bpg as u64;
        let mut bb = start;
        if sparse_super_group(g) {
            for i in 0..super_blocks {
                mark_block(&mut used, start + i)?;
            }
            bb = start + super_blocks;
        }
        let ib = bb + 1;
        let it = ib + 1;
        for b in [bb, ib] {
            mark_block(&mut used, b)?;
        }
        let it_blocks = itable_blocks as u64;
        for i in 0..it_blocks {
            mark_block(&mut used, it + i)?;
        }
        gdt[g as usize].block_bitmap = bb;
        gdt[g as usize].inode_bitmap = ib;
        gdt[g as usize].inode_table = it;
    }

    let mut cursor = g0_it + g0_itable;

    // ---- inode numbers ----
    let mut next_ino: u32 = 11;
    for n in nodes.iter_mut() {
        if n.ino == 0 {
            if next_ino >= total_inodes {
                bail!("inode table exhausted (use a larger --size)");
            }
            n.ino = next_ino;
            next_ino += 1;
        }
    }
    // Hard links: entries that shared an inode in the source must share one here.
    let mut link_groups: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        if let Some(li) = n.entry.ino {
            if n.kind != Kind::Dir {
                link_groups.entry(li).or_default().push(i);
            }
        }
    }
    for idxs in link_groups.values() {
        if idxs.len() < 2 {
            continue;
        }
        let first = idxs[0];
        let ino = nodes[first].ino;
        nodes[first].hardlink_count = idxs.len() as u16;
        for &j in &idxs[1..] {
            nodes[j].ino = ino;
            nodes[j].secondary = true;
        }
    }
    // Renumber so hard-link duplicates do not leave holes in the inode bitmap.
    let mut next_ino: u32 = 11;
    for i in 0..nodes.len() {
        if nodes[i].ino == ROOT_INO || nodes[i].secondary {
            continue;
        }
        nodes[i].ino = next_ino;
        next_ino += 1;
    }
    for idxs in link_groups.values() {
        let ino = nodes[idxs[0]].ino;
        for &j in idxs {
            nodes[j].ino = ino;
        }
    }
    let inodes_used_total = next_ino - 1;

    // ---- content ----
    // Created up front so file data can be streamed straight to its blocks
    // instead of buffering whole files in memory.
    let mut f = create_sized(out, size)?;
    let mut inode_blobs: HashMap<u32, Vec<u8>> = HashMap::new();

    // children lookup (name -> ino) per dir
    let mut dir_entries_map: HashMap<usize, Vec<(String, u32, u8)>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        for &c in &n.children {
            let cn = &nodes[c];
            let ft = match cn.kind {
                Kind::Dir => FT_DIR,
                Kind::File => FT_REG_FILE,
                Kind::Link => FT_SYMLINK,
            };
            dir_entries_map
                .entry(i)
                .or_default()
                .push((cn.name.clone(), cn.ino, ft));
        }
    }

    for (i, n) in nodes.iter().enumerate() {
        if n.secondary {
            continue;
        }
        let e = &n.entry;
        let mut inode = Inode {
            mode: match n.kind {
                Kind::Dir => S_IFDIR | (e.mode as u16 & 0o7777),
                Kind::File => S_IFREG | (e.mode as u16 & 0o7777),
                Kind::Link => S_IFLNK | 0o777,
            },
            uid: e.uid,
            gid: e.gid,
            links: match n.kind {
                Kind::Dir => 2 + n.children.iter().filter(|&&c| nodes[c].kind == Kind::Dir).count() as u16,
                _ => n.hardlink_count.max(1),
            },
            mtime: (if e.mtime > 0 { e.mtime } else { opt.mtime as i64 }) as u32,
            ctime: e.mtime.max(opt.mtime as i64) as u32,
            atime: e.mtime.max(opt.mtime as i64) as u32,
            ..Default::default()
        };

        match n.kind {
            Kind::File => {
                let host = root.join(n.path.trim_start_matches('/').replace('/', std::path::MAIN_SEPARATOR_STR));
                let mut src = File::open(&host).with_context(|| format!("open {}", host.display()))?;
                let flen = src.metadata()?.len();
                inode.size = flen;
                let nblk = flen.div_ceil(bs as u64);
                let mut exts: Vec<ExtentLeaf> = Vec::new();
                let mut logical: u32 = 0;
                let mut left = nblk;
                let mut block = vec![0u8; bs as usize];
                let mut run: Vec<u8> = Vec::new();
                let mut run_logical: u32 = 0;
                while left > 0 {
                    let off = logical as u64 * bs as u64;
                    let n = (flen - off).min(bs as u64) as usize;
                    src.seek(SeekFrom::Start(off))?;
                    src.read_exact(&mut block[..n])?;
                    if n < block.len() {
                        for b in block[n..].iter_mut() {
                            *b = 0;
                        }
                    }
                    if block.iter().all(|x| *x == 0) {
                        // keep a hole instead of storing zeros, so images whose
                        // source files are sparse still fit their partition
                        if !run.is_empty() {
                            flush_run(&mut f, &mut run, &mut run_logical, &mut used, &mut cursor, &mut exts, bs)?;
                        }
                        logical += 1;
                        left -= 1;
                        continue;
                    }
                    if run.is_empty() {
                        run_logical = logical;
                    }
                    run.extend_from_slice(&block);
                    logical += 1;
                    left -= 1;
                    if run.len() >= (8 << 20) {
                        flush_run(&mut f, &mut run, &mut run_logical, &mut used, &mut cursor, &mut exts, bs)?;
                    }
                }
                if !run.is_empty() {
                    flush_run(&mut f, &mut run, &mut run_logical, &mut used, &mut cursor, &mut exts, bs)?;
                }
                inode.blocks = exts.iter().map(|e| e.len as u64).sum::<u64>() * (bs as u64 / 512);
                inode.flags = EXT4_EXTENTS_FL;
                let (iblock, nodes_) =
                    build_extent_tree(&exts, bs, &mut || alloc_contig(1, &mut used, &mut cursor))?;
                inode.block = iblock;
                inode.blocks += nodes_.len() as u64 * (bs as u64 / 512);
                for (blk, bytes) in nodes_ {
                    write_at(&mut f, blk * bs as u64, &bytes)?;
                }
            }
            Kind::Dir => {
                let mut ents: Vec<(u32, String, u8)> = vec![
                    (n.ino, ".".to_string(), FT_DIR),
                    (
                        n.parent.map(|p| nodes[p].ino).unwrap_or(n.ino),
                        "..".to_string(),
                        FT_DIR,
                    ),
                ];
                let mut kids = dir_entries_map.get(&i).cloned().unwrap_or_default();
                kids.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, ino, ft) in kids {
                    ents.push((ino, name, ft));
                }
                let blocks_needed = pack_dir_blocks(&ents, bs as usize)?.len() as u64;
                let packed = pack_dir_blocks(&ents, bs as usize)?;
                let mut exts: Vec<ExtentLeaf> = Vec::new();
                let mut logical = 0u32;
                let mut remaining = blocks_needed.max(1) as u64;
                let mut idx = 0usize;
                while remaining > 0 {
                    let (start, got) = alloc_run(remaining.min(8192), &mut used, &mut cursor)?;
                    for k in 0..got {
                        if let Some(blk) = packed.get(idx) {
                            write_at(&mut f, (start + k) * bs as u64, &blk)?;
                        }
                        idx += 1;
                    }
                    exts.push(ExtentLeaf {
                        logical,
                        len: got as u16,
                        start,
                    });
                    logical += got as u32;
                    remaining -= got;
                }
                inode.size = blocks_needed.max(1) * bs as u64;
                inode.blocks = blocks_needed.max(1) * (bs as u64 / 512);
                inode.flags = EXT4_EXTENTS_FL;
                let (iblock, nodes_) =
                    build_extent_tree(&exts, bs, &mut || alloc_contig(1, &mut used, &mut cursor))?;
                inode.block = iblock;
                inode.blocks += nodes_.len() as u64 * (bs as u64 / 512);
                for (blk, bytes) in nodes_ {
                    write_at(&mut f, blk * bs as u64, &bytes)?;
                }
            }
            Kind::Link => {
                let target = e.link.clone().unwrap_or_default();
                let tb = target.as_bytes();
                if tb.len() <= 60 {
                    let mut raw = [0u8; 60];
                    raw[..tb.len()].copy_from_slice(tb);
                    for k in 0..15 {
                        inode.block[k] = u32::from_le_bytes([
                            raw[k * 4],
                            raw[k * 4 + 1],
                            raw[k * 4 + 2],
                            raw[k * 4 + 3],
                        ]);
                    }
                    inode.size = tb.len() as u64;
                    inode.blocks = 0;
                    inode.flags = 0;
                } else {
                    let nblk = tb.len().div_ceil(bs as usize) as u64;
                    let start = alloc_contig(nblk, &mut used, &mut cursor)?;
                    for k in 0..nblk {
                        let begin = k as usize * bs as usize;
                        let end = (begin + bs as usize).min(tb.len());
                        write_at(&mut f, (start + k) * bs as u64, &tb[begin..end])?;
                    }
                    inode.size = tb.len() as u64;
                    inode.blocks = nblk * (bs as u64 / 512);
                    inode.flags = EXT4_EXTENTS_FL;
                    let exts = [ExtentLeaf {
                        logical: 0,
                        len: nblk as u16,
                        start,
                    }];
                    let (iblock, nodes_) =
                        build_extent_tree(&exts, bs, &mut || alloc_contig(1, &mut used, &mut cursor))?;
                    inode.block = iblock;
                    inode.blocks += nodes_.len() as u64 * (bs as u64 / 512);
                    for (blk, bytes) in nodes_ {
                        write_at(&mut f, blk * bs as u64, &bytes)?;
                    }
                }
            }
        }

        // SELinux context + file capabilities share one external xattr block
        let mut xattrs: Vec<Xattr> = Vec::new();
        if !e.ctx.is_empty() {
            xattrs.push(Xattr {
                name: "security.selinux".into(),
                value: format!("{}\0", e.ctx).into_bytes(),
            });
        }
        if e.caps != 0 {
            let mut v = Vec::with_capacity(20);
            v.extend_from_slice(&0x0200_0001u32.to_le_bytes());
            v.extend_from_slice(&(e.caps as u32).to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(&((e.caps >> 32) as u32).to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            xattrs.push(Xattr {
                name: "security.capability".into(),
                value: v,
            });
        }
        let mut blob = inode.serialize(inode_size as usize);
        if !xattrs.is_empty() {
            // Prefer the inode body: saves one 4 KiB block per inode and keeps
            // small partitions (vendor) inside their original size.
            let moff = 128 + 32; // i_extra_isize written by serialize()
            let mut inline_ok = false;
            if moff + 4 + 16 + 8 <= blob.len() {
                let mut area = blob[moff..].to_vec();
                if encode_xattr_inode(&mut area, &xattrs)? {
                    blob[moff..].copy_from_slice(&area);
                    inline_ok = true;
                }
            }
            if !inline_ok {
                let blk = alloc_contig(1, &mut used, &mut cursor)?;
                let bytes = encode_xattr_entries(&xattrs, bs as usize)?;
                write_at(&mut f, blk * bs as u64, &bytes)?;
                inode.file_acl = blk;
                // the xattr block counts towards i_blocks (512-byte sectors)
                inode.blocks += bs as u64 / 512;
                blob = inode.serialize(inode_size as usize);
            }
        }
        inode_blobs.insert(n.ino, blob);
    }

    // ---- write payload ----

    // ---- inode tables ----
    for (&ino, blob) in &inode_blobs {
        let idx = ino - 1;
        let group = idx / ipg;
        let within = idx % ipg;
        let gd = &gdt[group as usize];
        let off = gd.inode_table * bs as u64 + within as u64 * inode_size as u64;
        write_at(&mut f, off, blob)?;
    }

    // ---- bitmaps + group descriptors ----
    let mut total_free_blocks = 0u64;
    let mut total_free_inodes = 0u32;
    for g in 0..groups {
        let start = first_data_block as u64 + g as u64 * bpg as u64;
        let end = (start + bpg as u64).min(blocks);
        let mut bb = vec![0u8; bs as usize];
        let mut free = 0u32;
        for b in start..end {
            let rel = (b - start) as usize;
            if rel / 8 < bb.len() && used[b as usize] {
                bb[rel / 8] |= 1 << (rel % 8);
            }
            if !used[b as usize] {
                free += 1;
            }
        }
        // bits past the group end must be set
        for rel in (end - start) as usize..(bs as usize * 8) {
            if rel / 8 < bb.len() {
                bb[rel / 8] |= 1 << (rel % 8);
            }
        }
        let mut ib = vec![0u8; bs as usize];
        let mut ifree = 0u32;
        let gipg = ipg;
        for k in 0..gipg {
            let ino = g * ipg + k + 1;
            let allocated = ino <= inodes_used_total;
            if allocated {
                ib[k as usize / 8] |= 1 << (k % 8);
            } else {
                ifree += 1;
            }
        }
        // bits past the last inode of the group must be set
        for k in gipg..(bs as usize * 8) as u32 {
            ib[k as usize / 8] |= 1 << (k % 8);
        }
        gdt[g as usize].free_blocks = free.min(0xFFFF) as u16;
        gdt[g as usize].free_inodes = ifree.min(0xFFFF) as u16;
        gdt[g as usize].used_dirs = nodes
            .iter()
            .filter(|n| n.kind == Kind::Dir && ino_group(n.ino, ipg) == g)
            .count() as u16;
        // No GDT_CSUM feature: bg_itable_unused must stay 0, otherwise e2fsck
        // reports "group descriptor N marked uninitialized without feature set".
        gdt[g as usize].itable_unused = 0;
        write_at(&mut f, gdt[g as usize].block_bitmap * bs as u64, &bb)?;
        write_at(&mut f, gdt[g as usize].inode_bitmap * bs as u64, &ib)?;
        total_free_blocks += free as u64;
        total_free_inodes += ifree;
    }

    // ---- superblock ----
    let sb = Superblock {
        inodes_count: total_inodes,
        blocks_count: blocks,
        free_blocks: total_free_blocks,
        reserved_blocks: opt.reserved_blocks,
        free_inodes: total_free_inodes,
        first_data_block,
        block_size: bs,
        blocks_per_group: bpg,
        inodes_per_group: ipg,
        inode_size: inode_size as u16,
        desc_size,
        rev_level: 1,
        first_ino: 11,
        feature_compat: COMPAT_EXT_ATTR,
        feature_incompat: INCOMPAT_FILETYPE | INCOMPAT_EXTENTS,
        feature_ro_compat: RO_COMPAT_SPARSE_SUPER | RO_COMPAT_LARGE_FILE,
        uuid: opt.uuid.unwrap_or_else(|| {
            let mut h = Sha256::new();
            h.update(opt.volume_name.as_bytes());
            h.update(size.to_le_bytes());
            let d = h.finalize();
            let mut u = [0u8; 16];
            u.copy_from_slice(&d[..16]);
            u
        }),
        volume_name: opt.volume_name.clone(),
        mtime: opt.mtime,
        wtime: opt.mtime,
        lastcheck: opt.mtime,
    };
    let sb_bytes = sb.serialize();
    write_at(&mut f, 1024, &sb_bytes)?;

    // backup superblocks + GDT for sparse_super groups
    for g in 1..groups {
        if !sparse_super_group(g as u64) {
            continue;
        }
        let start = first_data_block as u64 + g as u64 * bpg as u64;
        if start + super_blocks > blocks {
            continue;
        }
        // 1 KiB filesystems keep the superblock at the group's first block;
        // larger block sizes keep it at byte offset 1024 inside that block.
        let sb_off = start * bs as u64 + if bs == 1024 { 0 } else { 1024 };
        write_at(&mut f, sb_off, &sb_bytes)?;
    }

    // group descriptors (primary + backups)
    let mut gdt_bytes = Vec::new();
    for g in &gdt {
        gdt_bytes.extend_from_slice(&g.serialize(desc_size));
    }
    let gdt_off = (first_data_block as u64 + 1) * bs as u64;
    write_at(&mut f, gdt_off, &gdt_bytes)?;
    for g in 1..groups {
        if !sparse_super_group(g as u64) {
            continue;
        }
        let start = first_data_block as u64 + g as u64 * bpg as u64;
        let off = start * bs as u64 + bs as u64;
        if off + gdt_bytes.len() as u64 <= size {
            write_at(&mut f, off, &gdt_bytes)?;
        }
    }

    f.sync_all()?;
    Ok(())
}

fn ino_group(ino: u32, ipg: u32) -> u32 {
    if ino == 0 {
        0
    } else {
        (ino - 1) / ipg
    }
}

fn alloc_run(mut want: u64, used: &mut Vec<bool>, cursor: &mut u64) -> Result<(u64, u64)> {
    loop {
        if let Ok(start) = alloc_contig(want, used, cursor) {
            return Ok((start, want));
        }
        if want <= 1 {
            bail!("no space left for file data (image too small, pass --size)");
        }
        want = (want / 2).max(1);
    }
}

fn flush_run(
    f: &mut File,
    data: &mut Vec<u8>,
    logical: &mut u32,
    used: &mut Vec<bool>,
    cursor: &mut u64,
    exts: &mut Vec<ExtentLeaf>,
    bs: u32,
) -> Result<()> {
    let mut off = 0usize;
    while off < data.len() {
        let blocks = ((data.len() - off) / bs as usize) as u64;
        let (start, got) = alloc_run(blocks.min(8192), used, cursor)?;
        let take = got as usize * bs as usize;
        write_at(f, start * bs as u64, &data[off..off + take])?;
        exts.push(ExtentLeaf {
            logical: *logical,
            len: got as u16,
            start,
        });
        *logical += got as u32;
        off += take;
    }
    data.clear();
    Ok(())
}

fn alloc_contig(n: u64, used: &mut Vec<bool>, cursor: &mut u64) -> Result<u64> {
    let mut start = *cursor;
    let mut wrapped = false;
    loop {
        let mut run = 0u64;
        while run < n {
            let b = start + run;
            if b >= used.len() as u64 || used[b as usize] {
                start = b + 1;
                run = 0;
                if start >= used.len() as u64 {
                    // reuse gaps left behind the cursor once before giving up
                    if wrapped || *cursor == 0 {
                        bail!(
                            "no space left for {} blocks (image too small, pass --size)",
                            n
                        );
                    }
                    wrapped = true;
                    start = 0;
                    continue;
                }
                continue;
            }
            run += 1;
        }
        for i in 0..n {
            used[(start + i) as usize] = true;
        }
        *cursor = (start + n).max(*cursor);
        return Ok(start);
    }
}

fn sparse_super_group(g: u64) -> bool {
    if g == 0 || g == 1 {
        return true;
    }
    fn is_pow(mut n: u64, base: u64) -> bool {
        if n < base {
            return false;
        }
        while n % base == 0 {
            n /= base;
        }
        n == 1
    }
    is_pow(g, 3) || is_pow(g, 5) || is_pow(g, 7)
}

/// Pack directory entries into as many blocks as needed; the last entry of each
/// block is stretched to the end of that block, as ext4 requires.
fn pack_dir_blocks(entries: &[(u32, String, u8)], bs: usize) -> Result<Vec<Vec<u8>>> {
    let mut blocks = Vec::new();
    let mut cur: Vec<(u32, String, u8)> = Vec::new();
    let mut cur_len = 0usize;
    for e in entries {
        let need = (8 + e.1.len() + 3) & !3;
        if cur_len + need > bs && !cur.is_empty() {
            blocks.push(render_block(&cur, bs)?);
            cur.clear();
            cur_len = 0;
        }
        cur.push(e.clone());
        cur_len += need;
    }
    if !cur.is_empty() {
        blocks.push(render_block(&cur, bs)?);
    }
    if blocks.is_empty() {
        blocks.push(render_block(&[], bs)?);
    }
    Ok(blocks)
}

fn render_block(entries: &[(u32, String, u8)], bs: usize) -> Result<Vec<u8>> {
    let mut b = vec![0u8; bs];
    let mut off = 0usize;
    for (i, (ino, name, ft)) in entries.iter().enumerate() {
        let nb = name.as_bytes();
        if nb.len() > 255 {
            bail!("file name longer than 255 bytes: {:?}", name);
        }
        let need = (8 + nb.len() + 3) & !3;
        if need > bs {
            bail!("directory entry too large for block size {}", bs);
        }
        let last = i + 1 == entries.len();
        let rec = if last { bs - off } else { need };
        b[off..off + 4].copy_from_slice(&ino.to_le_bytes());
        let rec16 = if rec == 65536 { 0u16 } else { rec as u16 };
        b[off + 4..off + 6].copy_from_slice(&rec16.to_le_bytes());
        b[off + 6] = nb.len() as u8;
        b[off + 7] = *ft;
        b[off + 8..off + 8 + nb.len()].copy_from_slice(nb);
        off += rec;
    }
    Ok(b)
}

