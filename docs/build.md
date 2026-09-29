# Build Guide

This document describes how to build the Oak Video Editor from source on
macOS, Linux, and Windows. For the Chinese version see
[`zh/build.md`](zh/build.md).

> **2026 note:** Oak is a pure Rust workspace. `cargo build` at the
> repository root produces the app (`oak-editor`), the CLI (`oak-cli`)
> and the render worker (`oak-worker`). The old C++/CMake tree lives on
> the `cpp-legacy` branch; nothing in this guide uses it.

---

## Prerequisites (all platforms)

- **git** — clone with submodules (`gpui/` is a submodule):
  ```sh
  git clone --recursive https://github.com/OakVideoEditorCommunity/oak.git
  cd oak
  # or, on an existing clone: git submodule update --init --recursive
  ```
- **Rust stable** via [rustup](https://rustup.rs/) (on Windows the default
  `x86_64-pc-windows-msvc` host toolchain — see the Windows section).
- **System dependencies via one script.** `tooling/install-deps.sh`
  installs everything a package manager provides: the build tools (C/C++
  toolchain, cmake + a C++ compiler for the vendored OpenColorIO build,
  meson/ninja/nasm for the FFmpeg dependency builds), the workspace's
  system libraries (PipeWire/JACK/ALSA/PulseAudio/sndfile for cpal,
  GL/Vulkan/XKB for the wgpu windowing stack), the headless-test stack
  (xvfb, Mesa software Vulkan, fonts) and the packaging tools.
- **FFmpeg 8.1, built by the project script.** Distro packages are too
  old for `ffmpeg-next` 9 and are deliberately not used:
  ```sh
  tooling/install-deps.sh        # every package-manager dependency
  tooling/ffmpeg/build-ffmpeg.sh # builds every codec library from source
                                 # (build-deps.sh), then clones release/8.1;
                                 # everything installs into .cache/ffmpeg
  ```
  `FFMPEG_DIR` does not need exporting: the committed
  `.cargo/config.toml` sets it relative to the workspace root
  (`ffmpeg-sys-next` cannot read `.env` at build-script time — the
  config entry is the only machine-agnostic way). The `oak-ffmpeg-link`
  build script panics without it; run `build-ffmpeg.sh` once before the
  first `cargo build`.
- **CI/CD follows the same path.** The Linux/macOS jobs run
  `tooling/install-deps.sh` (the single dependency step in both
  workflows) and `tooling/ffmpeg/build-ffmpeg.sh`, so the release
  binaries and local builds share one dependency set and one FFmpeg
  configuration (the built tree is cached in CI and rebuilt from scratch
  in CD). The Windows jobs download a prebuilt FFmpeg — BtbN's shared GPL
  build, the one ffmpeg.org links as the official Windows option — from
  its release page, verify it against the published `checksums.sha256`
  and point `FFMPEG_DIR` at it (no MSYS2 or vcpkg toolchain in CI).

## Quick start (macOS / Linux)

```sh
tooling/install-deps.sh         # Homebrew / apt / dnf / pacman
tooling/ffmpeg/build-ffmpeg.sh  # deps + FFmpeg, ~20–40 min, once
cargo build --workspace
cargo test  --workspace         # Linux: see "headless tests" below
```

---

## macOS

- macOS 12+, Xcode Command Line Tools (`xcode-select --install`), Homebrew.
- ```sh
  tooling/install-deps.sh         # brew: tools + librsvg + autotools
  tooling/ffmpeg/build-ffmpeg.sh
  cargo build --workspace
  cargo test  --workspace
  ```
- OpenColorIO is compiled from the vendored 2.5.2 sources and linked
  statically — no `brew install opencolorio` needed (cmake is installed
  by `install-deps.sh`).
- The GPU-gated tests (OFX GL overlay, hardware decode) run only with
  `OAK_GPU_TESTS=1`.

## Linux

- `tooling/install-deps.sh` (Debian/Ubuntu/openKylin, Fedora, Arch)
  installs the full set, including the PipeWire/JACK/ALSA/PulseAudio/
  sndfile dev packages (cpal's audio backends), the GL/Vulkan/XKB dev
  packages (the wgpu windowing stack) and the headless-test stack below.
- **Headless tests:** several gpui/UI tests open real windows through
  wgpu on Mesa's software Vulkan (xvfb and the Mesa Vulkan drivers are
  installed by `install-deps.sh`). Under a display-less session run:
  ```sh
  xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --workspace
  ```
