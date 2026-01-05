# Build Guide

This document describes how to build Olive from source. For Chinese, see
[`docs/build-zh.md`](docs/build-zh.md).

## Prerequisites

- CMake 3.20+
- Ninja (recommended)
- Qt 6 (with private headers)
- FFmpeg development libraries
- OpenImageIO
- OpenColorIO (2.x)
- OpenEXR
- Expat
- PortAudio
- OpenGL headers
- XKB common (Linux)

## Linux (Ubuntu/Debian)

Install dependencies:

```bash
sudo apt-get update
sudo apt-get install -y \
  ninja-build pkg-config \
  qt6-base-dev qt6-base-dev-tools qt6-base-private-dev qt6-tools-dev qt6-tools-dev-tools \
  libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev \
  libopencolorio-dev libopenimageio-dev libopenexr-dev libexpat1-dev \
  portaudio19-dev libgl1-mesa-dev libxkbcommon-dev
```

Configure and build:

```bash
cmake -S . -B build -G Ninja -DBUILD_TESTS=ON -DBUILD_QT6=ON
cmake --build build --config Release
```

Run tests:

```bash
ctest --test-dir build --output-on-failure -C Release
```

## macOS (Non-Official Support)

Note: macOS support is **non-official**. We only run CI automation on macOS
and do not perform manual testing.

Install dependencies:

```bash
brew update
brew install ninja pkg-config qt@6 ffmpeg openimageio opencolorio openexr portaudio expat
```

Build OpenTimelineIO (optional, required for OTIO support):

```bash
git clone --depth 1 --branch v0.16.0 https://github.com/PixarAnimationStudios/OpenTimelineIO.git
cmake -S OpenTimelineIO -B OpenTimelineIO/build -G Ninja \
  -DOTIO_SHARED_LIBS=ON \
  -DOTIO_PYTHON_BINDINGS=OFF \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="${PWD}/otio-install"
cmake --build OpenTimelineIO/build
cmake --install OpenTimelineIO/build
```

Configure and build:

```bash
export PATH="$(brew --prefix qt@6)/bin:$PATH"
export CMAKE_PREFIX_PATH="$(brew --prefix qt@6)"
export OTIO_LOCATION="${PWD}/otio-install"
export OCIO_LOCATION="$(brew --prefix opencolorio)"

cmake -S . -B build -G Ninja -DBUILD_TESTS=ON -DBUILD_QT6=ON \
  -DOTIO_LOCATION="${OTIO_LOCATION}" \
  -DOCIO_LOCATION="${OCIO_LOCATION}"
cmake --build build --config Release
```

Run tests:

```bash
ctest --test-dir build --output-on-failure -C Release
```

## Windows

Install Qt 6 (system installer or CI action). Use vcpkg for dependencies.

```powershell
choco install -y ninja
$env:VCPKG_ROOT = "C:\vcpkg"
& "$env:VCPKG_ROOT\vcpkg.exe" install ffmpeg openimageio opencolorio openexr expat portaudio --triplet x64-windows
```

Configure and build:

```powershell
cmake -S . -B build -G Ninja `
  -DBUILD_TESTS=ON `
  -DBUILD_QT6=ON `
  -DCMAKE_BUILD_TYPE=Release `
  -DCMAKE_TOOLCHAIN_FILE="$env:VCPKG_ROOT\scripts\buildsystems\vcpkg.cmake" `
  -DCMAKE_PREFIX_PATH="$env:Qt6_DIR"
cmake --build build --config Release
```

Run tests:

```powershell
ctest --test-dir build --output-on-failure -C Release
```
