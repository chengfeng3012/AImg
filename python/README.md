# pyaimg — typed Python API for the aimg toolchain

`pyaimg` 是 [AndroidImg_Rust](../README.md) 的 Python 封装：所有字节级工作在 Rust 二进制里完成，
Python 侧提供结构化类型、dataclass 结果对象、文本权限模型（`FsConfig` / `FileContexts` / `MetaDb`）和 `.pyi` 存根。

## 安装与指向二进制

```bash
pip install -e python/          # 或者把 python/ 加到 PYTHONPATH
export AIMG_BIN=/path/to/aimg   # Windows: set AIMG_BIN=F:\...\aimg.exe
```

查找顺序：`$AIMG_BIN` → `PATH` 上的 `aimg` → 仓库内 `target/{release,debug}/aimg[.exe]`。

## 快速上手

```python
import pyaimg as ai

# 权限：文本配置，不看 NTFS
fsc = ai.FsConfig.load("configs/fs_config.txt")
fc  = ai.FileContexts.load("configs/file_contexts")

# 目录树 -> ext4（uid/gid/mode/SELinux 全写进镜像）
ai.ext4.build("system_root/", "system.img",
              fs_config=fsc, file_contexts=fc, size="2G", label="system")

info = ai.ext4.info("system.img")        # Ext4Info
print(info.block_size, info.volume_name)

meta = ai.ext4.extract("system.img", "out/")   # 返回 MetaDb
sh = meta.get("/system/bin/sh")
print(sh.uid, sh.gid, sh.mode_octal, sh.context)
```

```python
# sparse
ai.sparse.from_raw("system.raw", "system.img")
ai.sparse.to_raw(["system.img_sparsechunk.0", "system.img_sparsechunk.1"], "super.raw")

# super.img
sup = ai.superimg.make("super.img",
                       [ai.superimg.PartitionImage("system_a", "system.img", "main")],
                       size="8G")
print(sup.partitions, sup.size_of("system_a"))
ai.superimg.unpack("super.img", "parts/", slot=0)

# payload.bin
ai.payload.make("payload.bin", [("system", "system.img")], chunk_size=2 << 20)
p = ai.payload.info("payload.bin")
print(p.partitions[0].op_types)
ai.payload.extract("payload.bin", "parts/")

# .dat
ai.dat.split("system.img", "dat/", prefix="system", version=4)
ai.dat.merge("dat/system.transfer.list", "dat/system.new.dat", "system.raw")

# boot / recovery
b = ai.boot.info("boot.img")
ai.boot.unpack("boot.img", "boot_dir/")
ai.boot.pack("boot_dir/", "new-boot.img")
```

## 约定

- 所有路径参数都接受 `str` / `os.PathLike`，返回 `pathlib.Path`。
- 失败统一抛 `pyaimg.AimgError`（含 `cmd` / `returncode` / `stdout` / `stderr`）。
- 结构化结果都是 `frozen dataclass`，字段名与 Rust JSON 输出一一对应。
- `meta.txt` 是权限的唯一真相：NTFS 只保留只读位与 mtime，`uid/gid/context` 只存在于文本和 ext4 镜像里。
- 类型存根：每个模块都有同名 `.pyi`，包内含 `py.typed`，mypy / pyright 直接可用。

## 测试

```bash
# 在容器里跑（不在宿主机装环境）
docker run --rm -v "$PWD/..":/w -w /w/python -e AIMG_BIN=/w/target/release/aimg \
  -e PYTHONPATH=/w/python python:3-slim \
  sh -c "pip install -q pytest && python -m pytest -q"
```

`tests/test_api.py` 覆盖：fs_config/file_contexts 语义、meta.txt 往返、ext4 build/info/extract、
sparse 往返、super 打包拆包、payload 生成提取、.dat 拆分合并、boot v0 往返、pipeline 组合、错误映射。
