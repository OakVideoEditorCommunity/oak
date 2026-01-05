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

mkdir ocio
cd ocio

git clone --depth 1 --branch "${OCIO_VERSION}" https://github.com/AcademySoftwareFoundation/OpenColorIO.git
cd OpenColorIO

mkdir build
cd build
cmake \
    -DCMAKE_INSTALL_PREFIX="${OLIVE_INSTALL_PREFIX}" \
    -DCMAKE_BUILD_TYPE=RelWithDebInfo \
    -DOCIO_BUILD_APPS=OFF \
    -DOCIO_BUILD_NUKE=OFF \
    -DOCIO_BUILD_DOCS=OFF \
    -DOCIO_BUILD_TESTS=OFF \
    -DOCIO_BUILD_GPU_TESTS=OFF \
    -DOCIO_USE_HEADLESS=OFF \
    -DOCIO_BUILD_PYTHON=OFF \
    -DOCIO_BUILD_JAVA=OFF \
    -DOCIO_WARNING_AS_ERROR=OFF \
    -DOCIO_INSTALL_EXT_PACKAGES=ALL \
    ..
make -j$(nproc)
make install

cd ../..

curl --location "https://github.com/imageworks/OpenColorIO-Configs/archive/v${OCIO_CONFIGS_VERSION}.tar.gz" -o "ocio-configs.tar.gz"
tar -zxf ocio-configs.tar.gz
cd "OpenColorIO-Configs-${OCIO_CONFIGS_VERSION}"

mkdir "${OLIVE_INSTALL_PREFIX}/openColorIO"
cp nuke-default/config.ocio "${OLIVE_INSTALL_PREFIX}/openColorIO/"
cp -r nuke-default/luts "${OLIVE_INSTALL_PREFIX}/openColorIO/"

cd ../..
rm -rf ocio
