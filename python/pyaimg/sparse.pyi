from __future__ import annotations

from pathlib import Path
from typing import List, Sequence, Union

from ._proc import PathLike
from .types import SparseInfo

__all__: List[str]


def info(path: PathLike) -> SparseInfo: ...
def is_sparse(path: PathLike) -> bool: ...
def to_raw(inputs: Union[PathLike, Sequence[PathLike]], out: PathLike) -> int: ...
def from_raw(raw: PathLike, out: PathLike, block_size: int = ...) -> None: ...
