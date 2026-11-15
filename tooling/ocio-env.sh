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

# Print the OpenColorIO build environment for cargo, one VAR=value per
# line — append to $GITHUB_ENV in CI/CD (`tooling/ocio-env.sh >> "$GITHUB_ENV"`)
# or eval locally.
#
# Policy: VENDORED OpenColorIO, statically linked, everywhere it builds.
# ocio-rs 0.3 (crates.io, required by Quercus 0.7.2) builds its vendored
# OpenColorIO/yaml-cpp on GCC >= 16; the old 0.2.1 <cstdint> backport fork
# is no longer patched in.
# Windows/MinGW is the exception: the vendored OCIO source needs
# MSVC-only constructs, so the MSYS2 system package is used instead —
# statically when it ships libOpenColorIO.a, dynamically otherwise (the
# CD packaging then bundles the DLL next to the binaries).
#
# Pre-set OCIO_RS_LINK / OCIO_INSTALL_DIR are honored, never clobbered.

set -euo pipefail

if [ -n "${OCIO_RS_LINK:-}" ]; then
	# The caller already chose a link mode; only make sure the real
	# bridge (not the stub) is on.
	echo "OCIO_RS_ENABLE_REAL=1"
	exit 0
fi

echo "OCIO_RS_ENABLE_REAL=1"

case "$(uname -s)" in
	MINGW* | MSYS* | CYGWIN*)
		if pkg-config --exists 'OpenColorIO >= 2.5' 2>/dev/null; then
			libdir=$(pkg-config --variable=libdir OpenColorIO)
			prefix=$(pkg-config --variable=prefix OpenColorIO)
			link=dynamic
			for dir in "$libdir" "$prefix/lib" "$prefix"; do
				if [ -f "$dir/libOpenColorIO.a" ]; then
					link=static
					break
				fi
			done
			echo "OCIO_INSTALL_DIR=$prefix"
			echo "OCIO_RS_LINK=$link"
			exit 0
		fi
		echo "error: no system OpenColorIO >= 2.5 on Windows, and the vendored" >&2
		echo "source does not build with MinGW — cannot configure OCIO." >&2
		exit 1
		;;
	*)
		# Vendored-source static build (ocio-sys `bundled`; no
		# OCIO_INSTALL_DIR).
		echo "OCIO_RS_LINK=static"
		;;
esac
