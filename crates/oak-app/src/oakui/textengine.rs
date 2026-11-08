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

use quercus_engine_rs::render::{
	font_families as sdk_font_families, textengine_install as sdk_textengine_install,
};

/// Installs this process's text measure/render backends into the shared engine,
/// as an app start-up step and again in each render worker process.
///
/// Idempotent: only the first call installs, so a later call cannot swap the
/// hooks out from under an in-flight layout. Until it runs, engine text nodes
/// have no backends and measure/render as empty. Thin re-export of
/// `quercus_engine_rs::render::textengine_install`; it does not register any
/// fonts by itself.
pub fn install() {
	sdk_textengine_install();
}

/// The system font families known to the shared text backend, sorted and
/// deduplicated, using the English family names a project stores.
///
/// These are the values the engine's font-family combo expects, and because the
/// backend is the same one render workers use, a name returned here resolves
/// identically when text is laid out. The list is cached by the backend and
/// invalidated when fonts are added, so it does not require [`install`] first —
/// the underlying font database is created on first use.
pub fn font_families() -> Vec<String> {
	sdk_font_families()
}
