//! `aimg` - pure-Rust Android image toolchain for Windows.
//!
//! Permission model: everything (uid/gid/mode/SELinux context) comes from text
//! files (`fs_config.txt`, `file_contexts`, `meta.txt`), never from NTFS ACLs.

use aimg::boot;
use aimg::dat;
use aimg::ext4::{self, Ext4};
use aimg::payload;
use aimg::perm::{FileContexts, FsConfig, Kind, MetaDb};
use aimg::sparse;
use aimg::superimg;
use aimg::util::parse_size;
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "aimg",
    version,
    about = "Pure-Rust Android image toolchain (sparse/super/boot/payload/dat/ext4)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Android sparse image <-> raw image
    Sparse {
        #[command(subcommand)]
        cmd: SparseCmd,
    },
    /// super.img dynamic partition metadata (lpdump/lpunpack/lpmake equivalent)
    Super {
        #[command(subcommand)]
        cmd: SuperCmd,
    },
    /// boot.img / recovery.img / vendor_boot.img
    Boot {
        #[command(subcommand)]
        cmd: BootCmd,
    },
    /// payload.bin (A/B OTA)
    Payload {
        #[command(subcommand)]
        cmd: PayloadCmd,
    },
    /// old style .dat block images (transfer.list + new.dat / patch.dat)
    Dat {
        #[command(subcommand)]
        cmd: DatCmd,
    },
    /// pure userspace ext4 (read + build), text-config driven permissions
    Ext4 {
        #[command(subcommand)]
        cmd: Ext4Cmd,
    },
    /// text permission config helpers (fs_config / file_contexts / meta.txt)
    Perm {
        #[command(subcommand)]
        cmd: PermCmd,
    },
}

#[derive(Subcommand)]
enum SparseCmd {
    /// sparse -> raw (multiple inputs are concatenated, sparsechunk use case)
    ToRaw {
        #[arg(required = true)]
        input: Vec<PathBuf>,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// raw -> sparse
    FromRaw {
        input: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, default_value = "4096")]
        block_size: u32,
    },
    /// show chunk layout
    Info { input: PathBuf },
}

#[derive(Subcommand)]
enum SuperCmd {
    /// dump LP metadata (lpdump)
    Dump {
        input: PathBuf,
        #[arg(long, default_value = "0")]
        slot: u32,
        #[arg(long)]
        json: bool,
    },
    /// extract logical partitions (lpunpack)
    Unpack {
        input: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, default_value = "0")]
        slot: u32,
        #[arg(long)]
        keep_suffix: bool,
        /// only extract these logical partitions (base names, comma separated)
        #[arg(long)]
        partitions: Option<String>,
        #[arg(long)]
        sparse: bool,
        #[arg(long, default_value = "4096")]
        sparse_block: u32,
    },
    /// build super.img (lpmake)
    Make {
        #[arg(short, long)]
        out: PathBuf,
        /// name=image[:group], repeatable
        #[arg(long = "partition", required = true)]
        partitions: Vec<String>,
        #[arg(long)]
        size: Option<String>,
        #[arg(long, default_value = "65536")]
        metadata_max_size: u32,
        #[arg(long, default_value = "2")]
        metadata_slots: u32,
        #[arg(long, default_value = "1048576")]
        alignment: u32,
        #[arg(long)]
        sparse: bool,
        #[arg(long, default_value = "4096")]
        sparse_block: u32,
        #[arg(long, default_value = "2")]
        minor_version: u16,
    },
}

