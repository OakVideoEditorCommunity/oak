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

//! Offscreen screenshot capture for the app window.
//!
//! Renders the full [`OakApp`] shell at 1600×900 (2× = 3200×1866 px) in an
//! offscreen macOS window and writes the PNGs to
//! `docs/screenshot-window.png` (zh-CN) and `docs/screenshot-window-en.png`
//! (en-US), then opens the project manager (M13 D4) on the same shell and
//! captures it to `docs/screenshot-manager.png` / `-en.png`, using the same
//! [`VisualTestAppContext`] machinery the gpui visual tests use. The window
//! is created at `(-10000, -10000)` so nothing flickers on screen.
//!
//! Unlike the real app's startup ([`oakapp::app::run`]) the example must
//! initialize the i18n layer itself, or every menu renders in the en-US
//! fallback while the panels fall back to their built-in Chinese defaults.
//! The active language is captured first and restored at the end, so the
//! persisted preference is untouched.
//!
//! Run it on the macOS main thread (examples run on the main thread, unlike
//! `#[test]` harness threads):
//!
//! ```text
//! cargo run --example screenshot               # 1600×900 → both PNGs
//! cargo run --example screenshot -- 1100 900   # any size (same filenames)
//! ```

#[cfg(target_os = "macos")]
use gpui_kit::{px, size, AnyWindowHandle, AppContext, Entity, Result, VisualTestAppContext};
#[cfg(target_os = "macos")]
use gpui_kit::platform::current_platform;
#[cfg(target_os = "macos")]
use oakapp::app::OakApp;
#[cfg(target_os = "macos")]
use oakapp::i18n;
#[cfg(target_os = "macos")]
use oakapp::oakui::MockEngine;

#[cfg(target_os = "macos")]
const DEFAULT_WIDTH: f32 = 1600.0;
#[cfg(target_os = "macos")]
const DEFAULT_HEIGHT: f32 = 900.0;
#[cfg(target_os = "macos")]
const OUT_ZH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/screenshot-window.png");
#[cfg(target_os = "macos")]
const OUT_EN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/screenshot-window-en.png");
#[cfg(target_os = "macos")]
const OUT_MGR_ZH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/screenshot-manager.png");
#[cfg(target_os = "macos")]
const OUT_MGR_EN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/screenshot-manager-en.png");
#[cfg(target_os = "macos")]
const OUT_PREF_ZH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/screenshot-preferences.png");
#[cfg(target_os = "macos")]
const OUT_PREF_EN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/screenshot-preferences-en.png");

/// Logical y of the timeline toolbar row, which sits at the top of the
/// bottom dock panel in the default layout: the dock starts at y 27.5 (the
/// menu bar height), the viewers get 60% of the remaining height, and a 6px
/// split handle separates them from the timeline. The toolbar is its 31px
/// first row. The assertion scans a small band around it so minor layout
/// drift does not false-negative.
#[cfg(target_os = "macos")]
const TOOLBAR_Y: f32 = 541.0;
#[cfg(target_os = "macos")]
const TOOLBAR_BAND: f32 = 44.0;

