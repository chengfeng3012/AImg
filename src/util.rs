//! Small shared helpers: aligned IO, size parsing, checksums.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub fn align_up(v: u64, a: u64) -> u64 {
    if a == 0 {
        return v;
    }
    (v + a - 1) / a * a
}

pub fn align_down(v: u64, a: u64) -> u64 {
    if a == 0 {
        return v;
    }
    v / a * a
}

/// Reject a single path component taken from image metadata. Anything that
/// could escape the output directory (`..`, separators, drive letters, NTFS
/// ADS, control characters) is refused.
pub fn safe_component(name: &str) -> Result<&str> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.ends_with('.')
        || name.ends_with(' ')
        || name.chars().any(|c| (c as u32) < 0x20)
    {
        bail!("unsafe path component {:?} from image metadata", name);
    }
    let upper = name.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    let reserved = matches!(base, "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        bail!("reserved Windows device name {:?} in image metadata", name);
    }
    Ok(name)
}

/// Validate an absolute image path (`/system/bin/sh`) segment by segment.
pub fn safe_image_path(path: &str) -> Result<()> {
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        if seg == "." || seg == ".." {
            bail!("unsafe path {:?} in image metadata", path);
        }
        safe_component(seg)?;
    }
    Ok(())
}

pub fn read_at(f: &mut File, off: u64, buf: &mut [u8]) -> Result<()> {
    f.seek(SeekFrom::Start(off))?;
    f.read_exact(buf)?;
    Ok(())
}

pub fn read_vec_at(f: &mut File, off: u64, len: u64) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len as usize];
    read_at(f, off, &mut buf)?;
    Ok(buf)
}

pub fn write_at(f: &mut File, off: u64, buf: &[u8]) -> Result<()> {
    f.seek(SeekFrom::Start(off))?;
    f.write_all(buf)?;
    Ok(())
}

pub fn file_len(p: &Path) -> Result<u64> {
    Ok(std::fs::metadata(p)?.len())
}

pub fn create_sized(p: &Path, size: u64) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(p)
        .with_context(|| format!("create {}", p.display()))?;
    f.set_len(size)?;
    Ok(f)
}

/// Parse sizes like "512", "4K", "2M", "8G" (base 1024).
pub fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty size");
    }
    let (num, mult) = match s.chars().last().unwrap().to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1024u64),
        'M' => (&s[..s.len() - 1], 1024 * 1024),
        'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        'T' => (&s[..s.len() - 1], 1024u64 * 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    let v: u64 = num.trim().parse().with_context(|| format!("bad size {}", s))?;
    v.checked_mul(mult)
        .with_context(|| format!("size {} overflows u64", s))
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(sha256(data))
}

pub fn copy_range(src: &mut File, dst: &mut File, src_off: u64, len: u64) -> Result<()> {
    let mut left = len;
    let mut off = src_off;
    let mut buf = vec![0u8; 1 << 20];
    while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        read_at(src, off, &mut buf[..n])?;
        dst.write_all(&buf[..n])?;
        off += n as u64;
        left -= n as u64;
    }
    Ok(())
}

pub fn ensure_parent(p: &Path) -> Result<()> {
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

pub fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).to_string()
}

pub fn put_cstr(dst: &mut [u8], s: &str) {
    for b in dst.iter_mut() {
        *b = 0;
    }
    let bytes = s.as_bytes();
    let n = bytes.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&bytes[..n]);
}

/// Human readable byte count.
pub fn human(n: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < U.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, U[0])
    } else {
        format!("{:.2} {}", v, U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_components() {
        for bad in ["..", ".", "a/b", "a\\b", "C:x", "con", "NUL", "a\u{1}"] {
            assert!(safe_component(bad).is_err(), "{} should be rejected", bad);
        }
        assert!(safe_component("system").is_ok());
        assert!(safe_component("lib64").is_ok());
    }

    #[test]
    fn rejects_unsafe_paths() {
        assert!(safe_image_path("/system/../../etc/passwd").is_err());
        assert!(safe_image_path("/system/bin/sh").is_ok());
    }
}
