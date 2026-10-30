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

//! `oakapp` — the Rust application layer of Oak, built on gpui-kit
//! (the registry `gpui-pre` + `gpui-base` + `gpui-component`; the custom
//! widgets live in `oak-widgets`, folding into `crate::widgets`).
//!
//! This is the start of the Rust rewrite of `app/` (Qt): a gpui window with
//! the main layout from the design (`design/`), dockable panels built from
//! the `oak_widgets` library, and an engine seam (`oakui`) with two
//! backends: the mock ([`oakui::MockEngine`]) feeding demo data, and the
//! real engine ([`oakui::RealEngine`]), a pure `quercus-engine-rs`
//! consumer. P7 removed every other Oak dependency: project sessions,
//! semantic edits, preview/export tasks, configuration, storage and OFX all
//! cross the SDK facade, and the app links no internal module crate.
//!
//! # Layout
//!
//! * [`app`] — the window shell: menu bar, dock layout, status bar, modal
//!   dialogs (file open/save-as, preferences, export), tick loop.
//! * [`dialogs`] — the preferences and export dialog content views.
//! * [`actions`] — the action registry: gpui actions, stable ids, default
//!   keys and routing (the single source behind the menus and shortcuts).
//! * [`manager`] — the project manager window (M13 D4): the library browser
//!   with new / open / rename / duplicate / delete / import / export.
//! * [`menus`] — the right-click menu layer: shared segments built from the
//!   action registry and the per-panel context-menu plumbing.
//! * [`panels`] — the dockable panels (viewers, timeline, inspector, ...).
//! * [`oakui`] — the engine gateway trait, the mock + real implementations,
//!   and the pure view-state logic (timecode, transport).
//!
//! # Running
//!
//! ```text
//! cargo run --bin oak-editor                        # the real engine, no project
//! cargo run --bin oak-editor -- path/to/project.ove    # open a project at startup
//! cargo run --bin oak-editor -- --mock              # the demo (mock) engine
//! cargo test                                         # unit tests (app + crates)
//! ```
//!
//! The engine backend is selected at startup: the real engine by default,
//! the mock with the `--mock` flag, `OAK_ENGINE=mock`, or the
//! `mock-engine` cargo feature.

pub mod actions;
pub mod app;
pub mod dialogs;
pub mod i18n;
pub mod logging;
pub mod manager;
pub mod oakui;
pub mod panels;
pub mod update;
pub mod widgets;

/// The application entry point (called from `main.rs`).
pub fn run() {
	// Install the stderr `log` backend before anything else runs, so wgpu
	// validation errors and platform warnings are visible (RUST_LOG sets
	// the verbosity; default is warn).
	logging::init();
	app::run();
}
