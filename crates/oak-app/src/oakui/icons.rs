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

//! Theme-aware toolbar icons.
//!
//! The toolbar/transport icons are the PNGs pulled from the legacy C++ app
//! (`app/ui/style/olive-{dark,light}/png` in the pre-Rust history), stored in
//! `assets/icons/{dark,light}/`. Each theme ships its own glyph color (white
//! on dark, black on light), so the active [`OakTheme`] picks the family.
//!
//! Icons render on a 16px logical grid from the 32px (2×) files; buttons give
//! them a 24px hit target and a localized tooltip.
//!
//! [`OakTheme`]: gpui_widgets::theme::OakTheme

use std::path::PathBuf;

use gpui::App;

/// Icon file names (without the `.png` suffix), mirroring the legacy set.
pub const ICON_ARROW: &str = "arrow";
pub const ICON_RAZOR: &str = "razor";
pub const ICON_RIPPLE: &str = "ripple";
pub const ICON_SLIP: &str = "slip";
pub const ICON_ROLLING: &str = "rolling";
pub const ICON_ZOOM: &str = "zoomin";
pub const ICON_ZOOM_IN: &str = "zoomin";
pub const ICON_ZOOM_OUT: &str = "zoomout";
pub const ICON_SLIDE: &str = "slide";
pub const ICON_TRACK_SELECT: &str = "track-tool";
pub const ICON_SNAP: &str = "magnet";
pub const ICON_PLAY: &str = "play";
pub const ICON_PAUSE: &str = "pause";
pub const ICON_PREV: &str = "prev";
pub const ICON_NEXT: &str = "next";
pub const ICON_REW: &str = "rew";
pub const ICON_FF: &str = "ff";

/// The theme-dependent filesystem path of an icon. The icons root is
/// resolved at runtime — the packagers install `assets/icons` next to the
/// binary (Windows/Linux packages), into `Contents/Resources` (macOS
/// bundle) or `/usr/share/oak` (deb/rpm/pacman system installs) — with
/// the dev checkout as a fallback. Absolute, so it works from any working
/// directory.
pub fn icon_path(name: &str, cx: &App) -> PathBuf {
	let theme = gpui_widgets::theme::current_theme(cx);
	let family = if theme.name == "Olive Dark" {
		"dark"
	} else {
		"light"
	};
	icon_root().join(family).join(format!("{name}.png"))
}

/// The first icons directory that exists, in precedence order (mirrors the
/// i18n pack search in `crate::i18n`). Cached: one process resolves one
/// directory.
fn icon_root() -> &'static PathBuf {
	static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
	ROOT.get_or_init(|| {
		let dev = || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons");
		let mut candidates = vec![dev()];
		if let Ok(exe) = std::env::current_exe() {
			if let Some(dir) = exe.parent() {
				candidates.push(dir.join("icons"));
				candidates.push(dir.join("assets/icons"));
				if let Some(contents) = dir.parent() {
					candidates.push(contents.join("Resources/icons"));
					candidates.push(contents.join("Resources/assets/icons"));
					// cargo-packager deb/AppImage: <exe>/../lib/oak-editor/icons.
					candidates.push(contents.join("lib/oak-editor/icons"));
					candidates.push(contents.join("share/oak/icons"));
				}
			}
		}
		candidates.push(PathBuf::from("assets/icons"));
		candidates
			.into_iter()
			.find(|dir| dir.join("dark").is_dir())
			.unwrap_or_else(dev)
	})
}

/// Registers the app's icon resolver, so widget-crate consumers (the viewer
/// transport bar) resolve the same theme-aware paths. Call at startup before
/// any window renders; re-registering after a theme switch is harmless.
pub fn init(cx: &mut App) {
	gpui_widgets::icons::set_resolver(
		std::sync::Arc::new(|name, cx| Some(icon_path(name, cx))),
		cx,
	);
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Every icon name resolves to an existing file for both themes — a
	/// missing PNG would render as a blank toolbar button.
	#[test]
	fn every_icon_file_exists_for_both_themes() {
		for name in [
			ICON_ARROW,
			ICON_RAZOR,
			ICON_RIPPLE,
			ICON_SLIP,
			ICON_ROLLING,
			ICON_ZOOM,
			ICON_ZOOM_IN,
			ICON_ZOOM_OUT,
			ICON_SLIDE,
			ICON_TRACK_SELECT,
			ICON_SNAP,
			ICON_PLAY,
			ICON_PAUSE,
			ICON_PREV,
			ICON_NEXT,
			ICON_REW,
			ICON_FF,
		] {
			for family in ["dark", "light"] {
				let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
					.join("../../assets/icons")
					.join(family)
					.join(format!("{name}.png"));
				assert!(
					path.exists(),
					"missing icon asset {path:?} (referenced as {name})"
				);
			}
		}
	}

	/// The theme-aware path picks the dark family for the default dark theme
	/// and the light family once a light theme is applied.
	#[gpui::test]
	fn icon_path_follows_the_active_theme(cx: &mut gpui::TestAppContext) {
		cx.update(|app| {
			gpui_widgets::theme::apply_theme(app, &gpui_widgets::theme::OakTheme::olive_dark());
			let dark = icon_path(ICON_PLAY, app);
			assert!(
				dark.ends_with("dark/play.png"),
				"dark theme → dark family, got {dark:?}"
			);

			gpui_widgets::theme::apply_theme(app, &gpui_widgets::theme::OakTheme::olive_light());
			let light = icon_path(ICON_PLAY, app);
			assert!(
				light.ends_with("light/play.png"),
				"light theme → light family, got {light:?}"
			);
		});
	}
}
