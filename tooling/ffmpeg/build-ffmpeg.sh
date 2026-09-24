#!/usr/bin/env bash
# Oak Video Editor - Non-Linear Video Editor
# Copyright (C) 2026 Oak Team
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <http://www.gnu.org/licenses/>.

# Builds a project-owned FFmpeg for the oak-codec/oak-audio `ffmpeg-next`
# dependency and installs it into .cache/ffmpeg. Point cargo at it with:
#
#   export FFMPEG_DIR="$(pwd)/.cache/ffmpeg"
#
# Why a script instead of ffmpeg-next's `build` cargo feature: the
# feature clones release/<crate-version>, tying the FFmpeg version to the
# crate version; the project pins its own (FFMPEG_VERSION below) and
# ships a known-good pairing. ffmpeg-next 9.0.0 compiles against both the
# FFmpeg 8.x and 9.x headers — verified locally with this script's builds
# of release/8.1 and release/9.0 — so the version below is a project
# choice, not a compatibility workaround.
#
# Oak is GPL, so the GPL-licensed parts of FFmpeg and every free-license
# external codec library are enabled. Hardware acceleration is enabled
# per host OS, and every optional piece (external libraries, VAAPI/VDPAU,
# ffnvcodec, ...) is probed first — anything the machine does not provide
# is silently left out, so the script works on a bare macOS/Linux box.
# Install the external libraries with tooling/install-deps.sh first.
#
# Usage: tooling/ffmpeg/build-ffmpeg.sh [-j N] [--shared]
#   -j N       parallel make jobs (default: nproc/sysctl)
#   --shared   build shared libraries instead of the default static+PIC

set -euo pipefail

FFMPEG_VERSION="8.1"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PREFIX="$ROOT/.cache/ffmpeg"
SRC="$ROOT/.cache/ffmpeg-src"
JOBS="$( (nproc 2>/dev/null) || sysctl -n hw.ncpu)"
SHARED=0

while [ $# -gt 0 ]; do
	case "$1" in
		-j) JOBS="$2"; shift 2 ;;
		--shared) SHARED=1; shift ;;
		*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

# --- Helpers ---------------------------------------------------------------

have_pkg() { pkg-config --exists "$1" 2>/dev/null; }

# Adds `--enable-<flag>` when pkg-config finds <package>.
COND_LIBS=()
enable_if_pkg() { # <pkg-config name> <configure flag>
	if have_pkg "$1"; then
		COND_LIBS+=("--enable-$2")
		echo "  + $2 (found $1)"
	else
		echo "  - $2 (no $1, skipped)"
	fi
}

OS="$(uname -s)"

# Homebrew keeps everything under its own prefix, off the compiler's
# default search paths; several .pc files (lame, snappy, theora's ogg
# link line) are not self-sufficient, so add the prefix globally.
if [ "$OS" = Darwin ]; then
	BREW_PREFIX="$(brew --prefix 2>/dev/null || echo /opt/homebrew)"
	FLAGS_EXTRA=("--extra-cflags=-I$BREW_PREFIX/include" "--extra-ldflags=-L$BREW_PREFIX/lib")
else
	FLAGS_EXTRA=()
fi

# --- Source ----------------------------------------------------------------

mkdir -p "$ROOT/.cache"
if [ ! -d "$SRC" ]; then
	echo ">> cloning FFmpeg release/$FFMPEG_VERSION"
	git clone --depth=1 -b "release/$FFMPEG_VERSION" \
		https://github.com/FFmpeg/FFmpeg "$SRC"
fi

# --- Configure flags --------------------------------------------------------

FLAGS=(
	"--prefix=$PREFIX"
	--enable-gpl
	--enable-version3
	--disable-doc
	--disable-debug
	--disable-programs
	--enable-avcodec --enable-avformat --enable-avfilter
	--enable-avutil --enable-swscale --enable-swresample
)
if [ "$SHARED" = 1 ]; then
	FLAGS+=(--enable-shared --disable-static)
else
	FLAGS+=(--enable-static --disable-shared --enable-pic)
fi
FLAGS+=("${FLAGS_EXTRA[@]}")

