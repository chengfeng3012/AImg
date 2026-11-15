//! Text-config driven permission model.
//!
//! Windows has no uid/gid, no POSIX mode bits and no SELinux labels. Instead of
//! fighting NTFS ACLs, this module keeps all of that in plain text:
//!
//! * `fs_config.txt`   - path -> uid / gid / mode / capabilities  (AOSP format)
//! * `file_contexts`   - regex -> SELinux context              (AOSP format)
//! * `meta.txt`        - the full resolved metadata of an extracted tree,
//!                       written as TSV so it survives any copy on NTFS and is
//!                       diff-able / hand-editable.
//!
//! The ext4 builder (`crate::ext4`) consumes `MetaDb` and bakes uid/gid/mode and
//! `security.selinux` xattrs into the image, so the produced image is correct
//! even though the host filesystem cannot represent any of it.

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path};

pub const DEFAULT_DIR_MODE: u32 = 0o755;
pub const DEFAULT_FILE_MODE: u32 = 0o644;
pub const DEFAULT_LINK_MODE: u32 = 0o777;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Dir,
    File,
    Link,
}

impl Kind {
    pub fn tag(&self) -> char {
        match self {
            Kind::Dir => 'D',
            Kind::File => 'F',
            Kind::Link => 'L',
        }
    }
    pub fn from_tag(c: char) -> Result<Self> {
        Ok(match c {
            'D' => Kind::Dir,
            'F' => Kind::File,
            'L' => Kind::Link,
            _ => bail!("bad entry kind '{}'", c),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: Kind,
    /// Absolute, '/'-separated path inside the image, e.g. `/system/bin/sh`.
    pub path: String,
    pub uid: u32,
    pub gid: u32,
    /// Permission bits only (0o7777 max), setuid/setgid/sticky preserved.
    pub mode: u32,
    pub ctx: String,
    pub caps: u64,
    pub size: u64,
    pub mtime: i64,
    pub link: Option<String>,
    /// Original inode number, used to preserve hard links across meta.txt.
    pub ino: Option<u32>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }
    pub fn mode_string(&self) -> String {
        format!("{:04o}", self.mode)
    }
}

// ---------------------------------------------------------------------------
// fs_config.txt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FsRule {
    pub path: String,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub caps: u64,
}

#[derive(Debug, Clone, Default)]
pub struct FsConfig {
    rules: Vec<FsRule>,
    map: std::collections::HashMap<String, usize>,
}

fn norm_path(p: &str) -> String {
    let p = p.trim();
    let p = p.replace('\\', "/");
    if p.starts_with('/') {
        p
    } else {
        format!("/{}", p)
    }
}

impl FsConfig {
    pub fn parse(text: &str) -> Result<Self> {
        let mut rules = Vec::new();
        for (ln, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split_whitespace();
            let path = match it.next() {
                Some(v) => norm_path(v),
                None => continue,
            };
            let uid: u32 = it
                .next()
                .with_context(|| format!("fs_config line {}: missing uid", ln + 1))?
                .parse()
                .with_context(|| format!("fs_config line {}: bad uid", ln + 1))?;
            let gid: u32 = it
                .next()
                .with_context(|| format!("fs_config line {}: missing gid", ln + 1))?
                .parse()
                .with_context(|| format!("fs_config line {}: bad gid", ln + 1))?;
            let mode_s = it
                .next()
                .with_context(|| format!("fs_config line {}: missing mode", ln + 1))?;
            let mode = u32::from_str_radix(mode_s.trim_start_matches("0o"), 8)
                .with_context(|| format!("fs_config line {}: bad mode {}", ln + 1, mode_s))?;
            let mut caps = 0u64;
            for extra in it {
                if let Some(v) = extra.strip_prefix("capabilities=") {
                    caps = u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or(0);
                }
            }
            rules.push(FsRule {
                path,
                uid,
                gid,
                mode,
                caps,
            });
        }
        // Longest path first so lookup is "most specific wins".
        rules.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
        let mut map = std::collections::HashMap::new();
        for (i, r) in rules.iter().enumerate() {
            map.entry(r.path.clone()).or_insert(i);
        }
        Ok(Self { rules, map })
    }

