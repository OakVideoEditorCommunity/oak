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

//! The content views of the app's modal dialogs: preferences (the settings
//! panel mirroring the C++ tabbed preferences: general / rendering / cache
//! / proxy / project / audio) and export (format + output path).
//!
//! Each view owns its widgets and emits nothing itself — the host
//! (`crate::app::OakApp`) reads the state (format / path) when a dialog
//! button is clicked, and the preferences view writes its choices straight
//! into the oak_core config store on selection. Theme/language changes
//! additionally emit a [`PreferencesEvent`] so the host can re-apply the
//! shell chrome immediately.

use crate::oakui::component::OfxColorPicker;
use crate::oakui::component::controls::SliderModel;
use crate::oakui::component::controls::SliderValue;
use crate::oakui::component::controls::ValueKind;
use crate::oakui::component::controls::{CheckBox, CheckBoxEvent, CheckState};
use crate::oakui::component::controls::{ComboBox, ComboBoxEvent, ComboBoxOption};
use crate::oakui::component::controls::{SpinBox, SpinBoxEvent};
use crate::oakui::component::text_input;
use crate::widgets::timeline::FrameRate;
use gpui_kit::base::input::InputState;
use gpui_kit::colors::DefaultColors;
use gpui_kit::component::input::InputEvent;
use gpui_kit::prelude::*;
use gpui_kit::{
	App, ClickEvent, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
	InteractiveElement, Keystroke, PathPromptOptions, Render, SharedString, Window, div, px,
};

use crate::actions::ActionId;
use crate::i18n;
use crate::oakui::engine::proxy_params_from_config;
use crate::oakui::real::{
	CONFIG_KEY_DEFAULT_TRANSITION_SEC, CONFIG_KEY_DISK_CACHE_PATH, CONFIG_KEY_FFMPEG_PATH,
	CONFIG_KEY_PG_URL, CONFIG_KEY_PREVIEW_WINDOW, CONFIG_KEY_PROXY_DIVIDER,
	CONFIG_KEY_RENDERER_BACKEND, CONFIG_KEY_SNAPSHOT_INTERVAL_SEC, CONFIG_KEY_STORAGE_BACKEND,
	CONFIG_KEY_USE_PROXY, DEFAULT_PREVIEW_WINDOW_FORWARD, DEFAULT_SNAPSHOT_INTERVAL_SEC,
	DEFAULT_TRANSITION_SEC, EXPORT_FORMAT_MP4, audio_input_device, audio_input_devices,
	audio_output_device, audio_output_devices, config_get_bool, config_get_int, config_get_string,
	config_set_bool, config_set_int, config_set_string, encoding_formats, proxy_dividers,
	renderer_backends, set_audio_input_device, set_audio_output_device, set_theme_dark,
	theme_is_dark,
};
// The `DisplayBitDepth` config key lives with the format mapping it
// drives (oak-render's backend); the preferences dropdown and the
// window-layer consumer share the same key.
use quercus_engine_rs::core::CONFIG_KEY_DISPLAY_BIT_DEPTH;

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

/// The proxy settings dialog content: the global generation settings
/// plus the per-footage proxy list (the gpui port of the C++
/// `ProxyDialog`). All footage of the open project is listed — the gpui
/// shell opens the dialog from the Tools menu without a footage
/// selection, so the C++ "selected footage" group becomes "footage".
pub struct ProxyDialogContent<E: crate::oakui::engine::AppEngine> {
	engine: Entity<E>,
	divider: Entity<ComboBox>,
	width: Entity<SpinBox>,
	height: Entity<SpinBox>,
	crf: Entity<SpinBox>,
	preset: Entity<ComboBox>,
	include_audio: Entity<CheckBox>,
	max_concurrent: Entity<SpinBox>,
	ffmpeg_path: Entity<PathField>,
	custom_params: Entity<CheckBox>,
	/// Snapshot of the footage rows (refreshed after generate / delete).
	rows: Vec<crate::oakui::engine::ProxyFootageRow>,
	/// The divider values in dropdown order (1 = custom size).
	dividers: Vec<i32>,
}

