use super::*;

/// Subtitle style editor (字幕 → 字幕样式): font family, size, fill and
/// outline. The host applies the style to the selected Text clip through the
/// SDK's `set_subtitle_style`.
pub struct SubtitleStyleContent {
	font_family: Entity<TextValue>,
	font_size: Entity<SpinBox>,
	color: Entity<OfxColorPicker>,
	outline_enabled: Entity<CheckBox>,
	outline_width: Entity<SpinBox>,
	outline_color: Entity<OfxColorPicker>,
	error: Option<String>,
}

impl SubtitleStyleContent {
	/// Builds the subtitle style editor seeded from `style`.
	///
	/// `style` supplies the initial values: font family (text field), font size
	/// in points (spin box `1.0..=400.0`, step `1.0`), fill color, outline
	/// enable flag, outline width in points (spin box `0.0..=40.0`, step
	/// `0.5`), and outline color. `window` is required to build the input
	/// widgets and `cx` to create the entities. The outline checkbox is
	/// subscribed via [`subscribe_checkbox`] so clicks take effect immediately;
	/// the error slot starts empty.
	pub fn new(
		style: &quercus_engine_rs::editor::SubtitleStyle,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let font_family = text_field(style.font_family.clone(), window, cx);
		let font_size = cx.new(|cx| {
			SpinBox::new(
				81,
				SliderModel::new(
					ValueKind::Float,
					1.0,
					400.0,
					1.0,
					f64::from(style.font_size),
				),
				window,
				cx,
			)
		});
		let color = cx.new(|cx| OfxColorPicker::new(83, rgba_from_array(style.color), window, cx));
		let outline_enabled = cx.new(|cx| {
			CheckBox::new(
				82,
				if style.outline_enabled {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("subtitle.style.outline"))
		});
		subscribe_checkbox(&outline_enabled, cx);
		let outline_width = cx.new(|cx| {
			SpinBox::new(
				84,
				SliderModel::new(
					ValueKind::Float,
					0.0,
					40.0,
					0.5,
					f64::from(style.outline_width),
				),
				window,
				cx,
			)
		});
		let outline_color =
			cx.new(|cx| OfxColorPicker::new(85, rgba_from_array(style.outline_color), window, cx));
		Self {
			font_family,
			font_size,
			color,
			outline_enabled,
			outline_width,
			outline_color,
			error: None,
		}
	}

	/// Reads the form back into a [`quercus_engine_rs::editor::SubtitleStyle`].
	///
	/// The editable values are taken from the widgets: font family string, font
	/// size in points (`1.0..=400.0`), fill and outline colors from the color
	/// pickers' committed colors, the outline enable flag, and the outline width
	/// in points (`0.0..=40.0`). The remaining fields are not exposed in this
	/// dialog and are fixed here: `align = 1`, `margin_h = 0.0`,
	/// `margin_v = 0.0`, `line_spacing = 1.0`. The host passes the result to
	/// the SDK's `set_subtitle_style`; `cx` is only used to read the widgets.
	///
	/// Currently always returns `Ok`; the `Result` is kept so input validation
	/// can be added later without changing callers.
	pub fn style(&self, cx: &App) -> Result<quercus_engine_rs::editor::SubtitleStyle, String> {
		let color = color_from_rgba(self.color.read(cx).committed());
		let outline_color = color_from_rgba(self.outline_color.read(cx).committed());
		Ok(quercus_engine_rs::editor::SubtitleStyle {
			font_family: self.font_family.read(cx).value(cx).to_string(),
			font_size: self.font_size.read(cx).value().to_f64() as f32,
			color,
			outline_color,
			outline_enabled: self.outline_enabled.read(cx).state() == CheckState::Checked,
			outline_width: self.outline_width.read(cx).value().to_f64() as f32,
			align: 1,
			margin_h: 0.0,
			margin_v: 0.0,
			line_spacing: 1.0,
		})
	}

	/// Sets or clears the validation error shown under the subtitle form.
	///
	/// `error` renders in red below the fields while it is `Some`; `None`
	/// removes the row. `cx` is notified so the dialog repaints. The host calls
	/// this with the engine's message when the style cannot be applied, leaving
	/// the dialog open.
	pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
		self.error = error;
		cx.notify();
	}
}

impl Render for SubtitleStyleContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				&colors,
				i18n::tr("subtitle.style.font").into(),
				self.font_family.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("subtitle.style.size").into(),
				self.font_size.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("subtitle.style.color").into(),
				self.color.clone(),
			))
			.child(self.outline_enabled.clone())
			.child(form_row(
				&colors,
				i18n::tr("subtitle.style.outline_width").into(),
				self.outline_width.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("subtitle.style.outline_color").into(),
				self.outline_color.clone(),
			))
			.child(if let Some(error) = &self.error {
				div()
					.debug_selector(|| "subtitle-style-error".into())
					.text_color(gpui_kit::rgb(0xe5484d))
					.text_xs()
					.child(error.clone())
			} else {
				div()
			})
	}
}
