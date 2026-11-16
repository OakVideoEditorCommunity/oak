use super::*;

/// A text field for the sequence name, shaped like the export path field
/// (its own editor so the name can be replaced without retyping).
pub struct TextValue {
	pub(crate) editor: Entity<InputState>,
}

impl TextValue {
	/// Returns the text currently held by the field.
	///
	/// A [`SharedString`] snapshot of the inner editor's value, i.e. whatever was
	/// last typed or set by [`Self::set_value`]. `app` is only needed to read the
	/// entity; no validation is applied.
	pub fn value(&self, app: &App) -> SharedString {
		self.editor.read(app).value()
	}

	/// Replaces the text shown in the field.
	///
	/// `value` is converted into a [`SharedString`] and pushed into the field's
	/// own editor, so the caller does not have to retype or rebuild the widget;
	/// `window` is required by the editor's setter and `cx` is notified so the row
	/// repaints. This is a programmatic change, so no input event is emitted.
	pub fn set_value(
		&mut self,
		value: impl Into<SharedString>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let value = value.into();
		self.editor.update(cx, |editor, cx| {
			editor.set_value(value.as_ref(), window, cx);
		});
		cx.notify();
	}
}

impl Render for TextValue {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let weak = self.editor.downgrade();
		div()
			.rounded_md()
			.border_1()
			.border_color(colors.border)
			.bg(colors.background)
			.px_2()
			.py_1()
			.child(
				text_input("oak-seq-name", cx)
					.state(weak)
					.accepts_input(true),
			)
	}
}

/// The initial state of the sequence format fields.
pub struct SequenceFormatSeed {
	/// The selected preset index (0 = custom).
	pub preset: usize,
	/// The width shown in the spin box (presets override it).
	pub width: u32,
	/// The height shown in the spin box.
	pub height: u32,
	/// The selected frame-rate index, or `None` for the custom default.
	pub rate: Option<usize>,
	/// Whether the interlaced checkbox is checked.
	pub interlaced: bool,
}

impl SequenceFormatSeed {
	/// The defaults the new-sequence dialog starts from: HD 1080p25.
	pub fn hd_1080p25() -> Self {
		Self {
			preset: 3,
			width: 1920,
			height: 1080,
			rate: Some(2),
			interlaced: false,
		}
	}

	/// Builds a seed matching a source format (a footage's probed video
	/// stream, or a sequence's current parameters): the matching preset
	/// when the dimensions/rate appear in it, the custom entry otherwise,
	/// and the rate pick's exact match when the rate is one of the common
	/// options (else the custom default 25 must be selected explicitly —
	/// the drop-choice seed follows the probe, so it only seeds what is
	/// known).
	pub fn from_format(format: &crate::oakui::engine::VideoFormat, interlaced: bool) -> Self {
		let preset = (1..=SEQUENCE_PRESET_COUNT)
			.find(|i| {
				sequence_preset_format(*i)
					== Some((
						format.width,
						format.height,
						format.rate.num,
						format.rate.den,
					))
			})
			.unwrap_or(0);
		let rate = SEQUENCE_RATES
			.iter()
			.position(|r| *r == (format.rate.num, format.rate.den));
		Self {
			preset,
			width: format.width,
			height: format.height,
			rate,
			interlaced,
		}
	}
}

/// The format controls shared by the new-sequence and sequence-properties
/// dialogs: a preset combo that fills the numeric fields, width / height
/// spin boxes, a frame-rate combo and an interlaced checkbox. Picking a
/// preset overwrites the numeric fields; editing any of them snaps the
/// preset back to *custom*.
pub struct SequenceFormatFields {
	pub(crate) preset: Entity<ComboBox>,
	pub(crate) width: Entity<SpinBox>,
	pub(crate) height: Entity<SpinBox>,
	pub(crate) rate: Entity<ComboBox>,
	pub(crate) interlaced: Entity<CheckBox>,
}