impl<E: crate::oakui::engine::AppEngine> ProxyDialogContent<E> {
	/// Builds the proxy settings dialog seeded from the persisted config.
	///
	/// Reads the global generation defaults through `proxy_params_from_config`
	/// (width, height, divider, CRF, preset, include-audio) and seeds the matching
	/// widgets. A divider or preset that is not one of the offered options falls
	/// back to the first divider (1 = custom size) and to the `veryfast` preset.
	/// The max-concurrent spinner reads `ProxyMaxConcurrent`, clamped to `1..=16`
	/// (default `1`); the ffmpeg field reads `CONFIG_KEY_FFMPEG_PATH`; and the
	/// custom-params checkbox starts checked when any listed footage already has
	/// custom params. `engine` supplies the footage rows via `proxy_rows()` and is
	/// stored for the later generate / delete / accept actions; `window` is needed
	/// for the widgets and `cx` creates the entities.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let params = proxy_params_from_config();

		let dividers: Vec<i32> = vec![1, 2, 4, 8];
		let divider_options = dividers
			.iter()
			.enumerate()
			.map(|(i, d)| ComboBoxOption::new(i, divider_label(*d)))
			.collect();
		let divider = cx.new(|cx| ComboBox::new(21, divider_options, window, cx));
		let divider_selected = dividers
			.iter()
			.position(|d| *d == params.divider)
			.unwrap_or(0);
		divider.update(cx, |combo, cx| {
			combo.set_selected(Some(divider_selected), cx)
		});

		let width = cx.new(|cx| {
			SpinBox::new(
				22,
				SliderModel::new(
					ValueKind::Integer,
					160.0,
					4096.0,
					16.0,
					f64::from(params.width),
				),
				window,
				cx,
			)
		});
		let height = cx.new(|cx| {
			SpinBox::new(
				23,
				SliderModel::new(
					ValueKind::Integer,
					120.0,
					2160.0,
					8.0,
					f64::from(params.height),
				),
				window,
				cx,
			)
		});
		let crf = cx.new(|cx| {
			SpinBox::new(
				24,
				SliderModel::new(ValueKind::Integer, 0.0, 51.0, 1.0, f64::from(params.crf)),
				window,
				cx,
			)
		});

		let preset_options = PROXY_PRESETS
			.iter()
			.enumerate()
			.map(|(i, name)| ComboBoxOption::new(i, *name))
			.collect();
		let preset = cx.new(|cx| ComboBox::new(25, preset_options, window, cx));
		let preset_selected = PROXY_PRESETS
			.iter()
			.position(|name| *name == params.preset)
			.unwrap_or(2);
		preset.update(cx, |combo, cx| {
			combo.set_selected(Some(preset_selected), cx)
		});