- OpenColorIO is the vendored static build, as on macOS.

## Windows (MSVC)

The Windows build targets **x86_64-pc-windows-msvc** — the rustup default
on Windows — with BtbN's prebuilt shared FFmpeg and the vendored static
OCIO. This is exactly what CI does on `warp-windows-2025-vs2026-x64-32x`
(`.github/workflows/ci.yml`); the old MSYS2/UCRT64 flow is no longer
supported.

1. Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/)
   (VS 2022 or later) with the **Desktop development with C++** workload —
   it provides the MSVC linker plus the C++ compiler and CMake the
   vendored OCIO build needs.
2. Install Rust via [rustup](https://rustup.rs/); the default host
   toolchain is already `stable-x86_64-pc-windows-msvc`.
3. Download BtbN's prebuilt FFmpeg (GPL, shared — the build ffmpeg.org
   links as the official Windows option, from the release/8.1 branch),
   verify it against the release's `checksums.sha256` and unpack it into
   `.cache/ffmpeg` (PowerShell, from the workspace root):
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
4. Environment (PowerShell, per session or in the user environment):
   ```powershell
   # .cargo/config.toml already points FFMPEG_DIR at .cache/ffmpeg; the
   # test binaries load the FFmpeg DLLs, so bin/ must be on PATH
   # (otherwise the first test exits with STATUS_DLL_NOT_FOUND):
   $env:PATH = "$PWD\.cache\ffmpeg\bin;$env:PATH"
   $env:PKG_CONFIG_PATH = "$PWD\.cache\ffmpeg\lib\pkgconfig"
   # Vendored static OCIO — the MSYS2 package was the workaround, not the
   # preference; no OCIO_INSTALL_DIR needed:
   $env:OCIO_RS_ENABLE_REAL = "1"
   $env:OCIO_RS_LINK = "static"
   ```
5. ```powershell
   cargo build --workspace
   cargo test  --workspace
   ```

---

## OpenColorIO summary

| Platform | Source | Linkage | Notes |
|----------|--------|---------|-------|
| Linux / macOS | vendored 2.5.2 (`ocio-sys` `bundled` feature, on by default) | static | needs cmake + C++ compiler |
| Windows | vendored 2.5.2 (same `bundled` feature) | static | needs VS Build Tools (MSVC C++ + CMake); set `OCIO_RS_ENABLE_REAL=1`, `OCIO_RS_LINK=static` |

Without the `bundled` feature and without `OCIO_RS_ENABLE_REAL=1`,
`ocio-sys` builds a stub and every colour test early-returns. The
bundled feature is enabled unconditionally by `oak-render`, so a plain
`cargo build` always gets the real thing.

## Packaging

Distribution packages are built in containers (no host dependencies
beyond Docker/Podman):

```sh
tooling/package/build-deb.sh  # Debian 13  → .deb
tooling/package/build-rpm.sh  # Fedora 41  → .rpm
tooling/package/build-pkg.sh  # Arch Linux → .pkg.tar.zst
```

All runtime dependencies are declared in the respective package metadata
(`packaging/`, `tooling/package/oak.spec`, `tooling/package/PKGBUILD`) —
the packages are self-contained except for the documented system
libraries; see `docs/project-storage.md` for what lands where.

## Troubleshooting

- **`oak-ffmpeg-link` panics about `FFMPEG_DIR`** — run
  `tooling/ffmpeg/build-ffmpeg.sh` once; it installs into
  `.cache/ffmpeg`, which `.cargo/config.toml` points at.
- **IDE builds fail (RustRover etc.)** — IDEs that cannot inject
  environment variables into cargo can read a git-ignored `.env` at the
  workspace root with `FFMPEG_DIR=...` (and `PKG_CONFIG_PATH=...` if
  your codec libraries live in a custom prefix).
- **Windows: `FFMPEG_DIR` is set but the linker cannot find the FFmpeg
  import libraries** — unpack the BtbN archive into `.cache/ffmpeg`
  (see the Windows section) and point `PKG_CONFIG_PATH` at
  `.cache\ffmpeg\lib\pkgconfig`.
- **Windows: tests exit immediately with `STATUS_DLL_NOT_FOUND`** — the
  FFmpeg runtime DLLs are not on `PATH`; add `.cache\ffmpeg\bin` (see
  the Windows section).
- **Empty `gpui/` directory** — `git submodule update --init --recursive`.
- **Linux tests open windows and hang/fail** — use the `xvfb-run` line
  from the Linux section.
