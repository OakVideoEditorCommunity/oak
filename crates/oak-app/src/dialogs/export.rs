use super::*;

/// A text field with the same shape as the file dialog's path field.
pub struct PathField {
	pub(crate) editor: Entity<InputState>,
	/// Whether the field accepts input (disabled fields dim and drop the
	/// text-input handler, like the checkbox disabled state).
	pub(crate) enabled: bool,
}

impl PathField {
	/// Returns the path text currently held by the field.
	///
	/// A [`SharedString`] snapshot of the inner editor's value, i.e. what the user
	/// typed or what [`Self::set_path`] last wrote. `app` is only used to read the
	/// entity; the value is not validated or normalized here.
	pub fn path(&self, app: &App) -> SharedString {
		self.editor.read(app).value()
	}

	/// Replaces the path text shown in the field.
	///
	/// `path` is converted into a [`SharedString`] and written into the field's
	/// own editor, so the caller does not have to retype it; `window` is required
	/// by the editor's setter and `cx` is notified so the field repaints. This is
	/// a programmatic change, so it emits no input event.
	pub fn set_path(
		&mut self,
		path: impl Into<SharedString>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let path = path.into();
		self.editor.update(cx, |editor, cx| {
			editor.set_value(path.as_ref(), window, cx);
		});
		cx.notify();
	}

	/// Enables or disables input in the field.
	///
	/// With `enabled == false` the render path drops the text-input handler and
	/// lowers the field's opacity, making it read-only; `true` restores editing.
	/// Setting the value the field already has is a no-op, so no repaint is
	/// scheduled; otherwise `cx` is notified.
	pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
		if self.enabled == enabled {
			return;
		}
		self.enabled = enabled;
		cx.notify();
	}

	/// The input state the field starts in (chainable after construction).
	pub fn with_enabled(mut self, enabled: bool) -> Self {
		self.enabled = enabled;
		self
	}
}

impl Render for PathField {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let weak = self.editor.downgrade();
		let enabled = self.enabled;
		div()
			.rounded_md()
			.border_1()
			.border_color(colors.border)
			.bg(colors.background)
			.px_2()
			.py_1()
			.opacity(if enabled { 1.0 } else { 0.45 })
			.child(
				text_input("gpui-widgets-export-path", cx)
					.state(weak)
					.accepts_input(enabled),
			)
	}
}

/// The export dialog content: the container-format dropdown and the output
/// path field.
pub struct ExportDialogContent {
	pub(crate) sequence: Entity<ComboBox>,
	pub(crate) format: Entity<ComboBox>,
	pub(crate) video_codec: Entity<ComboBox>,
	pub(crate) audio_codec: Entity<ComboBox>,
	pub(crate) color: Entity<ComboBox>,
	pub(crate) range: Entity<ComboBox>,
	pub(crate) resolution_w: Entity<SpinBox>,
	pub(crate) resolution_h: Entity<SpinBox>,
	pub(crate) frame_rate: Entity<SpinBox>,
	pub(crate) rate_control: Entity<ComboBox>,
	pub(crate) bitrate: Entity<SpinBox>,
	pub(crate) quality: Entity<SpinBox>,
	pub(crate) path: Entity<PathField>,
	/// (format id, display name, extension) in dropdown order.
	pub(crate) formats: Vec<(i32, String, String)>,
	/// The container format for the current codec lists.
	pub(crate) active_format: i32,
	/// (entry id, display name) of the pickable sequences, in dropdown
	/// order; populated by the host from the open project.
	pub(crate) sequences: Vec<(u64, SharedString)>,
	/// Kept alive so format / codec / rate-control changes update the dialog.
	_subscriptions: Vec<gpui_kit::Subscription>,
}

/// Whether `video_codec` supports CRF / CQP quality-based rate control.
///
/// H.264 (`1`), H.264 RGB (`2`), H.265 (`3`) and AV1 (`18`) support constant
/// quality modes; mezzanine/intraframe codecs (DNxHD, ProRes, CineForm,
/// MPEG-2) stay in bitrate mode.
pub fn codec_supports_quality_control(video_codec: i32) -> bool {
	matches!(video_codec, 1 | 2 | 3 | 18)
}

