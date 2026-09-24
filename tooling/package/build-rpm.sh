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

# Build the .rpm: stage into a buildroot and let rpmbuild's automatic
# dependency discovery (find-requires) compute the Requires from the
# binaries' NEEDED entries. Run from the repo root after
# `cargo build --release`. Usage: tooling/package/build-rpm.sh <version>
set -euo pipefail

VERSION="${1:?usage: build-rpm.sh <version>}"
TOP=$(pwd)/target/pkg/rpm
rm -rf "$TOP"
mkdir -p "$TOP"/{BUILD,RPMS,SOURCES,SPECS,BUILDROOT}

# Stage the payload; the spec's %install copies it into rpmbuild's own
# %{buildroot}. rpm 4.20+ (Fedora 43) computes the buildroot itself and
# ignores a caller-supplied `buildroot` define, so the spec must install
# through the macro.
STAGE="$TOP/stage"
mkdir -p "$STAGE/usr/bin" "$STAGE/usr/share/applications" \
	"$STAGE/usr/share/icons/hicolor/512x512/apps" "$STAGE/usr/share/oak/i18n" \
	"$STAGE/usr/share/oak/icons" \
	"$STAGE/usr/share/icons/hicolor/scalable/apps"
install -m755 target/release/oak-editor target/release/oak-cli target/release/oak-worker \
	"$STAGE/usr/bin/"
install -m644 packaging/oak.desktop "$STAGE/usr/share/applications/oak.desktop"
install -m644 icons/icon.png "$STAGE/usr/share/icons/hicolor/512x512/apps/oak.png"
install -m644 Oak_Icon.svg "$STAGE/usr/share/icons/hicolor/scalable/apps/oak.svg"
install -m644 assets/i18n/*.yaml "$STAGE/usr/share/oak/i18n/"
# The theme-aware UI glyphs (resolved at runtime from
# /usr/share/oak/icons; see crates/oak-app/src/oakui/icons.rs).
cp -a assets/icons/. "$STAGE/usr/share/oak/icons/"

rpmbuild -bb \
	--define "_topdir $TOP" \
	--define "_version $VERSION" \
	--define "_oak_stage $STAGE" \
	--define "source_date_epoch_from_changelog 0" \
	--define "_binary_payload w6.zstdio" \
	tooling/package/oak.spec

find "$TOP/RPMS" -name '*.rpm' -exec mv {} target/release/ \;
ls target/release/*.rpm
