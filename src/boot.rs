//! `boot.img` / `recovery.img` / `vendor_boot.img` (header v0..v4).
//!
//! Rewritten from AOSP `system/tools/mkbootimg` (`bootimg.h`, mkbootimg.py,
//! unpack_bootimg.py) plus the AVB footer handling that `avbtool.py` performs.
//! Everything is pure userspace: no loop device, no root.

use crate::util::*;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::Path;

pub const BOOT_MAGIC: &[u8; 8] = b"ANDROID!";
pub const VENDOR_BOOT_MAGIC: &[u8; 8] = b"VNDRBOOT";
pub const AVB_FOOTER_MAGIC: &[u8; 4] = b"AVBf";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AvbFooter {
    pub version_major: u32,
    pub version_minor: u32,
    pub original_image_size: u64,
    pub vbmeta_offset: u64,
    pub vbmeta_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorRamdiskDesc {
    pub name: String,
    pub ramdisk_type: u32,
    pub size: u32,
    pub offset: u32,
    #[serde(default)]
    pub board_id: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootDesc {
    /// "boot" or "vendor_boot"
    pub kind: String,
    pub header_version: u32,
    pub page_size: u32,
    pub kernel_addr: u32,
    pub ramdisk_addr: u32,
    pub second_addr: u32,
    pub tags_addr: u32,
    pub dtb_addr: u64,
    pub recovery_dtbo_offset: u64,
    pub name: String,
    pub cmdline: String,
    pub extra_cmdline: String,
    pub os_version: u32,
    pub signature_size: u32,
    pub avb: Option<AvbFooter>,
    #[serde(default)]
    pub vendor_ramdisks: Vec<VendorRamdiskDesc>,
}

impl Default for BootDesc {
    fn default() -> Self {
        Self {
            kind: "boot".into(),
            header_version: 0,
            page_size: 2048,
            kernel_addr: 0x1000_8000,
            ramdisk_addr: 0x1100_0000,
            second_addr: 0x10f0_0000,
            tags_addr: 0x1000_0100,
            dtb_addr: 0x11f0_0000,
            recovery_dtbo_offset: 0,
            name: String::new(),
            cmdline: String::new(),
            extra_cmdline: String::new(),
            os_version: 0,
            signature_size: 0,
            avb: None,
            vendor_ramdisks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BootImage {
    pub desc: BootDesc,
    pub kernel: Vec<u8>,
    pub ramdisk: Vec<u8>,
    pub second: Vec<u8>,
    pub recovery_dtbo: Vec<u8>,
    pub dtb: Vec<u8>,
    pub signature: Vec<u8>,
    /// vendor_boot only: (desc, data)
    pub vendor_ramdisks: Vec<(VendorRamdiskDesc, Vec<u8>)>,
    pub bootconfig: Vec<u8>,
    pub avb_footer: Option<AvbFooter>,
}

fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64at(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}
fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

pub fn parse_avb_footer(data: &[u8]) -> Option<AvbFooter> {
    if data.len() < 64 {
        return None;
    }
    let foot = &data[data.len() - 64..];
    if &foot[0..4] != AVB_FOOTER_MAGIC {
        return None;
    }
    Some(AvbFooter {
        version_major: u32at(foot, 4),
        version_minor: u32at(foot, 8),
        original_image_size: u64at(foot, 12),
        vbmeta_offset: u64at(foot, 20),
        vbmeta_size: u64at(foot, 28),
    })
}

impl BootImage {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 8 {
            bail!("file too small");
        }
        if &data[0..8] == VENDOR_BOOT_MAGIC {
            return Self::parse_vendor(data);
        }
        if &data[0..8] != BOOT_MAGIC {
            bail!("not an Android boot image (bad magic)");
        }
        if data.len() < 44 {
            bail!("boot header truncated");
        }
        let header_version = u32at(data, 40);
        let need = match header_version {
            0 => 1632,
            1 => 1648,
            2 => 1660,
            3 => 1580,
            4 => 1584,
            other => bail!("unsupported boot header version {}", other),
        };
        if data.len() < need {
            bail!("boot image truncated: need {} bytes, got {}", need, data.len());
        }
        let page_size = if header_version >= 3 {
            4096
        } else {
            u32at(data, 36)
        };
        // v3/v4 store page_size implicitly (always 4096); offsets 24..40 are reserved.
        if header_version < 3 && (page_size == 0 || !page_size.is_power_of_two()) {
            bail!("bad page size {}", page_size);
        }
        let avb_footer = parse_avb_footer(data);
        let mut desc = BootDesc {
            kind: "boot".into(),
            header_version,
            page_size,
            kernel_addr: u32at(data, 12),
            ramdisk_addr: u32at(data, 20),
            second_addr: u32at(data, 28),
            tags_addr: u32at(data, 32),
            dtb_addr: 0,
            recovery_dtbo_offset: 0,
            name: if header_version < 3 { cstr(&data[48..64]) } else { String::new() },
            cmdline: if header_version < 3 {
                cstr(&data[64..576])
            } else {
                cstr(&data[44..1580])
            },
            extra_cmdline: if header_version < 3 {
                cstr(&data[608..1632])
            } else {
                String::new()
            },
            os_version: if header_version < 3 { u32at(data, 44) } else { u32at(data, 16) },
            signature_size: 0,
            avb: avb_footer,
            vendor_ramdisks: Vec::new(),
        };
        if header_version >= 3 {
            // v3/v4 have no load addresses: those offsets hold sizes/reserved.
            desc.kernel_addr = 0;
            desc.ramdisk_addr = 0;
            desc.second_addr = 0;
            desc.tags_addr = 0;
        }

        let mut kernel_size = u32at(data, 8) as u64;
        let mut ramdisk_size = u32at(data, 16) as u64;
        let second_size = u32at(data, 24) as u64;
        let mut recovery_dtbo_size = 0u64;
        let mut dtb_size = 0u64;

        if header_version >= 1 {
            if data.len() < 1648 {
                bail!("truncated v1 header");
            }
            recovery_dtbo_size = u32at(data, 1632) as u64;
            desc.recovery_dtbo_offset = u64at(data, 1636);
        }
        if header_version >= 2 {
            if data.len() < 1660 {
                bail!("truncated v2 header");
            }
            dtb_size = u32at(data, 1648) as u64;
            desc.dtb_addr = u64at(data, 1652);
        }
        if header_version >= 3 {
            // v3/v4: 4096 byte header, fixed page size
            if page_size != 4096 {
                bail!("v{} boot image must use 4096 page size", header_version);
            }
            kernel_size = u32at(data, 8) as u64;
            ramdisk_size = u32at(data, 12) as u64;
            desc.os_version = u32at(data, 16);
            desc.cmdline = cstr(&data[44..1580]);
            desc.extra_cmdline = String::new();
            desc.name = String::new();
            if header_version >= 4 {
                desc.signature_size = u32at(data, 1580);
            }
        }

        let pg = page_size as u64;
        let mut off = pg; // header occupies one page
        let kernel = take(data, off, kernel_size)?;
        off += align_up(kernel_size, pg);
        let ramdisk = take(data, off, ramdisk_size)?;
        off += align_up(ramdisk_size, pg);
        let mut second = Vec::new();
        if header_version < 3 && second_size > 0 {
            second = take(data, off, second_size)?;
            off += align_up(second_size, pg);
        }
        let mut recovery_dtbo = Vec::new();
        if header_version >= 1 && header_version < 3 && recovery_dtbo_size > 0 {
            recovery_dtbo = take(data, off, recovery_dtbo_size)?;
            off += align_up(recovery_dtbo_size, pg);
        }
        let mut dtb = Vec::new();
        if header_version == 2 && dtb_size > 0 {
            dtb = take(data, off, dtb_size)?;
            off += align_up(dtb_size, pg);
        }
        let mut signature = Vec::new();
        if header_version >= 4 && desc.signature_size > 0 {
            signature = take(data, off, desc.signature_size as u64)?;
        }

        Ok(Self {
            desc,
            kernel,
            ramdisk,
            second,
            recovery_dtbo,
            dtb,
            signature,
            vendor_ramdisks: Vec::new(),
            bootconfig: Vec::new(),
            avb_footer,
        })
    }

    fn parse_vendor(data: &[u8]) -> Result<Self> {
        if data.len() < 16 {
            bail!("vendor_boot header truncated");
        }
        let header_version = u32at(data, 8);
        let page_size = u32at(data, 12);
        let need = if header_version >= 4 { 2128 } else { 2112 };
        if data.len() < need {
            bail!("truncated vendor_boot header: need {} bytes, got {}", need, data.len());
        }
        if page_size == 0 || !page_size.is_power_of_two() {
            bail!("bad vendor_boot page size {}", page_size);
        }
        let vendor_ramdisk_size = u32at(data, 24) as u64;
        let mut desc = BootDesc {
            kind: "vendor_boot".into(),
            header_version,
            page_size,
            kernel_addr: u32at(data, 16),
            ramdisk_addr: u32at(data, 20),
            second_addr: 0,
            tags_addr: u32at(data, 2076),
            dtb_addr: u64at(data, 2104),
            recovery_dtbo_offset: 0,
            name: cstr(&data[2080..2096]),
            cmdline: cstr(&data[28..2076]),
            extra_cmdline: String::new(),
            os_version: 0,
            signature_size: 0,
            avb: parse_avb_footer(data),
            vendor_ramdisks: Vec::new(),
        };
        if data.len() < 2112 {
            bail!("truncated vendor_boot header");
        }
        let dtb_size = u32at(data, 2100) as u64;
        let pg = page_size as u64;
        let header_pages = align_up(if header_version >= 4 { 2128 } else { 2112 }, pg);
        let mut off = header_pages;
        let vramdisk = take(data, off, vendor_ramdisk_size)?;
        off += align_up(vendor_ramdisk_size, pg);
        let dtb = take(data, off, dtb_size)?;
        off += align_up(dtb_size, pg);

        let mut vendor_ramdisks = Vec::new();
        let mut bootconfig = Vec::new();
        if header_version >= 4 {
            if data.len() < 2128 {
                bail!("truncated vendor_boot v4 header");
            }
            let table_size = u32at(data, 2112) as u64;
            let entry_num = u32at(data, 2116);
            let entry_size = u32at(data, 2120) as usize;
            let bootconfig_size = u32at(data, 2124) as u64;
            if entry_size < 108 {
                bail!("vendor_boot ramdisk entry_size {} < 108", entry_size);
            }
            if entry_num as u64 * entry_size as u64 > table_size {
                bail!("vendor_boot ramdisk table ({} x {}) overflows {} bytes", entry_num, entry_size, table_size);
            }
            let table = take(data, off, table_size)?;
            off += align_up(table_size, pg);
            bootconfig = take(data, off, bootconfig_size)?;
            for i in 0..entry_num as usize {
                let o = i * entry_size;
                if o + 108 > table.len() {
                    break;
                }
                let rd = VendorRamdiskDesc {
                    size: u32at(&table, o),
                    offset: u32at(&table, o + 4),
                    ramdisk_type: u32at(&table, o + 8),
                    name: cstr(&table[o + 12..o + 44]),
                    board_id: (0..16).map(|k| u32at(&table, o + 44 + k * 4)).collect(),
                };
                let start = rd.offset as usize;
                let end = start + rd.size as usize;
                let data = if end <= vramdisk.len() {
                    vramdisk[start..end].to_vec()
                } else {
                    Vec::new()
                };
                desc.vendor_ramdisks.push(rd.clone());
                vendor_ramdisks.push((rd, data));
            }
            // keep the raw section as "ramdisk" so repack can round-trip
            let ramdisk_raw = vramdisk.clone();
            return Ok(Self {
                desc,
                kernel: Vec::new(),
                ramdisk: ramdisk_raw,
                second: Vec::new(),
                recovery_dtbo: Vec::new(),
                dtb,
                signature: Vec::new(),
                vendor_ramdisks,
                bootconfig,
                avb_footer: parse_avb_footer(data),
            });
        }

        Ok(Self {
            desc,
            kernel: Vec::new(),
            ramdisk: vramdisk,
            second: Vec::new(),
            recovery_dtbo: Vec::new(),
            dtb,
            signature: Vec::new(),
            vendor_ramdisks,
            bootconfig,
            avb_footer: parse_avb_footer(data),
        })
    }

    /// Serialize back into an image. Sizes always come from the actual payloads.
    pub fn build(&self) -> Result<Vec<u8>> {
        if self.desc.kind == "vendor_boot" {
            return self.build_vendor();
        }
        let d = &self.desc;
        let v = d.header_version;
        let pg = d.page_size as u64;
        if pg == 0 || !pg.is_power_of_two() {
            bail!("bad page size {}", pg);
        }
        if v >= 3 {
            if pg != 4096 {
                bail!("boot header v{} requires page_size 4096 (got {})", v, pg);
            }
        } else {
            let need: u64 = match v {
                0 => 1632,
                1 => 1648,
                _ => 1660,
            };
            if pg < need {
                bail!("page_size {} is too small for a v{} header", pg, v);
            }
        }
        let mut hdr = vec![0u8; pg as usize];
        hdr[0..8].copy_from_slice(BOOT_MAGIC);
        if v >= 3 {
            put_u32(&mut hdr, 8, self.kernel.len() as u32);
            put_u32(&mut hdr, 12, self.ramdisk.len() as u32);
            put_u32(&mut hdr, 16, d.os_version);
            let header_size = if v >= 4 { 1584 } else { 1580 };
            put_u32(&mut hdr, 20, header_size);
            // v3 layout: magic 0..8, kernel_size 8, ramdisk_size 12, os_version 16,
            // header_size 20, reserved 24..40, header_version 40, cmdline 44..1580
            put_u32(&mut hdr, 40, v);
            put_cstr(&mut hdr[44..1580], &d.cmdline);
            if v >= 4 {
                put_u32(&mut hdr, 1580, self.signature.len() as u32);
            }
        } else {
            put_u32(&mut hdr, 8, self.kernel.len() as u32);
            put_u32(&mut hdr, 12, if self.kernel.is_empty() { 0 } else { d.kernel_addr });
            put_u32(&mut hdr, 16, self.ramdisk.len() as u32);
            put_u32(&mut hdr, 20, if self.ramdisk.is_empty() { 0 } else { d.ramdisk_addr });
            put_u32(&mut hdr, 24, self.second.len() as u32);
            put_u32(&mut hdr, 28, if self.second.is_empty() { 0 } else { d.second_addr });
            put_u32(&mut hdr, 32, d.tags_addr);
            put_u32(&mut hdr, 36, d.page_size);
            put_u32(&mut hdr, 40, v);
            put_u32(&mut hdr, 44, d.os_version);
            put_cstr(&mut hdr[48..64], &d.name);
            put_cstr(&mut hdr[64..576], &d.cmdline);
            put_cstr(&mut hdr[608..1632], &d.extra_cmdline);
            // id[8] = sha1(kernel|ramdisk|second[|recovery_dtbo][|dtb])
            let mut h = Sha1::new();
            h.update(&self.kernel);
            h.update(&self.ramdisk);
            h.update(&self.second);
            if v >= 1 {
                h.update(&self.recovery_dtbo);
            }
            if v == 2 {
                h.update(&self.dtb);
            }
            let digest = h.finalize();
            for b in hdr[576..608].iter_mut() {
                *b = 0;
            }
            hdr[576..576 + digest.len()].copy_from_slice(&digest);
            if v >= 1 {
                put_u32(&mut hdr, 1632, self.recovery_dtbo.len() as u32);
                put_u64(&mut hdr, 1636, d.recovery_dtbo_offset);
                let hs = if v >= 2 { 1660 } else { 1648 };
                put_u32(&mut hdr, 1644, hs);
            }
            if v >= 2 {
                put_u32(&mut hdr, 1648, self.dtb.len() as u32);
                put_u64(&mut hdr, 1652, if self.dtb.is_empty() { 0 } else { d.dtb_addr });
            }
        }

        let mut out = hdr;
        pad_to(&mut out, pg);
        push_padded(&mut out, &self.kernel, pg);
        push_padded(&mut out, &self.ramdisk, pg);
        if v < 3 {
            push_padded(&mut out, &self.second, pg);
            if v >= 1 {
                push_padded(&mut out, &self.recovery_dtbo, pg);
            }
            if v == 2 {
                push_padded(&mut out, &self.dtb, pg);
            }
        } else if v >= 4 {
            push_padded(&mut out, &self.signature, pg);
        }
        Ok(out)
    }

    fn build_vendor(&self) -> Result<Vec<u8>> {
        let d = &self.desc;
        let v = d.header_version;
        let pg = d.page_size as u64;
        if v < 3 {
            bail!("vendor_boot requires header version >= 3");
        }
        let header_len: usize = if v >= 4 { 2128 } else { 2112 };
        if pg == 0 || !pg.is_power_of_two() {
            bail!("bad vendor_boot page size {}", pg);
        }
        if v >= 4 && pg != 4096 {
            bail!("vendor_boot v4 requires page_size 4096 (got {})", pg);
        }
        if pg < header_len as u64 {
            bail!("page_size {} is smaller than the vendor_boot header", pg);
        }
        let mut hdr = vec![0u8; align_up(header_len as u64, pg) as usize];
        hdr[0..8].copy_from_slice(VENDOR_BOOT_MAGIC);
        put_u32(&mut hdr, 8, v);
        put_u32(&mut hdr, 12, d.page_size);
        put_u32(&mut hdr, 16, d.kernel_addr);
        put_u32(&mut hdr, 20, d.ramdisk_addr);
        put_u32(&mut hdr, 24, self.ramdisk.len() as u32);
        put_cstr(&mut hdr[28..2076], &d.cmdline);
        put_u32(&mut hdr, 2076, d.tags_addr);
        put_cstr(&mut hdr[2080..2096], &d.name);
        put_u32(&mut hdr, 2096, header_len as u32);
        put_u32(&mut hdr, 2100, self.dtb.len() as u32);
        put_u64(&mut hdr, 2104, d.dtb_addr);

        let mut table = Vec::new();
        let mut cursor: u32 = 0;
        if v >= 4 {
            for (rd, data) in &self.vendor_ramdisks {
                let mut e = vec![0u8; 108];
                put_u32(&mut e, 0, data.len() as u32);
                put_u32(&mut e, 4, cursor);
                cursor += data.len() as u32;
                put_u32(&mut e, 8, rd.ramdisk_type);
                put_cstr(&mut e[12..44], &rd.name);
                for (k, v) in rd.board_id.iter().take(16).enumerate() {
                    put_u32(&mut e, 44 + k * 4, *v);
                }
                table.extend_from_slice(&e);
            }
            if !self.vendor_ramdisks.is_empty() && self.ramdisk.len() as u32 != cursor {
                bail!(
                    "vendor_ramdisk section is {} bytes but fragments sum to {}",
                    self.ramdisk.len(),
                    cursor
                );
            }
            put_u32(&mut hdr, 2112, table.len() as u32);
            put_u32(&mut hdr, 2116, self.vendor_ramdisks.len() as u32);
            put_u32(&mut hdr, 2120, 108);
            put_u32(&mut hdr, 2124, self.bootconfig.len() as u32);
        }

        let mut out = hdr;
        push_padded(&mut out, &self.ramdisk, pg);
        push_padded(&mut out, &self.dtb, pg);
        if v >= 4 {
            push_padded(&mut out, &table, pg);
            push_padded(&mut out, &self.bootconfig, pg);
        }
        Ok(out)
    }
}

fn take(data: &[u8], off: u64, len: u64) -> Result<Vec<u8>> {
    let start = off as usize;
    let end = start + len as usize;
    if len == 0 {
        return Ok(Vec::new());
    }
    if end > data.len() {
        bail!(
            "image truncated: need {} bytes at offset {}, file has {}",
            len,
            off,
            data.len()
        );
    }
    Ok(data[start..end].to_vec())
}

fn pad_to(out: &mut Vec<u8>, align: u64) {
    let target = align_up(out.len() as u64, align) as usize;
    out.resize(target, 0);
}

fn push_padded(out: &mut Vec<u8>, data: &[u8], align: u64) {
    if data.is_empty() {
        return;
    }
    out.extend_from_slice(data);
    pad_to(out, align);
}

/// Unpack an image into a directory: payload files + `boot.json`.
pub fn unpack(path: &Path, outdir: &Path) -> Result<BootDesc> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut img = BootImage::parse(&data)?;
    // Drop AVB padding so the extracted payloads are the real ones.
    if let Some(f) = img.avb_footer {
        if f.original_image_size > 0 && (f.original_image_size as usize) < data.len() {
            let trimmed = &data[..f.original_image_size as usize];
            img = BootImage::parse(trimmed)?;
            img.avb_footer = Some(f);
        }
    }
    fs::create_dir_all(outdir)?;
    let kind = img.desc.kind.clone();
    if kind == "boot" {
        fs::write(outdir.join("kernel"), &img.kernel)?;
        fs::write(outdir.join("ramdisk"), &img.ramdisk)?;
        if !img.second.is_empty() {
            fs::write(outdir.join("second"), &img.second)?;
        }
        if !img.recovery_dtbo.is_empty() {
            fs::write(outdir.join("recovery_dtbo"), &img.recovery_dtbo)?;
        }
        if !img.dtb.is_empty() {
            fs::write(outdir.join("dtb"), &img.dtb)?;
        }
        if !img.signature.is_empty() {
            fs::write(outdir.join("boot_signature"), &img.signature)?;
        }
    } else {
        fs::write(outdir.join("vendor_ramdisk"), &img.ramdisk)?;
        fs::write(outdir.join("dtb"), &img.dtb)?;
        for (i, (rd, data)) in img.vendor_ramdisks.iter().enumerate() {
            let name = if rd.name.is_empty() {
                format!("vendor_ramdisk.{}", i)
            } else {
                format!("vendor_ramdisk.{}", rd.name)
            };
            fs::write(outdir.join(name), data)?;
        }
        if !img.bootconfig.is_empty() {
            fs::write(outdir.join("bootconfig"), &img.bootconfig)?;
        }
    }
    let desc_path = outdir.join("boot.json");
    fs::write(&desc_path, serde_json::to_string_pretty(&img.desc)?)?;
    Ok(img.desc)
}

/// Repack a directory produced by [`unpack`].
pub fn pack(dir: &Path, out: &Path) -> Result<()> {
    let desc_path = dir.join("boot.json");
    let text = fs::read_to_string(&desc_path)
        .with_context(|| format!("read {}", desc_path.display()))?;
    let desc: BootDesc = serde_json::from_str(&text)?;
    let read_opt = |n: &str| -> Result<Vec<u8>> {
        let p = dir.join(n);
        if p.exists() {
            Ok(fs::read(p)?)
        } else {
            Ok(Vec::new())
        }
    };
    let mut img = BootImage {
        desc: desc.clone(),
        kernel: read_opt("kernel")?,
        ramdisk: read_opt("ramdisk")?,
        second: read_opt("second")?,
        recovery_dtbo: read_opt("recovery_dtbo")?,
        dtb: read_opt("dtb")?,
        signature: read_opt("boot_signature")?,
        vendor_ramdisks: Vec::new(),
        bootconfig: read_opt("bootconfig")?,
        avb_footer: None,
    };
    if desc.kind == "vendor_boot" {
        img.ramdisk = read_opt("vendor_ramdisk")?;
        if desc.header_version >= 4 && !desc.vendor_ramdisks.is_empty() {
            let mut raw = Vec::new();
            let mut offset = 0u32;
            for rd in &desc.vendor_ramdisks {
                let name = if rd.name.is_empty() {
                    format!("vendor_ramdisk.{}", img.vendor_ramdisks.len())
                } else {
                    format!("vendor_ramdisk.{}", rd.name)
                };
                let data = read_opt(&name)?;
                let mut d = rd.clone();
                d.offset = offset;
                d.size = data.len() as u32;
                offset += data.len() as u32;
                raw.extend_from_slice(&data);
                img.vendor_ramdisks.push((d, data));
            }
            // Rebuild the section from the (possibly edited) fragments so the
            // table entries and the byte range always agree.
            img.ramdisk = raw;
        }
    }
    let bytes = img.build()?;
    crate::util::ensure_parent(out)?;
    fs::write(out, &bytes)?;
    Ok(())
}

/// Print a short summary (mirrors `unpack_bootimg.py` output style).
pub fn describe(path: &Path) -> Result<String> {
    let data = fs::read(path)?;
    let img = BootImage::parse(&data)?;
    let d = &img.desc;
    let mut s = String::new();
    s.push_str(&format!("kind: {}\n", d.kind));
    s.push_str(&format!("header_version: {}\n", d.header_version));
    s.push_str(&format!("page_size: {}\n", d.page_size));
    s.push_str(&format!("os_version: {:#x}\n", d.os_version));
    s.push_str(&format!("cmdline: {}\n", d.cmdline.trim_end_matches('\0')));
    s.push_str(&format!("kernel: {} bytes\n", img.kernel.len()));
    s.push_str(&format!("ramdisk: {} bytes\n", img.ramdisk.len()));
    if !img.second.is_empty() {
        s.push_str(&format!("second: {} bytes\n", img.second.len()));
    }
    if !img.recovery_dtbo.is_empty() {
        s.push_str(&format!(
            "recovery_dtbo: {} bytes @ {:#x}\n",
            img.recovery_dtbo.len(),
            d.recovery_dtbo_offset
        ));
    }
    if !img.dtb.is_empty() {
        s.push_str(&format!("dtb: {} bytes\n", img.dtb.len()));
    }
    if !img.signature.is_empty() {
        s.push_str(&format!("boot_signature: {} bytes\n", img.signature.len()));
    }
    for (rd, data) in &img.vendor_ramdisks {
        s.push_str(&format!(
            "vendor_ramdisk[{}]: type={} {} bytes\n",
            rd.name,
            rd.ramdisk_type,
            data.len()
        ));
    }
    if !img.bootconfig.is_empty() {
        s.push_str(&format!("bootconfig: {} bytes\n", img.bootconfig.len()));
    }
    if let Some(f) = img.avb_footer {
        s.push_str(&format!(
            "avb_footer: original_image_size={} vbmeta_offset={} vbmeta_size={}\n",
            f.original_image_size, f.vbmeta_offset, f.vbmeta_size
        ));
    } else {
        s.push_str("avb_footer: none\n");
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_boot(v: u32, page: u32) -> Vec<u8> {
        let d = BootDesc {
            header_version: v,
            page_size: page,
            cmdline: "console=ttyMSM0 androidboot.hardware=qcom".into(),
            name: "test".into(),
            ..Default::default()
        };
        let img = BootImage {
            desc: d,
            kernel: vec![0xAB; 5000],
            ramdisk: vec![0xCD; 3000],
            second: vec![],
            recovery_dtbo: if v >= 1 && v < 3 { vec![0x11; 100] } else { vec![] },
            dtb: if v == 2 { vec![0x22; 200] } else { vec![] },
            signature: if v >= 4 { vec![0x33; 64] } else { vec![] },
            vendor_ramdisks: vec![],
            bootconfig: vec![],
            avb_footer: None,
        };
        img.build().unwrap()
    }

    #[test]
    fn boot_roundtrip_all_versions() {
        for v in 0..=4u32 {
            let page = if v >= 3 { 4096 } else { 2048 };
            let bytes = mk_boot(v, page);
            let img = BootImage::parse(&bytes).unwrap();
            assert_eq!(img.desc.header_version, v);
            assert_eq!(img.kernel.len(), 5000, "v{}", v);
            assert_eq!(img.ramdisk.len(), 3000, "v{}", v);
            let again = img.build().unwrap();
            let img2 = BootImage::parse(&again).unwrap();
            assert_eq!(img2.kernel, img.kernel);
            assert_eq!(img2.ramdisk, img.ramdisk);
            if v >= 1 && v < 3 {
                assert_eq!(img.recovery_dtbo.len(), 100);
            }
            if v == 2 {
                assert_eq!(img.dtb.len(), 200);
            }
        }
    }

    #[test]
    fn vendor_boot_v4_roundtrip() {
        let d = BootDesc {
            kind: "vendor_boot".into(),
            header_version: 4,
            page_size: 4096,
            cmdline: "androidboot.vendor=1".into(),
            vendor_ramdisks: vec![VendorRamdiskDesc {
                name: "platform".into(),
                ramdisk_type: 1,
                size: 0,
                offset: 0,
                board_id: Vec::new(),
            }],
            ..Default::default()
        };
        let img = BootImage {
            desc: d,
            kernel: vec![],
            ramdisk: vec![0x77; 1000],
            second: vec![],
            recovery_dtbo: vec![],
            dtb: vec![0x88; 300],
            signature: vec![],
            vendor_ramdisks: vec![(
                VendorRamdiskDesc {
                    name: "platform".into(),
                    ramdisk_type: 1,
                    size: 1000,
                    offset: 0,
                    board_id: Vec::new(),
                },
                vec![0x77; 1000],
            )],
            bootconfig: b"androidboot.foo=bar\n".to_vec(),
            avb_footer: None,
        };
        let bytes = img.build().unwrap();
        let back = BootImage::parse(&bytes).unwrap();
        assert_eq!(back.desc.kind, "vendor_boot");
        assert_eq!(back.dtb.len(), 300);
        assert_eq!(back.bootconfig, b"androidboot.foo=bar\n");
    }

    #[test]
    fn avb_footer_detect() {
        let mut b = mk_boot(2, 2048);
        b.resize(align_up(b.len() as u64, 4096) as usize + 64, 0);
        let n = b.len();
        b[n - 64..n - 60].copy_from_slice(AVB_FOOTER_MAGIC);
        put_u64(&mut b, n - 64 + 12, 12345);
        let f = parse_avb_footer(&b).unwrap();
        assert_eq!(f.original_image_size, 12345);
    }
}