/// Captures the app shell screenshots: a zh-CN and an en-US window PNG plus
/// the preferences and project-manager dialogs, all at 2×.
///
/// Optional positional arguments are `width` and `height` in logical points
/// (defaults 1600×900); anything else is ignored. The active UI language is
/// captured first and restored before returning, so the persisted preference
/// is untouched.
///
/// # Errors
///
/// Propagates the screenshot and IO errors from creating the output
/// directory or writing the PNGs.
#[cfg(target_os = "macos")]
fn main() -> Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	let width = args
		.first()
		.and_then(|s| s.parse().ok())
		.unwrap_or(DEFAULT_WIDTH);
	let height = args
		.get(1)
		.and_then(|s| s.parse().ok())
		.unwrap_or(DEFAULT_HEIGHT);

	let mut cx = VisualTestAppContext::new(current_platform(false));
	cx.update(|app| app.init_colors());

	// Initialize the UI language like the real app's startup would: zh-CN
	// for the primary screenshot, then en-US for the English one. The
	// original persisted language is restored at the end.
	let original = i18n::language_code();
	i18n::set_language_code("zh-CN");
	{
		let (handle, root) = open_shell(&mut cx, width, height);
		let image = cx.capture_screenshot(handle.into())?;
		std::fs::create_dir_all(std::path::Path::new(OUT_ZH).parent().unwrap())?;
		image.save(OUT_ZH)?;
		println!("wrote {OUT_ZH} ({}×{})", image.width(), image.height());
		assert_toolbar(&image, "zh-CN");
		capture_preferences(&mut cx, handle, &root, OUT_PREF_ZH)?;
		capture_manager(&mut cx, handle, &root, OUT_MGR_ZH)?;
	}
	i18n::set_language_code("en-US");
	{
		let (handle, root) = open_shell(&mut cx, width, height);
		let image = cx.capture_screenshot(handle.into())?;
		std::fs::create_dir_all(std::path::Path::new(OUT_EN).parent().unwrap())?;
		image.save(OUT_EN)?;
		println!("wrote {OUT_EN} ({}×{})", image.width(), image.height());
		assert_toolbar(&image, "en-US");
		capture_preferences(&mut cx, handle, &root, OUT_PREF_EN)?;
		capture_manager(&mut cx, handle, &root, OUT_MGR_EN)?;
	}
	i18n::set_language_code(&original);

	Ok(())
}

/// Opens the app shell offscreen and lets the layout settle (see
/// [`settle`]). Returns the typed window handle and the root entity so the
/// caller can still drive the shell (the manager capture) after the plain
/// screenshot.
///
/// `cx` drives the offscreen window; `width` and `height` are the logical
/// window size (1600×900 by default, i.e. 3200×1866 pixels at 2×). The shell
/// is built with the real app's 14px rem size so text metrics match
/// production.
///
/// # Panics
///
/// Panics when the offscreen window cannot be opened (e.g. Metal is
/// unavailable) or when no root entity was produced.
#[cfg(target_os = "macos")]
fn open_shell(
	cx: &mut VisualTestAppContext,
	width: f32,
	height: f32,
) -> (
	gpui_kit::WindowHandle<OakApp<MockEngine>>,
	Entity<OakApp<MockEngine>>,
) {
	let mut root_slot = None;
	let window = cx
		.open_offscreen_window(size(px(width), px(height)), |window, cx| {
			// Compact pro-app text metrics, matching the real app's startup
			// (`src/app.rs run_with` sets rem 14px; gpui's default is 16px).
			window.set_rem_size(px(14.0));
			let root = cx.new(|cx| OakApp::<MockEngine>::new(window, None, cx));
			root_slot = Some(root.clone());
			root
		})
		.expect("offscreen window opens");
	settle(cx, window.into());
	(window, root_slot.expect("root entity"))
}

/// Draws enough frames for the layout to settle and the async toolbar-icon
/// assets to decode: the node editor fits its graph once the canvas size is
/// known, the viewers upload their first CPU frame, and the PNG toolbar
/// icons load through the background executor on the frame after the asset
/// future resolves.
///
/// `cx` is the visual-test context and `handle` the offscreen window to
/// draw. Two batches of 16 draws are run, each preceded and followed by
/// `run_until_parked`, so async asset work that needs a frame plus a parked
/// executor also completes.
///
/// # Panics
///
/// Panics when the window has been closed, since the draw cannot run.
#[cfg(target_os = "macos")]
fn settle(cx: &mut VisualTestAppContext, handle: AnyWindowHandle) {
	for _ in 0..16 {
		cx.run_until_parked();
		cx.update_window(handle, |_root, window, app| {
			let _ = window.draw(app);
		})
		.expect("window still open");
	}
	cx.run_until_parked();

	for _ in 0..16 {
		cx.run_until_parked();
		cx.update_window(handle, |_root, window, app| {
			let _ = window.draw(app);
		})
		.expect("window still open");
	}
	cx.run_until_parked();
}

