"""Old style ``.dat`` block images (transfer.list / new.dat / patch.dat)."""

from __future__ import annotations

import math
import os
from pathlib import Path
from typing import Tuple

from . import _proc
from ._proc import PathLike
from .types import TransferInfo

__all__ = ["info", "merge", "split", "patch", "brotli_decompress", "TransferInfo"]

BLOCK_SIZE = 4096


def info(transfer: PathLike) -> TransferInfo:
    """Parse ``*.transfer.list`` (JSON backed)."""
    return TransferInfo.from_dict(_proc.run_json(["dat", "info", transfer]))


def merge(transfer: PathLike, dat: PathLike, out: PathLike) -> int:
    """``sdat2img``: transfer.list + new.dat -> raw image; returns image size."""
    _proc.run(["dat", "merge", transfer, dat, "--out", out])
    return os.path.getsize(out)


def split(
    img: PathLike, out: PathLike, prefix: str = "system", version: int = 4
) -> Tuple[int, int]:
    """``img2sdat``: raw image -> ``<prefix>.new.dat`` + transfer.list."""
    _proc.run(
        [
            "dat",
            "split",
            img,
            "--out",
            out,
            "--prefix",
            prefix,
            "--version",
            str(version),
        ]
    )
    blocks = math.ceil(os.path.getsize(img) / BLOCK_SIZE)
    return blocks, blocks * BLOCK_SIZE


def patch(
    base: PathLike,
    transfer: PathLike,
    dat: PathLike,
    patch_dat: PathLike,
    out: PathLike,
) -> None:
    """Apply ``system.patch.dat`` on top of a base image (differential OTA)."""
    _proc.run(["dat", "patch", base, transfer, dat, patch_dat, "--out", out])


def brotli_decompress(src: PathLike, out: PathLike) -> int:
    """Expand a ``system.new.dat.br`` into a sparse data stream."""
    _proc.run(["dat", "br", src, "--out", out])
    return os.path.getsize(out)
