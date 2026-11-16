use super::*;

/// The About dialog's content: the app name + version, the GPL-3.0 license
/// line and the Olive fork notice (the C++ AboutDialog's text block; the
/// patrons scroller is not ported).
pub struct AboutContent;

impl AboutContent {
	/// Builds the static about text.
	pub fn new() -> Self {
		Self
	}
}

/// The update-available prompt: the remote version, the running build and
/// the release notes (rendered as plain text — the API sends Markdown).
pub struct UpdateDialogContent {
	version: SharedString,
	notes: SharedString,
}

impl UpdateDialogContent {
	/// Builds the update prompt from one latest-release response.
	///
	/// `version` is the remote version string, copied verbatim and shown in the
	/// "update available" line. `notes` is the release-notes Markdown sent by the
	/// API; it is trimmed, and if nothing remains it is replaced with the
	/// localized `update.no_notes` text so the dialog always has a body. The notes
	/// are stored and rendered as plain text — Markdown is not parsed.
	pub fn new(version: &str, notes: &str) -> Self {
		let notes = notes.trim();
		let notes = if notes.is_empty() {
			i18n::tr("update.no_notes").to_string()
		} else {
			notes.to_string()
		};
		Self {
			version: version.to_string().into(),
			notes: notes.into(),
		}
	}
}

impl Render for UpdateDialogContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let available = i18n::tr("update.available")
			.replace("{version}", self.version.as_ref())
			.replace("{current}", crate::update::current_version());
		div()
			.id("update-dialog-content")
			.flex()
			.flex_col()
			.gap_2()
			.w_full()
			.child(div().text_color(colors.text).child(available))
			.child(
				div()
					.text_color(colors.disabled)
					.text_xs()
					.child(i18n::tr("update.notes")),
			)
			.child(
				div()
					.id("update-dialog-notes")
					.max_h(px(220.0))
					.overflow_y_scroll()
					.text_color(colors.text)
					.child(self.notes.clone()),
			)
	}
}

impl Render for AboutContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div()
			.flex()
			.flex_col()
			.gap_2()
			.child(
				div()
					.text_color(colors.text)
					.font_weight(gpui_kit::FontWeight::BOLD)
					.child(format!("Oak Video Editor {}", env!("CARGO_PKG_VERSION"))),
			)
			.child(
				div()
					.text_color(colors.text)
					.child(i18n::tr("about.description")),
			)
			.child(
				div()
					.text_color(colors.text)
					.child(i18n::tr("about.thanks")),
			)
			.child(
				div()
					.text_color(colors.disabled)
					.text_xs()
					.child(i18n::tr("about.fork_notice")),
			)
	}
}

// ---------------------------------------------------------------------------
// Sequence: 新建序列 (New Sequence) + 序列属性 (Sequence Properties)
// ---------------------------------------------------------------------------

/// The frame-rate choices offered for a sequence, as rational pairs in
/// dropdown order (matching [`SEQUENCE_RATE_OPTIONS`]).
pub(crate) const SEQUENCE_RATES: &[(u32, u32)] = &[
	(24000, 1001), // 23.98
	(24, 1),       // 24
	(25, 1),       // 25
	(30000, 1001), // 29.97
	(30, 1),       // 30
	(50, 1),       // 50
	(60000, 1001), // 59.94
	(60, 1),       // 60
];

/// The frame-rate labels, in the same order as [`SEQUENCE_RATES`].
const SEQUENCE_RATE_OPTIONS: &[&str] = &["23.98", "24", "25", "29.97", "30", "50", "59.94", "60"];

/// The number of preset entries (indices 1..=COUNT; 0 is the custom entry,
/// which leaves the width / height / frame-rate fields free).
pub(crate) const SEQUENCE_PRESET_COUNT: usize = 6;

/// The preset formats offered for a sequence.
pub(crate) fn sequence_preset_options() -> Vec<ComboBoxOption> {
	vec![
		ComboBoxOption::new(0, i18n::tr("seqprops.preset.custom")),
		ComboBoxOption::new(1, i18n::tr("seqprops.preset.pal")),
		ComboBoxOption::new(2, i18n::tr("seqprops.preset.ntsc")),
		ComboBoxOption::new(3, i18n::tr("seqprops.preset.hd_1080_25")),
		ComboBoxOption::new(4, i18n::tr("seqprops.preset.hd_1080_30")),
		ComboBoxOption::new(5, i18n::tr("seqprops.preset.uhd_4k")),
		ComboBoxOption::new(6, i18n::tr("seqprops.preset.dci_4k")),
	]
}

/// The `(width, height, rate-num, rate-den)` of a preset entry, or `None`
/// for the custom entry.
pub(crate) fn sequence_preset_format(index: usize) -> Option<(u32, u32, u32, u32)> {
	match index {
		1 => Some((720, 576, 25, 1)),       // PAL
		2 => Some((720, 480, 30000, 1001)), // NTSC
		3 => Some((1920, 1080, 25, 1)),     // HD 1080p25
		4 => Some((1920, 1080, 30, 1)),     // HD 1080p30
		5 => Some((3840, 2160, 25, 1)),     // 4K UHD
		6 => Some((4096, 2160, 24, 1)),     // 4K DCI
		_ => None,
	}
}

/// The frame-rate choices for the sequence dialogs.
pub(crate) fn sequence_rate_options() -> Vec<ComboBoxOption> {
	SEQUENCE_RATE_OPTIONS
		.iter()
		.enumerate()
		.map(|(i, label)| ComboBoxOption::new(i, *label))
		.collect()
}
