"""pytest suite for the pyaimg typed API (runs against the real binary)."""

from __future__ import annotations

import struct
from pathlib import Path

import pytest

import pyaimg as ai


def _tree(root: Path) -> None:
    (root / "system/bin").mkdir(parents=True)
    (root / "system/etc").mkdir(parents=True)
    (root / "system/bin/sh").write_bytes(b"\x7fELF fake sh")
    (root / "system/bin/blob").write_bytes(bytes(range(256)) * 64)
    (root / "system/etc/build.prop").write_bytes(b"ro.product.name=aimg\n")


def _configs(tmp: Path) -> tuple[Path, Path]:
    fsc = tmp / "fs_config.txt"
    fc = tmp / "file_contexts"
    fsc.write_text(
        "/system 0 0 0755\n"
        "/system/bin 0 2000 0755\n"
        "/system/bin/sh 0 2000 0755\n"
        "/system/bin/blob 0 2000 0755\n"
        "/system/etc 0 0 0755\n"
        "/system/etc/build.prop 0 0 0644\n"
    )
    fc.write_text(
        "/system(/.*)? u:object_r:system_file:s0\n"
        "/system/bin/sh u:object_r:shell_exec:s0\n"
    )
    return fsc, fc


def _boot_image(path: Path) -> None:
    page = 2048
    kernel = b"K" * 4096
    ramdisk = b"R" * 2048
    hdr = bytearray(page)
    hdr[0:8] = b"ANDROID!"
    struct.pack_into("<I", hdr, 8, len(kernel))
    struct.pack_into("<I", hdr, 12, 0x10008000)
    struct.pack_into("<I", hdr, 16, len(ramdisk))
    struct.pack_into("<I", hdr, 20, 0x11000000)
    struct.pack_into("<I", hdr, 24, 0)
    struct.pack_into("<I", hdr, 28, 0x10F00000)
    struct.pack_into("<I", hdr, 32, 0x10000100)
    struct.pack_into("<I", hdr, 36, page)
    struct.pack_into("<I", hdr, 40, 0)
    struct.pack_into("<I", hdr, 44, 0x0C000000)
    hdr[48:52] = b"test"
    cmd = b"console=ttyMSM0"
    hdr[64 : 64 + len(cmd)] = cmd
    path.write_bytes(bytes(hdr) + kernel + ramdisk)


def test_fs_config_and_contexts() -> None:
    cfg = ai.FsConfig.parse(
        "/system 0 0 0755\n/system/bin 0 2000 0755\n/system/bin/su 0 0 04755 capabilities=0x40\n"
    )
    # files inherit uid/gid from the nearest parent rule, mode falls back to 0644
    assert cfg.lookup("/system/bin/sh") == (0, 2000, 0o644, 0)
    assert cfg.lookup("/system/bin/su")[:3] == (0, 0, 0o4755)
    assert cfg.lookup("/system/bin/su")[3] == 0x40

    fc = ai.FileContexts.parse(
        "/system(/.*)? u:object_r:system_file:s0\n/system/bin/sh u:object_r:shell_exec:s0\n"
    )
    assert fc.lookup("/system/bin/sh") == "u:object_r:shell_exec:s0"
    assert fc.lookup("/system/etc/x") == "u:object_r:system_file:s0"
    assert fc.lookup("/data/x") == "u:object_r:unlabeled:s0"


def test_meta_db_roundtrip(tmp_path: Path) -> None:
    entry = ai.MetaEntry(
        kind="F",
        path="/system/bin/sh",
        uid=0,
        gid=2000,
        mode=0o755,
        context="u:object_r:shell_exec:s0",
        caps=0,
        size=3,
        mtime=1,
    )
    db = ai.MetaDb([entry])
    p = tmp_path / "meta.txt"
    db.save(p)
    back = ai.MetaDb.load(p)
    assert len(back) == 1
    got = back.get("/system/bin/sh")
    assert got is not None and got.mode_octal == "0755"
    assert got.context == "u:object_r:shell_exec:s0"
    assert got.is_file