/// Returns `(min, max, default)` quality bounds for `video_codec`.
///
/// AV1 (`18`) uses a `0..=63` scale (default `30`); H.264 and H.265 use
/// `0..=51` (default `23`). Lower values mean higher quality.
pub fn codec_quality_bounds(video_codec: i32) -> (i32, i32, i32) {
	match video_codec {
		18 => (0, 63, 30),
		_ => (0, 51, 23),
	}
}

fn rate_control_options(supports_quality: bool) -> Vec<ComboBoxOption> {
	let mut opts = vec![ComboBoxOption::new(
		0,
		i18n::tr("export.rate_control.bitrate"),
	)];
	if supports_quality {
		opts.push(ComboBoxOption::new(1, i18n::tr("export.rate_control.crf")));
		opts.push(ComboBoxOption::new(2, i18n::tr("export.rate_control.cqp")));
	}
	opts
}

impl ExportDialogContent {
	/// Builds the export dialog from the engine's format and codec tables.
	///
	/// The format list comes from the engine's encoding-format enumeration
	/// (entries with an empty extension are dropped) and starts on MP4. The
	/// video / audio codec combos are seeded from the MP4-compatible tables, the
	/// color combo defaults to SDR (Rec.709, 8-bit output) and the range combo to
	/// the whole sequence. Resolution, frame rate and bitrate start at `0`, which
	/// the engine reads as "keep the sequence's / codec's default". The path field
	/// and the sequence picker start empty; the host fills the picker through
	/// [`Self::set_sequences`], and while it is empty OK stays disabled.
	///
	/// Note: the format-change subscription is created and dropped in the same
	/// statement (deliberately kept to preserve behavior), so in the current
	/// implementation switching the container does not call
	/// [`Self::apply_format`]. `window` is required by the widgets and `cx`
	/// creates the entities.
	pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		let formats: Vec<(i32, String, String)> = encoding_formats()
			.into_iter()
			.filter(|(_, _, ext)| !ext.is_empty())
			.collect();
		let options = formats
			.iter()
			.enumerate()
			.map(|(i, (_, name, ext))| ComboBoxOption::new(i, format!("{name} (.{ext})")))
			.collect();
		let format = cx.new(|cx| {
			ComboBox::new(3, options, window, cx)
				.with_placeholder(i18n::tr("export.format.placeholder"))
		});
		let mp4_index = formats
			.iter()
			.position(|(id, _, _)| *id == EXPORT_FORMAT_MP4)
			.unwrap_or(0);
		format.update(cx, |combo, cx| combo.set_selected(Some(mp4_index), cx));

		let active_format = formats
			.get(mp4_index)
			.map(|(id, _, _)| *id)
			.unwrap_or(EXPORT_FORMAT_MP4);

		// Video / audio codec combo boxes, populated by the container.
		// The lists come straight from `Format::get_video_codecs` /
		// `get_audio_codecs`: only codecs the container supports appear,
		// so an incompatible codec is unselectable by construction.
		let codec_options = |codes: Vec<i32>| {
			codes
				.iter()
				.enumerate()
				.map(|(i, code)| {
					ComboBoxOption::new(
						i,
						quercus_engine_rs::codec::Codec::from_i32(*code)
							.unwrap_or(quercus_engine_rs::codec::Codec::H264)
							.name(),
					)
				})
				.collect::<Vec<_>>()
		};
		let video_codecs = compatible_video_codecs(active_format);
		let initial_video_codec = video_codecs.first().copied().unwrap_or(1);
		let audio_codecs = compatible_audio_codecs(active_format);
		let video_codec = cx.new(|cx| {
			ComboBox::new(5, codec_options(video_codecs), window, cx)
				.with_placeholder(i18n::tr("export.video_codec"))
		});
		video_codec.update(cx, |combo, cx| combo.set_selected(Some(0), cx));
		let audio_codec = cx.new(|cx| {
			ComboBox::new(6, codec_options(audio_codecs), window, cx)
				.with_placeholder(i18n::tr("export.audio_codec"))
		});
		audio_codec.update(cx, |combo, cx| combo.set_selected(Some(0), cx));

