"""``boot.img`` / ``recovery.img`` / ``vendor_boot.img`` (header v0..v4)."""

from __future__ import annotations

from pathlib import Path

from . import _proc
from ._proc import PathLike
from .types import BootInfo

__all__ = ["info", "unpack", "pack", "BootInfo"]


def info(path: PathLike) -> BootInfo:
    """Parse the boot header (JSON backed)."""
    return BootInfo.from_dict(_proc.run_json(["boot", "info", path]))


def unpack(path: PathLike, out: PathLike) -> BootInfo:
    """Extract kernel/ramdisk/dtb/... plus ``boot.json`` into ``out``."""
    _proc.run(["boot", "unpack", path, "--out", out])
    return info(path)


def pack(directory: PathLike, out: PathLike) -> None:
    """Repack a directory produced by :func:`unpack`."""
    _proc.run(["boot", "pack", directory, "--out", out])
