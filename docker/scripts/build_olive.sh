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

git clone --depth 1 https://github.com/olive-editor/olive.git
cd olive
mkdir build
cd build
cmake .. -G "Ninja"
cmake --build .

cmake --install app --prefix appdir/usr

# TODO: Can the following libs be excluded?
#libQt5DBus.so,\
#libQt5MultimediaGstTools.so,\
#libQt5MultimediaWidgets.so,\

/usr/local/linuxdeployqt-x86_64.AppImage \
  appdir/usr/share/applications/org.olivevideoeditor.Olive.desktop \
  -appimage \
  -exclude-libs=\
libQt5Pdf.so,\
libQt5Qml.so,\
libQt5QmlModels.so,\
libQt5Quick.so,\
libQt5VirtualKeyboard.so \
  -bundle-non-qt-libs \
  -executable=appdir/usr/bin/crashpad_handler \
  -executable=appdir/usr/bin/minidump_stackwalk \
  -executable=appdir/usr/bin/olive-crashhandler \
  --appimage-extract-and-run

./Olive*.AppImage --appimage-extract-and-run --version