		let color = cx.new(|cx| {
			let options = vec![
				ComboBoxOption::new(0, i18n::tr("export.color.sdr")),
				ComboBoxOption::new(1, i18n::tr("export.color.hdr")),
			];
			ComboBox::new(7, options, window, cx).with_placeholder(i18n::tr("export.color"))
		});
		color.update(cx, |combo, cx| combo.set_selected(Some(0), cx));

		let range = cx.new(|cx| {
			let options = vec![
				ComboBoxOption::new(0, i18n::tr("export.range.all")),
				ComboBoxOption::new(1, i18n::tr("export.range.inout")),
			];
			ComboBox::new(8, options, window, cx).with_placeholder(i18n::tr("export.range"))
		});
		range.update(cx, |combo, cx| combo.set_selected(Some(0), cx));

		// Resolution / frame rate / bitrate SPIN boxes: 0 = keep the
		// sequence's / the codec's default (the engine treats 0 that way),
		// any typed value is honored (the backend accepts arbitrary
		// width/height/rate/bitrate).
		let resolution_w = cx.new(|cx| {
			SpinBox::new(
				9,
				SliderModel::new(ValueKind::Integer, 0.0, 16384.0, 4.0, 0.0),
				window,
				cx,
			)
		});
		let resolution_h = cx.new(|cx| {
			SpinBox::new(
				12,
				SliderModel::new(ValueKind::Integer, 0.0, 16384.0, 4.0, 0.0),
				window,
				cx,
			)
		});
		let frame_rate = cx.new(|cx| {
			SpinBox::new(
				10,
				SliderModel::new(ValueKind::Float, 0.0, 240.0, 0.1, 0.0),
				window,
				cx,
			)
		});
		let rate_control = cx.new(|cx| {
			let options = rate_control_options(codec_supports_quality_control(initial_video_codec));
			ComboBox::new(14, options, window, cx).with_placeholder(i18n::tr("export.rate_control"))
		});
		rate_control.update(cx, |combo, cx| combo.set_selected(Some(0), cx));

		let bitrate = cx.new(|cx| {
			SpinBox::new(
				11,
				SliderModel::new(ValueKind::Integer, 0.0, 500_000_000.0, 1_000_000.0, 0.0),
				window,
				cx,
			)
		});
		let (min_q, max_q, default_q) = codec_quality_bounds(initial_video_codec);
		let quality = cx.new(|cx| {
			SpinBox::new(
				15,
				SliderModel::new(
					ValueKind::Integer,
					min_q as f64,
					max_q as f64,
					1.0,
					default_q as f64,
				),
				window,
				cx,
			)
		});

