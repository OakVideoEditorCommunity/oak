# 构建指南

本文档介绍如何在 macOS、Linux 和 Windows 上从源码构建 Oak 视频编辑器。
英文版见 [`../build.md`](../build.md)。

> **2026 年说明：** Oak 现在是纯 Rust workspace。在仓库根目录执行
> `cargo build` 会产出应用（`oak-editor`）、命令行工具（`oak-cli`）
> 和渲染进程（`oak-worker`）。旧的 C++/CMake 代码保留在
> `cpp-legacy` 分支，本指南不涉及它。

---

## 通用前置条件

- **git** —— 克隆时必须带子模块（`gpui/` 是子模块）：
  ```sh
  git clone --recursive https://github.com/OakVideoEditorCommunity/oak.git
  cd oak
  # 已有克隆则：git submodule update --init --recursive
  ```
- **Rust stable**（通过 [rustup](https://rustup.rs/) 安装；Windows 使用
  默认的 `x86_64-pc-windows-msvc` 工具链——见 Windows 章节）。
- **系统依赖由一个脚本统一安装。** `tooling/install-deps.sh` 安装所有
  包管理器提供的依赖：构建工具（C/C++ 工具链；cmake + C++ 编译器用于
  vendored OpenColorIO 构建；meson/ninja/nasm 用于 FFmpeg 依赖构建）、
  workspace 链接的系统库（cpal 的 PipeWire/JACK/ALSA/PulseAudio/sndfile
  音频后端，wgpu 窗口栈的 GL/Vulkan/XKB)、无头测试栈（xvfb、Mesa 软件
  Vulkan、字体）以及打包工具。
- **FFmpeg 8.1，由项目脚本构建。** 发行版自带版本对 `ffmpeg-next` 9
  来说太旧，刻意不使用：
  ```sh
  tooling/install-deps.sh        # 全部包管理器依赖
  tooling/ffmpeg/build-ffmpeg.sh # 先从源码构建全部编解码库（build-deps.sh），
                                 # 再克隆 release/8.1；全部安装到 .cache/ffmpeg
  ```
  `FFMPEG_DIR` 无需手动导出：仓库内提交的 `.cargo/config.toml` 已按
  workspace 根的相对路径设置（`ffmpeg-sys-next` 的构建脚本读不了
  `.env`，这是唯一与机器无关的方式）。缺少它时 `oak-ffmpeg-link` 的
  构建脚本会直接 panic：首次 `cargo build` 前请先跑一次
  `build-ffmpeg.sh`。

## 快速开始（macOS / Linux）

```sh
tooling/install-deps.sh         # Homebrew / apt / dnf / pacman
tooling/ffmpeg/build-ffmpeg.sh  # 依赖 + FFmpeg，约 20–40 分钟，只需一次
cargo build --workspace
cargo test  --workspace         # Linux：见下文"无头测试"
```

---

## macOS

- macOS 12+、Xcode Command Line Tools（`xcode-select --install`）、Homebrew。
- ```sh
  tooling/install-deps.sh         # brew：构建工具 + librsvg + autotools
  tooling/ffmpeg/build-ffmpeg.sh
  cargo build --workspace
  cargo test  --workspace
  ```
- OpenColorIO 由 vendored 2.5.2 源码编译并静态链接——不需要
  `brew install opencolorio`（cmake 由 `install-deps.sh` 安装）。
- GPU 相关测试（OFX GL 叠加层、硬件解码）仅在设置 `OAK_GPU_TESTS=1`
  时运行。

## Linux

- `tooling/install-deps.sh`（支持 Debian/Ubuntu/openKylin、Fedora、Arch）
  会装齐全部依赖，包括 PipeWire/JACK/ALSA/PulseAudio/sndfile 开发包
  （cpal 音频后端）、GL/Vulkan/XKB 开发包（wgpu 窗口栈）以及下面的
  无头测试栈。
- **无头测试：** 部分 gpui/UI 测试会通过 wgpu 在 Mesa 软件 Vulkan
  （lavapipe）上打开真实窗口（xvfb 和 Mesa Vulkan 驱动已由
  `install-deps.sh` 安装）。无显示环境下请运行：
  ```sh
  xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --workspace
  ```
- OpenColorIO 与 macOS 相同，使用 vendored 静态构建。

## Windows（MSVC）

Windows 构建目标是 **x86_64-pc-windows-msvc**（rustup 在 Windows 上的
默认工具链），搭配 BtbN 预编译的 shared FFmpeg 和 vendored 静态 OCIO。
这与 CI 在 `warp-windows-2025-vs2026-x64-32x` 上的做法完全一致
（见 `.github/workflows/ci.yml`）；旧的 MSYS2/UCRT64 流程不再受支持。

1. 安装 [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/)
   （VS 2022 或更新），勾选**使用 C++ 的桌面开发**工作负载——它提供
   MSVC 链接器，以及 vendored OCIO 构建所需的 C++ 编译器和 CMake。
