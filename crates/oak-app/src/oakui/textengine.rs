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

//! SDK entry points for the shared text backend. Workers install the
//! same implementation in their own processes.

use oak_engine_rust_sdk::render::{
	font_families as sdk_font_families, textengine_install as sdk_textengine_install,
};

pub fn install() {
	sdk_textengine_install();
}

pub fn font_families() -> Vec<String> {
	sdk_font_families()
}
