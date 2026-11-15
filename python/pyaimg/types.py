"""Value types returned by the typed API."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import List, Optional, Sequence, Tuple


def _b(v: Optional[Sequence[int]]) -> bytes:
    return bytes(v or ())


# ---------------------------------------------------------------------------
# super.img
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SuperGeometry:
    metadata_max_size: int
    metadata_slot_count: int
    logical_block_size: int


@dataclass(frozen=True)
class SuperPartition:
    name: str
    attributes: int
    first_extent_index: int
    num_extents: int
    group_index: int

    @property
    def readonly(self) -> bool:
        return bool(self.attributes & 1)


@dataclass(frozen=True)
class SuperExtent:
    num_sectors: int
    target_type: int
    target_data: int
    target_source: int

    @property
    def size(self) -> int:
        return self.num_sectors * 512


@dataclass(frozen=True)
class SuperGroup:
    name: str
    flags: int
    maximum_size: int


@dataclass(frozen=True)
class SuperBlockDevice:
    first_logical_sector: int
    alignment: int
    alignment_offset: int
    size: int
    partition_name: str
    flags: int


@dataclass(frozen=True)
class SuperInfo:
    major: int
    minor: int
    header_flags: int
    geometry: SuperGeometry
    partitions: Tuple[SuperPartition, ...]
    extents: Tuple[SuperExtent, ...]
    groups: Tuple[SuperGroup, ...]
    block_devices: Tuple[SuperBlockDevice, ...]

    @classmethod
    def from_dict(cls, d: dict) -> "SuperInfo":
        g = d.get("geometry") or {}
        return cls(
            major=int(d.get("major", 0)),
            minor=int(d.get("minor", 0)),
            header_flags=int(d.get("header_flags", 0)),
            geometry=SuperGeometry(
                metadata_max_size=int(g.get("metadata_max_size", 0)),
                metadata_slot_count=int(g.get("metadata_slot_count", 0)),
                logical_block_size=int(g.get("logical_block_size", 0)),
            ),
            partitions=tuple(
                SuperPartition(
                    name=p["name"],
                    attributes=int(p.get("attributes", 0)),
                    first_extent_index=int(p.get("first_extent_index", 0)),
                    num_extents=int(p.get("num_extents", 0)),
                    group_index=int(p.get("group_index", 0)),
                )
                for p in d.get("partitions", [])
            ),
            extents=tuple(
                SuperExtent(
                    num_sectors=int(e.get("num_sectors", 0)),
                    target_type=int(e.get("target_type", 0)),
                    target_data=int(e.get("target_data", 0)),
                    target_source=int(e.get("target_source", 0)),
                )
                for e in d.get("extents", [])
            ),
            groups=tuple(
                SuperGroup(
                    name=g_.get("name", ""),
                    flags=int(g_.get("flags", 0)),
                    maximum_size=int(g_.get("maximum_size", 0)),
                )
                for g_ in d.get("groups", [])
            ),
            block_devices=tuple(
                SuperBlockDevice(
                    first_logical_sector=int(b.get("first_logical_sector", 0)),
                    alignment=int(b.get("alignment", 0)),
                    alignment_offset=int(b.get("alignment_offset", 0)),
                    size=int(b.get("size", 0)),
                    partition_name=b.get("partition_name", ""),
                    flags=int(b.get("flags", 0)),
                )
                for b in d.get("block_devices", [])
            ),
        )

    def partition(self, name: str) -> Optional[SuperPartition]:
        for p in self.partitions:
            if p.name == name:
                return p
        return None

    def size_of(self, name: str) -> int:
        p = self.partition(name)
        if p is None:
            return 0
        total = 0
        for i in range(p.num_extents):
            e = self.extents[p.first_extent_index + i]
            total += e.num_sectors
        return total * 512

    def group_of(self, name: str) -> Optional[str]:
        p = self.partition(name)
        if p is None or p.group_index >= len(self.groups):
            return None
        return self.groups[p.group_index].name


# ---------------------------------------------------------------------------
# payload.bin
# ---------------------------------------------------------------------------

OP_NAMES = {
    0: "REPLACE",
    1: "REPLACE_BZ",
    2: "MOVE",
    3: "BSDIFF",
    4: "SOURCE_COPY",
    5: "SOURCE_BSDIFF",
    6: "ZERO",
    7: "DISCARD",
    8: "REPLACE_XZ",
    10: "BROTLI_BSDIFF",
}


@dataclass(frozen=True)
class PayloadExtent:
    start_block: int
    num_blocks: int

    @property
    def size(self) -> int:
        return self.num_blocks * 4096


@dataclass(frozen=True)
class PayloadOp:
    type: int
    data_offset: int
    data_length: int
    dst_extents: Tuple[PayloadExtent, ...]
    src_extents: Tuple[PayloadExtent, ...]
    dst_length: int
    data_sha256: bytes
    src_sha256: bytes

    @property
    def type_name(self) -> str:
        return OP_NAMES.get(self.type, f"OP_{self.type}")


@dataclass(frozen=True)
class PayloadPartition:
    name: str
    size: int
    ops: Tuple[PayloadOp, ...]
    filesystem_type: str
    sha256: Optional[bytes]

    @property
    def op_types(self) -> Tuple[str, ...]:
        return tuple(o.type_name for o in self.ops)


@dataclass(frozen=True)
class PayloadInfo:
    version: int
    block_size: int
    minor_version: int
    max_timestamp: int
    partial_update: bool
    manifest_size: int
    metadata_signature_size: int
    partitions: Tuple[PayloadPartition, ...]

    @classmethod
    def from_dict(cls, d: dict) -> "PayloadInfo":
        parts: List[PayloadPartition] = []
        for p in d.get("partitions", []):
            info = p.get("new_info") or {}
            ops = tuple(
                PayloadOp(
                    type=int(o.get("ty", 0)),
                    data_offset=int(o.get("data_offset", 0)),
                    data_length=int(o.get("data_length", 0)),
                    dst_extents=tuple(
                        PayloadExtent(
                            start_block=int(e.get("start_block", 0)),
                            num_blocks=int(e.get("num_blocks", 0)),
                        )
                        for e in o.get("dst_extents", [])
                    ),
                    src_extents=tuple(
                        PayloadExtent(
                            start_block=int(e.get("start_block", 0)),
                            num_blocks=int(e.get("num_blocks", 0)),
                        )
                        for e in o.get("src_extents", [])
                    ),
                    dst_length=int(o.get("dst_length", 0)),
                    data_sha256=_b(o.get("data_sha256")),
                    src_sha256=_b(o.get("src_sha256")),
                )
                for o in p.get("ops", [])
            )
            parts.append(
                PayloadPartition(
                    name=p.get("name", ""),
                    size=int(info.get("size", 0)),
                    ops=ops,
                    filesystem_type=p.get("filesystem_type", ""),
                    sha256=_b(info.get("hash")) or None,
                )
            )
        return cls(
            version=int(d.get("version", 0)),
            block_size=int(d.get("block_size", 4096)),
            minor_version=int(d.get("minor_version", 0)),
            max_timestamp=int(d.get("max_timestamp", 0)),
            partial_update=bool(d.get("partial_update", False)),
            manifest_size=int(d.get("manifest_size", 0)),
            metadata_signature_size=int(d.get("metadata_signature_size", 0)),
            partitions=tuple(parts),
        )

    def partition(self, name: str) -> Optional[PayloadPartition]:
        for p in self.partitions:
            if p.name == name:
                return p
        return None


# ---------------------------------------------------------------------------
# ext4 / boot / meta
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Ext4Info:
    block_size: int
    blocks_count: int
    inodes_count: int
    inode_size: int
    first_data_block: int
    blocks_per_group: int
    inodes_per_group: int
    free_blocks: int
    free_inodes: int
    group_count: int
    feature_compat: int
    feature_incompat: int
    feature_ro_compat: int
    volume_name: str
    uuid: str

    @classmethod
    def from_dict(cls, d: dict) -> "Ext4Info":
        return cls(**{k: d.get(k) for k in cls.__dataclass_fields__})  # type: ignore[arg-type]


@dataclass(frozen=True)
class AvbFooter:
    version_major: int
    version_minor: int
    original_image_size: int
    vbmeta_offset: int
    vbmeta_size: int


@dataclass(frozen=True)
class BootInfo:
    kind: str
    header_version: int
    page_size: int
    os_version: int
    cmdline: str
    kernel_size: int
    ramdisk_size: int
    second_size: int
    recovery_dtbo_size: int
    dtb_size: int
    signature_size: int
    avb: Optional[AvbFooter]

    @classmethod
    def from_dict(cls, d: dict) -> "BootInfo":
        avb = d.get("avb_footer")
        return cls(
            kind=d.get("kind", ""),
            header_version=int(d.get("header_version", 0)),
            page_size=int(d.get("page_size", 0)),
            os_version=int(d.get("os_version", 0)),
            cmdline=d.get("cmdline", ""),
            kernel_size=int(d.get("kernel_size", 0)),
            ramdisk_size=int(d.get("ramdisk_size", 0)),
            second_size=int(d.get("second_size", 0)),
            recovery_dtbo_size=int(d.get("recovery_dtbo_size", 0)),
            dtb_size=int(d.get("dtb_size", 0)),
            signature_size=int(d.get("signature_size", 0)),
            avb=AvbFooter(
                version_major=int(avb.get("version_major", 0)),
                version_minor=int(avb.get("version_minor", 0)),
                original_image_size=int(avb.get("original_image_size", 0)),
                vbmeta_offset=int(avb.get("vbmeta_offset", 0)),
                vbmeta_size=int(avb.get("vbmeta_size", 0)),
            )
            if avb
            else None,
        )


@dataclass(frozen=True)
class MetaEntry:
    """One row of ``meta.txt`` - the authoritative permission record."""

    kind: str  # "D" directory, "F" file, "L" symlink
    path: str
    uid: int
    gid: int
    mode: int
    context: str
    caps: int
    size: int
    mtime: int
    link: Optional[str] = None
    ino: Optional[int] = None

    @classmethod
    def from_tsv(cls, line: str) -> "MetaEntry":
        f = line.split("\t")
        if len(f) < 8:
            raise ValueError(f"bad meta.txt row: {line!r}")
        link = f[9] if len(f) > 9 and f[9] else None
        return cls(
            kind=f[0],
            path=f[1],
            uid=int(f[2]),
            gid=int(f[3]),
            mode=int(f[4], 8),
            context=f[5],
            caps=int(f[6], 16),
            size=int(f[7]),
            mtime=int(f[8]),
            link=link,
            ino=int(f[10]) if len(f) > 10 and f[10] else None,
        )

    def to_tsv(self) -> str:
        return "\t".join(
            [
                self.kind,
                self.path,
                str(self.uid),
                str(self.gid),
                f"{self.mode:04o}",
                self.context,
                f"0x{self.caps:x}",
                str(self.size),
                str(self.mtime),
                self.link or "",
                str(self.ino) if self.ino is not None else "",
            ]
        )

    @property
    def mode_octal(self) -> str:
        return f"{self.mode:04o}"

    @property
    def is_dir(self) -> bool:
        return self.kind == "D"

    @property
    def is_file(self) -> bool:
        return self.kind == "F"

    @property
    def is_symlink(self) -> bool:
        return self.kind == "L"


@dataclass(frozen=True)
class TransferCommand:
    """One transfer.list command, ``name`` is new/zero/erase/copy/bsdiff."""

    name: str
    src: Tuple[Tuple[int, int], ...] = ()
    dst: Tuple[Tuple[int, int], ...] = ()

    @property
    def blocks(self) -> int:
        return sum(b - a for a, b in (self.dst or self.src))


@dataclass(frozen=True)
class TransferInfo:
    version: int
    total_blocks: int
    commands: Tuple[TransferCommand, ...]

    @classmethod
    def from_dict(cls, d: dict) -> "TransferInfo":
        cmds: List[TransferCommand] = []
        for raw in d.get("cmds", []):
            (name, payload), = raw.items()
            if name in ("New", "Zero", "Erase"):
                cmds.append(
                    TransferCommand(
                        name=name.lower(), dst=tuple((int(a), int(b)) for a, b in payload)
                    )
                )
            elif name in ("Copy", "Move"):
                cmds.append(
                    TransferCommand(
                        name=name.lower(),
                        src=tuple((int(a), int(b)) for a, b in payload["src"]),
                        dst=tuple((int(a), int(b)) for a, b in payload["dst"]),
                    )
                )
            elif name == "Bsdiff":
                cmds.append(
                    TransferCommand(
                        name="bsdiff",
                        src=tuple((int(a), int(b)) for a, b in payload["src"]),
                        dst=tuple((int(a), int(b)) for a, b in payload["dst"]),
                    )
                )
            elif name == "Stash":
                cmds.append(
                    TransferCommand(
                        name="stash",
                        src=tuple((int(a), int(b)) for a, b in payload.get("ranges", [])),
                        dst=(),
                    )
                )
            elif name == "Free":
                cmds.append(TransferCommand(name="free", src=(), dst=()))
        return cls(
            version=int(d.get("version", 0)),
            total_blocks=int(d.get("total_blocks", 0)),
            commands=tuple(cmds),
        )


# ---------------------------------------------------------------------------
# sparse images
# ---------------------------------------------------------------------------

SPARSE_MAGIC = 0xED26FF3A
SPARSE_CHUNK_NAMES = {
    0xCAC1: "RAW",
    0xCAC2: "FILL",
    0xCAC3: "DONT_CARE",
    0xCAC4: "CRC32",
}


@dataclass(frozen=True)
class SparseChunk:
    type: int
    chunk_sz: int
    total_sz: int
    offset: int

    @property
    def type_name(self) -> str:
        return SPARSE_CHUNK_NAMES.get(self.type, f"0x{self.type:x}")


@dataclass(frozen=True)
class SparseInfo:
    major: int
    minor: int
    block_size: int
    total_blocks: int
    total_chunks: int
    checksum: int
    chunks: Tuple[SparseChunk, ...]

    @property
    def raw_size(self) -> int:
        return self.total_blocks * self.block_size

    @classmethod
    def from_file(cls, path: "PathLike") -> "SparseInfo":  # type: ignore[name-defined]
        import struct

        with open(path, "rb") as f:
            head = f.read(28)
            if len(head) < 28:
                raise ValueError(f"{path}: file too small to be a sparse image")
            magic, major, minor, file_hdr_sz, chunk_hdr_sz, blk_sz, total_blks, total_chunks, csum = struct.unpack(
                "<IHHHHIIII", head
            )
            if magic != SPARSE_MAGIC:
                raise ValueError(f"{path}: not an Android sparse image")
            chunks: List[SparseChunk] = []
            off = file_hdr_sz
            for _ in range(total_chunks):
                f.seek(off)
                ch = f.read(12)
                ctype, _res, chunk_sz, total_sz = struct.unpack("<HHII", ch)
                chunks.append(
                    SparseChunk(
                        type=ctype, chunk_sz=chunk_sz, total_sz=total_sz, offset=off
                    )
                )
                off += total_sz
        return cls(
            major=major,
            minor=minor,
            block_size=blk_sz,
            total_blocks=total_blks,
            total_chunks=total_chunks,
            checksum=csum,
            chunks=tuple(chunks),
        )
