from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Sequence, Tuple, Union

from ._proc import PathLike
from .types import PayloadInfo

__all__: List[str]


@dataclass(frozen=True)
class BuildPartition:
    name: str
    image: str


def info(path: PathLike) -> PayloadInfo: ...


def extract(
    path: PathLike,
    out: PathLike,
    partitions: Optional[Sequence[str]] = ...,
    old: Optional[PathLike] = ...,
    verify: bool = ...,
) -> List[Path]: ...


def make(
    out: PathLike,
    partitions: Sequence[Union[BuildPartition, Tuple[str, PathLike]]],
    chunk_size: int = ...,
    compress: bool = ...,
    key: Optional[PathLike] = ...,
    max_timestamp: int = ...,
) -> None: ...
