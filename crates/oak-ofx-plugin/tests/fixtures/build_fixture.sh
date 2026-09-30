#!/bin/bash
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

# Build the CI fixture OFX plugin into a real .ofx.bundle layout under
# "$1" (the directory the scan is pointed at via OFX_PLUGIN_PATH).
set -euo pipefail

OUT_DIR="${1:?usage: build_fixture.sh <output-dir>}"
SRC="$(cd "$(dirname "$0")" && pwd)/ci_test_plugin.c"
BUNDLE="$OUT_DIR/OakCiTest.ofx.bundle"

case "$(uname -s)" in
	Darwin)
		PLATFORM_DIR="Contents/MacOS"
		# .ofx bundles are Mach-O bundles (loadable modules).
		LINK_FLAGS=(-bundle)
		CC_BIN="${CC:-clang}"
		;;
	Linux)
		ARCH="$(uname -m)"
		case "$ARCH" in
			x86_64) PLATFORM_DIR="Contents/Linux-x86-64" ;;
			aarch64|arm64) PLATFORM_DIR="Contents/Linux-aarch64" ;;
			*) echo "unsupported Linux arch: $ARCH" >&2; exit 1 ;;
		esac
		LINK_FLAGS=(-shared -fPIC)
		CC_BIN="${CC:-cc}"
		;;
	*)
		echo "unsupported OS: $(uname -s)" >&2
		exit 1
		;;
esac

mkdir -p "$BUNDLE/$PLATFORM_DIR"
"$CC_BIN" -O2 "${LINK_FLAGS[@]}" "$SRC" -o "$BUNDLE/$PLATFORM_DIR/OakCiTest.ofx"
echo "fixture plugin built: $BUNDLE/$PLATFORM_DIR/OakCiTest.ofx"
