from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Sequence, Tuple, Union

from ._proc import PathLike
from .types import SuperInfo

__all__: List[str]


@dataclass(frozen=True)
class PartitionImage:
    name: str
    image: str
    group: str = ...


def dump(path: PathLike, slot: int = ...) -> SuperInfo: ...


def unpack(
    path: PathLike,
    out: PathLike,
    slot: int = ...,
    keep_suffix: bool = ...,
    sparse: bool = ...,
    sparse_block: int = ...,
) -> List[Path]: ...


def make(
    out: PathLike,
    partitions: Sequence[Union[PartitionImage, Tuple[str, PathLike]]],
    size: Optional[Union[int, str]] = ...,
    metadata_max_size: int = ...,
    metadata_slots: int = ...,
    alignment: int = ...,
    sparse: bool = ...,
    sparse_block: int = ...,
    minor_version: int = ...,
) -> SuperInfo: ...