    pub fn load(p: &Path) -> Result<Self> {
        let text = fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
        Self::parse(&text)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Exact match first, then nearest ancestor directory, then defaults.
    pub fn lookup(&self, path: &str, kind: &Kind) -> (u32, u32, u32, u64) {
        let p = norm_path(path);
        if let Some(&i) = self.map.get(&p) {
            let r = &self.rules[i];
            return (r.uid, r.gid, r.mode, r.caps);
        }
        let mut cur = p.as_str();
        while let Some(idx) = cur.rfind('/') {
            cur = &cur[..idx];
            if cur.is_empty() {
                break;
            }
            if let Some(&i) = self.map.get(cur) {
                let r = &self.rules[i];
                let mode = match kind {
                    Kind::Dir => DEFAULT_DIR_MODE,
                    Kind::File => DEFAULT_FILE_MODE,
                    Kind::Link => DEFAULT_LINK_MODE,
                };
                return (r.uid, r.gid, mode, r.caps);
            }
        }
        let mode = match kind {
            Kind::Dir => DEFAULT_DIR_MODE,
            Kind::File => DEFAULT_FILE_MODE,
            Kind::Link => DEFAULT_LINK_MODE,
        };
        (0, 0, mode, 0)
    }
}

// ---------------------------------------------------------------------------
// file_contexts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CtxRule {
    pub regex: Regex,
    pub spec: String,
    /// Literal prefix of the pattern; used to skip regex work cheaply.
    pub literal: String,
    /// Literal prefix length, used to emulate selabel "most specific wins".
    pub literal_len: usize,
    pub order: usize,
    pub file_type: Option<char>,
}

#[derive(Debug, Clone, Default)]
pub struct FileContexts {
    rules: Vec<CtxRule>,
}

fn literal_prefix_len(p: &str) -> usize {
    let mut n = 0;
    for c in p.chars() {
        if ".[]()*+?{}|^$\\".contains(c) {
            break;
        }
        n += 1;
    }
    n
}

impl FileContexts {
    pub fn parse(text: &str) -> Result<Self> {
        let mut rules = Vec::new();
        for (ln, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Optional leading type selector: -d -f -l -c -b -p -s
            let mut file_type = None;
            let mut rest = line;
            if let Some(stripped) = line.strip_prefix('-') {
                let mut ch = stripped.chars();
                if let Some(t) = ch.next() {
                    if "-dflcbps".contains(t) {
                        file_type = Some(t);
                        rest = ch.as_str().trim_start();
                    }
                }
            }
            let mut it = rest.split_whitespace();
            let pat = match it.next() {
                Some(v) => v,
                None => continue,
            };
            let mut spec = match it.next() {
                Some(v) => v.to_string(),
                None => continue,
            };
            // Optional trailing mode spec ("--" or "-<<none>>") is ignored.
            if let Some(m) = it.next() {
                if m != "--" && m != "-<<none>>" {
                    spec = format!("{} {}", spec, m);
                }
            }
            let full = if pat.starts_with('/') {
                pat.to_string()
            } else {
                format!("/{}", pat)
            };
            let regex = Regex::new(&format!("^{}$", full))
                .with_context(|| format!("file_contexts line {}: bad regex '{}'", ln + 1, pat))?;
            let literal: String = full
                .chars()
                .take_while(|c| !".[]()*+?{}|^$\\".contains(*c))
                .collect();
            rules.push(CtxRule {
                regex,
                spec,
                literal,
                literal_len: literal_prefix_len(&full),
                order: rules.len(),
                file_type,
            });
        }
        Ok(Self { rules })
    }