		let include_audio = cx.new(|cx| {
			CheckBox::new(
				26,
				if params.include_audio {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("proxydialog.include_audio"))
		});

		let max_concurrent = cx.new(|cx| {
			SpinBox::new(
				28,
				SliderModel::new(
					ValueKind::Integer,
					1.0,
					16.0,
					1.0,
					config_get_int("ProxyMaxConcurrent", 1).clamp(1, 16) as f64,
				),
				window,
				cx,
			)
		});

		let ffmpeg_path = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			PathField {
				editor,
				enabled: true,
			}
		});
		ffmpeg_path.update(cx, |field, cx| {
			field.set_path(config_get_string(CONFIG_KEY_FFMPEG_PATH), window, cx)
		});

		let rows = engine.read(cx).proxy_rows();
		let any_custom = rows.iter().any(|row| row.has_custom);
		let custom_params = cx.new(|cx| {
			CheckBox::new(
				27,
				if any_custom {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("proxydialog.custom"))
		});

		Self {
			engine,
			divider,
			width,
			height,
			crf,
			preset,
			include_audio,
			max_concurrent,
			ffmpeg_path,
			custom_params,
			rows,
			dividers,
		}
	}

	/// The generation params currently edited in the dialog (the C++
	/// `current_params`).
	pub fn current_params(&self, cx: &App) -> crate::oakui::engine::ProxyParamsUi {
		let divider = self
			.divider
			.read(cx)
			.selected()
			.and_then(|i| self.dividers.get(i))
			.copied()
			.unwrap_or(1);
		let preset = self
			.preset
			.read(cx)
			.selected()
			.and_then(|i| PROXY_PRESETS.get(i))
			.unwrap_or(&"veryfast")
			.to_string();
		crate::oakui::engine::ProxyParamsUi {
			width: self.width.read(cx).value().to_f64() as i32,
			height: self.height.read(cx).value().to_f64() as i32,
			divider,
			crf: self.crf.read(cx).value().to_f64() as i32,
			preset,
			include_audio: self.include_audio.read(cx).state() == CheckState::Checked,
		}
	}

	/// Writes the global settings into the config store (the C++
	/// `save_global_settings`).
	pub fn save_global_settings(&self, cx: &App) {
		let params = self.current_params(cx);
		config_set_int("ProxyWidth", i64::from(params.width));
		config_set_int("ProxyHeight", i64::from(params.height));
		config_set_int("ProxyDivider", i64::from(params.divider));
		config_set_int("ProxyCRF", i64::from(params.crf));
		config_set_string("ProxyPreset", &params.preset);
		config_set_bool("ProxyIncludeAudio", params.include_audio);
		config_set_int(
			"ProxyMaxConcurrent",
			self.max_concurrent
				.read(cx)
				.value()
				.to_f64()
				.clamp(1.0, 16.0) as i64,
		);
		config_set_string(
			CONFIG_KEY_FFMPEG_PATH,
			self.ffmpeg_path.read(cx).path(cx).trim(),
		);
	}

	/// Generates proxies for every footage row with a video stream (the
	/// C++ Generate Proxies button): with the custom checkbox set, the
	/// edited params become each footage's custom params first.
	pub fn generate(&mut self, cx: &mut Context<Self>) {
		let custom = self.custom_params.read(cx).state() == CheckState::Checked;
		let params = self.current_params(cx);
		let ids: Vec<(u64, bool)> = self
			.rows
			.iter()
			.map(|row| (row.id, row.can_generate))
			.collect();
		for (id, can_generate) in ids {
			if !can_generate {
				continue;
			}
			if custom {
				self.engine.update(cx, |engine, cx| {
					engine.proxy_set_custom_params(id, params.clone(), cx)
				});
			}
			if let Err(err) = self
				.engine
				.update(cx, |engine, cx| engine.proxy_generate(id, cx))
			{
				println!("[proxy] generate failed for {id}: {err}");
			}
		}
		self.refresh(cx);
	}

	/// Deletes every footage row's proxy (the C++ Delete Proxies button).
	pub fn delete(&mut self, cx: &mut Context<Self>) {
		let ids: Vec<u64> = self.rows.iter().map(|row| row.id).collect();
		for id in ids {
			self.engine
				.update(cx, |engine, cx| engine.proxy_delete(id, cx));
		}
		self.refresh(cx);
	}

	/// Applies the dialog (the C++ `accept`): saves the global settings
	/// and sets or clears each footage's custom params per the checkbox.
	pub fn accept(&mut self, cx: &mut Context<Self>) {
		self.save_global_settings(cx);
		let custom = self.custom_params.read(cx).state() == CheckState::Checked;
		let params = self.current_params(cx);
		let ids: Vec<u64> = self.rows.iter().map(|row| row.id).collect();
		for id in ids {
			if custom {
				self.engine.update(cx, |engine, cx| {
					engine.proxy_set_custom_params(id, params.clone(), cx)
				});
			} else {
				self.engine
					.update(cx, |engine, cx| engine.proxy_clear_custom_params(id, cx));
			}
		}
		self.refresh(cx);
	}

	/// Re-reads the footage rows (after generate / delete / accept).
	pub fn refresh(&mut self, cx: &mut Context<Self>) {
		self.rows = self.engine.read(cx).proxy_rows();
		cx.notify();
	}
}

