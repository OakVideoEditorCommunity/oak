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
- **Rust stable**（通过 [rustup](https://rustup.rs/) 安装；Windows
  请改用 MSYS2 自带 Rust——见 Windows 章节）。
- **C 工具链 + cmake + pkg-config + nasm** —— cmake 和 C++ 编译器
  用于 vendored OpenColorIO 构建（Linux/macOS），nasm 用于 FFmpeg
  汇编。
- **FFmpeg 8.1，由项目脚本构建。** 发行版自带版本对 `ffmpeg-next` 9
  来说太旧，刻意不使用：
  ```sh
  tooling/install-deps.sh        # 编解码/滤镜库 + 构建工具
  tooling/ffmpeg/build-ffmpeg.sh # 克隆 release/8.1，安装到 .cache/ffmpeg
  ```
  `FFMPEG_DIR` 无需手动导出：仓库内提交的 `.cargo/config.toml` 已按
  workspace 根的相对路径设置（`ffmpeg-sys-next` 的构建脚本读不了
  `.env`，这是唯一与机器无关的方式）。缺少它时 `oak-ffmpeg-link` 的
  构建脚本会直接 panic：首次 `cargo build` 前请先跑一次
  `build-ffmpeg.sh`。

## 快速开始（macOS / Linux）

```sh
tooling/install-deps.sh         # Homebrew / apt / dnf / pacman
tooling/ffmpeg/build-ffmpeg.sh  # 约 10–20 分钟，只需一次
cargo build --workspace
cargo test  --workspace         # Linux：见下文"无头测试"
```

---

## macOS

- macOS 12+、Xcode Command Line Tools（`xcode-select --install`）、Homebrew。
- ```sh
  brew install cmake pkg-config
  tooling/install-deps.sh
  tooling/ffmpeg/build-ffmpeg.sh
  cargo build --workspace
  cargo test  --workspace
  ```
- OpenColorIO 由 vendored 2.5.2 源码编译并静态链接——不需要
  `brew install opencolorio`（但需要 cmake）。
- GPU 相关测试（OFX GL 叠加层、硬件解码）仅在设置 `OAK_GPU_TESTS=1`
  时运行。

## Linux

- `tooling/install-deps.sh` 支持 Debian/Ubuntu、Fedora、Arch。
  另外需要安装：
  ```sh
  # Debian/Ubuntu
  sudo apt-get install -y cmake \
    libpipewire-0.3-dev libspa-0.2-dev libjack-jackd2-dev \
    libasound2-dev libpulse-dev libsndfile1-dev \
    libgl1-mesa-dev libvulkan-dev libxkbcommon-dev libxkbcommon-x11-dev
  ```
  （PipeWire/JACK/ALSA/PulseAudio/sndfile 开发包是 cpal 的音频后端；
  GL/Vulkan/XKB 是 wgpu 窗口栈。）
- **无头测试：** 部分 gpui/UI 测试会通过 wgpu 在 Mesa 软件 Vulkan
  （lavapipe）上打开真实窗口。无显示环境下请运行：
  ```sh
  sudo apt-get install -y xvfb mesa-vulkan-drivers
  xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --workspace
  ```
- OpenColorIO 与 macOS 相同，使用 vendored 静态构建。

## Windows（MSYS2 UCRT64）

Windows 构建目标是 **x86_64-pc-windows-gnu**，使用 MSYS2 自带 Rust；
不支持 MSVC 工具链（构建脚本会发出 MSVC 链接器不接受的 Unix 风格
链接参数）。

1. 安装 [MSYS2](https://www.msys2.org/)，打开 **UCRT64** 终端。
2. ```sh
   pacman -Syu
   pacman -S --needed mingw-w64-ucrt-x86_64-rust \
     mingw-w64-ucrt-x86_64-cmake mingw-w64-ucrt-x86_64-opencolorio
   tooling/install-deps.sh        # 必须在 UCRT64 终端内运行
   tooling/ffmpeg/build-ffmpeg.sh
   ```
3. 环境变量（写入 shell rc 或每次会话导出）：
   ```sh
   # vendored OCIO 源码含有仅 MSVC 可编译的构造，Windows 改为动态链接
   # MSYS2 的 OpenColorIO 2.5.2：
   export OCIO_RS_ENABLE_REAL=1 OCIO_INSTALL_DIR=/ucrt64 OCIO_RS_LINK=dynamic
   # mingw-w64（2025 年 11 月后）把 _assert 转发到 libmingwex.a 里的
   # __msvcrt_assert，而 rustc 的链接顺序把 -lmingwex 放在最后；末尾
   # 追加 -lmsvcrt 让链接器再扫一遍 CRT 导入库
   # （否则报 undefined _fileno/_setmode/__imp___msvcrt_assert）。
   export RUSTFLAGS="-C link-args=-lmsvcrt"
   # 如果 shell 继承了 MSVC 的 INCLUDE/LIB（某些 CI runner 会向每个
   # 步骤注入），务必清除——它们会污染 MinGW 编译：
   unset INCLUDE LIB
   ```
4. ```sh
   cargo build --workspace
   cargo test  --workspace
   ```

---

## OpenColorIO 一览

| 平台 | 来源 | 链接方式 | 备注 |
|------|------|----------|------|
| Linux / macOS | vendored 2.5.2（`ocio-sys` 的 `bundled` 特性，默认开启） | 静态 | 需要 cmake + C++ 编译器 |
| Windows | MSYS2 `mingw-w64-ucrt-x86_64-opencolorio` | 动态 | 设置 `OCIO_INSTALL_DIR=/ucrt64`、`OCIO_RS_LINK=dynamic` |

既没有 `bundled` 特性也没有 `OCIO_RS_ENABLE_REAL=1` 时，`ocio-sys`
构建为 stub，所有色彩测试直接跳过。`oak-render` 无条件启用 bundled
特性，因此在 Linux/macOS 上直接 `cargo build` 就会得到真实 OCIO。

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
- **pacman 报 "Operation too slow"** —— MSYS2 镜像偶尔卡顿；
  `install-deps.sh` 会自动重试三次，手动重跑也会借助 `--needed`
  断点续装。
- **Windows：`undefined reference to _fileno/_setmode/__imp___msvcrt_assert`**
  —— 设置 `RUSTFLAGS="-C link-args=-lmsvcrt"`（见 Windows 章节）。
- **Windows：出现 MSVC 风格的链接错误** —— 你用的是 MSVC 版 Rust；
  请改用 MSYS2 自带 Rust（`x86_64-pc-windows-gnu`）。
- **`gpui/` 目录为空** —— `git submodule update --init --recursive`。
- **Linux 测试开窗口卡死/失败** —— 使用 Linux 章节的 `xvfb-run`
  命令。
