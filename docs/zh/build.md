# 构建指南

本文档介绍如何从源码构建 Oak Video Editor。

## 依赖

- CMake 3.20+
- Ninja（推荐）
- Qt 6（含私有头文件）
- FFmpeg 开发库
- OpenImageIO
- OpenColorIO（2.x）
- OpenEXR
- Expat
- PortAudio
- OpenGL 头文件
- XKB common（Linux）

## Linux（Ubuntu/Debian）

安装依赖：

```bash
sudo apt-get update
sudo apt-get install -y \
  ninja-build pkg-config \
  qt6-base-dev qt6-base-dev-tools qt6-base-private-dev qt6-tools-dev qt6-tools-dev-tools \
  libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev \
  libopencolorio-dev libopenimageio-dev libopenexr-dev libexpat1-dev \
  portaudio19-dev libgl1-mesa-dev libxkbcommon-dev
```

配置并构建：

```bash
cmake -S . -B build -G Ninja -DBUILD_TESTS=ON -DBUILD_QT6=ON
cmake --build build --config Release
```

运行测试：

```bash
ctest --test-dir build --output-on-failure -C Release
```

## macOS（非官方支持）

说明：macOS **非官方支持**，目前只做 CI 自动化测试，不做人工测试。

安装依赖：

```bash
brew update
brew install ninja pkg-config qt@6 ffmpeg openimageio opencolorio openexr portaudio expat
```

构建 OpenTimelineIO（可选，如需 OTIO 支持）：

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

配置并构建：

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

运行测试：

```bash
ctest --test-dir build --output-on-failure -C Release
```

## Windows

Qt 6 请使用系统安装器或 CI action。其余依赖建议用 vcpkg。

```powershell
choco install -y ninja
$env:VCPKG_ROOT = "C:\vcpkg"
& "$env:VCPKG_ROOT\vcpkg.exe" install ffmpeg openimageio opencolorio openexr expat portaudio --triplet x64-windows
```

配置并构建：

```powershell
cmake -S . -B build -G Ninja `
  -DBUILD_TESTS=ON `
  -DBUILD_QT6=ON `
  -DCMAKE_BUILD_TYPE=Release `
  -DCMAKE_TOOLCHAIN_FILE="$env:VCPKG_ROOT\scripts\buildsystems\vcpkg.cmake" `
  -DCMAKE_PREFIX_PATH="$env:Qt6_DIR"
cmake --build build --config Release
```

运行测试：

```powershell
ctest --test-dir build --output-on-failure -C Release
```