    pub fn load(p: &Path) -> Result<Self> {
        let text = fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
        Self::parse(&text)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn lookup(&self, path: &str, kind: &Kind) -> String {
        let p = norm_path(path);
        let want = match kind {
            Kind::Dir => 'd',
            Kind::File => 'f',
            Kind::Link => 'l',
        };
        let mut best: Option<&CtxRule> = None;
        for r in &self.rules {
            if !r.literal.is_empty() && !p.starts_with(&r.literal) {
                continue;
            }
            if let Some(t) = r.file_type {
                if t != want && !(t == 'f' && kind == &Kind::File) {
                    // be permissive: 'f' also covers symlinks in some trees
                    if !(kind == &Kind::Link && t == 'f') {
                        continue;
                    }
                }
            }
            if r.regex.is_match(&p) {
                match best {
                    None => best = Some(r),
                    Some(b) => {
                        if r.literal_len > b.literal_len
                            || (r.literal_len == b.literal_len && r.order > b.order)
                        {
                            best = Some(r);
                        }
                    }
                }
            }
        }
        best.map(|r| r.spec.clone())
            .unwrap_or_else(|| "u:object_r:unlabeled:s0".to_string())
    }
}

// ---------------------------------------------------------------------------
// meta.txt (TSV)
// ---------------------------------------------------------------------------

pub const META_MAGIC: &str = "# aimg-meta v1";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetaDb {
    pub entries: Vec<Entry>,
}

impl MetaDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan a directory tree and resolve metadata from the text configs.
    pub fn from_dir(root: &Path, fsc: &FsConfig, fc: &FileContexts) -> Result<Self> {
        let mut entries = Vec::new();
        let (ruid, rgid, rmode, rcaps) = fsc.lookup("/", &Kind::Dir);
        let rctx = fc.lookup("/", &Kind::Dir);
        let rmtime = fs::metadata(root)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        entries.push(Entry {
            ino: None,
            kind: Kind::Dir,
            path: "/".into(),
            uid: ruid,
            gid: rgid,
            mode: rmode,
            ctx: rctx,
            caps: rcaps,
            size: 0,
            mtime: rmtime,
            link: None,
        });
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let rd = match fs::read_dir(&dir) {
                Ok(v) => v,
                Err(e) => bail!("read_dir {}: {}", dir.display(), e),
            };
            for de in rd {
                let de = de?;
                let p = de.path();
                let meta = fs::symlink_metadata(&p)?;
                let rel = p
                    .strip_prefix(root)
                    .with_context(|| format!("strip prefix {}", p.display()))?;
                let mut vpath = String::from("/");
                let mut first = true;
                for c in rel.components() {
                    if let Component::Normal(s) = c {
                        if !first {
                            vpath.push('/');
                        }
                        vpath.push_str(&s.to_string_lossy());
                        first = false;
                    }
                }
                if vpath == "/meta.txt" {
                    continue;
                }
                let ft = meta.file_type();
                let kind = if ft.is_symlink() {
                    Kind::Link
                } else if ft.is_dir() {
                    Kind::Dir
                } else {
                    Kind::File
                };
                if kind == Kind::Dir {
                    stack.push(p.clone());
                }
                let (uid, gid, mode, caps) = fsc.lookup(&vpath, &kind);
                let ctx = fc.lookup(&vpath, &kind);
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let link = if kind == Kind::Link {
                    Some(
                        fs::read_link(&p)?
                            .to_string_lossy()
                            .replace('\\', "/"),
                    )
                } else {
                    None
                };
                entries.push(Entry {
                    ino: None,
                    kind,
                    path: vpath,
                    uid,
                    gid,
                    mode,
                    ctx,
                    caps,
                    size: if ft.is_file() { meta.len() } else { 0 },
                    mtime,
                    link,
                });
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self { entries })
    }

    pub fn load(p: &Path) -> Result<Self> {
        let text = fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut entries = Vec::new();
        for (ln, raw) in text.lines().enumerate() {
            let line = raw.trim_end_matches(['\r', '\n']);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 8 {
                bail!("meta line {}: expected >=8 tab fields, got {}", ln + 1, f.len());
            }
            let kind = Kind::from_tag(f[0].chars().next().unwrap_or('?'))?;
            entries.push(Entry {
                kind,
                path: f[1].to_string(),
                uid: f[2].parse().with_context(|| format!("meta line {}: uid", ln + 1))?,
                gid: f[3].parse().with_context(|| format!("meta line {}: gid", ln + 1))?,
                mode: u32::from_str_radix(f[4], 8)
                    .with_context(|| format!("meta line {}: mode", ln + 1))?,
                ctx: f[5].to_string(),
                caps: u64::from_str_radix(f[6].trim_start_matches("0x"), 16).unwrap_or(0),
                size: f[7].parse().unwrap_or(0),
                mtime: f.get(8).and_then(|v| v.parse().ok()).unwrap_or(0),
                link: f.get(9).filter(|s| !s.is_empty()).map(|s| s.to_string()),
                ino: f.get(10).and_then(|v| v.parse().ok()),
            });
        }
        Ok(Self { entries })
    }

    pub fn save(&self, p: &Path) -> Result<()> {
        crate::util::ensure_parent(p)?;
        let mut out = String::new();
        out.push_str(META_MAGIC);
        out.push('\n');
        out.push_str("# kind\tpath\tuid\tgid\tmode\tcontext\tcaps\tsize\tmtime\tlink_target\tino\n");
        for e in &self.entries {
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{:04o}\t{}\t0x{:x}\t{}\t{}\t{}\t{}\n",
                e.kind.tag(),
                e.path,
                e.uid,
                e.gid,
                e.mode,
                e.ctx,
                e.caps,
                e.size,
                e.mtime,
                e.link.clone().unwrap_or_default(),
                e.ino.map(|v| v.to_string()).unwrap_or_default()
            ));
        }
        fs::write(p, out).with_context(|| format!("write {}", p.display()))?;
        Ok(())
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    pub fn by_path(&self) -> HashMap<&str, &Entry> {
        self.entries.iter().map(|e| (e.path.as_str(), e)).collect()
    }

