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

# Installs EVERY package-manager dependency of the Oak Rust workspace:
# the build tools (compilers, cmake, meson, ninja, nasm, pkg-config),
# the system libraries the workspace links (PipeWire/JACK/ALSA/PulseAudio/
# sndfile for cpal, GL/Vulkan/XKB for the wgpu windowing stack), the
# headless-test stack (xvfb, Mesa software Vulkan, fonts, gdb), and the
# packaging tools (dpkg-dev, rpm-build, makepkg's base-devel, librsvg for
# the app icon).
#
# What is deliberately NOT here: codec libraries. FFmpeg itself and every
# external codec/filter library it links are built from source as static
# archives by tooling/ffmpeg/build-deps.sh + tooling/ffmpeg/build-ffmpeg.sh
# (both install into .cache/ffmpeg). The only libraries that still come
# from the OS are the driver/hardware interfaces (libva, libdrm,
# ffnvcodec headers).
#
# CI/CD runs this same script as the ONLY dependency step (see
# .github/workflows/ci.yml and .github/workflows/cd.yml), so local builds,
# CI and the release packages share one tool set by construction; the
# Windows CI/CD jobs use the prebuilt FFmpeg archive instead and never
# touch the MSYS2 branch.
#
# Supported: Homebrew (macOS), MSYS2 UCRT64 (Windows), Debian/Ubuntu/
# openKylin, Fedora, Arch. Run it yourself — nothing in the build invokes
# it automatically (it needs sudo on Linux).
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
			make diffutils git curl \
			mingw-w64-ucrt-x86_64-toolchain mingw-w64-ucrt-x86_64-pkgconf \
			mingw-w64-ucrt-x86_64-nasm mingw-w64-ucrt-x86_64-cmake \
			mingw-w64-ucrt-x86_64-ninja mingw-w64-ucrt-x86_64-meson
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
		# Homebrew's pkgconf installs a `pkg-config` symlink, which is the
		# name crates/oak-ffmpeg-link/build.rs invokes; nasm/meson/ninja
		# are what the project FFmpeg build (tooling/ffmpeg/build-ffmpeg.sh
		# + build-deps.sh) requires. librsvg is the app-icon renderer
		# (rsvg-convert). The autotools cover any source tarball whose
		# shipped configure needs regenerating.
		run brew install git curl cmake meson ninja nasm pkg-config \
			librsvg autoconf automake libtool autoconf-archive
		;;
	Linux)
		if command -v apt-get >/dev/null; then
		run "${SUDO[@]}" apt-get update
		# One list for every apt-based distro (Debian/Ubuntu/openKylin).
		# Codec libraries are deliberately absent: they are built from
		# source by tooling/ffmpeg/build-deps.sh. libva/libdrm stay —
		# they are the GPU driver interface, not codec dependencies.
		#
		# Groups:
		#   toolchain + FFmpeg-deps build tools
		#   cpal audio backends (pipewire/jack/alsa/pulse/sndfile)
		#   wgpu windowing stack (GL/Vulkan/XKB)
		#   headless tests (xvfb, Mesa software Vulkan, fonts, gdb)
		#   packaging (dpkg-dev, librsvg for the icon, patchelf)
		PKGS=(
			build-essential clang libclang-dev cmake meson ninja-build
			python3 pkgconf pkg-config nasm curl git zip unzip tar patch
			xz-utils autoconf autoconf-archive automake libtool
			libpipewire-0.3-dev libspa-0.2-dev libjack-jackd2-dev
			libasound2-dev libpulse-dev libsndfile1-dev
			libgl-dev libglvnd-dev libgl1-mesa-dev libgl1-mesa-dri
			mesa-vulkan-drivers libvulkan-dev
			libxkbcommon-dev libxkbcommon-x11-dev
			xvfb gdb file fonts-dejavu-core icc-profiles-free
			dpkg-dev librsvg2-bin patchelf
			libva-dev libdrm-dev
		)
		# A single unavailable package (e.g. pkgconf on openKylin, which
		# ships pkg-config under its classic name) must not kill the whole
		# install: degrade to installing the rest individually.
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
			# No RPM Fusion anymore: the patent-encumbered codec libraries
			# (x264/x265) it provided are now built from source by
			# tooling/ffmpeg/build-deps.sh, so only build tools, the
			# workspace system libraries and the GPU driver interfaces
			# come from dnf. Weak dependencies (docs, optional tooling)
			# are skipped and downloads run wide. The perl modules are
			# what the from-source builds' configure scripts call.
			DNFPKGS=(
				gcc gcc-c++ clang clang-devel cmake meson ninja-build
				python3 pkgconf-pkg-config nasm curl git zip unzip tar
				patch xz which autoconf autoconf-archive automake libtool
				perl-IPC-Cmd perl-FindBin perl-File-Basename perl-File-Compare
				perl-File-Copy perl-File-Path perl-File-Temp perl-Time-Piece
				pipewire-devel jack-audio-connection-kit-devel
				alsa-lib-devel pulseaudio-libs-devel libsndfile-devel
				mesa-libGL-devel mesa-vulkan-drivers
				vulkan-headers vulkan-loader-devel
				libxkbcommon-devel libxkbcommon-x11-devel
				xorg-x11-server-Xvfb xorg-x11-xauth gdb file dejavu-sans-fonts
				rpm-build librsvg2-tools
				libva-devel libdrm-devel
			)
			if ! run "${SUDO[@]}" dnf install -y --setopt=install_weak_deps=False \
				--setopt=max_parallel_downloads=16 "${DNFPKGS[@]}"; then
				echo "Some packages are unavailable here; installing the rest" >&2
				run "${SUDO[@]}" dnf install -y --skip-unavailable \
					--setopt=install_weak_deps=False --setopt=max_parallel_downloads=16 \
					"${DNFPKGS[@]}"
			fi
		elif command -v pacman >/dev/null; then
			run "${SUDO[@]}" pacman -S --needed --noconfirm \
				base-devel clang cmake meson ninja \
				python pkgconf nasm curl git zip unzip tar patch xz which \
				autoconf autoconf-archive automake libtool \
				pipewire jack2 alsa-lib libpulse libsndfile \
				mesa vulkan-headers vulkan-icd-loader \
				libxkbcommon libxkbcommon-x11 \
				xorg-server-xvfb xorg-xauth gdb file ttf-dejavu \
				librsvg \
				ffnvcodec-headers libva libdrm
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
