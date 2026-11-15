"""High level helpers that chain the individual stages together."""

from __future__ import annotations

from pathlib import Path
from typing import List, Optional, Sequence, Union

from . import ext4, payload, sparse, superimg
from ._proc import PathLike
from .payload import BuildPartition
from .superimg import PartitionImage
from .types import SuperInfo

__all__ = ["build_system_image", "pack_super", "unpack_super", "unpack_ota"]


def build_system_image(
    directory: PathLike,
    out: PathLike,
    meta: Optional[PathLike] = None,
    fs_config: Optional[PathLike] = None,
    file_contexts: Optional[PathLike] = None,
    size: Optional[object] = None,
    block_size: int = 4096,
    label: str = "system",
    make_sparse: bool = False,
) -> Path:
    """Directory tree -> ext4 image (optionally sparsified)."""
    out_path = Path(out)
    ext4.build(
        directory,
        out_path,
        meta=meta,
        fs_config=fs_config,
        file_contexts=file_contexts,
        size=size,
        block_size=block_size,
        label=label,
    )
    if make_sparse:
        sp = out_path.with_suffix(".simg")
        sparse.from_raw(out_path, sp, block_size)
        return sp
    return out_path


def pack_super(
    partitions: Sequence[Union[PartitionImage, tuple]],
    out: PathLike,
    size: Optional[object] = None,
    sparse_out: bool = False,
    metadata_slots: int = 2,
) -> SuperInfo:
    """Wrap partition images into a dynamic-partition ``super.img``."""
    return superimg.make(
        out,
        partitions,
        size=size,
        metadata_slots=metadata_slots,
        sparse=sparse_out,
    )


def unpack_super(
    super_img: PathLike, out: PathLike, slot: int = 0, sparse_out: bool = False
) -> List[Path]:
    """Extract logical partitions from ``super.img``."""
    return superimg.unpack(super_img, out, slot=slot, sparse=sparse_out)


def unpack_ota(
    payload_bin: PathLike,
    out: PathLike,
    partitions: Optional[Sequence[str]] = None,
    old: Optional[PathLike] = None,
) -> List[Path]:
    """Extract partition images from ``payload.bin`` (or an OTA zip)."""
    return payload.extract(payload_bin, out, partitions=partitions, old=old)


def pack_payload(
    images: Sequence[Union[BuildPartition, tuple]],
    out: PathLike,
    chunk_size: int = 2 * 1024 * 1024,
    compress: bool = False,
    key: Optional[PathLike] = None,
) -> None:
    """Generate a full ``payload.bin`` from partition images."""
    payload.make(out, images, chunk_size=chunk_size, compress=compress, key=key)
