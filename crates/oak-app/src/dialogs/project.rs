use super::*;

/// The rename dialog: a single text field prefilled with the entry's
/// current name (the project-explorer 重命名 context item; the host reads
/// [`Self::value`] on OK and calls the engine's `rename_entry`).
pub struct RenameContent {
	field: Entity<TextValue>,
}

impl RenameContent {
	/// Builds the rename dialog with the name field pre-filled from `current`.
	///
	/// `current` is the entry's existing name and is moved into the field;
	/// `window` is needed for the input state and `cx` to create the entities.
	/// The field is seeded before returning, so [`Self::value`] can be read
	/// immediately.
	pub fn new(current: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let field = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			TextValue { editor }
		});
		field.update(cx, |field, cx| field.set_value(current, window, cx));
		Self { field }
	}

	/// Returns the name currently typed into the field.
	///
	/// A [`SharedString`] snapshot of the text, which the host passes to the
	/// engine's `rename_entry` on OK. The name may be empty if the user cleared
	/// the pre-filled value; this function performs no validation.
	pub fn value(&self, cx: &App) -> SharedString {
		self.field.read(cx).value(cx)
	}
}

impl Render for RenameContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div().flex().flex_col().gap_3().w_full().child(form_row(
			&colors,
			i18n::tr("project.context.rename").into(),
			self.field.clone(),
		))
	}
}

/// The project export dialog (文件 → 导出工程文件…): choose the project
/// file format (OTIO / OVE / FCPXML) and the output path. The host reads
/// [`Self::format`] + [`Self::path`] on OK.
pub struct ExportProjectDialogContent {
	pub(crate) format: Entity<ComboBox>,
	/// (format id, display name) in dropdown order.
	formats: Vec<(i32, String)>,
}

/// The project-file format ids (mirror the engine's serializer dispatch;
/// the extension is derived on the host).
pub const PROJECT_FORMAT_OTIO: i32 = 0;
pub const PROJECT_FORMAT_OVE: i32 = 1;
pub const PROJECT_FORMAT_FCPXML: i32 = 2;

impl ExportProjectDialogContent {
	/// Builds the project export dialog, defaulting to OTIO.
	///
	/// `window` is required to build the format combo and `cx` to create it. The
	/// combo lists OTIO / OVE / FCPXML with display names, and the selection
	/// starts on [`PROJECT_FORMAT_OTIO`]. The output path is deliberately not
	/// part of this dialog: on OK the host opens the platform save dialog and
	/// suggests a name carrying [`Self::extension`].
	pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		let formats: Vec<(i32, String)> = vec![
			(PROJECT_FORMAT_OTIO, "OpenTimelineIO (.otio)".to_string()),
			(PROJECT_FORMAT_OVE, "Oak (.ove)".to_string()),
			(
				PROJECT_FORMAT_FCPXML,
				"Final Cut Pro XML (.fcpxml)".to_string(),
			),
		];
		let options = formats
			.iter()
			.enumerate()
			.map(|(i, (_, name))| ComboBoxOption::new(i, name.clone()))
			.collect();
		let format = cx.new(|cx| {
			ComboBox::new(4, options, window, cx)
				.with_placeholder(i18n::tr("project.export.format"))
		});
		format.update(cx, |combo, cx| {
			combo.set_selected(Some(PROJECT_FORMAT_OTIO as usize), cx)
		});
		Self { format, formats }
	}

	/// Returns the selected project-file format id.
	///
	/// One of [`PROJECT_FORMAT_OTIO`], [`PROJECT_FORMAT_OVE`],
	/// [`PROJECT_FORMAT_FCPXML`], in dropdown order. When the combo has no
	/// selection, or its stored index is out of range, this falls back to
	/// [`PROJECT_FORMAT_OTIO`] so callers always receive a valid id.
	pub fn format(&self, cx: &App) -> i32 {
		let Some(selected) = self.format.read(cx).selected() else {
			return PROJECT_FORMAT_OTIO;
		};
		self.formats
			.get(selected)
			.map(|(id, _)| *id)
			.unwrap_or(PROJECT_FORMAT_OTIO)
	}

	/// Returns the selected project format's file extension, without the dot.
	///
	/// `"otio"`, `"ove"` or `"fcpxml"` according to [`Self::format`]; an
	/// unrecognized id falls back to `"otio"`. The host appends it to the
	/// suggested file name it hands to the platform save dialog.
	pub fn extension(&self, cx: &App) -> &'static str {
		match self.format(cx) {
			PROJECT_FORMAT_OVE => "ove",
			PROJECT_FORMAT_FCPXML => "fcpxml",
			_ => "otio",
		}
	}
}

impl Render for ExportProjectDialogContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div().flex().flex_col().gap_3().w_full().child(form_row(
			&colors,
			i18n::tr("project.export.format").into(),
			self.format.clone(),
		))
	}
}

/// The new-project dialog (文件 → 新建项目): project name + the first
/// sequence's format (preset / custom width-height-rate / interlaced).
/// The host reads [`Self::name`] + [`Self::format`]; the engine's
/// library_create_project then seeds the first sequence.
pub struct NewProjectContent {
	pub(crate) name: Entity<TextValue>,
	pub(crate) format: Entity<SequenceFormatFields>,
}

impl NewProjectContent {
	/// Builds the new-project dialog with a localized default name and the
	/// HD 1080p25 format preset.
	///
	/// `window` is required to create the input widgets and `cx` to create the
	/// entities. The name field is seeded with the `manager.new.default_name`
	/// translation, and the format fields are built from
	/// [`SequenceFormatSeed::hd_1080p25`] (1920×1080, 25 fps, progressive).
	pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		let name = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			TextValue { editor }
		});
		name.update(cx, |field, cx| {
			field.set_value(i18n::tr("manager.new.default_name"), window, cx)
		});
		let format =
			cx.new(|cx| SequenceFormatFields::build(SequenceFormatSeed::hd_1080p25(), window, cx));
		Self { name, format }
	}

	/// Returns the project name currently typed into the name field.
	///
	/// The value is a [`SharedString`] snapshot of the field text. It is empty
	/// only if the user cleared the localized default the dialog was seeded
	/// with; this function performs no validation. Read by the host on OK.
	pub fn name(&self, cx: &App) -> SharedString {
		self.name.read(cx).value(cx)
	}

	/// Returns the video format currently selected in the format fields.
	///
	/// The [`crate::oakui::engine::VideoFormat`] holds width and height in
	/// pixels plus a rational frame rate; it reflects whatever preset or custom
	/// width/height/rate combination the fields currently show, so it changes as
	/// the user edits them. Read by the host on OK.
	pub fn format(&self, cx: &App) -> crate::oakui::engine::VideoFormat {
		self.format.read(cx).format(cx)
	}

	/// Returns whether interlaced scanning is selected for the new project.
	///
	/// Reads the interlaced checkbox inside the embedded
	/// [`SequenceFormatFields`]; `false` means progressive. The host combines it
	/// with [`Self::format`] when creating the project and its first sequence.
	pub fn interlaced(&self, cx: &App) -> bool {
		self.format.read(cx).interlaced(cx)
	}
}

impl Render for NewProjectContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				&colors,
				i18n::tr("manager.new.project_name").into(),
				self.name.clone(),
			))
			.child(self.format.read(cx).rows(&colors))
	}
}