/// The dropdown label of a resolution divider (the C++ combo labels).
fn divider_label(divider: i32) -> String {
	match divider {
		1 => i18n::tr("proxydialog.resolution.custom").into(),
		2 => i18n::tr("proxydialog.resolution.half").into(),
		4 => i18n::tr("proxydialog.resolution.quarter").into(),
		8 => i18n::tr("proxydialog.resolution.eighth").into(),
		other => format!("1/{other}"),
	}
}

/// Returns the localized display label for a proxy lifecycle state.
///
/// Maps a [`crate::oakui::engine::ProxyMediaState`] to its `proxydialog.state.*`
/// translation: missing, generating, ready or failed. The match is exhaustive, so
/// every state has a label and the function cannot fail.
fn proxy_state_label(state: crate::oakui::engine::ProxyMediaState) -> String {
	use crate::oakui::engine::ProxyMediaState;
	match state {
		ProxyMediaState::Missing => i18n::tr("proxydialog.state.missing"),
		ProxyMediaState::Generating => i18n::tr("proxydialog.state.generating"),
		ProxyMediaState::Ready => i18n::tr("proxydialog.state.ready"),
		ProxyMediaState::Failed => i18n::tr("proxydialog.state.failed"),
	}
	.into()
}

impl<E: crate::oakui::engine::AppEngine> Render for ProxyDialogContent<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();

		let footage_list = div().flex().flex_col().gap_1().children(
			self.rows
				.iter()
				.map(|row| {
					let mut state = proxy_state_label(row.state);
					if row.has_custom {
						state.push_str(i18n::tr("proxydialog.custom_suffix"));
					}
					let enabled =
						row.enabled && row.state == crate::oakui::engine::ProxyMediaState::Ready;
					let dot = if enabled {
						colors.selected
					} else {
						colors.disabled
					};
					div()
						.flex()
						.items_center()
						.gap_2()
						.child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(dot))
						.child(
							div()
								.flex_1()
								.overflow_hidden()
								.text_ellipsis()
								.text_color(colors.text)
								.child(row.name.clone()),
						)
						.child(div().text_color(colors.disabled).text_xs().child(state))
				})
				.collect::<Vec<_>>(),
		);

		let footage_group = div()
			.flex()
			.flex_col()
			.gap_2()
			.child(section_header(
				&colors,
				i18n::tr("proxydialog.footage_group").into(),
			))
			.child(
				div()
					.id("proxy-footage-list")
					.max_h(px(160.0))
					.overflow_y_scroll()
					.child(if self.rows.is_empty() {
						div()
							.text_color(colors.disabled)
							.text_xs()
							.child(i18n::tr("proxydialog.no_footage"))
					} else {
						footage_list
					}),
			)
			.child(self.custom_params.clone());

		let settings_group = div()
			.flex()
			.flex_col()
			.gap_2()
			.child(section_header(
				&colors,
				i18n::tr("proxydialog.global").into(),
			))
			.child(form_row(
				&colors,
				i18n::tr("proxydialog.resolution").into(),
				self.divider.clone(),
			))
			.child(
				div()
					.flex()
					.gap_3()
					.child(form_row(
						&colors,
						i18n::tr("proxydialog.width").into(),
						self.width.clone(),
					))
					.child(form_row(
						&colors,
						i18n::tr("proxydialog.height").into(),
						self.height.clone(),
					)),
			)
			.child(form_row(
				&colors,
				i18n::tr("proxydialog.max_concurrent").into(),
				self.max_concurrent.clone(),
			))
			.child(
				div()
					.flex()
					.gap_3()
					.child(form_row(
						&colors,
						i18n::tr("proxydialog.crf").into(),
						self.crf.clone(),
					))
					.child(form_row(
						&colors,
						i18n::tr("proxydialog.preset").into(),
						self.preset.clone(),
					)),
			)
			.child(self.include_audio.clone())
			.child(form_row(
				&colors,
				i18n::tr("proxydialog.ffmpeg").into(),
				self.ffmpeg_path.clone(),
			));

		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(footage_group)
			.child(settings_group)
	}
}

// ---------------------------------------------------------------------------
// Project properties (the C++ File > Project Properties dialog)
// ---------------------------------------------------------------------------

