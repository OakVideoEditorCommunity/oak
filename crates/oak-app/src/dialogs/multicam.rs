use super::*;

/// The multicam wizard dialog content: pick the angle footage, choose
/// the sync mode, name the multicam sequence. The host reads the inputs
/// through [`Self::selection`] / [`Self::sync_mode`] / [`Self::name`]
/// when the OK button fires.
pub struct MulticamWizardContent<E: crate::oakui::engine::AppEngine> {
	engine: Entity<E>,
	/// The name field (prefilled "Multi-Cam").
	name: Entity<TextValue>,
	/// Sync mode combo (0 = audio waveform, 1 = source timecode, 2 = no
	/// alignment).
	pub(crate) sync: Entity<ComboBox>,
	/// Selection state per footage row: `(entry, checked)`.
	pub(crate) rows: Vec<(crate::oakui::engine::WizardFootage, bool)>,
}

/// The wizard's sync mode combo values (display order).
const WIZARD_SYNC_MODES: &[&str] = &[
	"multicam.sync.audio",
	"multicam.sync.timecode",
	"multicam.sync.none",
];

impl<E: crate::oakui::engine::AppEngine> MulticamWizardContent<E> {
	/// Builds the multicam wizard seeded with the engine's wizard footage.
	///
	/// `engine` is stored for the host's later sync/create calls and is read once
	/// here through `multicam_wizard_footage()`; an error or `None` yields an
	/// empty angle list, in which case the dialog shows the "no footage" hint and
	/// the host refuses to create. The name field is seeded with the
	/// `multicam.wizard.default_name` translation, the sync combo lists the three
	/// localized modes with selection `0` (audio correlation), and every footage
	/// row starts unchecked. `window` is needed for the widgets and `cx` to
	/// create the entities.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let name = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			TextValue { editor }
		});
		name.update(cx, |field, cx| {
			field.set_value(i18n::tr("multicam.wizard.default_name"), window, cx)
		});
		let sync = cx.new(|cx| {
			let options = WIZARD_SYNC_MODES
				.iter()
				.enumerate()
				.map(|(i, key)| ComboBoxOption::new(i, i18n::tr(key)))
				.collect();
			ComboBox::new(60, options, window, cx)
		});
		sync.update(cx, |combo, cx| combo.set_selected(Some(0), cx));
		let rows = engine
			.read(cx)
			.multicam_wizard_footage()
			.unwrap_or_default()
			.into_iter()
			.map(|entry| (entry, false))
			.collect();
		Self {
			engine,
			name,
			sync,
			rows,
		}
	}

	/// Flips the checked state of the angle row at `index`.
	///
	/// `index` is a position in the wizard's footage list; an out-of-range index
	/// is ignored silently (no panic and no notify). On a successful toggle `cx`
	/// is notified so the row's check mark repaints. Rendered rows call this from
	/// their click handler, including clicks on the row's padding.
	pub fn toggle_row(&mut self, index: usize, cx: &mut Context<Self>) {
		if let Some(row) = self.rows.get_mut(index) {
			row.1 = !row.1;
			cx.notify();
		}
	}

	/// The angle entries currently checked, in row order.
	pub fn selection(&self) -> Vec<crate::oakui::engine::WizardFootage> {
		self.rows
			.iter()
			.filter(|row| row.1)
			.map(|row| row.0.clone())
			.collect()
	}

	/// Returns the selected sync mode as a combo index.
	///
	/// `0` = sync by audio-waveform correlation, `1` = by source timecode, `2` =
	/// no alignment. The combo defaults to `0`, and an unset selection also
	/// reports `0`. The host switches on this value when it commits the wizard.
	pub fn sync_mode(&self, cx: &App) -> usize {
		self.sync.read(cx).selected().unwrap_or(0)
	}

	/// Returns the multicam sequence name currently typed into the field.
	///
	/// A [`SharedString`] snapshot of the text, pre-filled with the localized
	/// `multicam.wizard.default_name` ("Multi-Cam") at construction. The host
	/// reads it when the wizard is accepted; an empty name is possible and is not
	/// validated here.
	pub fn name(&self, cx: &App) -> SharedString {
		self.name.read(cx).value(cx)
	}

	/// Whether the engine offers any wizard footage (drives the empty
	/// hint row).
	pub fn has_rows(&self) -> bool {
		!self.rows.is_empty()
	}

	/// The engine entity (the host needs it to run the sync + create).
	pub fn engine(&self) -> &Entity<E> {
		&self.engine
	}
}

impl<E: crate::oakui::engine::AppEngine> Render for MulticamWizardContent<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let mut rows_div = div().flex().flex_col().gap_1().w_full();
		for (index, (entry, checked)) in self.rows.iter().enumerate() {
			let is_checked = *checked;
			let row = div()
				.id(ElementId::named_usize("multicam-wizard-angle", index))
				.flex()
				.items_center()
				.gap_2()
				.px_1()
				.py_0p5()
				.rounded_sm()
				.hover(|style| style.bg(colors.container))
				.cursor_pointer()
				.on_click(
					cx.listener(move |this: &mut Self, _event: &ClickEvent, _window, cx| {
						this.toggle_row(index, cx);
					}),
				)
				.child(
					div()
						.size(px(12.0))
						.flex()
						.items_center()
						.justify_center()
						.bg(if is_checked {
							colors.selected
						} else {
							colors.background
						})
						.text_color(colors.text)
						.children(is_checked.then(|| {
							crate::widgets::glyphs::icon(
								crate::widgets::glyphs::Glyph::Check,
								colors.text,
							)
						})),
				)
				.child(
					div()
						.flex_1()
						.text_color(colors.text)
						.child(entry.name.clone()),
				)
				.child(
					div().text_xs().text_color(colors.disabled).child(
						entry
							.duration_s
							.map(|d| format!("{d:.1}s"))
							.unwrap_or_default(),
					),
				);
			rows_div = rows_div.child(row);
		}
		let angles = div()
			.id("multicam-wizard-angles")
			.max_h(px(220.0))
			.overflow_y_scroll()
			.child(rows_div);
		let stock = if self.rows.is_empty() {
			div()
				.text_color(colors.disabled)
				.text_xs()
				.child(i18n::tr("multicam.wizard.no_footage"))
				.into_any_element()
		} else {
			div().into_any_element()
		};
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				&colors,
				i18n::tr("multicam.wizard.name").into(),
				self.name.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("multicam.wizard.sync").into(),
				self.sync.clone(),
			))
			.child(
				div()
					.text_sm()
					.text_color(colors.text)
					.child(i18n::tr("multicam.wizard.angles_label")),
			)
			.child(angles)
			.child(stock)
	}
}
