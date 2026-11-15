from __future__ import annotations

from pathlib import Path
from typing import List, Optional, Sequence, Union

from ._proc import PathLike
from .payload import BuildPartition
from .superimg import PartitionImage
from .types import SuperInfo

__all__: List[str]


def build_system_image(
    directory: PathLike,
    out: PathLike,
    meta: Optional[PathLike] = ...,
    fs_config: Optional[PathLike] = ...,
    file_contexts: Optional[PathLike] = ...,
    size: Optional[object] = ...,
    block_size: int = ...,
    label: str = ...,
    make_sparse: bool = ...,
) -> Path: ...


def pack_super(
    partitions: Sequence[Union[PartitionImage, tuple]],
    out: PathLike,
    size: Optional[object] = ...,
    sparse_out: bool = ...,
    metadata_slots: int = ...,
) -> SuperInfo: ...


def unpack_super(
    super_img: PathLike, out: PathLike, slot: int = ..., sparse_out: bool = ...
) -> List[Path]: ...


def unpack_ota(
    payload_bin: PathLike,
    out: PathLike,
    partitions: Optional[Sequence[str]] = ...,
    old: Optional[PathLike] = ...,
) -> List[Path]: ...


def pack_payload(
    images: Sequence[Union[BuildPartition, tuple]],
    out: PathLike,
    chunk_size: int = ...,
    compress: bool = ...,
    key: Optional[PathLike] = ...,
) -> None: ...
