"""Process runner for the ``aimg`` binary."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import List, Sequence, Union

PathLike = Union[str, "os.PathLike[str]"]


class AimgError(RuntimeError):
    """Raised when the ``aimg`` binary exits non-zero."""

    def __init__(self, cmd: Sequence[str], returncode: int, stdout: str, stderr: str) -> None:
        self.cmd = list(cmd)
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr
        detail = (stderr or stdout).strip()
        super().__init__(f"aimg {' '.join(self.cmd[1:])} failed ({returncode}): {detail}")


def binary() -> str:
    """Locate the ``aimg`` executable.

    Order: ``$AIMG_BIN`` -> ``aimg`` on ``PATH`` -> ``target/{release,debug}/aimg``
    relative to the repository root.
    """
    env = os.environ.get("AIMG_BIN")
    if env:
        if not Path(env).exists():
            raise FileNotFoundError(f"AIMG_BIN points to a missing file: {env}")
        return env
    found = shutil.which("aimg")
    if found:
        return found
    here = Path(__file__).resolve()
    for parent in here.parents:
        for rel in (
            "target/release/aimg",
            "target/release/aimg.exe",
            "target/debug/aimg",
            "target/debug/aimg.exe",
        ):
            cand = parent / rel
            if cand.exists():
                return str(cand)
    raise FileNotFoundError("aimg binary not found; set AIMG_BIN or put it on PATH")


def run(args: Sequence[PathLike], *, check: bool = True) -> str:
    """Run ``aimg <args>`` and return stdout."""
    cmd: List[str] = [binary()] + [str(a) for a in args]
    proc = subprocess.run(
        cmd, capture_output=True, text=True, encoding="utf-8", errors="replace"
    )
    if check and proc.returncode != 0:
        raise AimgError(cmd, proc.returncode, proc.stdout, proc.stderr)
    return proc.stdout


def run_json(args: Sequence[PathLike]) -> dict:
    """Run a subcommand that supports ``--json`` and decode the result."""
    return json.loads(run(list(args) + ["--json"]))
