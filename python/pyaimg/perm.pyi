from __future__ import annotations

from typing import Dict, Iterable, Iterator, List, Optional, Sequence, Tuple

from ._proc import PathLike
from .types import MetaEntry

DEFAULT_DIR_MODE: int
DEFAULT_FILE_MODE: int
DEFAULT_LINK_MODE: int
META_HEADER: str
META_COLUMNS: str


class FsConfig:
    def __init__(self, rules: Sequence[Tuple[str, int, int, int, int]]) -> None: ...

    @classmethod
    def parse(cls, text: str) -> "FsConfig": ...

    @classmethod
    def load(cls, path: PathLike) -> "FsConfig": ...

    def lookup(self, path: str, kind: str = ...) -> Tuple[int, int, int, int]: ...

    def __len__(self) -> int: ...


class FileContexts:
    def __init__(self, rules: Sequence[Tuple[str, str, int, int]]) -> None: ...

    @classmethod
    def parse(cls, text: str) -> "FileContexts": ...

    @classmethod
    def load(cls, path: PathLike) -> "FileContexts": ...

    def lookup(self, path: str, kind: str = ...) -> str: ...

    def __len__(self) -> int: ...


class MetaDb:
    entries: List[MetaEntry]

    def __init__(self, entries: Optional[Iterable[MetaEntry]] = ...) -> None: ...

    @classmethod
    def load(cls, path: PathLike) -> "MetaDb": ...

    @classmethod
    def parse(cls, text: str) -> "MetaDb": ...

    def save(self, path: PathLike) -> None: ...

    def get(self, path: str) -> Optional[MetaEntry]: ...

    def filter(self, prefix: str) -> List[MetaEntry]: ...

    def __iter__(self) -> Iterator[MetaEntry]: ...

    def __len__(self) -> int: ...


def export(
    directory: PathLike,
    out: PathLike,
    fs_config: Optional[PathLike] = ...,
    file_contexts: Optional[PathLike] = ...,
) -> MetaDb: ...


def apply(meta: PathLike, directory: PathLike) -> int: ...


def show(meta: PathLike, path: str) -> str: ...
