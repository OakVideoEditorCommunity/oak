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

# Build the .deb by hand: stage the release binaries + resources, compute
# the FULL runtime dependency set with dpkg-shlibdeps (Debian-family names
# of the build distro), and pack with dpkg-deb. Run from the repo root
# after `cargo build --release`.
#
# Usage: tooling/package/build-deb.sh <version> [<variant>]
#   <variant> marks the build distro in the package version and file name:
#   CD passes "debian" for the general build and "openkylin" for the
#   openKylin builds. The "+" suffix is valid Debian version syntax and
#   sorts above the plain version.
set -euo pipefail

VERSION="${1:?usage: build-deb.sh <version> [<variant>]}"
VARIANT="${2:-}"
if [ -n "$VARIANT" ]; then
	DEB_VERSION="${VERSION}+${VARIANT}"
else
	DEB_VERSION="$VERSION"
fi
ARCH="$(dpkg --print-architecture)"
STAGING=target/pkg/deb
rm -rf "$STAGING"
mkdir -p "$STAGING/usr/bin" "$STAGING/usr/share/applications" \
	"$STAGING/usr/share/icons/hicolor/512x512/apps" "$STAGING/usr/share/oak/i18n" \
	"$STAGING/usr/share/oak/icons" \
	"$STAGING/usr/share/icons/hicolor/scalable/apps" "$STAGING/DEBIAN"

install -m755 target/release/oak-editor target/release/oak-cli target/release/oak-worker \
	"$STAGING/usr/bin/"
install -m644 packaging/oak.desktop "$STAGING/usr/share/applications/oak.desktop"
if [ -f icons/icon.png ]; then
	install -m644 icons/icon.png "$STAGING/usr/share/icons/hicolor/512x512/apps/oak.png"
else
	# The icon step warns and skips when rsvg-convert is unavailable;
	# the scalable icon still installs.
	echo "warning: icons/icon.png missing; shipping the scalable icon only"
fi
install -m644 Oak_Icon.svg "$STAGING/usr/share/icons/hicolor/scalable/apps/oak.svg"
install -m644 assets/i18n/*.yaml "$STAGING/usr/share/oak/i18n/"
# The theme-aware UI glyphs (resolved at runtime from
# /usr/share/oak/icons; see crates/oak-app/src/oakui/icons.rs).
cp -a assets/icons/. "$STAGING/usr/share/oak/icons/"

# vcpkg's libva/libva-drm are shared libraries that FFmpeg links
# dynamically. Debian 12 / openKylin carry an older libva than FFmpeg 8
# expects (vaMapBuffer2), so ship vcpkg's copies next to the app and give
# the executables a relative RUNPATH (`$ORIGIN`) — a system libva can then
# not shadow them. `VCPKG_LIB` is the vcpkg_installed/<triplet>/lib dir
# (CD passes it; a local build without it just skips the bundle).
if [ -n "${VCPKG_LIB:-}" ]; then
	mkdir -p "$STAGING/usr/lib/oak-editor"
	bundled=0
	for so in "$VCPKG_LIB"/libva*.so.* "$VCPKG_LIB"/libdrm*.so.*; do
		[ -f "$so" ] || continue
		install -m755 "$so" "$STAGING/usr/lib/oak-editor/"
		bundled=1
	done
	if [ "$bundled" = 1 ]; then
		for bin in "$STAGING"/usr/bin/*; do
			patchelf --set-rpath '$ORIGIN/../lib/oak-editor' "$bin"
		done
	fi
fi

# The full shlib dependency set (FFmpeg/OCIO are statically linked, so
# mostly base-OS packages appear). dpkg-shlibdeps only runs inside a
# Debian source tree, so give it a synthetic one;
# `--ignore-missing-info` skips libraries no distro package provides
# (those are the vcpkg copies bundled above).
DEPS_DIR=target/pkg/deb-deps
rm -rf "$DEPS_DIR"
mkdir -p "$DEPS_DIR/debian"
cat > "$DEPS_DIR/debian/control" <<'EOF'
Source: oak-editor
Section: video
Priority: optional
Maintainer: Oak Team
Standards-Version: 4.6.0

Package: oak-editor
Architecture: any
Description: Oak Video Editor
EOF
STAGING_ABS="$PWD/$STAGING"
DEPS=$(cd "$DEPS_DIR" && for bin in "$STAGING_ABS"/usr/bin/*; do
	dpkg-shlibdeps -O --ignore-missing-info "$bin" || exit 1
done \
	| sed 's/^shlibs:Depends=//' | tr ',' '\n' | sed 's/^ //;s/ $//' | sort -u \
	| paste -sd, -)
echo "declared deps: $DEPS"

cat > "$STAGING/DEBIAN/control" <<EOF
Package: oak-editor
Version: $DEB_VERSION
Section: video
Priority: optional
Architecture: $ARCH
Maintainer: Oak Team
Depends: $DEPS
Description: Oak Video Editor — a free, open-source non-linear video editor
 Oak is a non-linear video editor written in Rust (OpenFX plug-in host,
 proxy editing, multicam, hardware decoding).
EOF

dpkg-deb --root-owner-group --build "$STAGING" "target/release/oak-editor_${DEB_VERSION}_${ARCH}.deb"
echo "built target/release/oak-editor_${DEB_VERSION}_${ARCH}.deb"
