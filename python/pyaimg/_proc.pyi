from __future__ import annotations

import os
from typing import List, Sequence, Union

PathLike = Union[str, "os.PathLike[str]"]


class AimgError(RuntimeError):
    cmd: List[str]
    returncode: int
    stdout: str
    stderr: str

    def __init__(self, cmd: Sequence[str], returncode: int, stdout: str, stderr: str) -> None: ...


def binary() -> str: ...
def run(args: Sequence[PathLike], *, check: bool = ...) -> str: ...
def run_json(args: Sequence[PathLike]) -> dict: ...
