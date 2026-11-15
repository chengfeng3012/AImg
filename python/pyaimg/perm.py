"""Text-driven permission model: fs_config.txt / file_contexts / meta.txt.

Nothing here touches NTFS ACLs - uid, gid, mode and the SELinux context live in
plain text and are baked into the ext4 image by :func:`pyaimg.ext4.build`.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Dict, Iterable, Iterator, List, Optional, Sequence, Tuple

from . import _proc
from ._proc import PathLike
from .types import MetaEntry

DEFAULT_DIR_MODE = 0o755
DEFAULT_FILE_MODE = 0o644
DEFAULT_LINK_MODE = 0o777

META_HEADER = "# aimg-meta v1"
META_COLUMNS = "# kind\tpath\tuid\tgid\tmode\tcontext\tcaps\tsize\tmtime\tlink_target\tino"


def _norm(path: str) -> str:
    p = path.strip().replace("\\", "/")
    return p if p.startswith("/") else "/" + p


class FsConfig:
    """``fs_config.txt``: path -> uid/gid/mode/capabilities."""

    def __init__(self, rules: Sequence[Tuple[str, int, int, int, int]]) -> None:
        self._rules: Dict[str, Tuple[int, int, int, int]] = {}
        for path, uid, gid, mode, caps in rules:
            self._rules[_norm(path)] = (uid, gid, mode, caps)

    @classmethod
    def parse(cls, text: str) -> "FsConfig":
        rules = []
        for line in text.splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            f = line.split()
            if len(f) < 4:
                raise ValueError(f"bad fs_config line: {line!r}")
            caps = 0
            for extra in f[4:]:
                if extra.startswith("capabilities="):
                    caps = int(extra.split("=", 1)[1], 16)
            rules.append((f[0], int(f[1]), int(f[2]), int(f[3], 8), caps))
        return cls(rules)

    @classmethod
    def load(cls, path: PathLike) -> "FsConfig":
        return cls.parse(Path(path).read_text(encoding="utf-8"))

    def lookup(self, path: str, kind: str = "F") -> Tuple[int, int, int, int]:
        p = _norm(path)
        if p in self._rules:
            return self._rules[p]
        cur = p
        while "/" in cur:
            cur = cur.rsplit("/", 1)[0]
            if not cur:
                break
            if cur in self._rules:
                uid, gid, _mode, caps = self._rules[cur]
                mode = {"D": DEFAULT_DIR_MODE, "F": DEFAULT_FILE_MODE, "L": DEFAULT_LINK_MODE}[
                    kind
                ]
                return uid, gid, mode, caps
        mode = {"D": DEFAULT_DIR_MODE, "F": DEFAULT_FILE_MODE, "L": DEFAULT_LINK_MODE}[kind]
        return 0, 0, mode, 0

    def __len__(self) -> int:
        return len(self._rules)


class FileContexts:
    """``file_contexts``: regex -> SELinux context (longest literal prefix wins)."""

    def __init__(self, rules: Sequence[Tuple[str, str, int, int]]) -> None:
        self._rules = [
            (re.compile("^{}$".format(pattern)), spec, lit, idx)
            for pattern, spec, lit, idx in rules
        ]

    @classmethod
    def parse(cls, text: str) -> "FileContexts":
        rules = []
        for idx, line in enumerate(text.splitlines()):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            f = line.split()
            if not f:
                continue
            pat = f[0]
            spec = f[1] if len(f) > 1 else "u:object_r:unlabeled:s0"
            if len(f) > 1 and len(pat) == 2 and pat[0] == "-" and pat[1] in "dflcbps":
                pat, spec = f[1], (f[2] if len(f) > 2 else spec)
            if not pat.startswith("/"):
                pat = "/" + pat
            # everything before the first regex metacharacter counts as literal
            lit = len(re.match(r"[^.$^?*+\[\](){}|\\]*", pat).group(0))  # type: ignore[union-attr]
            rules.append((pat, spec, lit, idx))
        return cls(rules)

    @classmethod
    def load(cls, path: PathLike) -> "FileContexts":
        return cls.parse(Path(path).read_text(encoding="utf-8"))

    def lookup(self, path: str, kind: str = "F") -> str:
        p = _norm(path)
        best = None
        for rx, spec, lit, idx in self._rules:
            if not rx.match(p):
                continue
            if best is None or (lit, idx) > (best[2], best[3]):
                best = (rx, spec, lit, idx)
        return best[1] if best else "u:object_r:unlabeled:s0"

    def __len__(self) -> int:
        return len(self._rules)


class MetaDb:
    """In-memory ``meta.txt``: the authoritative permission table for a tree."""

    def __init__(self, entries: Optional[Iterable[MetaEntry]] = None) -> None:
        self.entries: List[MetaEntry] = list(entries or ())
        self.entries.sort(key=lambda e: e.path)

    @classmethod
    def load(cls, path: PathLike) -> "MetaDb":
        text = Path(path).read_text(encoding="utf-8")
        rows = [
            MetaEntry.from_tsv(line)
            for line in text.splitlines()
            if line and not line.startswith("#")
        ]
        return cls(rows)

    @classmethod
    def parse(cls, text: str) -> "MetaDb":
        rows = [
            MetaEntry.from_tsv(line)
            for line in text.splitlines()
            if line and not line.startswith("#")
        ]
        return cls(rows)

    def save(self, path: PathLike) -> None:
        p = Path(path)
        p.parent.mkdir(parents=True, exist_ok=True)
        lines = [META_HEADER, META_COLUMNS]
        lines.extend(e.to_tsv() for e in self.entries)
        p.write_text("\n".join(lines) + "\n", encoding="utf-8")

    def get(self, path: str) -> Optional[MetaEntry]:
        target = _norm(path)
        for e in self.entries:
            if e.path == target:
                return e
        return None

    def filter(self, prefix: str) -> List[MetaEntry]:
        pre = _norm(prefix)
        return [e for e in self.entries if e.path == pre or e.path.startswith(pre + "/")]

    def __iter__(self) -> Iterator[MetaEntry]:
        return iter(self.entries)

    def __len__(self) -> int:
        return len(self.entries)


def export(
    directory: PathLike,
    out: PathLike,
    fs_config: Optional[PathLike] = None,
    file_contexts: Optional[PathLike] = None,
) -> MetaDb:
    """Scan ``directory`` and write ``meta.txt`` using the text configs."""
    args = ["perm", "export", "--dir", directory, "--out", out]
    if fs_config is not None:
        args += ["--fs-config", fs_config]
    if file_contexts is not None:
        args += ["--file-contexts", file_contexts]
    _proc.run(args)
    return MetaDb.load(out)


def apply(meta: PathLike, directory: PathLike) -> int:
    """Re-apply readonly bits + mtimes to a directory (uid/gid/SELinux stay in text)."""
    out = _proc.run(["perm", "apply", "--meta", meta, "--dir", directory])
    digits = re.search(r"applied (\d+) entries", out)
    return int(digits.group(1)) if digits else 0


def show(meta: PathLike, path: str) -> str:
    """Human readable single entry (same formatting as the CLI)."""
    return _proc.run(["perm", "show", "--meta", meta, path])
