# CI：Gitea 发版 → GitHub 构建发布

本仓库使用两段式流水线：

1. **私有 Gitea**：发布 Release 后，把该 Release 的源码 tarball 作为**一个 commit** 同步到 GitHub。
   同步时会删除 `.gitea/`，GitHub 侧看不到私有实例的 Runner/工作流配置。
2. **GitHub**：收到 push 后自动 `cargo test/build`，为 Windows 与 Linux 各打一个包并创建 Release。

## 文件

```
.gitea/workflows/mirror-release-to-github.yml   私有侧：Release → GitHub commit
.github/workflows/build-release.yml             GitHub：push → 测试/构建/发布
scripts/ci/mirror_release_to_github.sh          同步脚本（可本地 dry-run）
.gitattributes                                  export-ignore: /.gitea
```

## 私有侧配置（Gitea）

在仓库设置中添加：

| 类型 | 名称 | 说明 |
|---|---|---|
| Secret | `GITEA_TOKEN` | 读取本仓库 Release / 归档的访问令牌 |
| Secret | `GH_TOKEN` | GitHub PAT，`repo` 权限（或 fine-grained: Contents 读写） |
| Variable | `GH_REPO` | GitHub 仓库，`owner/name`（也接受完整 URL，脚本会归一化） |
| Variable | `GH_BRANCH` | 可选，默认 `main` |

Runner 标签使用 `ubuntu-latest`；仓库需启用 Actions。工作流触发于 `release: published`，
也支持手动 `workflow_dispatch` 并填写 tag。

同步用的 tarball 优先取 Release 资产中第一个 `.tar.gz`/`.tgz`；没有资产时回退到
Gitea 的 `archive/<tag>.tar.gz` 接口。

## GitHub 侧配置

无需额外 Secret：工作流使用内置 `GITHUB_TOKEN`，已在 `permissions.contents: write`
中声明。每次 push 到 `main`/`master`（或手动触发）都会构建全平台矩阵，然后以
`v<Cargo.toml version>-<run_number>` 创建 Release，上传所有产物。

### native（原生 runner）

| runner | target | 产物 |
|---|---|---|
| ubuntu-latest | x86_64-unknown-linux-gnu | `aimg-x86_64-unknown-linux-gnu.tar.gz` |
| ubuntu-latest | x86_64-unknown-linux-musl | `aimg-x86_64-unknown-linux-musl.tar.gz` |
| windows-latest | x86_64-pc-windows-msvc | `aimg-x86_64-pc-windows-msvc.zip` |
| ubuntu-latest + mingw-w64-i686 | i686-pc-windows-gnu | `aimg-i686-pc-windows-gnu.zip` |
| windows-11-arm | aarch64-pc-windows-msvc | `aimg-aarch64-pc-windows-msvc.zip` |
### zig（ubuntu-latest + cargo-zigbuild）

| target | 产物 |
|---|---|
| aarch64-apple-darwin | `aimg-aarch64-apple-darwin.tar.gz` |
| x86_64-apple-darwin | `aimg-x86_64-apple-darwin.tar.gz` |

macOS 的 Mach-O 由 Zig 0.13 交叉链接（`cargo zigbuild`），不依赖 macOS runner。

### cross（ubuntu-latest + cross-rs）

`aarch64-unknown-linux-gnu`、`armv7-unknown-linux-gnueabihf`、`arm-unknown-linux-gnueabihf`、
`i686-unknown-linux-gnu`、`riscv64gc-unknown-linux-gnu`、`powerpc64le-unknown-linux-gnu`、
`aarch64-unknown-linux-musl`、`armv7-unknown-linux-musleabihf`、`x86_64-unknown-freebsd`，
每个 target 一个 `tar.gz`。

### Android（ubuntu-latest + NDK r26d，API 21）

`aarch64-linux-android`、`armv7-linux-androideabi`、`x86_64-linux-android`、
`i686-linux-android`，每个 target 一个 `tar.gz`（PIE 可执行文件，推送到设备后用
`adb push` + `chmod +x` 运行）。

构建脚本会按 target 设置 `CC_<target>` / `AR_<target>` / `CARGO_TARGET_<TARGET>_LINKER`
指向 NDK 的 `*-clang` 包装器，C 依赖（bzip2-sys、lzma-sys）随 Rust 代码一起交叉编译。

测试只在原生 x86_64 以及可原生运行的 runner 上执行；交叉目标只构建不做测试。


## 本地演练

```bash
export GITEA_URL=https://<your-gitea>
export GITEA_REPO=owner/repo
export GITEA_TOKEN=...
export GH_TOKEN=...
export GH_REPO=owner/repo
export TAG=v0.1.0
bash scripts/ci/mirror_release_to_github.sh
```

脚本只做：下载归档 → 解包 → 克隆 GitHub → 清空工作树（保留 `.git`）→ 覆盖 →
删除 `.gitea` → 单次 commit → push。无差异时直接退出，不产生空 commit。
