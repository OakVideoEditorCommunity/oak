//! Oak's olive themes: the QSS palettes translated to gpui colors, with
//! runtime switching.
//!
//! [`OakTheme::olive_dark`] / [`OakTheme::olive_light`] mirror
//! `oak/app/ui/style/olive-{dark,light}/palette.ini`. Applying a theme via
//! [`apply_theme`] swaps the global [`Colors`](gpui_kit::colors::Colors) — so
//! every widget that reads `cx.default_colors()` re-themes immediately — and
//! stores the full theme in a [`ThemeGlobal`] for widgets that want the
//! extended fields (accent, link, alternate base).

use gpui_kit::{App, Global, Rgba, colors::GlobalColors, rgb};
use std::sync::Arc;

/// Oak's olive theme palette.
#[derive(Debug, Clone, PartialEq)]
pub struct OakTheme {
	/// The theme's display name.
	pub name: gpui_kit::SharedString,
	/// Window background (panels, bars).
	pub window: Rgba,
	/// Base background (content areas, inputs).
	pub base: Rgba,
	/// Alternate base (stripes, separators).
	pub alternate_base: Rgba,
	/// Primary text.
	pub text: Rgba,
	/// Accent (selection, highlight).
	pub accent: Rgba,
	/// Text on the accent.
	pub accent_text: Rgba,
	/// Link color.
	pub link: Rgba,
	/// Disabled text.
	pub disabled_text: Rgba,
	/// Disabled button text.
	pub disabled_button_text: Rgba,
}

fn rgba(hex: u32) -> Rgba {
	let r = ((hex >> 16) & 0xff) as f32 / 255.0;
	let g = ((hex >> 8) & 0xff) as f32 / 255.0;
	let b = (hex & 0xff) as f32 / 255.0;
	Rgba { r, g, b, a: 1.0 }
}

impl OakTheme {
	/// The olive-dark palette (oak's default).
	///
	/// Tuned to the design's professional dark scheme: a near-black content
	/// base, slightly raised panel containers, a low-contrast separator and a
	/// dimmed secondary text — the blue accent stays the single saturated hue.
	pub fn olive_dark() -> Self {
		Self {
			name: "Olive Dark".into(),
			window: rgba(0x23272D),
			base: rgba(0x16181D),
			alternate_base: rgba(0x2E333B),
			text: rgb(0xE8EAED),
			accent: rgba(0x2A82DA),
			accent_text: rgb(0xffffff),
			link: rgba(0xE0B040),
			disabled_text: rgba(0x7D838C),
			disabled_button_text: rgba(0x565C66),
		}
	}

	/// The olive-light palette.
	pub fn olive_light() -> Self {
		Self {
			name: "Olive Light".into(),
			window: rgba(0xD0D0D0),
			base: rgba(0xF0F0F0),
			alternate_base: rgba(0xD0D0D0),
			text: rgb(0x000000),
			accent: rgba(0x2A82DA),
			accent_text: rgb(0xffffff),
			link: rgba(0x2A82DA),
			disabled_text: rgba(0x808080),
			disabled_button_text: rgba(0x808080),
		}
	}

	/// The border color derived from this theme (alternate base darkened for
	/// dark themes, lightened for light themes).
	pub fn border(&self) -> Rgba {
		let factor = if relative_luminance(self.text) > relative_luminance(self.base) {
			0.7
		} else {
			1.25
		};
		scale_luma(self.alternate_base, factor)
	}

	/// Map this theme onto gpui's [`Colors`](gpui_kit::colors::Colors) struct so
	/// `cx.default_colors()` picks it up.
	pub fn colors(&self) -> gpui_kit::colors::Colors {
		gpui_kit::colors::Colors {
			text: self.text,
			selected_text: self.accent_text,
			background: self.base,
			disabled: self.disabled_text,
			selected: self.accent,
			border: self.border(),
			separator: self.alternate_base,
			container: self.window,
		}
	}
}

/// The current full theme, set by [`apply_theme`].
pub struct ThemeGlobal(pub Arc<OakTheme>);

impl Global for ThemeGlobal {}

/// Apply a theme: swaps the global [`Colors`](gpui_kit::colors::Colors) (re-theming
/// every widget that reads `cx.default_colors()`) and stores the full theme.
pub fn apply_theme(cx: &mut App, theme: &OakTheme) {
	cx.set_global(GlobalColors(Arc::new(theme.colors())));
	cx.set_global(ThemeGlobal(Arc::new(theme.clone())));
	// gpui-kit components (Input, the dock chrome, buttons) read the
	// component theme — map the Oak palette onto it so built-ins match.
	// `Theme::change` (not a bare mode write) so the registry's default
	// light/dark theme loads; `Theme::update` (not `global_mut`) so the
	// renderable tokens reconcile from the edited colors.
	let dark = relative_luminance(theme.text) > relative_luminance(theme.base);
	gpui_kit::component::theme::Theme::change(
		if dark {
			gpui_kit::component::theme::ThemeMode::Dark
		} else {
			gpui_kit::component::theme::ThemeMode::Light
		},
		None,
		cx,
	);
	gpui_kit::component::theme::Theme::update(cx, |component| {
		let colors = &mut component.colors;
		colors.background = theme.base.into();
		colors.foreground = theme.text.into();
		colors.border = theme.border().into();
		colors.muted_foreground = theme.disabled_text.into();
		colors.secondary = theme.window.into();
		colors.secondary_foreground = theme.text.into();
		colors.accent = theme.accent.into();
		colors.accent_foreground = theme.accent_text.into();
		colors.primary = theme.accent.into();
		colors.primary_foreground = theme.accent_text.into();
		colors.caret = theme.text.into();
		colors.selection = theme.accent.into();
		colors.input = theme.base.into();
		colors.tab_foreground = theme.text.into();
		colors.tab_active_foreground = theme.text.into();
		colors.tab_active = theme.window.into();
	});
}

