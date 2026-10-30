// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! Build-time link configuration.

fn main() {
	if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
		// The static FFmpeg's external codec libraries pull in `-lz`, and
		// on some machines that resolves to a package-manager copy whose
		// install name is @rpath/libz.1.dylib; without an LC_RPATH entry
		// the binary dies at launch ("Library not loaded"). Map the rpath
		// at the real system library.
		println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib");
		println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
		println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../deps");
	}
}