impl SequenceFormatFields {
	/// Builds the format fields from `seed` and wires their cross-updates.
	///
	/// `seed` supplies the initial preset index (0 = custom), the width and
	/// height shown in the spin boxes (pixels, integer `16.0..=8192.0`, step
	/// `2.0`), the frame-rate combo selection (`None` = no selection) and the
	/// interlaced flag. `window` is required for the widgets and `cx` for the new
	/// entities.
	///
	/// The subscriptions make the controls consistent: choosing a non-custom
	/// preset overwrites width/height (and the rate when the preset's rate is one
	/// of the common options), while editing width, height or the rate snaps the
	/// preset back to *custom*. The interlaced checkbox follows the standard
	/// request-only pattern and accepts each toggle. The programmatic setters used
	/// here emit no events, so these subscriptions cannot loop.
	pub fn build(seed: SequenceFormatSeed, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let preset = cx.new(|cx| ComboBox::new(40, sequence_preset_options(), window, cx));
		preset.update(cx, |combo, cx| combo.set_selected(Some(seed.preset), cx));

		let width = cx.new(|cx| {
			SpinBox::new(
				41,
				SliderModel::new(ValueKind::Integer, 16.0, 8192.0, 2.0, f64::from(seed.width)),
				window,
				cx,
			)
		});
		let height = cx.new(|cx| {
			SpinBox::new(
				42,
				SliderModel::new(
					ValueKind::Integer,
					16.0,
					8192.0,
					2.0,
					f64::from(seed.height),
				),
				window,
				cx,
			)
		});

		let rate = cx.new(|cx| ComboBox::new(43, sequence_rate_options(), window, cx));
		rate.update(cx, |combo, cx| combo.set_selected(seed.rate, cx));

		let interlaced = cx.new(|cx| {
			CheckBox::new(
				44,
				if seed.interlaced {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("seqprops.interlaced"))
		});

		// Picking a preset fills the numeric fields. The programmatic
		// set_value/set_selected calls below emit no events, so this never
		// loops back into itself.
		cx.subscribe(&preset, |this, _preset, event: &ComboBoxEvent, cx| {
			let ComboBoxEvent::Selected { value } = event;
			if let Some((w, h, num, den)) = sequence_preset_format(*value) {
				this.width.update(cx, |spin, cx| {
					spin.set_value(SliderValue::Integer(i64::from(w)), cx)
				});
				this.height.update(cx, |spin, cx| {
					spin.set_value(SliderValue::Integer(i64::from(h)), cx)
				});
				if let Some(index) = SEQUENCE_RATES.iter().position(|r| *r == (num, den)) {
					this.rate
						.update(cx, |combo, cx| combo.set_selected(Some(index), cx));
				}
				cx.notify();
			}
		})
		.detach();

		// Editing a dimension or the frame rate reverts to the custom entry.
		cx.subscribe(&width, |this, _spin, event: &SpinBoxEvent, cx| {
			if let SpinBoxEvent::ValueChanged { .. } = event {
				this.preset
					.update(cx, |combo, cx| combo.set_selected(Some(0), cx));
				cx.notify();
			}
		})
		.detach();
		cx.subscribe(&height, |this, _spin, event: &SpinBoxEvent, cx| {
			if let SpinBoxEvent::ValueChanged { .. } = event {
				this.preset
					.update(cx, |combo, cx| combo.set_selected(Some(0), cx));
				cx.notify();
			}
		})
		.detach();
		cx.subscribe(&rate, |this, _combo, event: &ComboBoxEvent, cx| {
			let ComboBoxEvent::Selected { .. } = event;
			this.preset
				.update(cx, |combo, cx| combo.set_selected(Some(0), cx));
			cx.notify();
		})
		.detach();

		// The interlaced checkbox is request-only: the host accepts the
		// toggled state back (the standard checkbox pattern).
		cx.subscribe(&interlaced, |_this, check, event: &CheckBoxEvent, cx| {
			let CheckBoxEvent::Toggled { state, .. } = event;
			check.update(cx, |check, cx| check.set_state(*state, cx));
		})
		.detach();

		Self {
			preset,
			width,
			height,
			rate,
			interlaced,
		}
	}

	/// Returns the video format described by the current field values.
	///
	/// Width and height are read from the spin boxes and clamped to at least 1
	/// pixel (the widgets themselves allow `16..=8192`). The frame rate comes from
	/// the rate combo index mapped through the common rate list; with nothing
	/// selected, or an out-of-range index, it falls back to 25/1. The result is
	/// therefore always a usable [`crate::oakui::engine::VideoFormat`].
	pub fn format(&self, cx: &App) -> crate::oakui::engine::VideoFormat {
		let width = self.width.read(cx).value().to_f64().max(1.0) as u32;
		let height = self.height.read(cx).value().to_f64().max(1.0) as u32;
		let (num, den) = self
			.rate
			.read(cx)
			.selected()
			.and_then(|i| SEQUENCE_RATES.get(i).copied())
			.unwrap_or((25, 1));
		crate::oakui::engine::VideoFormat {
			width,
			height,
			rate: FrameRate::new(num.max(1), den.max(1)),
		}
	}

	/// Returns whether the interlaced checkbox is currently checked.
	///
	/// Unchecked means progressive. The checkbox is seeded from the
	/// [`SequenceFormatSeed`] and is request-only, so [`Self::build`] installs a
	/// subscription that accepts each toggle; read the state here afterwards.
	pub fn interlaced(&self, cx: &App) -> bool {
		self.interlaced.read(cx).state() == CheckState::Checked
	}

	/// Returns the labeled rows for the format controls.
	///
	/// Builds a column containing the preset combo, a width + height row side by
	/// side, the frame-rate combo and the interlaced checkbox, each wrapped by
	/// [`form_row`] with a localized caption. `colors` supplies the theme colors
	/// that `form_row` uses for those captions. The returned [`gpui_kit::Div`] is
	/// not attached to any parent; callers add it to their own layout.
	pub fn rows(&self, colors: &gpui_kit::colors::Colors) -> gpui_kit::Div {
		let size_row = div()
			.flex()
			.gap_3()
			.child(form_row(
				colors,
				i18n::tr("seqprops.width").into(),
				self.width.clone(),
			))
			.child(form_row(
				colors,
				i18n::tr("seqprops.height").into(),
				self.height.clone(),
			));
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				colors,
				i18n::tr("seqprops.preset").into(),
				self.preset.clone(),
			))
			.child(size_row)
			.child(form_row(
				colors,
				i18n::tr("seqprops.frame_rate").into(),
				self.rate.clone(),
			))
			.child(form_row(
				colors,
				i18n::tr("seqprops.interlaced").into(),
				self.interlaced.clone(),
			))
	}
}

