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

# Installs the system dependencies of the Oak Rust workspace:
# the free-license external codec/filter libraries FFmpeg is configured
# with (see crates/oak-codec/Cargo.toml), plus the build tools. FFmpeg
# itself is built from source by tooling/ffmpeg/build-ffmpeg.sh (which
# installs into .cache/ffmpeg) and is NOT installed here.
#
# CI/CD runs this same script (see .github/workflows/ci.yml and
# .github/workflows/cd.yml), so local builds and the release packages
# share one dependency set; the Windows CI/CD jobs use the prebuilt
# FFmpeg archive instead and never touch the MSYS2 branch.
#
# Supported: Homebrew (macOS), MSYS2 UCRT64 (Windows), Debian/Ubuntu,
# Fedora, Arch. Run it yourself — nothing in the build invokes it
# automatically (it needs sudo on Linux).
#
# Usage: tooling/install-deps.sh

set -euo pipefail

run() { echo "+ $*"; "$@"; }

# Containers (the CI/CD jobs) run as root without sudo; use sudo only when
# the caller is not root.
if [ "$(id -u)" = "0" ]; then
	SUDO=()
else
	SUDO=(sudo)
fi

if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* || -n "${MSYSTEM:-}" ]]; then
	if [ "${MSYSTEM:-}" != "UCRT64" ]; then
		echo "Please run this from the MSYS2 UCRT64 shell (MSYSTEM=$MSYSTEM)." >&2
		exit 1
	fi
	# Pacman mirrors occasionally stall mid-download (CI hits "Operation
	# too slow" on .sig retrieval); retry the whole install a few times —
	# --needed makes each retry resume where the last one stopped.
	for attempt in 1 2 3; do
		if run pacman -S --needed --noconfirm \
			make diffutils \
			mingw-w64-ucrt-x86_64-toolchain mingw-w64-ucrt-x86_64-pkgconf \
			mingw-w64-ucrt-x86_64-nasm \
			mingw-w64-ucrt-x86_64-x264 mingw-w64-ucrt-x86_64-x265 \
			mingw-w64-ucrt-x86_64-dav1d mingw-w64-ucrt-x86_64-libvpx \
			mingw-w64-ucrt-x86_64-openjpeg2 \
			mingw-w64-ucrt-x86_64-libtheora mingw-w64-ucrt-x86_64-libwebp \
			mingw-w64-ucrt-x86_64-lame mingw-w64-ucrt-x86_64-opus \
			mingw-w64-ucrt-x86_64-libvorbis mingw-w64-ucrt-x86_64-speex \
			mingw-w64-ucrt-x86_64-snappy mingw-w64-ucrt-x86_64-libass \
			mingw-w64-ucrt-x86_64-freetype mingw-w64-ucrt-x86_64-fribidi \
			mingw-w64-ucrt-x86_64-fontconfig mingw-w64-ucrt-x86_64-gnutls
		then
			exit 0
		fi
		echo "pacman install attempt $attempt failed; retrying" >&2
		sleep 5
	done
	echo "pacman install failed after 3 attempts" >&2
	exit 1
fi

case "$(uname -s)" in
	Darwin)
		run brew install cmake pkg-config nasm \
			x264 x265 dav1d libvpx openjpeg theora webp \
			lame opus libvorbis speex snappy libass freetype fribidi \
			fontconfig gnutls
		;;
	Linux)
		if command -v apt-get >/dev/null; then
		run "${SUDO[@]}" apt-get update
		PKGS=(
			build-essential clang libclang-dev cmake python3 pkg-config nasm
			libx264-dev libx265-dev libdav1d-dev libvpx-dev
			libopenjp2-7-dev libtheora-dev libwebp-dev
			libmp3lame-dev libopus-dev libvorbis-dev libspeex-dev
			libsnappy-dev libass-dev libfreetype-dev libfribidi-dev
			libfontconfig-dev libgnutls28-dev
			libva-dev libdrm-dev
			git
		)
		# A single unavailable package (e.g. a codec library a minimal
		# image does not carry) must not kill the whole install: the
		# FFmpeg configure probes every optional piece, so degrade to
		# installing the rest individually.
		if ! run "${SUDO[@]}" apt-get install -y "${PKGS[@]}"; then
			echo "Some packages are unavailable here; installing the rest individually" >&2
			for pkg in "${PKGS[@]}"; do
				"${SUDO[@]}" apt-get install -y "$pkg" >/dev/null 2>&1 ||
					echo "skipping unavailable package: $pkg" >&2
			done
		fi
		# ffnvcodec headers (NVDEC for the project FFmpeg build) are NOT
		# in Debian/Ubuntu apt under a stable name: `libffnvcodec-dev`
		# was dropped from noble. The headers are distribution-free, so
		# install them from the official GitHub mirror (code.videolan.org
		# serves git behind an anti-bot challenge that CI runners hit).
		# Re-runs start from a clean checkout so the script stays
		# idempotent.
		rm -rf /tmp/nv-codec-headers
		run "${SUDO[@]}" git clone --depth 1 https://github.com/FFmpeg/nv-codec-headers.git /tmp/nv-codec-headers
		run "${SUDO[@]}" make -C /tmp/nv-codec-headers install PREFIX=/usr
		elif command -v dnf >/dev/null; then
			# x264/x265 are patent-encumbered: Fedora's own repositories
			# do not carry them, RPM Fusion does. Install its release
			# package first (fetched over HTTPS from the official mirror;
			# dnf skips the OpenPGP check for the commandline package and
			# imports the repository keys for everything it pulls after).
			if ! rpm -q rpmfusion-free-release >/dev/null 2>&1; then
				run "${SUDO[@]}" dnf install -y --setopt=install_weak_deps=False \
					"https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm"
			fi
			DNFPKGS=(
				gcc gcc-c++ clang clang-devel cmake python3 pkgconf-pkg-config nasm
				x264-devel x265-devel libdav1d-devel libvpx-devel
				openjpeg-devel libtheora-devel libwebp-devel
				lame-devel opus-devel libvorbis-devel speex-devel
				snappy-devel libass-devel freetype-devel fribidi-devel
				fontconfig-devel gnutls-devel libva-devel libdrm-devel
			)
			if ! run "${SUDO[@]}" dnf install -y --setopt=install_weak_deps=False \
				--setopt=max_parallel_downloads=16 "${DNFPKGS[@]}"; then
				echo "Some packages are unavailable here; installing the rest" >&2
				run "${SUDO[@]}" dnf install -y --skip-unavailable \
					--setopt=install_weak_deps=False --setopt=max_parallel_downloads=16 \
					"${DNFPKGS[@]}"
			fi
		elif command -v pacman >/dev/null; then
			run "${SUDO[@]}" pacman -S --needed --noconfirm base-devel clang cmake python pkgconf nasm \
				x264 x265 dav1d libvpx openjpeg2 libtheora libwebp \
				lame opus libvorbis speex snappy libass freetype2 fribidi \
				fontconfig gnutls ffnvcodec-headers libva libdrm
		else
			echo "Unsupported Linux distribution (need apt-get, dnf or pacman)." >&2
			exit 1
		fi
		;;
	*)
		echo "Unsupported platform: $(uname -s)" >&2
		exit 1
		;;
esac

echo "Done. Now run tooling/ffmpeg/build-ffmpeg.sh once, then 'cargo build'."