    /// Best-effort application to a real directory on the host: set the read
    /// only attribute according to write bits and restore mtimes. NTFS cannot
    /// carry uid/gid/SELinux, those stay in `meta.txt` and in the built image.
    pub fn apply_to_dir(&self, root: &Path) -> Result<usize> {
        let mut n = 0;
        for e in &self.entries {
            let rel = e.path.trim_start_matches('/');
            let p = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if !p.exists() && e.kind != Kind::Link {
                continue;
            }
            if e.kind == Kind::Link {
                continue; // symlinks are handled by the extractor
            }
            let mut perms = fs::metadata(&p)?.permissions();
            let ro = e.mode & 0o222 == 0;
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(ro);
            let _ = fs::set_permissions(&p, perms);
            if e.mtime > 0 {
                let t = filetime::FileTime::from_unix_time(e.mtime, 0);
                let _ = filetime::set_file_mtime(&p, t);
            }
            n += 1;
        }
        Ok(n)
    }

    pub fn insert(&mut self, e: Entry) {
        if let Some(pos) = self.entries.iter().position(|x| x.path == e.path) {
            self.entries[pos] = e;
        } else {
            self.entries.push(e);
        }
    }
}

/// Default metadata for a path that has no explicit rule.
pub fn default_entry(path: &str, kind: Kind, size: u64) -> Entry {
    let mode = match kind {
        Kind::Dir => DEFAULT_DIR_MODE,
        Kind::File => DEFAULT_FILE_MODE,
        Kind::Link => DEFAULT_LINK_MODE,
    };
    Entry {
        kind,
        path: norm_path(path),
        uid: 0,
        gid: 0,
        mode,
        ctx: "u:object_r:unlabeled:s0".to_string(),
        caps: 0,
        size,
        mtime: 0,
        link: None,
        ino: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_config_lookup() {
        let t = "\
# comment
/system 0 0 0755
/system/bin 0 2000 0755
/system/bin/sh 0 2000 0755
/system/bin/su 0 0 04755 capabilities=0x40
";
        let c = FsConfig::parse(t).unwrap();
        assert_eq!(c.lookup("/system/bin/sh", &Kind::File), (0, 2000, 0o755, 0));
        // inherited uid/gid, default file mode
        assert_eq!(c.lookup("/system/bin/ls", &Kind::File), (0, 2000, 0o644, 0));
        assert_eq!(c.lookup("/system/bin/su", &Kind::File).2, 0o4755);
        assert_eq!(c.lookup("/system/bin/su", &Kind::File).3, 0x40);
        assert_eq!(c.lookup("/vendor/x", &Kind::File), (0, 0, 0o644, 0));
    }

    #[test]
    fn file_contexts_most_specific() {
        let t = "\
/system(/.*)? u:object_r:system_file:s0
/system/bin/sh u:object_r:shell_exec:s0
/system/bin/.* u:object_r:system_bin:s0
";
        let c = FileContexts::parse(t).unwrap();
        assert_eq!(c.lookup("/system/bin/sh", &Kind::File), "u:object_r:shell_exec:s0");
        assert_eq!(c.lookup("/system/bin/ls", &Kind::File), "u:object_r:system_bin:s0");
        assert_eq!(c.lookup("/system/etc/x", &Kind::File), "u:object_r:system_file:s0");
        assert_eq!(c.lookup("/data/x", &Kind::File), "u:object_r:unlabeled:s0");
    }

    #[test]
    fn meta_roundtrip() {
        let mut db = MetaDb::new();
        db.insert(Entry {
            kind: Kind::Dir,
            path: "/system".into(),
            uid: 0,
            gid: 0,
            mode: 0o755,
            ctx: "u:object_r:system_file:s0".into(),
            caps: 0,
            size: 0,
            mtime: 1700000000,
            link: None,            ino: None,
        });
        db.insert(Entry {
            kind: Kind::Link,
            path: "/system/bin/su".into(),
            uid: 0,
            gid: 0,
            mode: 0o777,
            ctx: "u:object_r:su_exec:s0".into(),
            caps: 0,
            size: 0,
            mtime: 0,
            link: Some("/system/xbin/su".into()),            ino: None,
        });
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("meta.txt");
        db.save(&p).unwrap();
        let back = MetaDb::load(&p).unwrap();
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.entries[1].link.as_deref(), Some("/system/xbin/su"));
        assert_eq!(back.entries[1].ctx, "u:object_r:su_exec:s0");
    }
}
