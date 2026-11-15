from __future__ import annotations

from typing import List, Optional

from ._proc import PathLike
from .perm import MetaDb
from .types import Ext4Info

__all__: List[str]


def info(img: PathLike) -> Ext4Info: ...
def ls(img: PathLike, limit: int = ...) -> str: ...


def extract(
    img: PathLike,
    out: PathLike,
    meta: Optional[PathLike] = ...,
    file_contexts: Optional[PathLike] = ...,
) -> MetaDb: ...


def build(
    directory: PathLike,
    out: PathLike,
    meta: Optional[PathLike] = ...,
    fs_config: Optional[PathLike] = ...,
    file_contexts: Optional[PathLike] = ...,
    size: Optional[object] = ...,
    block_size: int = ...,
    label: str = ...,
) -> None: ...
