"""``super.img`` dynamic partitions (lpdump / lpunpack / lpmake)."""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Sequence, Tuple, Union

from . import _proc
from ._proc import PathLike
from .types import SuperInfo

__all__ = ["PartitionImage", "dump", "unpack", "make", "SuperInfo"]


@dataclass(frozen=True)
class PartitionImage:
    """One logical partition to place into a rebuilt ``super.img``."""

    name: str
    image: str
    group: str = "default"


def _spec(p: Union[PartitionImage, Tuple[str, PathLike]]) -> str:
    if isinstance(p, PartitionImage):
        return f"{p.name}={p.image}:{p.group}"
    name, image = p
    return f"{name}={image}"


def dump(path: PathLike, slot: int = 0) -> SuperInfo:
    """Parse LP metadata (``lpdump`` equivalent)."""
    return SuperInfo.from_dict(_proc.run_json(["super", "dump", path, "--slot", str(slot)]))


def unpack(
    path: PathLike,
    out: PathLike,
    slot: int = 0,
    keep_suffix: bool = False,
    sparse: bool = False,
    sparse_block: int = 4096,
) -> List[Path]:
    """Extract logical partitions (``lpunpack`` equivalent)."""
    args = ["super", "unpack", path, "--out", out, "--slot", str(slot)]
    if keep_suffix:
        args.append("--keep-suffix")
    if sparse:
        args += ["--sparse", "--sparse-block", str(sparse_block)]
    _proc.run(args)
    outdir = Path(out)
    return sorted(p for p in outdir.iterdir() if p.is_file())


def make(
    out: PathLike,
    partitions: Sequence[Union[PartitionImage, Tuple[str, PathLike]]],
    size: Optional[Union[int, str]] = None,
    metadata_max_size: int = 65536,
    metadata_slots: int = 2,
    alignment: int = 1 << 20,
    sparse: bool = False,
    sparse_block: int = 4096,
    minor_version: int = 2,
) -> SuperInfo:
    """Build ``super.img`` (``lpmake`` equivalent) and return its metadata."""
    args: List[str] = ["super", "make", "--out", out]
    for p in partitions:
        args += ["--partition", _spec(p)]
    if size is not None:
        args += ["--size", str(size)]
    args += [
        "--metadata-max-size",
        str(metadata_max_size),
        "--metadata-slots",
        str(metadata_slots),
        "--alignment",
        str(alignment),
        "--minor-version",
        str(minor_version),
    ]
    if sparse:
        args += ["--sparse", "--sparse-block", str(sparse_block)]
    _proc.run(args)
    return dump(out)