/// The project-properties dialog content (the C++ `ProjectPropertiesDialog`):
/// the read-only project name, the per-project OCIO config override and the
/// disk-cache location. Apply happens through [`Self::commit`], which the
/// host runs on the OK button — an invalid OCIO config keeps the dialog open
/// with the error shown under the OCIO row.
pub struct ProjectPropertiesContent<E: crate::oakui::engine::AppEngine> {
	engine: Entity<E>,
	ocio_config: Entity<PathField>,
	cache_location: Entity<ComboBox>,
	custom_cache_path: Entity<PathField>,
	/// The cache location selected in the combo (0 = default location,
	/// 1 = alongside the project, 2 = custom path; see
	/// [`crate::oakui::engine::AppEngine::project_cache_location`]).
	cache_setting: i32,
	/// The pipeline working colorspace combo (ACEScg / sRGB legacy).
	working_space: Entity<ComboBox>,
	/// The delivery output gamut combo (sRGB / Display P3 / BT.2020).
	output_gamut: Entity<ComboBox>,
	/// The delivery output transfer combo (sRGB / gamma 2.2 / PQ / HLG).
	output_transfer: Entity<ComboBox>,
	/// The commit error shown under the OCIO row (an invalid config keeps
	/// the dialog open, like the C++ accept()).
	error: Option<String>,
}