		let path = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			PathField {
				editor,
				enabled: true,
			}
		});

		// The sequence picker is filled by the host (it owns the project);
		// an empty list disables the dialog's OK via the picker staying
		// unselected.
		let sequence = cx.new(|cx| {
			ComboBox::new(13, Vec::new(), window, cx).with_placeholder(i18n::tr("export.sequence"))
		});

		// Persist subscriptions so container, codec, and rate-control changes
		// keep the dependent dropdowns and conditional quality/bitrate row in sync.
		let format_sub = cx.subscribe(&format, |this, _format, event: &ComboBoxEvent, cx| {
			let ComboBoxEvent::Selected { value } = event;
			let fmt = this
				.formats
				.get(*value)
				.map(|(id, _, _)| *id)
				.unwrap_or(EXPORT_FORMAT_MP4);
			this.apply_format(fmt, cx);
		});
		let codec_sub = cx.subscribe(&video_codec, |this, _codec, _event: &ComboBoxEvent, cx| {
			this.sync_rate_control_for_codec(cx);
		});
		let rc_sub = cx.subscribe(&rate_control, |_this, _rc, _event: &ComboBoxEvent, cx| {
			cx.notify();
		});

		Self {
			sequence,
			format,
			video_codec,
			audio_codec,
			color,
			range,
			resolution_w,
			resolution_h,
			frame_rate,
			rate_control,
			bitrate,
			quality,
			path,
			formats,
			active_format,
			sequences: Vec::new(),
			_subscriptions: vec![format_sub, codec_sub, rc_sub],
		}
	}

	/// Populates the sequence picker from the open project and preselects
	/// `selected` (falling back to the first entry — the dialog always has
	/// a target to export).
	pub fn set_sequences(
		&mut self,
		sequences: Vec<(u64, SharedString)>,
		selected: Option<u64>,
		cx: &mut Context<Self>,
	) {
		let preselect = selected
			.and_then(|id| sequences.iter().position(|(entry, _)| *entry == id))
			.or(if sequences.is_empty() { None } else { Some(0) });
		let options = sequences
			.iter()
			.enumerate()
			.map(|(i, (_, name))| ComboBoxOption::new(i, name.clone()))
			.collect::<Vec<_>>();
		self.sequence
			.update(cx, |combo, cx| combo.set_options(options, cx));
		self.sequence
			.update(cx, |combo, cx| combo.set_selected(preselect, cx));
		self.sequences = sequences;
		cx.notify();
	}

	/// The picked sequence's project-entry id (`None` only when the
	/// project has no sequences — the host refuses to open the dialog
	/// then).
	pub fn selected_sequence(&self, cx: &App) -> Option<u64> {
		self.sequence
			.read(cx)
			.selected()
			.and_then(|index| self.sequences.get(index).map(|(id, _)| *id))
	}

	/// Rebuilds the codec lists for container `fmt` and selects the first entry.
	///
	/// `fmt` is a [`quercus_engine_rs::codec::Format`] id. Both lists come from
	/// the container's compatibility tables, so a codec the container cannot carry
	/// is never offered; a codec id missing from the label table falls back to
	/// H.264 for video and AAC for audio. Also records `fmt` as the active format
	/// reported by [`Self::settings`] and syncs rate-control / quality bounds.
	/// Callers: [`Self::new`]'s persisted format-combo subscription and tests.
	pub(crate) fn apply_format(&mut self, fmt: i32, cx: &mut Context<Self>) {
		let video_codes = compatible_video_codecs(fmt);
		let audio_codes = compatible_audio_codecs(fmt);
		let v_opts = video_codes
			.iter()
			.enumerate()
			.map(|(i, code)| {
				ComboBoxOption::new(
					i,
					quercus_engine_rs::codec::Codec::from_i32(*code)
						.unwrap_or(quercus_engine_rs::codec::Codec::H264)
						.name(),
				)
			})
			.collect::<Vec<_>>();
		let a_opts = audio_codes
			.iter()
			.enumerate()
			.map(|(i, code)| {
				ComboBoxOption::new(
					i,
					quercus_engine_rs::codec::Codec::from_i32(*code)
						.unwrap_or(quercus_engine_rs::codec::Codec::AAC)
						.name(),
				)
			})
			.collect::<Vec<_>>();
		self.video_codec
			.update(cx, |combo, cx| combo.set_options(v_opts, cx));
		self.video_codec
			.update(cx, |combo, cx| combo.set_selected(Some(0), cx));
		self.audio_codec
			.update(cx, |combo, cx| combo.set_options(a_opts, cx));
		self.audio_codec
			.update(cx, |combo, cx| combo.set_selected(Some(0), cx));
		self.active_format = fmt;
		self.sync_rate_control_for_codec(cx);
	}

	/// Returns the currently selected video codec id for `active_format`.
	pub(crate) fn active_video_codec(&self, cx: &App) -> i32 {
		compatible_video_codecs(self.active_format)
			.get(self.video_codec.read(cx).selected().unwrap_or(0))
			.copied()
			.unwrap_or(1)
	}

	/// Syncs the rate-control dropdown and quality bounds to the active video
	/// codec. Codecs without CRF/CQP support fall back to Bitrate-only; codecs
	/// that support quality control clamp the current quality value into the
	/// codec's valid range (`0..=51` for H.264/H.265, `0..=63` for AV1).
	pub(crate) fn sync_rate_control_for_codec(&mut self, cx: &mut Context<Self>) {
		let codec = self.active_video_codec(cx);
		let supports_quality = codec_supports_quality_control(codec);
		let prev = self.rate_control.read(cx).selected().unwrap_or(0);
		let next = if supports_quality { prev.min(2) } else { 0 };
		self.rate_control.update(cx, |combo, cx| {
			combo.set_options(rate_control_options(supports_quality), cx);
			combo.set_selected(Some(next), cx);
		});
		if supports_quality {
			let (min_q, max_q, _) = codec_quality_bounds(codec);
			let cur = self.quality.read(cx).value().to_f64().round() as i32;
			let clamped = cur.clamp(min_q, max_q);
			self.quality.update(cx, |spin, cx| {
				spin.set_model(
					SliderModel::new(
						ValueKind::Integer,
						min_q as f64,
						max_q as f64,
						1.0,
						clamped as f64,
					),
					cx,
				);
			});
		}
		cx.notify();
	}

	/// Returns the container-format id currently shown in the combo.
	///
	/// With no selection, or an index outside the format table, it falls back to
	/// [`EXPORT_FORMAT_MP4`].
	pub fn format(&self, cx: &App) -> i32 {
		let Some(selected) = self.format.read(cx).selected() else {
			return EXPORT_FORMAT_MP4;
		};
		self.formats
			.get(selected)
			.map(|(id, _, _)| *id)
			.unwrap_or(EXPORT_FORMAT_MP4)
	}

	/// Returns the selected format's file extension, without the leading dot.
	///
	/// Taken from the format table entry (for example `"mp4"`); with no selection,
	/// or an index outside the table, it falls back to `"mp4"`. The host appends
	/// it to the suggested file name in the platform save dialog so the output
	/// always carries the container's extension.
	pub fn extension(&self, cx: &App) -> String {
		let Some(selected) = self.format.read(cx).selected() else {
			return "mp4".to_string();
		};
		self.formats
			.get(selected)
			.map(|(_, _, ext)| ext.clone())
			.unwrap_or_else(|| "mp4".to_string())
	}

	/// Returns the output path currently typed into the field.
	///
	/// A [`SharedString`] snapshot of the text. It may be empty: the field starts
	/// empty and the host normally pre-fills it with [`Self::set_path`]. Nothing
	/// is validated here — an unusable path surfaces as an engine error when the
	/// export starts.
	pub fn path(&self, cx: &App) -> SharedString {
		self.path.read(cx).path(cx)
	}

	/// Pre-fills the output path field.
	///
	/// `path` is converted into a [`SharedString`] and written into the nested
	/// [`PathField`]; `window` is required by that field's editor and `cx` is
	/// notified so the dialog repaints. The host calls this with a suggested name
	/// derived from the sequence and the selected format.
	pub fn set_path(
		&mut self,
		path: impl Into<SharedString>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let path = path.into();
		self.path
			.update(cx, |content, cx| content.set_path(path, window, cx));
		cx.notify();
	}

	/// The full export settings (codec/color/bit-depth/range/size/fps/
	/// bitrate) the engine consumes. The codec combos expose only the
	/// container's compatible codes, so the picked ids are always valid
	/// for `format`.
	pub fn settings(&self, cx: &App) -> crate::oakui::engine::ExportSettings {
		let video_codec = self.active_video_codec(cx);
		let audio_codec = compatible_audio_codecs(self.active_format)
			.get(self.audio_codec.read(cx).selected().unwrap_or(0))
			.copied()
			.unwrap_or(12);
		let hdr = self.color.read(cx).selected() == Some(1);
		let in_out = self.range.read(cx).selected() == Some(1);
		let w = self.resolution_w.read(cx).value().to_f64().round() as i32;
		let h = self.resolution_h.read(cx).value().to_f64().round() as i32;
		let size = if w > 0 && h > 0 { (w, h) } else { (0, 0) };
		let frame_rate = self.frame_rate.read(cx).value().to_f64();
		let video_bitrate = self.bitrate.read(cx).value().to_f64().round() as i64;
		let rate_control = if codec_supports_quality_control(video_codec) {
			self.rate_control.read(cx).selected().unwrap_or(0).min(2) as i32
		} else {
			0
		};
		let (min_q, max_q, _) = codec_quality_bounds(video_codec);
		let video_quality =
			(self.quality.read(cx).value().to_f64().round() as i32).clamp(min_q, max_q);
		crate::oakui::engine::ExportSettings {
			format: self.active_format,
			video_codec,
			audio_codec,
			size,
			frame_rate,
			video_bitrate,
			rate_control,
			video_quality,
			audio_bitrate: 0,
			bit_depth: if hdr { 10 } else { 8 },
			color_primaries: if hdr { 9 } else { 1 },
			color_transfer: if hdr { 16 } else { 1 },
			color_space: if hdr { 9 } else { 1 },
			range: if in_out { Some((0.0, 0.0)) } else { None },
		}
	}
}

