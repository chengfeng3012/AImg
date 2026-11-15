from __future__ import annotations

from dataclasses import dataclass
from typing import List, Optional, Sequence, Tuple


def _b(v: Optional[Sequence[int]]) -> bytes: ...


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
    def readonly(self) -> bool: ...


@dataclass(frozen=True)
class SuperExtent:
    num_sectors: int
    target_type: int
    target_data: int
    target_source: int

    @property
    def size(self) -> int: ...


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
    def from_dict(cls, d: dict) -> "SuperInfo": ...

    def partition(self, name: str) -> Optional[SuperPartition]: ...
    def size_of(self, name: str) -> int: ...
    def group_of(self, name: str) -> Optional[str]: ...


@dataclass(frozen=True)
class PayloadExtent:
    start_block: int
    num_blocks: int

    @property
    def size(self) -> int: ...


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
    def type_name(self) -> str: ...


@dataclass(frozen=True)
class PayloadPartition:
    name: str
    size: int
    ops: Tuple[PayloadOp, ...]
    filesystem_type: str
    sha256: Optional[bytes]

    @property
    def op_types(self) -> Tuple[str, ...]: ...


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
    def from_dict(cls, d: dict) -> "PayloadInfo": ...

    def partition(self, name: str) -> Optional[PayloadPartition]: ...


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
    def from_dict(cls, d: dict) -> "Ext4Info": ...


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
    def from_dict(cls, d: dict) -> "BootInfo": ...


@dataclass(frozen=True)
class MetaEntry:
    kind: str
    path: str
    uid: int
    gid: int
    mode: int
    context: str
    caps: int
    size: int
    mtime: int
    link: Optional[str] = ...
    ino: Optional[int] = ...

    @classmethod
    def from_tsv(cls, line: str) -> "MetaEntry": ...

    def to_tsv(self) -> str: ...

    @property
    def mode_octal(self) -> str: ...

    @property
    def is_dir(self) -> bool: ...

    @property
    def is_file(self) -> bool: ...

    @property
    def is_symlink(self) -> bool: ...


@dataclass(frozen=True)
class TransferCommand:
    name: str
    src: Tuple[Tuple[int, int], ...] = ...
    dst: Tuple[Tuple[int, int], ...] = ...

    @property
    def blocks(self) -> int: ...


@dataclass(frozen=True)
class TransferInfo:
    version: int
    total_blocks: int
    commands: Tuple[TransferCommand, ...]

    @classmethod
    def from_dict(cls, d: dict) -> "TransferInfo": ...


SPARSE_MAGIC: int
SPARSE_CHUNK_NAMES: dict


@dataclass(frozen=True)
class SparseChunk:
    type: int
    chunk_sz: int
    total_sz: int
    offset: int

    @property
    def type_name(self) -> str: ...


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
    def raw_size(self) -> int: ...

    @classmethod
    def from_file(cls, path: str) -> "SparseInfo": ...