/// The new-sequence dialog content: the sequence name plus the format
/// fields. The host reads the name / format when the OK button is clicked.
pub struct NewSequenceContent<E: crate::oakui::engine::AppEngine> {
	engine: Entity<E>,
	pub(crate) name: Entity<TextValue>,
	pub(crate) format: Entity<SequenceFormatFields>,
	/// The commit error shown under the form (a rejected create keeps the
	/// dialog open).
	error: Option<String>,
}

impl<E: crate::oakui::engine::AppEngine> NewSequenceContent<E> {
	/// Builds the new-sequence dialog with the localized default name and the
	/// HD 1080p25 format seed.
	///
	/// `engine` is stored for [`Self::commit`] to create the sequence, `window`
	/// is required for the input widgets, and `cx` creates the entities. This is
	/// a thin wrapper over [`Self::new_seeded`] with
	/// [`SequenceFormatSeed::hd_1080p25`] (1920×1080, 25 fps, progressive); use
	/// `new_seeded` directly to start from probed footage parameters.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		Self::new_seeded(engine, SequenceFormatSeed::hd_1080p25(), window, cx)
	}

	/// Builds the content seeded with the default name and `seed`'s format
	/// (the footage-probe seed when a drop created the dialog, the HD
	/// 1080p25 defaults otherwise).
	pub fn new_seeded(
		engine: Entity<E>,
		seed: SequenceFormatSeed,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let name = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			TextValue { editor }
		});
		name.update(cx, |field, cx| {
			field.set_value(i18n::tr("seqprops.default_name"), window, cx)
		});

		let format = cx.new(|cx| SequenceFormatFields::build(seed, window, cx));

		Self {
			engine,
			name,
			format,
			error: None,
		}
	}

	/// Returns the sequence name currently typed into the field.
	///
	/// A [`SharedString`] snapshot of the text, seeded with the localized
	/// `seqprops.default_name` at construction. The host reads it on OK and hands
	/// it to the engine when creating the sequence.
	pub fn name(&self, cx: &App) -> SharedString {
		self.name.read(cx).value(cx)
	}

	/// Returns the video format currently shown by the format fields.
	///
	/// Width and height come from the spin boxes (in pixels) and the frame rate
	/// from the rate combo, so the value tracks the user's edits and the last
	/// applied preset. See [`SequenceFormatFields::format`] for the fallbacks.
	pub fn format(&self, cx: &App) -> crate::oakui::engine::VideoFormat {
		self.format.read(cx).format(cx)
	}

	/// Returns whether interlaced scanning is currently selected.
	///
	/// Reads the format fields' checkbox; `false` means progressive. The host
	/// combines it with [`Self::name`] and [`Self::format`] when creating the
	/// sequence.
	pub fn interlaced(&self, cx: &App) -> bool {
		self.format.read(cx).interlaced(cx)
	}

	/// Applies the dialog (the C++ `accept()`): creates the sequence with the
	/// entered name / format and clears the error row.
	pub fn commit(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
		let name = self.name(cx).to_string();
		let format = self.format(cx);
		let interlaced = self.interlaced(cx);
		self.engine.update(cx, |engine, cx| {
			engine.create_sequence_with_params(name, format, interlaced, cx)
		})?;
		self.set_error(None, cx);
		Ok(())
	}

	/// Sets or clears the error shown under the new-sequence form.
	///
	/// `msg` renders in red below the fields while it is `Some`; `None` removes
	/// the row, which is what a successful [`Self::commit`] does. `cx` is notified
	/// so the dialog repaints. The host passes the engine's message when creating
	/// the sequence fails, leaving the dialog open.
	pub fn set_error(&mut self, msg: Option<String>, cx: &mut Context<Self>) {
		self.error = msg;
		cx.notify();
	}

	/// The commit error currently shown (`None` while the last commit
	/// applied cleanly).
	pub fn error(&self) -> Option<&String> {
		self.error.as_ref()
	}
}