/// The video codecs the container `format` accepts (the compatibility
/// table; the dialog only offers these).
pub fn compatible_video_codecs(format: i32) -> Vec<i32> {
	let Some(container) = quercus_engine_rs::codec::Format::from_i32(format) else {
		return vec![1]; // H.264 fallback
	};
	container.video_codecs().iter().map(|c| c as i32).collect()
}

/// Returns the audio codec ids the container `format` accepts.
///
/// `format` is a [`quercus_engine_rs::codec::Format`] id; the result is that
/// container's audio-codec table in the engine's order, which is exactly what the
/// export dialog offers, so an incompatible codec cannot be picked. An unknown
/// container id yields the single-element fallback `[12]` (AAC) instead of
/// returning an error or an empty list.
pub fn compatible_audio_codecs(format: i32) -> Vec<i32> {
	let Some(container) = quercus_engine_rs::codec::Format::from_i32(format) else {
		return vec![12]; // AAC fallback
	};
	container.audio_codecs().iter().map(|c| c as i32).collect()
}

impl Render for ExportDialogContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let video_codec = self.active_video_codec(cx);
		let supports_quality = codec_supports_quality_control(video_codec);
		let rate_control = if supports_quality {
			self.rate_control.read(cx).selected().unwrap_or(0)
		} else {
			0
		};
		let quality_row = if rate_control == 0 {
			form_row(
				&colors,
				i18n::tr("export.bitrate").into(),
				self.bitrate.clone(),
			)
		} else {
			let (min_q, max_q, _) = codec_quality_bounds(video_codec);
			let label = format!("{} ({min_q}–{max_q})", i18n::tr("export.quality"));
			form_row(&colors, label.into(), self.quality.clone())
		};
		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				&colors,
				i18n::tr("export.sequence").into(),
				self.sequence.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("export.format").into(),
				self.format.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("export.video_codec").into(),
				self.video_codec.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("export.audio_codec").into(),
				self.audio_codec.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("export.color").into(),
				self.color.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("export.range").into(),
				self.range.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("export.resolution").into(),
				div()
					.flex()
					.gap_2()
					.child(self.resolution_w.clone())
					.child(div().text_color(colors.disabled).text_xs().child("×"))
					.child(self.resolution_h.clone()),
			))
			.child(form_row(
				&colors,
				i18n::tr("export.framerate").into(),
				self.frame_rate.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("export.rate_control").into(),
				self.rate_control.clone(),
			))
			.child(quality_row)
			.child(form_row(
				&colors,
				i18n::tr("export.path").into(),
				self.path.clone(),
			))
			.child(
				div()
					.text_color(colors.disabled)
					.text_xs()
					.child(i18n::tr("export.hint")),
			)
	}
}

// ---------------------------------------------------------------------------
// Proxy settings (the C++ Tools > Proxy Settings dialog)
// ---------------------------------------------------------------------------

/// The ffmpeg encoder presets the proxy dialog offers (the C++
/// `ProxyDialog` preset combo, same order).
pub const PROXY_PRESETS: &[&str] = &[
	"ultrafast",
	"superfast",
	"veryfast",
	"faster",
	"fast",
	"medium",
	"slow",
	"slower",
	"veryslow",
];
