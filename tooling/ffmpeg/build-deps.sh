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

# Builds every external library the project FFmpeg links against as
# STATIC-ONLY (+PIC) archives into .cache/ffmpeg — the same prefix
# tooling/ffmpeg/build-ffmpeg.sh installs into. The produced FFmpeg is
# then self-contained: no Homebrew/distro codec package appears on the
# final link line (the libopenh264 soname problem), and a package-manager
# upgrade can no longer break the link from under the build. TLS/network
# is out of scope for an editor (build-ffmpeg.sh configures
# --disable-network), so there is deliberately no gnutls here. Hardware
# acceleration keeps using the OS/driver interface libraries
# (VideoToolbox, VAAPI, libdrm), which are probed by build-ffmpeg.sh and
# not built here.
#
# Invoked by tooling/ffmpeg/build-ffmpeg.sh when the deps stamp is
# missing or stale; can also be run standalone. Each library is marked
# under $PREFIX/.deps/<name> after a successful install, so a re-run
# resumes where a failed one stopped; a missing/stale .deps-complete
# stamp wipes the prefix and rebuilds everything (the stamp also guards
# against mixing this tree with a prefix built from system libraries).

set -euo pipefail

# Bump when the dependency set, a version or a build flag changes: every
# consumer (including the CI FFmpeg cache) rebuilds from scratch.
DEPS_STAMP="3"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PREFIX="$ROOT/.cache/ffmpeg"
DEPS_SRC="$ROOT/.cache/ffmpeg-deps-src"
JOBS="$( (nproc 2>/dev/null) || sysctl -n hw.ncpu)"
OS="$(uname -s)"

# --- Stamp gate ---------------------------------------------------------------

if [ -f "$PREFIX/.deps-complete" ] && [ "$(cat "$PREFIX/.deps-complete")" = "$DEPS_STAMP" ]; then
	echo ">> deps already built (stamp $DEPS_STAMP); nothing to do"
	exit 0
fi
if [ -d "$PREFIX" ]; then
	echo ">> deps stamp missing/stale — wiping $PREFIX for a clean rebuild"
	rm -rf "$PREFIX"
fi
mkdir -p "$PREFIX" "$DEPS_SRC"

# --- Environment ---------------------------------------------------------------

# Every probe below must resolve against the private prefix FIRST; the
# system pkg-config path stays appended for the few things that are
# legitimately system-provided (none during the deps build itself, but
# ffmpeg's configure follows this script).
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"

: "${CFLAGS:=-O2}"
: "${CXXFLAGS:=-O2}"
export CFLAGS="$CFLAGS -fPIC" CXXFLAGS="$CXXFLAGS -fPIC"
# Autotools probes resolve sibling deps (ogg for vorbis/theora, freetype
# for libass) through these as well as through pkg-config.
export CPPFLAGS="${CPPFLAGS:-} -I$PREFIX/include"
export LDFLAGS="${LDFLAGS:-} -L$PREFIX/lib"

if [ "$OS" = Darwin ]; then
	# Match the Rust link line (-mmacosx-version-min); honored by clang,
	# cmake and meson alike.
	export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
fi

# --- Helpers ---------------------------------------------------------------

marker() { [ -f "$PREFIX/.deps/$1" ]; }
mark() { mkdir -p "$PREFIX/.deps"; touch "$PREFIX/.deps/$1"; }

# fetch_tar <name> <url>: download (cached in $DEPS_SRC; partial files
# resume) and extract to $DEPS_SRC/<name>, stripping the tarball's
# top-level directory.
fetch_tar() { # <name> <url>
	local name="$1" url="$2"
	local tarball="$DEPS_SRC/$(basename "$url")"
	if [ ! -f "$tarball" ]; then
		echo ">> download $name: $url"
		curl -fSL --retry 5 --retry-delay 2 --retry-all-errors -C - \
			-o "$tarball" "$url"
	fi
	rm -rf "${DEPS_SRC:?}/$name"
	mkdir -p "$DEPS_SRC/$name"
	tar -xf "$tarball" --strip-components=1 -C "$DEPS_SRC/$name"
}