#[derive(Subcommand)]
enum BootCmd {
    /// unpack to a directory with boot.json + payloads
    Unpack {
        input: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// repack a directory produced by `boot unpack`
    Pack {
        dir: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// print header info
    Info {
        input: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PayloadCmd {
    Info {
        input: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Extract {
        input: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(short, long)]
        partitions: Option<String>,
        /// directory with base images for delta payloads
        #[arg(long)]
        old: Option<PathBuf>,
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        verify: Option<bool>,
    },
    /// build a full payload.bin from images
    Make {
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long = "partition", required = true)]
        partitions: Vec<String>,
        #[arg(long, default_value = "2097152")]
        chunk_size: u64,
        #[arg(long)]
        compress: bool,
        #[arg(long)]
        key: Option<PathBuf>,
        #[arg(long, default_value = "0")]
        max_timestamp: i64,
    },
}

#[derive(Subcommand)]
enum DatCmd {
    /// transfer.list + new.dat -> raw img
    Merge {
        transfer: PathBuf,
        dat: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// raw img -> new.dat + transfer.list + empty patch.dat
    Split {
        img: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, default_value = "system")]
        prefix: String,
        #[arg(long, default_value = "4")]
        version: u32,
    },
    /// apply system.patch.dat onto a base image
    Patch {
        base: PathBuf,
        transfer: PathBuf,
        dat: PathBuf,
        patch: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// decompress a .dat.br
    Br {
        input: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    Info {
        transfer: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum Ext4Cmd {
    Info {
        img: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// list the tree
    Ls {
        img: PathBuf,
        #[arg(long, default_value = "500")]
        limit: usize,
    },
    /// extract to a directory + meta.txt
    Extract {
        img: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        meta: Option<PathBuf>,
        #[arg(long)]
        file_contexts: Option<PathBuf>,
    },
    /// build an image from a directory (meta.txt is generated when absent)
    Build {
        #[arg(long)]
        dir: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        meta: Option<PathBuf>,
        #[arg(long)]
        fs_config: Option<PathBuf>,
        #[arg(long)]
        file_contexts: Option<PathBuf>,
        #[arg(long)]
        size: Option<String>,
        #[arg(long, default_value = "4096")]
        block_size: u32,
        #[arg(long, default_value = "aimg")]
        label: String,
    },
}

#[derive(Subcommand)]
enum PermCmd {
    /// scan a directory and write meta.txt using the text configs
    Export {
        #[arg(long)]
        dir: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        fs_config: Option<PathBuf>,
        #[arg(long)]
        file_contexts: Option<PathBuf>,
    },
    /// apply meta.txt back onto a directory (readonly bits + mtimes only;
    /// uid/gid/SELinux live in the text file, not in NTFS)
    Apply {
        #[arg(long)]
        meta: PathBuf,
        #[arg(long)]
        dir: PathBuf,
    },
    /// print one entry
    Show {
        #[arg(long)]
        meta: PathBuf,
        path: String,
    },
}

/// Expand a sparse ext4 image into a temp raw file before opening it.
fn ext4_input(p: &PathBuf) -> Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    if aimg::sparse::file_is_sparse(p)? {
        let tmp = tempfile::NamedTempFile::new()?;
        aimg::sparse::to_raw(&[p.clone()], tmp.path())?;
        Ok((tmp.path().to_path_buf(), Some(tmp)))
    } else {
        Ok((p.clone(), None))
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }
}

fn read_opt_fsc(p: &Option<PathBuf>) -> Result<FsConfig> {
    match p {
        Some(p) => FsConfig::load(p),
        None => Ok(FsConfig::default()),
    }
}

fn read_opt_fc(p: &Option<PathBuf>) -> Result<FileContexts> {
    match p {
        Some(p) => FileContexts::load(p),
        None => Ok(FileContexts::default()),
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Sparse { cmd } => match cmd {
            SparseCmd::ToRaw { input, out } => {
                let n = sparse::to_raw(&input, &out)?;
                println!("wrote {} ({})", out.display(), aimg::util::human(n));
            }
            SparseCmd::FromRaw {
                input,
                out,
                block_size,
            } => {
                sparse::raw_to_sparse(&input, &out, block_size)?;
                println!("wrote {}", out.display());
            }
            SparseCmd::Info { input } => print!("{}", sparse::describe(&input)?),
        },

        Cmd::Super { cmd } => match cmd {
            SuperCmd::Dump { input, slot, json } => {
                let (mut f, _t) = superimg::open_maybe_sparse(&input)?;
                let meta = superimg::parse_slot(&mut f, slot)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&meta)?);
                } else {
                    print!("{}", meta.dump());
                }
            }
            SuperCmd::Unpack {
                input,
                out,
                slot,
                keep_suffix,
                partitions,
                sparse: sp,
                sparse_block,
            } => {
                let files = superimg::unpack(
                    &input,
                    &out,
                    &superimg::UnpackOptions {
                        slot: Some(slot),
                        partitions: partitions.map(|s| s.split(',').map(|x| x.trim().to_string()).collect()),
                        keep_suffix,
                        sparse_out: sp,
                        sparse_block,
                    },
                )?;
                for f in files {
                    println!("{}", f.display());
                }
            }
            SuperCmd::Make {
                out,
                partitions,
                size,
                metadata_max_size,
                metadata_slots,
                alignment,
                sparse: sp,
                sparse_block,
                minor_version,
            } => {
                let mut parts = Vec::new();
                for spec in partitions {
                    let (name, rest) = spec
                        .split_once('=')
                        .with_context(|| format!("bad --partition '{}', want name=image[:group]", spec))?;
                    // Windows drive letters ("F:\\...") must not be mistaken for a
                    // ":group" suffix: prefer the whole string when it exists, and
                    // otherwise only accept a suffix that cannot be a path.
                    let (image, group) = if std::path::Path::new(rest).exists() {
                        (rest.to_string(), "default".to_string())
                    } else {
                        match rest.rsplit_once(':') {
                            Some((img, g))
                                if !img.is_empty()
                                    && !g.is_empty()
                                    && !g.contains('/')
                                    && !g.contains('\\') =>
                            {
                                (img.to_string(), g.to_string())
                            }
                            _ => (rest.to_string(), "default".to_string()),
                        }
                    };
                    parts.push(superimg::BuildPartition {
                        name: name.to_string(),
                        image: PathBuf::from(image),
                        attributes: superimg::ATTR_READONLY,
                        group,
                        size: None,
                    });
                }
                let opt = superimg::BuildOptions {
                    super_size: size.as_deref().map(parse_size).transpose()?,
                    metadata_max_size,
                    metadata_slots,
                    device_alignment: alignment,
                    minor_version,
                    sparse: sp,
                    sparse_block,
                    ..Default::default()
                };
                let meta = superimg::build(&parts, &out, &opt)?;
                print!("{}", meta.dump());
                if sp {
                    println!(
                        "wrote {} (raw) and {} (sparse)",
                        out.display(),
                        out.with_extension("simg").display()
                    );
                } else {
                    println!("wrote {}", out.display());
                }
            }
        },

        Cmd::Boot { cmd } => match cmd {
            BootCmd::Unpack { input, out } => {
                let d = boot::unpack(&input, &out)?;
                println!(
                    "{}: {} v{} -> {}",
                    input.display(),
                    d.kind, d.header_version,
                    out.display()
                );
            }
            BootCmd::Pack { dir, out } => {
                boot::pack(&dir, &out)?;
                println!("wrote {} ({})", out.display(), aimg::util::human(aimg::util::file_len(&out)?));
            }
            BootCmd::Info { input, json } => {
                if json {
                    let img = boot::BootImage::parse(&std::fs::read(&input)?)?;
                    let v = serde_json::json!({
                        "kind": img.desc.kind,
                        "header_version": img.desc.header_version,
                        "page_size": img.desc.page_size,
                        "os_version": img.desc.os_version,
                        "cmdline": img.desc.cmdline,
                        "kernel_size": img.kernel.len(),
                        "ramdisk_size": img.ramdisk.len(),
                        "second_size": img.second.len(),
                        "recovery_dtbo_size": img.recovery_dtbo.len(),
                        "dtb_size": img.dtb.len(),
                        "signature_size": img.signature.len(),
                        "avb_footer": img.avb_footer,
                    });
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    print!("{}", boot::describe(&input)?);
                }
            }
        },

        Cmd::Payload { cmd } => match cmd {
            PayloadCmd::Info { input, json } => {
                let p = payload::Payload::open(&input)?;
                if json {
                    let v = serde_json::json!({
                        "version": p.version,
                        "manifest_size": p.manifest_bytes.len(),
                        "metadata_signature_size": p.metadata_signature.len(),
                        "block_size": p.manifest.block_size,
                        "minor_version": p.manifest.minor_version,
                        "max_timestamp": p.manifest.max_timestamp,
                        "partial_update": p.manifest.partial_update,
                        "partitions": p.manifest.partitions,
                    });
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    print!("{}", p.info());
                }
            }
            PayloadCmd::Extract {
                input,
                out,
                partitions,
                old,
                verify,
            } => {
                let files = payload::extract(
                    &input,
                    &payload::ExtractOptions {
                        partitions: partitions.map(|s| {
                            s.split(',').map(|x| x.trim().to_string()).collect()
                        }),
                        old_dir: old,
                        out_dir: out,
                        verify: verify.unwrap_or(false),
                    },
                )?;
                for f in files {
                    println!("{}", f.display());
                }
            }
            PayloadCmd::Make {
                out,
                partitions,
                chunk_size,
                compress,
                key,
                max_timestamp,
            } => {
                let mut parts = Vec::new();
                for spec in partitions {
                    let (name, image) = spec
                        .split_once('=')
                        .with_context(|| format!("bad --partition '{}', want name=image", spec))?;
                    parts.push(payload::BuildPartition {
                        name: name.to_string(),
                        image: PathBuf::from(image),
                    });
                }
                payload::build(
                    &parts,
                    &out,
                    &payload::BuildOptions {
                        chunk_size,
                        max_timestamp,
                        compress,
                        sign_key: key,
                    },
                )?;
                println!(
                    "wrote {} ({})",
                    out.display(),
                    aimg::util::human(aimg::util::file_len(&out)?)
                );
            }
        },

        Cmd::Dat { cmd } => match cmd {
            DatCmd::Merge { transfer, dat, out } => {
                let n = dat::merge(&transfer, &dat, &out)?;
                println!("wrote {} ({})", out.display(), aimg::util::human(n));
            }
            DatCmd::Split {
                img,
                out,
                prefix,
                version,
            } => {
                let (blocks, _) = dat::split(&img, version, &prefix, &out)?;
                println!(
                    "{}.new.dat / {}.transfer.list: {} blocks ({})",
                    prefix,
                    prefix,
                    blocks,
                    aimg::util::human(blocks * dat::BLOCK_SIZE)
                );
            }
            DatCmd::Patch {
                base,
                transfer,
                dat,
                patch,
                out,
            } => {
                dat::patch(&base, &transfer, &dat, &patch, &out)?;
                println!("wrote {}", out.display());
            }
            DatCmd::Br { input, out } => {
                let n = dat::brotli_decompress(&input, &out)?;
                println!("wrote {} ({})", out.display(), aimg::util::human(n));
            }
            DatCmd::Info { transfer, json } => {
                if json {
                    let tl = dat::load_transfer_list(&transfer)?;
                    println!("{}", serde_json::to_string_pretty(&tl)?);
                } else {
                    print!("{}", dat::describe(&transfer)?);
                }
            }
        },

        Cmd::Ext4 { cmd } => match cmd {
            Ext4Cmd::Info { img, json } => {
                let (path, _tmp) = ext4_input(&img)?;
                let f = Ext4::open(&path)?;
                if json {
                    let sb = &f.sb;
                    let v = serde_json::json!({
                        "block_size": sb.block_size,
                        "blocks_count": sb.blocks_count,
                        "inodes_count": sb.inodes_count,
                        "inode_size": sb.inode_size,
                        "first_data_block": sb.first_data_block,
                        "blocks_per_group": sb.blocks_per_group,
                        "inodes_per_group": sb.inodes_per_group,
                        "free_blocks": sb.free_blocks,
                        "free_inodes": sb.free_inodes,
                        "group_count": sb.group_count(),
                        "feature_compat": sb.feature_compat,
                        "feature_incompat": sb.feature_incompat,
                        "feature_ro_compat": sb.feature_ro_compat,
                        "volume_name": sb.volume_name,
                        "uuid": hex::encode(sb.uuid),
                    });
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    print!("{}", f.describe());
                }
            }
            Ext4Cmd::Ls { img, limit } => {
                let (path, _tmp) = ext4_input(&img)?;
                let mut f = Ext4::open(&path)?;
                print!("{}", f.tree(limit)?);
            }
            Ext4Cmd::Extract {
                img,
                out,
                meta,
                file_contexts,
            } => {
                let (path, _tmp) = ext4_input(&img)?;
                let mut f = Ext4::open(&path)?;
                let db = match &file_contexts {
                    Some(p) => {
                        let fc = FileContexts::load(p)?;
                        f.extract_tree_with_policy(&out, &fc)?
                    }
                    None => f.extract_tree(&out)?,
                };
                let meta_path = meta.unwrap_or_else(|| out.join("meta.txt"));
                db.save(&meta_path)?;
                println!("{} entries -> {} + {}", db.entries.len(), out.display(), meta_path.display());
            }
            Ext4Cmd::Build {
                dir,
                out,
                meta,
                fs_config,
                file_contexts,
                size,
                block_size,
                label,
            } => {
                let db = match meta {
                    Some(p) => {
                        let db = MetaDb::load(&p)?;
                        let known: std::collections::HashSet<String> =
                            db.entries.iter().map(|e| e.path.clone()).collect();
                        let mut missing = 0usize;
                        for de in walkdir::WalkDir::new(&dir).into_iter().filter_map(|e| e.ok()) {
                            if de.file_type().is_dir() {
                                continue;
                            }
                            let rel = de.path().strip_prefix(&dir).unwrap_or(de.path());
                            let vp = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
                            if vp != "/meta.txt" && !known.contains(&vp) {
                                missing += 1;
                            }
                        }
                        if missing > 0 {
                            eprintln!(
                                "warning: {} path(s) under {} are not listed in {} and will be skipped",
                                missing,
                                dir.display(),
                                p.display()
                            );
                        }
                        db
                    }
                    None => {
                        let fsc = read_opt_fsc(&fs_config)?;
                        let fc = read_opt_fc(&file_contexts)?;
                        let mut db = MetaDb::from_dir(&dir, &fsc, &fc)?;
                        db.insert(aimg::perm::default_entry("/", Kind::Dir, 0));
                        db
                    }
                };
                ext4::build_tree(
                    &dir,
                    &db,
                    &out,
                    &ext4::BuildOptions {
                        block_size,
                        size: size.as_deref().map(parse_size).transpose()?.unwrap_or(0),
                        volume_name: label,
                        ..Default::default()
                    },
                )?;
                println!(
                    "wrote {} ({})",
                    out.display(),
                    aimg::util::human(aimg::util::file_len(&out)?)
                );
            }
        },

        Cmd::Perm { cmd } => match cmd {
            PermCmd::Export {
                dir,
                out,
                fs_config,
                file_contexts,
            } => {
                let fsc = read_opt_fsc(&fs_config)?;
                let fc = read_opt_fc(&file_contexts)?;
                let db = MetaDb::from_dir(&dir, &fsc, &fc)?;
                db.save(&out)?;
                println!("{} entries -> {}", db.entries.len(), out.display());
            }
            PermCmd::Apply { meta, dir } => {
                if !dir.exists() {
                    bail!("directory not found: {}", dir.display());
                }
                let db = MetaDb::load(&meta)?;
                let n = db.apply_to_dir(&dir)?;
                println!("applied {} entries (readonly bits + mtimes)", n);
            }
            PermCmd::Show { meta, path } => {
                let db = MetaDb::load(&meta)?;
                let e = db
                    .get(&path)
                    .with_context(|| format!("{} not found in {}", path, meta.display()))?;
                println!(
                    "{:?} {} uid={} gid={} mode={} ctx={} caps={:#x} size={} link={:?}",
                    e.kind,
                    e.path,
                    e.uid,
                    e.gid,
                    e.mode_string(),
                    e.ctx,
                    e.caps,
                    e.size,
                    e.link
                );
            }
        },
    }
    Ok(())
}