/// Opens the preferences dialog on the shell (M12 P5b) and captures it: the
/// modal lists the grouped settings (general / rendering / cache / proxy /
/// project / audio). Drives the root ENTITY (not the window handle — see
/// [`capture_manager`]). The dialog closes afterwards so the manager
/// capture starts from a clean shell.
#[cfg(target_os = "macos")]
fn capture_preferences(
	cx: &mut VisualTestAppContext,
	handle: gpui_kit::WindowHandle<OakApp<MockEngine>>,
	root: &Entity<OakApp<MockEngine>>,
	out: &str,
) -> Result<()> {
	root.update(cx, |app, cx| app.open_preferences(cx));
	settle(cx, handle.into());
	let image = cx.capture_screenshot(handle.into())?;
	image.save(out)?;
	println!("wrote {out} ({}×{})", image.width(), image.height());
	// Close the dialog (the Close button path: commit + dismiss).
	root.update(cx, |app, cx| app.close_modal(cx));
	cx.run_until_parked();
	Ok(())
}

/// Opens the project manager on the shell (M13 D4) and captures it: the
/// modal card lists the mock library with its per-project stats. Drives the
/// root ENTITY (not the window handle — a window update borrows the window,
/// and building the modal inside it would re-enter it).
#[cfg(target_os = "macos")]
fn capture_manager(
	cx: &mut VisualTestAppContext,
	handle: gpui_kit::WindowHandle<OakApp<MockEngine>>,
	root: &Entity<OakApp<MockEngine>>,
	out: &str,
) -> Result<()> {
	root.update(cx, |app, cx| app.show_project_manager(cx));
	settle(cx, handle.into());
	let image = cx.capture_screenshot(handle.into())?;
	image.save(out)?;
	println!("wrote {out} ({}×{})", image.width(), image.height());
	Ok(())
}

/// The timeline toolbar's tool icons (16px at 2× = 32px on 48px pitch) must
/// render: the toolbar is the 31px row at the top of the bottom dock panel.
/// Scan the tool cells for bright glyph pixels, so a broken icon load fails
/// the capture loudly instead of shipping an empty toolbar.
///
/// `image` is the 2× screenshot of the whole window; `language` only tags the
/// diagnostics and the panic message. Each of the eight tool cells (48px
/// logical pitch in x) is scanned for pixels brighter than 150 in all
/// channels within `TOOLBAR_BAND` of `TOOLBAR_Y`, and a cell counts as
/// rendered once it has more than 20 such pixels.
///
/// # Panics
///
/// Panics when fewer than 6 of the 8 cells render, so a screenshot run fails
/// instead of writing a toolbar-less image.
#[cfg(target_os = "macos")]
fn assert_toolbar(image: &image::RgbaImage, language: &str) {
	// The image is 2× the logical size; convert logical → pixel y. TOOLBAR_Y
	// is measured from the window's top edge.
	let mut rendered = 0usize;
	for (index, cell_x) in [12u32, 44, 76, 108, 140, 172, 204, 236].iter().enumerate() {
		let mut bright = 0u32;
		for dy in 0..(TOOLBAR_BAND as i32 * 2) {
			for dx in 0..40i32 {
				let x = (*cell_x as i32 + dx) as u32;
				let y = ((TOOLBAR_Y as i32 - TOOLBAR_BAND as i32 / 2) * 2 + dy).max(0) as u32;
				if x >= image.width() || y >= image.height() {
					continue;
				}
				let p = image.get_pixel(x, y);
				if p[0] > 150 && p[1] > 150 && p[2] > 150 {
					bright += 1;
				}
			}
		}
		println!("[screenshot] {language} toolbar tool {index} bright pixels: {bright}");
		if bright > 20 {
			rendered += 1;
		}
	}
	assert!(
		rendered >= 6,
		"{language} timeline toolbar icons did not render (only {rendered}/8 tool cells had pixels)"
	);
}

#[cfg(not(target_os = "macos"))]
fn main() {
	eprintln!("the screenshot example is macOS-only (offscreen Metal rendering)");
}
