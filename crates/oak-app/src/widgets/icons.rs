//! Theme-aware toolbar icon resolution.
//!
//! The widget crates render icons from PNG files (a 16px logical grid, with
//! 2× files for retina); the host application owns the icon files and knows
//! which theme is active, so it registers a resolver here once at startup.
//! Widgets that want an icon ask [`path`], and fall back to a text/glyph
//! label when no resolver (or no matching file) is registered — which keeps
//! gpui_widgets self-contained under its own tests.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, Global};

/// Resolves the file path of a named icon (e.g. `"play"`) for the current
/// theme, or `None` when the icon does not exist.
pub type IconResolver = Arc<dyn Fn(&str, &App) -> Option<PathBuf>>;

/// The registered icon resolver (set by the host app).
struct IconResolverGlobal(IconResolver);

impl Global for IconResolverGlobal {}

/// Registers the host's icon resolver. Call once at startup, before any
/// window renders.
pub fn set_resolver(resolver: IconResolver, cx: &mut App) {
	cx.set_global(IconResolverGlobal(resolver));
}

/// The file path of the named icon in the current theme, if resolvable.
///
/// `name` is the icon's logical name (e.g. `"play"`); `cx` supplies the global
/// resolver installed by [`set_resolver`], which knows the active theme. Returns
/// `None` when no resolver was registered (widgets then fall back to a text or
/// glyph label) or when the resolver has no file for `name`. The returned path
/// is owned and is not checked for existence here.
pub fn path(name: &str, cx: &App) -> Option<PathBuf> {
	cx.try_global::<IconResolverGlobal>()
		.and_then(|global| (global.0)(name, cx))
}