impl<E: crate::oakui::engine::AppEngine> ProjectPropertiesContent<E> {
	/// Builds the project-properties dialog seeded from the engine's project.
	///
	/// Reads `project_ocio_config`, `project_cache_location` (which yields the
	/// setting `0`/`1`/`2` plus the stored custom path) and
	/// `project_color_settings` from `engine` and seeds the matching fields. The
	/// custom-cache path field starts enabled only when the cache setting is `2`.
	/// The color combos are seeded by mapping the engine's canonical strings back
	/// to indices; an unknown string falls back to the first entry (ACEScg / sRGB
	/// / sRGB). A subscription on the cache combo keeps [`Self::cache_setting`]
	/// and the custom field's enabled state in sync with the selection. `window`
	/// is needed for the input widgets and `cx` to create the entities; the error
	/// slot starts empty.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let ocio_config = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			PathField {
				editor,
				enabled: true,
			}
		});
		let configured_ocio = engine.read(cx).project_ocio_config();
		ocio_config.update(cx, |field, cx| field.set_path(configured_ocio, window, cx));

		let cache_options = vec![
			ComboBoxOption::new(0, i18n::tr("projprops.cache.default")),
			ComboBoxOption::new(1, i18n::tr("projprops.cache.alongside")),
			ComboBoxOption::new(2, i18n::tr("projprops.cache.custom")),
		];
		let cache_location = cx.new(|cx| ComboBox::new(30, cache_options, window, cx));
		let (cache_setting, custom_path) = engine.read(cx).project_cache_location();
		cache_location.update(cx, |combo, cx| {
			combo.set_selected(Some(cache_setting as usize), cx)
		});
		// The custom-path field follows the combo selection live.
		cx.subscribe(
			&cache_location,
			|this, _combo, event: &ComboBoxEvent, cx| {
				let ComboBoxEvent::Selected { value } = event;
				let setting = *value as i32;
				this.cache_setting = setting;
				this.custom_cache_path
					.update(cx, |field, cx| field.set_enabled(setting == 2, cx));
				cx.notify();
			},
		)
		.detach();

		let custom_cache_path = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			PathField {
				editor,
				enabled: cache_setting == 2,
			}
		});
		custom_cache_path.update(cx, |field, cx| field.set_path(custom_path, window, cx));

		// --- Color pipeline: working colorspace + delivery output --------
		let working_options = vec![
			ComboBoxOption::new(0, i18n::tr("projprops.color.working.acescg")),
			ComboBoxOption::new(1, i18n::tr("projprops.color.working.srgb")),
		];
		let working_space = cx.new(|cx| ComboBox::new(30, working_options, window, cx));
		let gamut_options = vec![
			ComboBoxOption::new(0, i18n::tr("projprops.color.gamut.srgb")),
			ComboBoxOption::new(1, i18n::tr("projprops.color.gamut.p3")),
			ComboBoxOption::new(2, i18n::tr("projprops.color.gamut.bt2020")),
		];
		let output_gamut = cx.new(|cx| ComboBox::new(30, gamut_options, window, cx));
		let transfer_options = vec![
			ComboBoxOption::new(0, i18n::tr("projprops.color.transfer.srgb")),
			ComboBoxOption::new(1, i18n::tr("projprops.color.transfer.gamma22")),
			ComboBoxOption::new(2, i18n::tr("projprops.color.transfer.pq")),
			ComboBoxOption::new(3, i18n::tr("projprops.color.transfer.hlg")),
		];
		let output_transfer = cx.new(|cx| ComboBox::new(30, transfer_options, window, cx));
		let (working, gamut, transfer) = engine.read(cx).project_color_settings();
		working_space.update(cx, |combo, cx| {
			combo.set_selected(
				Some(match working.as_str() {
					"srgb" | "srgb_legacy" | "legacy" => 1,
					_ => 0,
				}),
				cx,
			)
		});
		output_gamut.update(cx, |combo, cx| {
			combo.set_selected(
				Some(match gamut.as_str() {
					"displayp3" | "p3" => 1,
					"bt2020" | "rec2020" | "2020" => 2,
					_ => 0,
				}),
				cx,
			)
		});
		output_transfer.update(cx, |combo, cx| {
			combo.set_selected(
				Some(match transfer.as_str() {
					"gamma22" | "2.2" => 1,
					"pq" | "st2084" => 2,
					"hlg" => 3,
					_ => 0,
				}),
				cx,
			)
		});

		Self {
			engine,
			ocio_config,
			cache_location,
			custom_cache_path,
			cache_setting,
			working_space,
			output_gamut,
			output_transfer,
			error: None,
		}
	}

	/// Returns the OCIO config path currently shown in its field.
	///
	/// This is the per-project override the dialog edits; an empty string means
	/// "use the application-wide default". [`Self::commit`] forwards the value to
	/// the engine, which validates it.
	pub fn ocio_config_path(&self, cx: &App) -> SharedString {
		self.ocio_config.read(cx).path(cx)
	}

	/// Replaces the text of the OCIO config path field.
	///
	/// `path` is converted into a [`SharedString`] and written into the field;
	/// `window` is required by the text editor and `cx` is notified so the row
	/// repaints. The 浏览… picker handler and tests call this. It only edits the
	/// widget — nothing is applied until [`Self::commit`] runs.
	pub fn set_ocio_config_path(
		&mut self,
		path: impl Into<SharedString>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let path = path.into();
		self.ocio_config
			.update(cx, |field, cx| field.set_path(path, window, cx));
		cx.notify();
	}

	/// Returns the custom disk-cache path currently entered.
	///
	/// Only meaningful when the cache location setting is `2` (custom); for the
	/// other settings [`Self::commit`] ignores the text and stores an empty path.
	pub fn custom_cache_path(&self, cx: &App) -> SharedString {
		self.custom_cache_path.read(cx).path(cx)
	}

	/// Replaces the text of the custom disk-cache path field.
	///
	/// `path` is converted into a [`SharedString`] and written into the field;
	/// `window` is required by the editor and `cx` is notified so the row
	/// repaints. Whether the value is persisted depends on the cache location
	/// selection at [`Self::commit`] time.
	pub fn set_custom_cache_path(
		&mut self,
		path: impl Into<SharedString>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let path = path.into();
		self.custom_cache_path
			.update(cx, |field, cx| field.set_path(path, window, cx));
		cx.notify();
	}

	/// Selects a disk-cache location option and updates the custom-path field.
	///
	/// `setting` is clamped to `0..=2`, where `0` = the default location, `1` =
	/// alongside the project, and `2` = a custom path. The combo selection and
	/// [`Self::cache_setting`] follow the clamped value, and the custom path
	/// becomes editable only for `2`. `cx` is notified so the form repaints. This
	/// mirrors the combo's own selection handler and is exposed for tests; nothing
	/// is applied until [`Self::commit`] runs.
	pub fn select_cache_setting(&mut self, setting: i32, cx: &mut Context<Self>) {
		let setting = setting.clamp(0, 2);
		self.cache_setting = setting;
		self.cache_location.update(cx, |combo, cx| {
			combo.set_selected(Some(setting as usize), cx)
		});
		self.custom_cache_path
			.update(cx, |field, cx| field.set_enabled(setting == 2, cx));
		cx.notify();
	}

	/// Applies the edited settings (the C++ `accept()`): validates and
	/// applies the OCIO config override first — an invalid config keeps the
	/// dialog open — then the disk-cache location and the color pipeline
	/// settings. Ok clears the error row.
	pub fn commit(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
		let ocio = self.ocio_config_path(cx).to_string();
		self.engine
			.update(cx, |engine, cx| engine.set_project_ocio_config(ocio, cx))?;
		let custom = self.custom_cache_path(cx).to_string();
		let setting = self.cache_setting;
		let path = if setting == 2 { custom } else { String::new() };
		self.engine.update(cx, |engine, cx| {
			engine.set_project_cache_location(setting, path, cx)
		});
		// The color pipeline settings: combo index → canonical setting
		// string via the colormath enums (single source of truth).
		let (working, gamut, transfer) = self.color_settings(cx);
		self.engine.update(cx, |engine, cx| {
			engine.set_project_color_settings(working, gamut, transfer, cx)
		});
		self.set_error(None, cx);
		Ok(())
	}

	/// Reads the three color-pipeline combos as canonical persisted strings.
	///
	/// Returns `(working, gamut, transfer)`: working is `"acescg"` (index 0) or
	/// `"srgb_legacy"` (index 1); gamut is `"srgb"`, `"displayp3"` or
	/// `"bt2020"`; transfer is `"srgb"`, `"gamma22"`, `"pq"` or `"hlg"`. An
	/// index outside those ranges, or no selection at all, falls back to the
	/// first entry (`"acescg"` / `"srgb"` / `"srgb"`), so the result is always a
	/// string the engine accepts. [`Self::commit`] passes the triple to
	/// `set_project_color_settings`; `cx` is only used to read the combos.
	fn color_settings(&self, cx: &App) -> (String, String, String) {
		let working = self
			.working_space
			.read(cx)
			.selected()
			.map(|i| if i == 1 { "srgb_legacy" } else { "acescg" })
			.unwrap_or("acescg")
			.to_string();
		let gamut = self
			.output_gamut
			.read(cx)
			.selected()
			.map(|i| match i {
				1 => "displayp3",
				2 => "bt2020",
				_ => "srgb",
			})
			.unwrap_or("srgb")
			.to_string();
		let transfer = self
			.output_transfer
			.read(cx)
			.selected()
			.map(|i| match i {
				1 => "gamma22",
				2 => "pq",
				3 => "hlg",
				_ => "srgb",
			})
			.unwrap_or("srgb")
			.to_string();
		(working, gamut, transfer)
	}

	/// Sets or clears the error shown under the OCIO config row.
	///
	/// `msg` renders in red beneath that row while it is `Some`; `None` removes
	/// it, which is what a successful [`Self::commit`] does. `cx` is notified so
	/// the dialog repaints. The host passes the engine's validation message for an
	/// invalid config, keeping the dialog open.
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

