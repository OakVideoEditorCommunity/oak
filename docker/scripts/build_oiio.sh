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

# TODO: Move to install_yumpackages.sh
yum install -y \
    libtiff \
    libtiff-devel \
    libpng \
    libpng-devel

git clone --depth 1 --branch "$JPEG_TURBO_VERSION" https://github.com/libjpeg-turbo/libjpeg-turbo
cd libjpeg-turbo
mkdir build
cd build
cmake -DCMAKE_INSTALL_PREFIX="${OLIVE_INSTALL_PREFIX}" \
      ..
make -j$(nproc)
make install
cd ../..
rm -rf libjpeg-turbo


# TODO: Make OIIO pick up WebP (missing: WEBPDEMUX_LIBRARY)
git clone --depth 1 --branch "$OIIO_VERSION" https://github.com/OpenImageIO/oiio.git
cd oiio

mkdir build
cd build
cmake -DCMAKE_INSTALL_PREFIX="${OLIVE_INSTALL_PREFIX}" \
      -DOIIO_BUILD_TOOLS=OFF \
      -DOIIO_BUILD_TESTS=OFF \
      -DVERBOSE=ON \
      -DUSE_PYTHON=0 \
      -DBoost_NO_BOOST_CMAKE=ON \
      ..
make -j$(nproc)
make install

cd ../..
rm -rf oiio
