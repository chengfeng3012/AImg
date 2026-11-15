"""Android sparse images (``simg2img`` / ``img2simg``)."""

from __future__ import annotations

import os
from pathlib import Path
from typing import List, Sequence, Union

from . import _proc
from ._proc import PathLike
from .types import SparseInfo

__all__ = ["info", "is_sparse", "to_raw", "from_raw", "SparseInfo"]


def info(path: PathLike) -> SparseInfo:
    """Read the sparse header/chunk table directly (no subprocess)."""
    return SparseInfo.from_file(path)


def is_sparse(path: PathLike) -> bool:
    with open(path, "rb") as f:
        head = f.read(4)
    return len(head) == 4 and int.from_bytes(head, "little") == 0xED26FF3A


def to_raw(inputs: Union[PathLike, Sequence[PathLike]], out: PathLike) -> int:
    """Expand one or more sparse images into a single raw image.

    Multiple inputs are concatenated in order, which is what
    ``system.img_sparsechunk.*`` needs.
    """
    if isinstance(inputs, (str, os.PathLike)):
        inputs = [inputs]
    args = ["sparse", "to-raw", *[str(i) for i in inputs], "--out", out]
    _proc.run(args)
    return os.path.getsize(out)


def from_raw(raw: PathLike, out: PathLike, block_size: int = 4096) -> None:
    """Convert a raw image into a sparse image."""
    _proc.run(
        ["sparse", "from-raw", raw, "--out", out, "--block-size", str(block_size)]
    )