2. 通过 [rustup](https://rustup.rs/) 安装 Rust；默认 host 工具链即
   `stable-x86_64-pc-windows-msvc`。
3. 下载 BtbN 预编译 FFmpeg（GPL、shared——ffmpeg.org 官方链接的
   Windows 构建，基于 release/8.1 分支），对照发布页的
   `checksums.sha256` 校验后解压到 `.cache/ffmpeg`（PowerShell，
   在 workspace 根目录执行）：
   ```powershell
   $base  = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest"
   $asset = "ffmpeg-n8.1-latest-win64-gpl-shared-8.1.zip"
   Invoke-WebRequest "$base/checksums.sha256" -OutFile "$env:TEMP\checksums.sha256"
   $expected = (Select-String -Path "$env:TEMP\checksums.sha256" `
     -Pattern ([regex]::Escape($asset) + "\s*$")).Line.Split()[0]
   Invoke-WebRequest "$base/$asset" -OutFile "$env:TEMP\$asset"
   if ((Get-FileHash "$env:TEMP\$asset").Hash -ne $expected.ToUpper()) {
     throw "FFmpeg checksum mismatch"
   }
   Expand-Archive "$env:TEMP\$asset" .cache\ffmpeg-extract -Force
   Move-Item (Get-ChildItem .cache\ffmpeg-extract -Directory).FullName .cache\ffmpeg
   ```
4. 环境变量（PowerShell，按会话设置或写入用户环境变量）：
   ```powershell
   # .cargo/config.toml 已把 FFMPEG_DIR 指向 .cache/ffmpeg；测试程序
   # 要加载 FFmpeg DLL，所以 bin/ 必须在 PATH 里（否则首个测试进程
   # 会以 STATUS_DLL_NOT_FOUND 退出）：
   $env:PATH = "$PWD\.cache\ffmpeg\bin;$env:PATH"
   $env:PKG_CONFIG_PATH = "$PWD\.cache\ffmpeg\lib\pkgconfig"
   # vendored 静态 OCIO——当初用 MSYS2 包只是权宜之计；无需
   # OCIO_INSTALL_DIR：
   $env:OCIO_RS_ENABLE_REAL = "1"
   $env:OCIO_RS_LINK = "static"
   ```
5. ```powershell
   cargo build --workspace
   cargo test  --workspace
   ```

---

## OpenColorIO 一览

| 平台 | 来源 | 链接方式 | 备注 |
|------|------|----------|------|
| Linux / macOS | vendored 2.5.2（`ocio-sys` 的 `bundled` 特性，默认开启） | 静态 | 需要 cmake + C++ 编译器 |
| Windows | vendored 2.5.2（同一个 `bundled` 特性） | 静态 | 需要 VS Build Tools（MSVC C++ + CMake）；设置 `OCIO_RS_ENABLE_REAL=1`、`OCIO_RS_LINK=static` |

既没有 `bundled` 特性也没有 `OCIO_RS_ENABLE_REAL=1` 时，`ocio-sys`
构建为 stub，所有色彩测试直接跳过。`oak-render` 无条件启用 bundled
特性，因此直接 `cargo build` 就会得到真实 OCIO。

## 打包

发行版包在容器中构建（宿主机只需要 Docker/Podman）：

```sh
tooling/package/build-deb.sh  # Debian 13  → .deb
tooling/package/build-rpm.sh  # Fedora 41  → .rpm
tooling/package/build-pkg.sh  # Arch Linux → .pkg.tar.zst
```

所有运行时依赖都声明在各自的打包元数据中（`packaging/`、
`tooling/package/oak.spec`、`tooling/package/PKGBUILD`）——除文档
列明的系统库外包是自包含的；文件安装位置见
`docs/project-storage.md`。

## 故障排查

- **`oak-ffmpeg-link` 报 `FFMPEG_DIR` panic** —— 先跑一次
  `tooling/ffmpeg/build-ffmpeg.sh`；它安装到 `.cache/ffmpeg`，
  `.cargo/config.toml` 已指向该目录。
- **IDE 构建失败（RustRover 等）** —— 无法向 cargo 注入环境变量的
  IDE 可以在 workspace 根放一个 git 忽略的 `.env`，写入
  `FFMPEG_DIR=...`（编解码库在自定义前缀时再加
  `PKG_CONFIG_PATH=...`）。
- **Windows：`FFMPEG_DIR` 已设置但链接器找不到 FFmpeg 导入库** ——
  把 BtbN 压缩包解压到 `.cache/ffmpeg`（见 Windows 章节），并将
  `PKG_CONFIG_PATH` 指向 `.cache\ffmpeg\lib\pkgconfig`。
- **Windows：测试启动即报 `STATUS_DLL_NOT_FOUND`** —— FFmpeg 运行时
  DLL 不在 `PATH` 里；把 `.cache\ffmpeg\bin` 加入 `PATH`（见 Windows
  章节）。
- **`gpui/` 目录为空** —— `git submodule update --init --recursive`。
- **Linux 测试开窗口卡死/失败** —— 使用 Linux 章节的 `xvfb-run`
  命令。