def test_ext4_build_info_extract(tmp_path: Path) -> None:
    root = tmp_path / "root"
    _tree(root)
    fsc, fc = _configs(tmp_path)
    img = tmp_path / "system.img"

    ai.ext4.build(
        root, img, fs_config=fsc, file_contexts=fc, size="16M", label="system"
    )
    assert img.exists() and img.stat().st_size == 16 * 1024 * 1024

    info = ai.ext4.info(img)
    assert info.block_size == 4096
    assert info.volume_name == "system"

    listing = ai.ext4.ls(img)
    assert "system/" in listing

    meta = ai.ext4.extract(img, tmp_path / "out")
    ent = meta.get("/system/bin/sh")
    assert ent is not None
    assert (ent.uid, ent.gid, ent.mode) == (0, 2000, 0o755)
    assert ent.context == "u:object_r:shell_exec:s0"
    assert (tmp_path / "out/system/bin/sh").read_bytes() == b"\x7fELF fake sh"


def test_sparse_roundtrip(tmp_path: Path) -> None:
    raw = tmp_path / "a.raw"
    raw.write_bytes(b"\0" * 8192 + b"\xAB" * 4096 + b"\0" * 4096)
    sp = tmp_path / "a.simg"
    ai.sparse.from_raw(raw, sp)
    assert ai.sparse.is_sparse(sp)
    i = ai.sparse.info(sp)
    assert i.block_size == 4096 and i.total_blocks == 4
    back = tmp_path / "b.raw"
    assert ai.sparse.to_raw(sp, back) == 16384
    assert back.read_bytes() == raw.read_bytes()


def test_super_roundtrip(tmp_path: Path) -> None:
    img = tmp_path / "system.img"
    img.write_bytes(b"\x22" * 300_000)
    sup = tmp_path / "super.img"
    info = ai.superimg.make(
        sup, [ai.superimg.PartitionImage("system_a", str(img), "main")], size="8M"
    )
    assert info.partition("system_a") is not None
    # partition size is aligned up to the 4096-byte logical block
    assert 300_000 <= info.size_of("system_a") < 310_000
    assert info.group_of("system_a") == "main"

    out = tmp_path / "unpacked"
    files = ai.superimg.unpack(sup, out, slot=0)
    assert any(p.name == "system.img" for p in files)
    got = (out / "system.img").read_bytes()
    assert got[:300_000] == img.read_bytes()


def test_payload_roundtrip(tmp_path: Path) -> None:
    img = tmp_path / "boot.img"
    img.write_bytes(bytes(range(256)) * 400)
    payload = tmp_path / "payload.bin"
    ai.payload.make(payload, [ai.payload.BuildPartition("boot", str(img))])
    info = ai.payload.info(payload)
    part = info.partition("boot")
    assert part is not None and part.size == img.stat().st_size
    assert part.op_types[0] in ("REPLACE", "REPLACE_XZ")

    out = tmp_path / "payload_out"
    files = ai.payload.extract(payload, out)
    assert [p.name for p in files] == ["boot.img"]
    assert (out / "boot.img").read_bytes() == img.read_bytes()


def test_dat_roundtrip(tmp_path: Path) -> None:
    img = tmp_path / "system.img"
    img.write_bytes(bytes((i % 251) for i in range(20 * 4096)))
    blocks, size = ai.dat.split(img, tmp_path, prefix="system", version=4)
    assert blocks == 20 and size == 20 * 4096
    tl = tmp_path / "system.transfer.list"
    info = ai.dat.info(tl)
    assert info.version == 4 and info.total_blocks == 20

    merged = tmp_path / "merged.img"
    assert ai.dat.merge(tl, tmp_path / "system.new.dat", merged) == size
    assert merged.read_bytes() == img.read_bytes()


def test_boot_roundtrip(tmp_path: Path) -> None:
    src = tmp_path / "boot.img"
    _boot_image(src)
    info = ai.boot.info(src)
    assert info.kind == "boot" and info.header_version == 0
    assert info.kernel_size == 4096 and info.ramdisk_size == 2048

    d = tmp_path / "boot_dir"
    ai.boot.unpack(src, d)
    assert (d / "kernel").read_bytes() == b"K" * 4096
    ai.boot.pack(d, tmp_path / "new-boot.img")
    again = ai.boot.info(tmp_path / "new-boot.img")
    assert again.kernel_size == 4096


def test_pipeline_build_system_image(tmp_path: Path) -> None:
    root = tmp_path / "root"
    _tree(root)
    fsc, fc = _configs(tmp_path)
    out = ai.pipeline.build_system_image(
        root,
        tmp_path / "system.img",
        fs_config=fsc,
        file_contexts=fc,
        size="16M",
        make_sparse=True,
    )
    assert out.suffix == ".simg"
    assert ai.sparse.is_sparse(out)


def test_error_mapping(tmp_path: Path) -> None:
    with pytest.raises(ai.AimgError):
        ai.ext4.info(tmp_path / "missing.img")