echo ">> external codec/filter libraries (enabled when found):"
# Free-license external encoders/decoders. GPL-compatible only; the
# non-free ones (fdk-aac, OpenSSL in some jurisdictions, ...) stay off.
enable_if_pkg x264 libx264
enable_if_pkg x265 libx265
enable_if_pkg svt-av1 libsvtav1
enable_if_pkg dav1d libdav1d
enable_if_pkg vpx libvpx
enable_if_pkg opus libopus
enable_if_pkg vorbis libvorbis
enable_if_pkg theora libtheora
enable_if_pkg lame libmp3lame
# Homebrew's lame.pc points its include dir at include/lame while FFmpeg
# includes <lame/lame.h>, and its library dir is off the default search
# path; pass both explicitly.
if have_pkg lame; then
	FLAGS+=("--extra-cflags=-I$(pkg-config --variable=includedir lame)")
	FLAGS+=("--extra-ldflags=-L$(pkg-config --variable=libdir lame)")
fi
enable_if_pkg twolame libtwolame
enable_if_pkg speex libspeex
# Homebrew's openjpeg installs libopenjp2.pc outside the default
# pkg-config search path.
if [ -d /opt/homebrew/lib/pkgconfig/openjpeg ]; then
	export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-}:/opt/homebrew/lib/pkgconfig/openjpeg"
fi
enable_if_pkg libopenjp2 libopenjpeg
# openh264 is redundant for us (decode: FFmpeg's native h264; encode:
# x264) and snappy only feeds the hap encoder; neither MinGW package
# satisfies the static link — skip both on Windows.
if [ -z "${MSYSTEM:-}" ]; then
	enable_if_pkg openh264 libopenh264
	enable_if_pkg snappy libsnappy
fi
enable_if_pkg webp libwebp
enable_if_pkg xvid libxvid
enable_if_pkg kvazaar libkvazaar
enable_if_pkg shine libshine
enable_if_pkg gsm libgsm
enable_if_pkg opencore-amrnb libopencore-amrnb
enable_if_pkg opencore-amrwb libopencore-amrwb
enable_if_pkg ilbc libilbc
# Subtitles / text rendering (free).
enable_if_pkg freetype2 libfreetype
enable_if_pkg fribidi libfribidi
enable_if_pkg fontconfig libfontconfig
enable_if_pkg libass libass
# TLS for network protocols (GPL-compatible).
if have_pkg gnutls; then
	COND_LIBS+=("--enable-gnutls")
	echo "  + gnutls"
else
	echo "  - gnutls (skipped)"
fi
FLAGS+=("${COND_LIBS[@]}")

echo ">> hardware acceleration:"
case "$OS" in
	Darwin)
		# VideoToolbox/AudioToolbox ship with the OS SDK — always on.
		FLAGS+=(--enable-videotoolbox --enable-audiotoolbox)
		echo "  + videotoolbox, audiotoolbox"
		;;
	Linux)
		if have_pkg libva; then FLAGS+=(--enable-vaapi); echo "  + vaapi"; else echo "  - vaapi (no libva)"; fi
		if have_pkg vdpau; then FLAGS+=(--enable-vdpau); echo "  + vdpau"; else echo "  - vdpau (no vdpau)"; fi
		if have_pkg libdrm; then FLAGS+=(--enable-libdrm); echo "  + libdrm"; else echo "  - libdrm"; fi
		;;
	MINGW*|MSYS*|CYGWIN*)
		FLAGS+=(--enable-d3d11va --enable-dxva2 --enable-mediafoundation)
		echo "  + d3d11va, dxva2, mediafoundation"
		;;
esac
# NVIDIA (ffnvcodec headers are distribution-free; enable when present).
if [ -d /usr/local/cuda ] || pkg-config --exists ffnvcodec 2>/dev/null; then
	FLAGS+=(--enable-nvdec --enable-nvenc --enable-cuda-llvm)
	echo "  + nvdec/nvenc/cuda"
else
	echo "  - nvdec/nvenc/cuda (no ffnvcodec headers)"
fi

# --- Build ------------------------------------------------------------------

echo ">> configure"
cd "$SRC"
./configure "${FLAGS[@]}"

echo ">> make -j$JOBS"
make -j"$JOBS"
make install

# Marker for the CI cache: a restored tree without it is incomplete (a
# failed run's partial save) and is discarded/ rebuilt.
touch "$PREFIX/.build-complete"

cat <<EOF

Done. To build Oak against this FFmpeg:

  export FFMPEG_DIR="$PREFIX"
  cargo build

(Unset FFMPEG_DIR to go back to the system pkg-config FFmpeg.)

NOTE: ffmpeg-sys-next's \`static\` feature BUNDLES these archives into its
rlib at build time, and cargo does not track archive changes — after
rebuilding this FFmpeg you MUST force the re-bundle, or every binary
silently keeps the previous FFmpeg objects:

  cargo clean -p ffmpeg-sys-next
EOF