impl<E: crate::oakui::engine::AppEngine> Render for ProjectPropertiesContent<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let project_name = self
			.engine
			.read(cx)
			.project()
			.map(|p| p.name.clone())
			.unwrap_or_default();

		// The 浏览… button picks an OCIO config through the platform file
		// dialog and fills the path field (the resolve is async, so the
		// picker's receiver is drained in a spawned task).
		let ocio_field = self.ocio_config.clone();
		let browse = div()
			.id("projprops-browse")
			.px_3()
			.py_1()
			.rounded_md()
			.bg(colors.background)
			.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
			.border_1()
			.border_color(colors.border)
			.text_color(colors.text)
			.cursor_pointer()
			.on_click(
				cx.listener(move |_this, _event: &gpui_kit::ClickEvent, _window, cx| {
					let receiver = cx.prompt_for_paths(PathPromptOptions {
						files: true,
						directories: false,
						multiple: false,
						prompt: None,
					});
					cx.spawn(async move |this, cx| {
						if let Ok(Ok(Some(paths))) = receiver.await {
							if let Some(path) = paths.first() {
								let path = path.to_string_lossy().into_owned();
								let _ = this.update_in(cx, |this, window, cx| {
									this.set_ocio_config_path(path, window, cx);
								});
							}
						}
					})
					.detach();
				}),
			)
			.child(i18n::tr("projprops.browse"));

		let ocio_row = div().flex().gap_2().child(ocio_field).child(browse);

		let custom_row = form_row(
			&colors,
			i18n::tr("projprops.cache.custom").into(),
			self.custom_cache_path.clone(),
		);

		div()
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.child(form_row(
				&colors,
				i18n::tr("projprops.name").into(),
				div().text_color(colors.text).child(project_name),
			))
			.child(
				form_row(&colors, i18n::tr("projprops.ocio_config").into(), ocio_row).child(
					if let Some(error) = &self.error {
						div()
							.debug_selector(|| "projprops-error".into())
							.text_color(gpui_kit::rgb(0xe5484d))
							.text_xs()
							.child(error.clone())
					} else {
						div()
					},
				),
			)
			.child(form_row(
				&colors,
				i18n::tr("projprops.cache.location").into(),
				self.cache_location.clone(),
			))
			.child(custom_row)
			.child(form_row(
				&colors,
				i18n::tr("projprops.color.working").into(),
				self.working_space.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("projprops.color.gamut").into(),
				self.output_gamut.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("projprops.color.transfer").into(),
				self.output_transfer.clone(),
			))
	}
}

