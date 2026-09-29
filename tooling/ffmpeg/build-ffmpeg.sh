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
# The build is fully self-contained: tooling/ffmpeg/build-deps.sh first
# builds every external codec/filter library from source as static-only
# archives into the SAME prefix, and FFmpeg is configured against that
# prefix only — no Homebrew/distro codec package appears on the final
# link line, so a package-manager upgrade can never break the link from
# under the build. The enabled set is FIXED (no opportunistic pkg-config
# probes), so every machine produces the same FFmpeg. TLS/network is
# disabled: an editor has no network inputs, and dropping the network
# stack drops the whole TLS (gnutls) dependency with it. Hardware
# acceleration still uses the OS/driver interfaces (VideoToolbox, VAAPI,
# libdrm, ffnvcodec), probed per host. Build tools come from
# tooling/install-deps.sh.
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

have_pkg() { pkg-config --exists "$1" 2>/dev/null; }

OS="$(uname -s)"

# --- External libraries --------------------------------------------------------
# Builds into the same prefix and self-gates on its stamp: a missing or
# stale stamp wipes the prefix (including this script's .build-complete
# marker, which is what forces the FFmpeg rebuild below), an up-to-date
# one is a no-op.
"$(dirname "$0")/build-deps.sh"

# --- Source ----------------------------------------------------------------

mkdir -p "$ROOT/.cache"
if [ ! -d "$SRC" ]; then
	echo ">> cloning FFmpeg release/$FFMPEG_VERSION"
	git clone --depth=1 -b "release/$FFMPEG_VERSION" \
		https://github.com/FFmpeg/FFmpeg "$SRC"
fi

# --- Configure flags --------------------------------------------------------

# Resolve the external libraries against the private prefix FIRST (they
# are all there); the system pkg-config path stays appended for the
# driver/OS interface probes below (libva, vdpau, libdrm, ffnvcodec).
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"

FLAGS=(
	"--prefix=$PREFIX"
	--enable-gpl
	--enable-version3
	--disable-doc
	--disable-debug
	--disable-programs
	# An editor has no network inputs; --disable-network removes every
	# network protocol and with it any need for a TLS library (gnutls).
	--disable-network
	--enable-avcodec --enable-avformat --enable-avfilter
	--enable-avutil --enable-swscale --enable-swresample
	# Static closure: probe with --static so the installed .pc files
	# carry the full transitive link line (Libs.private) that
	# crates/oak-ffmpeg-link/build.rs forwards to cargo.
	--pkg-config-flags=--static
	# lame ships no .pc file; every external lives in this prefix.
	"--extra-cflags=-I$PREFIX/include"
	"--extra-ldflags=-L$PREFIX/lib"
	# Fixed codec/filter set — deterministic across machines (the old
	# opportunistic probes are gone):
	--enable-libx264 --enable-libx265 --enable-libdav1d --enable-libvpx
	--enable-libopus --enable-libvorbis --enable-libtheora
	--enable-libmp3lame --enable-libspeex
	--enable-libopenjpeg --enable-libwebp --enable-libsnappy
	# Subtitles / text rendering:
	--enable-libfreetype --enable-libfribidi --enable-libass
	# xcbgrab (X11 screen capture) is an input the editor never uses,
	# and wherever a package manager happens to ship libxcb it drags
	# -lxcb/-lX11 onto the link line as system libraries.
	--disable-indev=xcbgrab
	# xlib/libxcb are [autodetect] even with xcbgrab disabled: on a macOS
	# host with Homebrew's libx11/libxcb installed, configure enables them
	# and stamps absolute Cellar -L paths into libavutil.pc. The editor
	# displays through its own UI and uses VAAPI via DRM on Linux, so X11
	# glue is never needed.
	--disable-xlib --disable-libxcb
)
if [ "$SHARED" = 1 ]; then
	FLAGS+=(--enable-shared --disable-static)
else
	FLAGS+=(--enable-static --disable-shared --enable-pic)
fi
if [ "$OS" = Linux ]; then
	# libass' font provider on Linux (macOS uses CoreText, Windows
	# DirectWrite; both are OS-provided).
	FLAGS+=(--enable-libfontconfig)
fi

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
		if have_pkg libdrm; then FLAGS+=(--enable-libdrm); echo "  + libdrm"; else echo "  - libdrm (no libdrm)"; fi
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