# fetch_git <name> <repo> <branch-or-tag>: shallow clone into
# $DEPS_SRC/<name> (reused across runs).
fetch_git() { # <name> <repo> <ref>
	local name="$1" repo="$2" ref="$3"
	if [ ! -d "$DEPS_SRC/$name" ]; then
		echo ">> clone $name ($ref)"
		git clone --depth=1 -b "$ref" "$repo" "$DEPS_SRC/$name"
	fi
}

# Old release tarballs (theora 2010, lame 2017, ...) ship config.guess/
# config.sub that predate aarch64-apple-darwin and cannot even guess the
# build type on Apple Silicon. Refresh both files from the canonical
# savannah repo (cached in $DEPS_SRC) before configuring.
refresh_config_guess() { # <source dir>
	local f
	for f in config.guess config.sub; do
		if [ ! -f "$DEPS_SRC/.$f" ]; then
			curl -fSL --retry 3 --retry-delay 2 \
				-o "$DEPS_SRC/.$f" "https://git.savannah.gnu.org/cgit/config.git/plain/$f"
		fi
	done
	find "$1" -name config.guess -exec cp "$DEPS_SRC/.config.guess" {} \; \
		-o -name config.sub -exec cp "$DEPS_SRC/.config.sub" {} \;
}

# configure_build <name> <url> [configure args...]
configure_build() {
	local name="$1" url="$2"
	shift 2
	if marker "$name"; then echo ">> $name: already built, skipping"; return; fi
	echo ">> $name: configure+make"
	fetch_tar "$name" "$url"
	refresh_config_guess "$DEPS_SRC/$name"
	# Per-source pre-configure fixups (declare pre_configure_<name> below).
	if declare -F "pre_configure_$name" >/dev/null; then
		"pre_configure_$name" "$DEPS_SRC/$name"
	fi
	(
		cd "$DEPS_SRC/$name"
		./configure --prefix="$PREFIX" --libdir="$PREFIX/lib" \
			--enable-static --disable-shared "$@"
		make -j"$JOBS"
		make install
	)
	mark "$name"
}

# configure_build_git <name> <repo> <ref> [configure args...] — for
# projects without release tarballs (x264, libvpx); their configure
# scripts live in the repo (no autoreconf needed).
configure_build_git() {
	local name="$1" repo="$2" ref="$3"
	shift 3
	if marker "$name"; then echo ">> $name: already built, skipping"; return; fi
	echo ">> $name: configure+make (git)"
	fetch_git "$name" "$repo" "$ref"
	(
		cd "$DEPS_SRC/$name"
		./configure --prefix="$PREFIX" --libdir="$PREFIX/lib" \
			--enable-static --disable-shared "$@"
		make -j"$JOBS"
		make install
	)
	mark "$name"
}

