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

# TODO: Check if this causes any problems. ASWF doesn't run a yum update.
yum update -y

# TODO: Add deps of deps which are explicitly listed in aswf-docker?
yum install --setopt=tsflags=nodocs -y \
    bzip2-devel \
    cups-libs \
    freetype-devel \
    giflib-devel \
    gstreamer1 \
    gstreamer1-devel \
    gstreamer1-plugins-bad-free \
    gstreamer1-plugins-bad-free-devel \
    libcurl-devel \
    libicu-devel \
    libmng-devel \
    LibRaw-devel \
    libwebp-devel \
    libXcomposite \
    libXcomposite-devel \
    libXcursor \
    libXcursor-devel \
    libxkbcommon \
    libxkbcommon-devel \
    libxkbcommon-x11-devel \
    libXScrnSaver \
    libXScrnSaver-devel \
    mesa-libGL-devel \
    numactl-devel \
    openjpeg2-devel \
    pciutils-devel \
    pulseaudio-libs \
    pulseaudio-libs-devel \
    python3-tkinter \
    xcb-util-image \
    xcb-util-image-devel \
    xcb-util-keysyms \
    xcb-util-keysyms-devel \
    xcb-util-renderutil \
    xcb-util-renderutil-devel \
    xcb-util-wm \
    xcb-util-wm-devel \
    zlib-devel

# This is needed for Xvfb to function properly.
dbus-uuidgen > /etc/machine-id

yum -y groupinstall "Development Tools"

# TODO: Below code installs the obsolete devtoolset-6.
#       Unclear which devtoolset it will be for VFX platform CY2021:
#       https://groups.google.com/forum/#!topic/vfx-platform-discuss/_-_CPw1fD3c

yum install -y --setopt=tsflags=nodocs centos-release-scl-rh yum-utils

if [[ $DTS_VERSION == 6 ]]; then
    # Use the centos vault as the original devtoolset-6 is not part of CentOS-7 anymore
    sed -i 's/7/7.6.1810/g; s|^#\s*\(baseurl=http://\)mirror|\1vault|g; /mirrorlist/d' /etc/yum.repos.d/CentOS-SCLo-*.repo
fi

yum install -y --setopt=tsflags=nodocs \
    "devtoolset-$DTS_VERSION-toolchain"

yum install -y epel-release

# Additional package that are not found initially
yum install -y \
    rh-git218 \
    portaudio-devel
#   lame-devel
#   libcaca-devel \
#   libdb4-devel \
#   libdc1394-devel \
#   openssl11-devel \
#   p7zip \
#   yasm-devel \
#   zvbi-devel

# TODO: Does clearing the cache have any negative side effects?
yum clean all

# HACK: Qt5GuiConfigExtras.cmake expects libGL.so in /usr/local/lib64 but it gets installed to /usr/lib64
ln -s /usr/lib64/libGL.so /usr/local/lib64/
# Alternatively, we could edit /usr/local/lib/cmake/Qt5Gui/Qt5GuiConfigExtras.cmake
# - _qt5gui_find_extra_libs(OPENGL "/usr/local/lib64/libGL.so" "" "")
# + _qt5gui_find_extra_libs(OPENGL "/usr/lib64/libGL.so" "" "")