// ---------------------------------------------------------------------------
// Preferences: the tabbed host (General + Keyboard)
// ---------------------------------------------------------------------------
mod about;
mod audio;
mod export;
mod keyboard;
mod multicam;
mod preferences;
mod preferences_dialog;
mod project;
mod search;
mod sequence;
mod subtitles;

pub(crate) use about::SEQUENCE_RATES;
pub use about::{AboutContent, UpdateDialogContent};
pub(crate) use about::{
	SEQUENCE_PRESET_COUNT, sequence_preset_format, sequence_preset_options, sequence_rate_options,
};
pub use audio::AudioPropertiesContent;
pub(crate) use audio::{color_from_rgba, rgba_from_array, subscribe_checkbox, text_field};
pub(crate) use export::PROXY_PRESETS;
pub use export::{ExportDialogContent, PathField};
#[cfg(test)]
pub(crate) use export::{compatible_audio_codecs, compatible_video_codecs};
#[cfg(test)]
pub(crate) use keyboard::keyboard_filter_matches;
#[cfg(test)]
pub(crate) use keyboard::{CaptureDecision, capture_decision};
pub use keyboard::{KeyboardEvent, KeyboardTabContent};
pub use multicam::MulticamWizardContent;
#[cfg(test)]
pub(crate) use preferences::backend_label;
pub use preferences::{PreferencesContent, PreferencesEvent};
pub(crate) use preferences::{form_row, section_header};
pub use preferences_dialog::PreferencesDialogContent;
#[cfg(test)]
pub(crate) use preferences_dialog::PreferencesTab;
pub(crate) use preferences_dialog::{pill_button, transparent};
pub use project::{
	ExportProjectDialogContent, NewProjectContent, PROJECT_FORMAT_FCPXML, PROJECT_FORMAT_OTIO,
	PROJECT_FORMAT_OVE, RenameContent,
};
pub use search::{ActionSearchContent, ActionSearchEvent};
#[cfg(test)]
pub(crate) use search::{search_filter_matches, selection_step};
pub use sequence::{
	DropSequenceChoice, DropSequenceChoiceContent, NewSequenceContent, SequenceFormatFields,
	SequenceFormatSeed, SequencePropertiesContent, TextValue,
};
pub use subtitles::SubtitleStyleContent;

#[cfg(test)]
mod tests;