/// The current theme, or olive-dark if none was applied.
pub fn current_theme(cx: &App) -> Arc<OakTheme> {
	cx.try_global::<ThemeGlobal>()
		.map(|global| global.0.clone())
		.unwrap_or_else(|| Arc::new(OakTheme::olive_dark()))
}

fn relative_luminance(color: Rgba) -> f32 {
	// Simple perceptual approximation (sRGB -> luma).
	0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b
}

fn scale_luma(color: Rgba, factor: f32) -> Rgba {
	let scale = |channel: f32| (channel * factor).clamp(0.0, 1.0);
	Rgba {
		r: scale(color.r),
		g: scale(color.g),
		b: scale(color.b),
		a: color.a,
	}
}

/// Hover background for a control resting on the accent (primary
/// buttons such as OK): the accent lightened a step, so the hover reads
/// on top of the already-blue surface. The palette has no separate
/// accent-hover tone, so derive one.
pub fn accent_hover(colors: &gpui_kit::colors::Colors) -> Rgba {
	scale_luma(colors.selected, 1.25)
}

/// Hover background for a neutral control resting on the content
/// background (secondary buttons, tabs, tool glyphs): the raised
/// container tone lightened a step so it reads on both `background`
/// and `container` surfaces.
pub fn neutral_hover(colors: &gpui_kit::colors::Colors) -> Rgba {
	scale_luma(colors.container, 1.35)
}

/// Hover background for a large row (history entries, project rows)
/// resting on the content background: the accent at low alpha — the
/// solid accent is too loud for row-sized areas and would read as a
/// selection change.
pub fn row_hover(colors: &gpui_kit::colors::Colors) -> gpui_kit::Hsla {
	gpui_kit::Hsla::from(colors.selected).opacity(0.3)
}

#[cfg(test)]
mod tests {
	use super::*;
	use gpui_kit::{TestAppContext, colors::DefaultColors};

	#[test]
	fn dark_theme_text_contrasts_with_base() {
		let theme = OakTheme::olive_dark();
		let text_luma = relative_luminance(theme.text);
		let base_luma = relative_luminance(theme.base);
		// Text must be clearly brighter than the base.
		assert!(text_luma - base_luma > 0.4, "dark theme lacks contrast");
		// The accent must be visible against both.
		let accent_luma = relative_luminance(theme.accent);
		assert!((accent_luma - base_luma).abs() > 0.1);
	}

	#[test]
	fn light_theme_text_contrasts_with_base() {
		let theme = OakTheme::olive_light();
		let text_luma = relative_luminance(theme.text);
		let base_luma = relative_luminance(theme.base);
		assert!(base_luma - text_luma > 0.5, "light theme lacks contrast");
	}

	#[test]
	fn colors_map_keeps_text_and_selected() {
		let colors = OakTheme::olive_dark().colors();
		assert_eq!(colors.selected, rgb(0x2A82DA));
		assert_eq!(colors.selected_text, rgb(0xffffff));
	}

	#[test]
	fn border_derives_from_alternate_base() {
		let dark = OakTheme::olive_dark();
		let border = dark.border();
		// Dark theme: border is darker than the alternate base.
		assert!(relative_luminance(border) < relative_luminance(dark.alternate_base));
		let light = OakTheme::olive_light();
		assert!(relative_luminance(light.border()) > relative_luminance(light.alternate_base));
	}

	#[gpui_kit::test]
	async fn apply_theme_switches_default_colors(cx: &mut TestAppContext) {
		cx.update(|cx| {
			gpui_kit::init(cx);
			cx.init_colors();
		});
		cx.update(|app| {
			apply_theme(app, &OakTheme::olive_light());
			let colors = app.default_colors().clone();
			assert_eq!(colors.background, rgb(0xF0F0F0));
			assert_eq!(colors.text, rgb(0x000000));
			// And the extended theme is queryable.
			assert_eq!(current_theme(app).name, "Olive Light");
		});
	}

	/// The component theme (the gpui-kit dock chrome, inputs, buttons)
	/// follows the Oak palette — colors AND the tokens derived from them
	/// (the regression this guards: a light default theme left white dock
	/// panels inside the dark app).
	#[gpui_kit::test]
	async fn apply_theme_maps_the_component_theme(cx: &mut TestAppContext) {
		use gpui_kit::component::theme::ActiveTheme as _;
		cx.update(|cx| {
			gpui_kit::init(cx);
			cx.init_colors();
		});
		cx.update(|app| {
			apply_theme(app, &OakTheme::olive_dark());
			let theme = app.theme();
			assert_eq!(
				theme.colors.background,
				OakTheme::olive_dark().base.into(),
				"the component theme's background follows the palette"
			);
			assert_eq!(
				theme.colors.foreground,
				OakTheme::olive_dark().text.into(),
				"the component theme's foreground follows the palette"
			);
			// The reconciled tokens moved with the colors (the dock chrome
			// paints from tokens).
			assert_eq!(
				theme.tokens.background.color, theme.colors.background,
				"the renderable tokens reconciled with the colors"
			);
			assert!(theme.is_dark(), "the component theme is in dark mode");
		});
	}
}
