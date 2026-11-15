from __future__ import annotations

from typing import List, Tuple

from ._proc import PathLike
from .types import TransferInfo

__all__: List[str]
BLOCK_SIZE: int


def info(transfer: PathLike) -> TransferInfo: ...
def merge(transfer: PathLike, dat: PathLike, out: PathLike) -> int: ...
def split(
    img: PathLike, out: PathLike, prefix: str = ..., version: int = ...
) -> Tuple[int, int]: ...
def patch(
    base: PathLike,
    transfer: PathLike,
    dat: PathLike,
    patch_dat: PathLike,
    out: PathLike,
) -> None: ...
def brotli_decompress(src: PathLike, out: PathLike) -> int: ...
