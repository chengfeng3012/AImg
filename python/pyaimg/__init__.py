"""Typed Python API for the ``aimg`` Android image toolchain.

The Rust binary does the byte level work; this package gives you a typed,
documented surface plus the text-permission model (``FsConfig`` /
``FileContexts`` / ``MetaDb``) in pure Python.

Example
-------
>>> import pyaimg as ai
>>> img = ai.ext4.build("system_root/", "system.img",
...                     fs_config="fs_config.txt",
...                     file_contexts="file_contexts",
...                     size="2G", label="system")
>>> meta = ai.ext4.extract("system.img", "out/")
>>> meta.get("/system/bin/sh").context
'u:object_r:shell_exec:s0'
"""

from __future__ import annotations

from . import boot, dat, ext4, payload, perm, pipeline, sparse, superimg
from ._proc import AimgError, binary
from .perm import FileContexts, FsConfig, MetaDb
from .types import (
    AvbFooter,
    BootInfo,
    Ext4Info,
    MetaEntry,
    PayloadExtent,
    PayloadInfo,
    PayloadOp,
    PayloadPartition,
    SparseChunk,
    SparseInfo,
    SuperBlockDevice,
    SuperExtent,
    SuperGeometry,
    SuperGroup,
    SuperInfo,
    SuperPartition,
    TransferCommand,
    TransferInfo,
)

__version__ = "0.1.0"

__all__ = [
    "__version__",
    "AimgError",
    "binary",
    "boot",
    "dat",
    "ext4",
    "payload",
    "perm",
    "pipeline",
    "sparse",
    "superimg",
    "FileContexts",
    "FsConfig",
    "MetaDb",
    "AvbFooter",
    "BootInfo",
    "Ext4Info",
    "MetaEntry",
    "PayloadExtent",
    "PayloadInfo",
    "PayloadOp",
    "PayloadPartition",
    "SparseChunk",
    "SparseInfo",
    "SuperBlockDevice",
    "SuperExtent",
    "SuperGeometry",
    "SuperGroup",
    "SuperInfo",
    "SuperPartition",
    "TransferCommand",
    "TransferInfo",
]
