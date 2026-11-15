"""Pure-userspace ext4 read/build, driven by text permissions."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Optional

from . import _proc, perm
from ._proc import PathLike
from .perm import MetaDb
from .types import Ext4Info

__all__ = ["info", "ls", "extract", "build", "Ext4Info"]


def info(img: PathLike) -> Ext4Info:
    """Superblock summary (JSON backed)."""
    return Ext4Info.from_dict(_proc.run_json(["ext4", "info", img]))


def ls(img: PathLike, limit: int = 500) -> str:
    """Text tree listing, same output as ``aimg ext4 ls``."""
    return _proc.run(["ext4", "ls", img, "--limit", str(limit)])


def extract(
    img: PathLike,
    out: PathLike,
    meta: Optional[PathLike] = None,
    file_contexts: Optional[PathLike] = None,
) -> MetaDb:
    """Extract the filesystem to ``out`` and return the parsed ``meta.txt``.

    ``meta`` overrides where the metadata manifest is written (defaults to
    ``<out>/meta.txt``).
    """
    args = ["ext4", "extract", img, "--out", out]
    if meta is not None:
        args += ["--meta", meta]
    if file_contexts is not None:
        args += ["--file-contexts", file_contexts]
    _proc.run(args)
    meta_path = Path(meta) if meta is not None else Path(out) / "meta.txt"
    return MetaDb.load(meta_path)


def build(
    directory: PathLike,
    out: PathLike,
    meta: Optional[PathLike] = None,
    fs_config: Optional[PathLike] = None,
    file_contexts: Optional[PathLike] = None,
    size: Optional[object] = None,
    block_size: int = 4096,
    label: str = "aimg",
) -> None:
    """Build an ext4 image from a directory tree.

    When ``meta`` is omitted it is generated on the fly from ``fs_config`` /
    ``file_contexts`` (the text permission model), never from NTFS ACLs.
    """
    args = ["ext4", "build", "--dir", directory, "--out", out]
    if meta is not None:
        args += ["--meta", meta]
    if fs_config is not None:
        args += ["--fs-config", fs_config]
    if file_contexts is not None:
        args += ["--file-contexts", file_contexts]
    if size is not None:
        args += ["--size", str(size)]
    args += ["--block-size", str(block_size), "--label", label]
    _proc.run(args)
    if not os.path.exists(out):
        raise _proc.AimgError(["ext4", "build"], 0, "", f"output missing: {out}")