impl<E: crate::oakui::engine::AppEngine> Render for NewSequenceContent<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let format_rows = self.format.read(cx).rows(&colors);
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				&colors,
				i18n::tr("seqprops.name").into(),
				self.name.clone(),
			))
			.child(format_rows)
			.child(if let Some(error) = &self.error {
				div()
					.debug_selector(|| "seqprops-error".into())
					.text_color(gpui_kit::rgb(0xe5484d))
					.text_xs()
					.child(error.clone())
			} else {
				div()
			})
	}
}

/// The sequence-properties dialog content: the sequence name plus the
/// format fields, seeded from the sequence's current parameters.
pub struct SequencePropertiesContent<E: crate::oakui::engine::AppEngine> {
	engine: Entity<E>,
	sequence_id: u64,
	pub(crate) name: Entity<TextValue>,
	pub(crate) format: Entity<SequenceFormatFields>,
	/// The commit error shown under the form.
	error: Option<String>,
}

impl<E: crate::oakui::engine::AppEngine> SequencePropertiesContent<E> {
	/// Builds the sequence-properties dialog seeded from the sequence's current
	/// parameters.
	///
	/// `engine` is queried through `sequence_parameters(sequence_id)` and the
	/// result seeds the name field, the preset (the first preset whose exact
	/// width/height/rate matches, otherwise the custom entry), the width/height
	/// spin boxes, the rate combo (selected only when the rate is one of the
	/// common options) and the interlaced checkbox. `sequence_id` is the
	/// project-entry id of the sequence being edited; it is kept so
	/// [`Self::commit`] updates that same sequence. When it is unknown the name is
	/// blank and the fields fall back to 1920×1080 at 25 fps progressive, but the
	/// preset combo shows *custom* rather than HD 1080p25 and the rate combo has
	/// no selection.
	///
	/// `window` is needed for the input widgets and `cx` to create the entities;
	/// the error slot starts empty.
	pub fn new(
		engine: Entity<E>,
		sequence_id: u64,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let current = engine.read(cx).sequence_parameters(sequence_id);

		let name = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			TextValue { editor }
		});
		let current_name = current.as_ref().map(|p| p.name.as_str()).unwrap_or("");
		name.update(cx, |field, cx| field.set_value(current_name, window, cx));

		let seed = match &current {
			Some(params) => {
				let preset = (1..=SEQUENCE_PRESET_COUNT)
					.find(|i| {
						sequence_preset_format(*i)
							== Some((
								params.format.width,
								params.format.height,
								params.format.rate.num,
								params.format.rate.den,
							))
					})
					.unwrap_or(0);
				let rate = SEQUENCE_RATES
					.iter()
					.position(|r| *r == (params.format.rate.num, params.format.rate.den));
				SequenceFormatSeed {
					preset,
					width: params.format.width,
					height: params.format.height,
					rate,
					interlaced: params.interlaced,
				}
			}
			None => SequenceFormatSeed {
				preset: 0,
				width: 1920,
				height: 1080,
				rate: None,
				interlaced: false,
			},
		};
		let format = cx.new(|cx| SequenceFormatFields::build(seed, window, cx));

		Self {
			engine,
			sequence_id,
			name,
			format,
			error: None,
		}
	}

	/// Returns the sequence name currently typed into the field.
	///
	/// A [`SharedString`] snapshot of the text, seeded from the sequence's
	/// current name (empty when the sequence is missing). The host reads it on
	/// OK and passes it to the engine's update call.
	pub fn name(&self, cx: &App) -> SharedString {
		self.name.read(cx).value(cx)
	}

	/// Returns the video format currently shown by the format fields.
	///
	/// Width and height come from the spin boxes (in pixels) and the frame rate
	/// from the rate combo, so the value tracks the user's edits and the last
	/// applied preset. See [`SequenceFormatFields::format`] for the fallbacks.
	pub fn format(&self, cx: &App) -> crate::oakui::engine::VideoFormat {
		self.format.read(cx).format(cx)
	}

	/// Returns whether interlaced scanning is currently selected.
	///
	/// Reads the format fields' checkbox; `false` means progressive. The host
	/// combines it with [`Self::name`] and [`Self::format`] when applying the
	/// edits.
	pub fn interlaced(&self, cx: &App) -> bool {
		self.format.read(cx).interlaced(cx)
	}

	/// Applies the edits (the C++ `accept()`): updates the sequence's name /
	/// format and clears the error row.
	pub fn commit(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
		let name = self.name(cx).to_string();
		let format = self.format(cx);
		let interlaced = self.interlaced(cx);
		self.engine.update(cx, |engine, cx| {
			engine.update_sequence_parameters(self.sequence_id, name, format, interlaced, cx)
		})?;
		self.set_error(None, cx);
		Ok(())
	}

	/// Sets or clears the error shown under the sequence-properties form.
	///
	/// `msg` renders in red below the fields while it is `Some`; `None` removes
	/// the row, which is what a successful [`Self::commit`] does. `cx` is notified
	/// so the dialog repaints. The host passes the engine's message when applying
	/// the edits fails, leaving the dialog open.
	pub fn set_error(&mut self, msg: Option<String>, cx: &mut Context<Self>) {
		self.error = msg;
		cx.notify();
	}

	/// The commit error currently shown (`None` while the last commit
	/// applied cleanly).
	pub fn error(&self) -> Option<&String> {
		self.error.as_ref()
	}
}