# cmake_build <name> <url> [source-subdir] [cmake args...]
cmake_build() {
	local name="$1" url="$2" subdir="${3:-}"
	if [ $# -ge 3 ]; then shift 3; else shift 2; fi
	if marker "$name"; then echo ">> $name: already built, skipping"; return; fi
	echo ">> $name: cmake"
	fetch_tar "$name" "$url"
	# Per-source pre-configure fixups (declare pre_configure_<name> below).
	if declare -F "pre_configure_$name" >/dev/null; then
		"pre_configure_$name" "$DEPS_SRC/$name"
	fi
	(
		cd "$DEPS_SRC/$name${subdir:+/$subdir}"
		cmake -S . -B build \
			-DCMAKE_INSTALL_PREFIX="$PREFIX" \
			-DCMAKE_INSTALL_LIBDIR=lib \
			-DCMAKE_BUILD_TYPE=Release \
			-DBUILD_SHARED_LIBS=OFF \
			-DCMAKE_POSITION_INDEPENDENT_CODE=ON \
			-DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
			"$@"
		cmake --build build -j "$JOBS"
		cmake --install build
	)
	mark "$name"
}

# meson_build <name> <url> [meson args...]
meson_build() {
	local name="$1" url="$2"
	shift 2
	if marker "$name"; then echo ">> $name: already built, skipping"; return; fi
	echo ">> $name: meson"
	fetch_tar "$name" "$url"
	(
		cd "$DEPS_SRC/$name"
		meson setup build --prefix="$PREFIX" --libdir=lib \
			--default-library=static --buildtype=release "$@"
		meson compile -C build
		meson install -C build
	)
	mark "$name"
}

# --- Subtitle / text stack -----------------------------------------------------

# freetype WITHOUT its optional integrations: each of zlib/bzip2/png/
# brotli/harfbuzz would leak into freetype2.pc's Libs.private and onto
# the final link line as a system library.
configure_build freetype \
	https://download.savannah.gnu.org/releases/freetype/freetype-2.13.3.tar.xz \
	--with-zlib=no --with-bzip2=no --with-png=no --with-harfbuzz=no --with-brotli=no

meson_build harfbuzz \
	https://github.com/harfbuzz/harfbuzz/releases/download/10.1.0/harfbuzz-10.1.0.tar.xz \
	-Dfreetype=enabled -Dglib=disabled -Dgobject=disabled -Dcairo=disabled \
	-Dicu=disabled -Dchafa=disabled -Dtests=disabled -Ddocs=disabled \
	-Dutilities=disabled -Dintrospection=disabled

meson_build fribidi \
	https://github.com/fribidi/fribidi/releases/download/v1.0.16/fribidi-1.0.16.tar.xz \
	-Dbin=false -Dtests=false -Ddocs=false

# libass' Unicode line breaker (UAX #14). libass puts it in the PUBLIC
# Requires of libass.pc, so without a private copy FFmpeg's configure
# resolves it from the system pkg-config path and stamps an absolute
# Homebrew Cellar path into libavfilter.pc.
configure_build libunibreak \
	https://github.com/adah1972/libunibreak/releases/download/libunibreak_6_1/libunibreak-6.1.tar.gz

# libass' system font provider: CoreText on macOS, DirectWrite on
# Windows, fontconfig on Linux — fontconfig (and its expat) is only
# built where libass actually needs it.
if [ "$OS" = Linux ]; then
	configure_build expat \
		https://github.com/libexpat/libexpat/releases/download/R_2_6_4/expat-2.6.4.tar.gz \
		--without-docbook
	configure_build fontconfig \
		https://www.freedesktop.org/software/fontconfig/release/fontconfig-2.15.0.tar.gz \
		--disable-docs --disable-nls
	configure_build libass \
		https://github.com/libass/libass/releases/download/0.17.3/libass-0.17.3.tar.gz \
		--disable-test
else
	configure_build libass \
		https://github.com/libass/libass/releases/download/0.17.3/libass-0.17.3.tar.gz \
		--disable-test --disable-fontconfig
fi

# --- Audio codecs ---------------------------------------------------------------

configure_build ogg \
	https://downloads.xiph.org/releases/ogg/libogg-1.3.5.tar.gz

# libvorbis 1.3.7's configure injects -force_cpusubtype_ALL into the
# link flags on Darwin, which current Xcode linkers reject (Homebrew's
# formula patches the same flag out).
pre_configure_vorbis() {
	sed -i.bak 's/-force_cpusubtype_ALL//g' "$1/configure" && rm -f "$1/configure.bak"
}

configure_build vorbis \
	https://downloads.xiph.org/releases/vorbis/libvorbis-1.3.7.tar.gz

configure_build theora \
	https://downloads.xiph.org/releases/theora/libtheora-1.1.1.tar.bz2 \
	--disable-examples --disable-spec

# Lame's home is SourceForge, whose mirror redirects stall badly from
# some networks (observed: the transfer trickling at bytes/sec then
# dying); the Debian pool carries the identical release tarball.
configure_build lame \
	https://deb.debian.org/debian/pool/main/l/lame/lame_3.100.orig.tar.gz \
	--disable-frontend

configure_build speex \
	https://downloads.xiph.org/releases/speex/speex-1.2.1.tar.gz \
	--disable-binaries

cmake_build opus \
	https://downloads.xiph.org/releases/opus/opus-1.5.2.tar.gz "" \
	-DOPUS_BUILD_PROGRAMS=OFF -DOPUS_BUILD_TESTING=OFF

# --- Video codecs / image ---------------------------------------------------------

# x264 publishes no release tarballs; github.com/mirror/x264 mirrors the
# videolan repo (code.videolan.org's git is behind an anti-bot challenge
# CI runners hit — see tooling/install-deps.sh).
configure_build_git x264 \
	https://github.com/mirror/x264.git stable \
	--enable-pic --disable-cli

# x265 3.6 pins CMP0025/CMP0054 to OLD; CMake 4 removed OLD behavior for
# these policies, and -DCMAKE_POLICY_VERSION_MINIMUM only relaxes
# cmake_minimum_required(), not explicit cmake_policy(SET ... OLD).
# Switching CMP0025 to NEW changes the Apple Clang compiler ID from "Clang"
# to "AppleClang", and x265's `STREQUAL "Clang"` test then misses — CLANG
# (hence GCC) never gets set, the whole if(GCC) block is skipped, and no
# NEON .S objects are compiled while -DHAVE_NEON is still defined, leaving
# libx265.a with undefined _x265_*_neon symbols (FFmpeg's configure then
# fails its link test with a misleading "x265 not found using pkg-config").
# Upstream 4.x fixed the detection with MATCHES "Clang"; do the same here.
pre_configure_x265() {
	sed -i.bak \
		-e 's/cmake_policy(SET CMP0025 OLD)/cmake_policy(SET CMP0025 NEW)/' \
		-e 's/cmake_policy(SET CMP0054 OLD)/cmake_policy(SET CMP0054 NEW)/' \
		-e 's/^\( *if(\${CMAKE_CXX_COMPILER_ID}\) STREQUAL "Clang")/\1 MATCHES "Clang")/' \
		"$1/source/CMakeLists.txt" && rm -f "$1/source/CMakeLists.txt.bak"
}

cmake_build x265 \
	https://bitbucket.org/multicoreware/x265_git/get/3.6.tar.gz source \
	-DENABLE_SHARED=OFF -DENABLE_CLI=OFF

meson_build dav1d \
	https://downloads.videolan.org/pub/videolan/dav1d/1.5.1/dav1d-1.5.1.tar.xz \
	-Denable_tools=false -Denable_tests=false

configure_build_git vpx \
	https://github.com/webmproject/libvpx.git v1.15.1 \
	--enable-pic --disable-examples --disable-tools --disable-docs --disable-unit-tests

cmake_build openjpeg \
	https://github.com/uclouvain/openjpeg/archive/refs/tags/v2.5.4.tar.gz "" \
	-DBUILD_CODEC=OFF -DBUILD_TESTING=OFF

cmake_build webp \
	https://storage.googleapis.com/downloads.webmproject.org/releases/webp/libwebp-1.4.0.tar.gz "" \
	-DWEBP_BUILD_ANIM_UTILS=OFF -DWEBP_BUILD_CWEBP=OFF -DWEBP_BUILD_DWEBP=OFF \
	-DWEBP_BUILD_GIF2WEBP=OFF -DWEBP_BUILD_IMG2WEBP=OFF -DWEBP_BUILD_VWEBP=OFF \
	-DWEBP_BUILD_WEBPINFO=OFF -DWEBP_BUILD_WEBPMUX=OFF -DWEBP_BUILD_EXTRAS=OFF

# snappy only feeds FFmpeg's hap encoder.
cmake_build snappy \
	https://github.com/google/snappy/archive/refs/tags/1.2.1.tar.gz "" \
	-DSNAPPY_BUILD_TESTS=OFF -DSNAPPY_BUILD_BENCHMARKS=OFF

# --- Done ---------------------------------------------------------------------

echo "$DEPS_STAMP" > "$PREFIX/.deps-complete"
echo ">> all FFmpeg dependency libraries built statically into $PREFIX"
