#!/usr/bin/env bash
#
# Oak Video Editor - Non-Linear Video Editor
# Copyright (C) 2025 Olive CE Team
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
#





set -ex

git clone --depth 1 --branch "$OTIO_VERSION" https://github.com/PixarAnimationStudios/OpenTimelineIO.git
cd OpenTimelineIO

#pip install --prefix="${OLIVE_INSTALL_PREFIX}" .

mkdir build
cd build
cmake .. -G "Ninja" \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_CXX_STANDARD="${CXX_STANDARD}" \
  -DCMAKE_INSTALL_PREFIX="${OLIVE_INSTALL_PREFIX}" \
  -DOTIO_PYTHON_INSTALL=OFF
cmake --build .
cmake --install .

cd ../..
rm -rf OpenTimelineIO