impl<E: crate::oakui::engine::AppEngine> Render for SequencePropertiesContent<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let format_rows = self.format.read(cx).rows(&colors);
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				&colors,
				i18n::tr("seqprops.name").into(),
				self.name.clone(),
			))
			.child(format_rows)
			.child(if let Some(error) = &self.error {
				div()
					.debug_selector(|| "seqprops-error".into())
					.text_color(gpui_kit::rgb(0xe5484d))
					.text_xs()
					.child(error.clone())
			} else {
				div()
			})
	}
}

/// The outcome of the drop-onto-empty-timeline choice (拖到空时间轴上的素材
/// 需要先创建一个序列：探测素材参数作为序列参数，还是手工指定参数)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSequenceChoice {
	/// Create the sequence from the footage's probed video parameters
	/// (the previous auto-create behaviour; the engine's `drop_footage`
	/// sizes the new sequence from the probe).
	UseFootageParams,
	/// Open the new-sequence dialog (seeded with the probed parameters)
	/// and create the sequence with the user's entered settings.
	SpecifyManually,
}

/// The content of the drop-onto-empty-timeline choice dialog: the
/// footage's probed video parameters (or a note when the footage is
/// pure audio). The host routes the modal's buttons to
/// [`DropSequenceChoice`].
pub struct DropSequenceChoiceContent {
	/// The footage's first probed video stream (`width, height, rate,
	/// interlaced`), when the footage carries one.
	probed: Option<(u32, u32, crate::widgets::timeline::FrameRate, bool)>,
}

impl DropSequenceChoiceContent {
	/// Builds the content seeded with the footage's probed video stream.
	pub fn new(probed: Option<(u32, u32, crate::widgets::timeline::FrameRate, bool)>) -> Self {
		Self { probed }
	}

	/// The footage's probed parameters shown in the body.
	pub fn probed(&self) -> Option<(u32, u32, crate::widgets::timeline::FrameRate, bool)> {
		self.probed
	}
}

impl Render for DropSequenceChoiceContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let body = match self.probed {
			Some((width, height, rate, interlaced)) => {
				let base = format!(
					"{} × {}, {}",
					width,
					height,
					crate::oakui::timecode::format_fps(rate),
				);
				if interlaced {
					format!("{base} · {}", i18n::tr("seqprops.interlaced"))
				} else {
					base
				}
			}
			None => i18n::tr("seqprops.drop.no_video").to_string(),
		};
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(
				div()
					.text_color(colors.disabled)
					.child(i18n::tr("seqprops.drop.caption")),
			)
			.child(
				div()
					.debug_selector(|| "drop-choice-params".into())
					.text_color(colors.text)
					.text_sm()
					.child(body),
			)
	}
}
