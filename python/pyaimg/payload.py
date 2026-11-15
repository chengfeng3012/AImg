"""``payload.bin`` (A/B OTA) extraction and generation."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Sequence, Tuple, Union

from . import _proc
from ._proc import PathLike
from .types import PayloadInfo

__all__ = ["BuildPartition", "info", "extract", "make", "PayloadInfo"]


@dataclass(frozen=True)
class BuildPartition:
    """A partition image to embed into a generated payload."""

    name: str
    image: str


def _spec(p: Union[BuildPartition, Tuple[str, PathLike]]) -> str:
    if isinstance(p, BuildPartition):
        return f"{p.name}={p.image}"
    name, image = p
    return f"{name}={image}"


def info(path: PathLike) -> PayloadInfo:
    """Parse the payload manifest (no extraction)."""
    return PayloadInfo.from_dict(_proc.run_json(["payload", "info", path]))


def extract(
    path: PathLike,
    out: PathLike,
    partitions: Optional[Sequence[str]] = None,
    old: Optional[PathLike] = None,
    verify: bool = True,
) -> List[Path]:
    """Extract partition images; ``old`` is required for delta payloads."""
    args = ["payload", "extract", path, "--out", out]
    if partitions:
        args += ["--partitions", ",".join(partitions)]
    if old is not None:
        args += ["--old", old]
    args += ["--verify", "true" if verify else "false"]
    _proc.run(args)
    outdir = Path(out)
    return sorted(p for p in outdir.iterdir() if p.is_file())


def make(
    out: PathLike,
    partitions: Sequence[Union[BuildPartition, Tuple[str, PathLike]]],
    chunk_size: int = 2 * 1024 * 1024,
    compress: bool = False,
    key: Optional[PathLike] = None,
    max_timestamp: int = 0,
) -> None:
    """Build a full ``payload.bin`` (minor_version 0)."""
    args: List[str] = ["payload", "make", "--out", out]
    for p in partitions:
        args += ["--partition", _spec(p)]
    args += ["--chunk-size", str(chunk_size), "--max-timestamp", str(max_timestamp)]
    if compress:
        args.append("--compress")
    if key is not None:
        args += ["--key", key]
    _proc.run(args)
