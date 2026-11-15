from __future__ import annotations

from typing import List

from ._proc import PathLike
from .types import BootInfo

__all__: List[str]


def info(path: PathLike) -> BootInfo: ...
def unpack(path: PathLike, out: PathLike) -> BootInfo: ...
def pack(directory: PathLike, out: PathLike) -> None: ...
