use super::*;

/// A request the preferences dialog emits for the host shell (the settings
/// themselves are written into the config store directly; these need
/// shell chrome — the menu bar / theme / key map — to re-render).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferencesEvent {
	/// The theme dropdown changed (the payload is the new dark flag).
	ThemeChanged(bool),
	/// The language dropdown changed (already applied to the i18n global).
	LanguageChanged,
	/// The custom shortcut overrides changed (the Keyboard tab); the host
	/// must re-bind the global key map and rebuild the menu bar so the new
	/// keys take effect immediately.
	ShortcutsChanged,
	/// The display color-management mode changed (the host re-evaluates the
	/// platform policy and retags the windows immediately — no restart).
	DisplayColorChanged,
}

impl gpui_kit::EventEmitter<PreferencesEvent> for PreferencesContent {}

/// The preferences dialog content, mirroring the C++ tabbed preferences as
/// one grouped panel:
///
/// * **常规 General** — language, theme.
/// * **渲染 Rendering** — the renderer backend.
/// * **缓存 Cache** — the disk cache directory (`DiskCachePath`) and the
///   playback pre-render window (`PlaybackPreRenderFrames`, the "cache
///   ahead" frames the preview scheduler fills during playback).
/// * **存储 Storage** — the write-through library backend
///   (`Storage/Backend`, SQLite / PostgreSQL) and the PostgreSQL
///   connection string (`Storage/PgUrl`).
/// * **代理 Proxy** — use proxy media (`UseProxyMedia`), proxy resolution
///   divider (`ProxyDivider`).
/// * **项目 Project** — the snapshot interval (`Storage/SnapshotIntervalSec`,
///   the write-through era's auto-save interval) and the default transition
///   length (`DefaultTransitionLength`).
/// * **音频 Audio** — the output / input devices (`AudioOutput` /
///   `AudioInput`, applied live through the oakaudio manager).
///
/// Every row writes into the config store on selection, so the choices
/// survive restarts (the app loads the config at startup and saves it on
/// exit).
pub struct PreferencesContent {
	pub(crate) backend: Entity<ComboBox>,
	/// The on-screen display bit depth (10-bit default, 8-bit fallback).
	/// The swapchain format is fixed at surface creation, so a change
	/// takes effect after a restart.
	pub(crate) display_bit_depth: Entity<ComboBox>,
	pub(crate) language: Entity<ComboBox>,
	pub(crate) theme: Entity<ComboBox>,
	/// 常规 General: check for a newer release on startup (config
	/// `CheckForUpdates`; the startup path reads the key live).
	pub(crate) check_updates: Entity<CheckBox>,
	pub(crate) cache_dir: Entity<PathField>,
	pub(crate) cache_ahead: Entity<SpinBox>,
	pub(crate) use_proxy: Entity<CheckBox>,
	pub(crate) hw_decode: Entity<CheckBox>,
	pub(crate) proxy_divider: Entity<ComboBox>,
	pub(crate) display_icc: Entity<CheckBox>,
	pub(crate) display_icc_path: Entity<PathField>,
	pub(crate) snapshot_interval: Entity<SpinBox>,
	pub(crate) transition_length: Entity<SpinBox>,
	pub(crate) audio_output: Entity<ComboBox>,
	pub(crate) audio_input: Entity<ComboBox>,
	pub(crate) storage_backend: Entity<ComboBox>,
	pub(crate) storage_pg_url: Entity<PathField>,
	/// Whether the write-through library uses PostgreSQL (drives the
	/// visibility of the connection-string field).
	pub(crate) storage_is_pg: bool,
	/// The backend options, in display order.
	pub(crate) backends: Vec<&'static str>,
	/// The proxy divider options, in display order (1 = full resolution).
	pub(crate) dividers: Vec<i64>,
	/// The output device names (dropdown order; index 0 is system default).
	pub(crate) output_devices: Vec<String>,
	/// The input device names (dropdown order; index 0 is system default).
	pub(crate) input_devices: Vec<String>,
}

impl PreferencesContent {
	/// Builds the general preferences form, seeding every widget from config.
	///
	/// Creates the rendering (backend, on-screen bit depth), general (language,
	/// theme, startup update check), cache (directory, pre-render window), proxy
	/// (enable, divider, hardware decoding), colour (display ICC enable and custom
	/// profile path), project (snapshot interval, default transition length),
	/// audio (output / input device) and storage (SQLite/PostgreSQL backend plus
	/// connection string) controls. Each control reads its current value through
	/// the config helpers and installs a subscription that writes changes back
	/// immediately — except the three free-text fields (cache directory, custom
	/// ICC path, PostgreSQL URL), whose text is committed on dialog close through
	/// `commit_cache_dir`, `commit_display_icc_path` and `commit_storage_pg_url`.
	/// Missing or unparsable values fall back to the defaults documented on the
	/// constants, and a stale device name falls back to "system default".
	///
	/// `window` is required by the input widgets; `cx` creates the entities and
	/// subscriptions.
	pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		// --- 渲染 Rendering: the renderer backend --------------------------
		let backends = renderer_backends();
		let current_backend = config_get_string(CONFIG_KEY_RENDERER_BACKEND);
		let backend_selected = backends
			.iter()
			.position(|b| b.eq_ignore_ascii_case(&current_backend))
			.unwrap_or(0);
		let backend_options = backends
			.iter()
			.enumerate()
			.map(|(i, name)| ComboBoxOption::new(i, backend_label(name)))
			.collect();
		let backend = cx.new(|cx| {
			ComboBox::new(1, backend_options, window, cx)
				.with_placeholder(i18n::tr("preferences.backend.placeholder"))
		});
		cx.subscribe(&backend, |this, _combo, event: &ComboBoxEvent, cx| {
			let ComboBoxEvent::Selected { value, .. } = event;
			if let Some(name) = this.backends.get(*value) {
				config_set_string(CONFIG_KEY_RENDERER_BACKEND, name);
				println!("[preferences] renderer backend → {name}");
			}
			let _ = cx;
		})
		.detach();
		backend.update(cx, |combo, cx| {
			combo.set_selected(Some(backend_selected), cx)
		});

		// --- 渲染 Rendering: on-screen display bit depth ---------------------
		// 10-bit is the default; 8-bit is the compatibility fallback. The
		// swapchain format is chosen once at window/surface creation, so a
		// change takes effect after a restart (see oak-render's
		// `DisplayBitDepth::present_formats`).
		let display_bit_depth = cx.new(|cx| {
			let options = vec![
				ComboBoxOption::new(0, i18n::tr("preferences.bit_depth.10bit")),
				ComboBoxOption::new(1, i18n::tr("preferences.bit_depth.8bit")),
			];
			ComboBox::new(12, options, window, cx)
		});
		cx.subscribe(
			&display_bit_depth,
			|_this, _combo, event: &ComboBoxEvent, cx| {
				let ComboBoxEvent::Selected { value, .. } = event;
				let depth = if *value == 1 { "8" } else { "10" };
				config_set_string(CONFIG_KEY_DISPLAY_BIT_DEPTH, depth);
				println!("[preferences] display bit depth → {depth}");
				let _ = cx;
			},
		)
		.detach();
		let bit_depth_selected = if config_get_string(CONFIG_KEY_DISPLAY_BIT_DEPTH) == "8" {
			1
		} else {
			0
		};
		display_bit_depth.update(cx, |combo, cx| {
			combo.set_selected(Some(bit_depth_selected), cx)
		});

		// --- 常规 General: language + theme --------------------------------
		// Data-driven: one option per discovered language pack, labelled
		// with the pack's own `language.name` — a newly dropped-in pack
		// appears here with no code change.
		let languages = i18n::available_languages();
		let language_options: Vec<_> = languages
			.iter()
			.enumerate()
			.map(|(index, code)| {
				ComboBoxOption::new(index, format!("{} ({code})", i18n::pack_native_name(code)))
			})
			.collect();
		let language = cx.new(|cx| {
			ComboBox::new(2, language_options, window, cx)
				.with_placeholder(i18n::tr("preferences.language.placeholder"))
		});
		let language_selected = languages
			.iter()
			.position(|code| *code == i18n::language_code())
			.unwrap_or(0);
		cx.subscribe(
			&language,
			move |_this, _combo, event: &ComboBoxEvent, cx| {
				let ComboBoxEvent::Selected { value, .. } = event;
				if let Some(code) = languages.get(*value) {
					crate::i18n::set_language_code(code);
					cx.emit(PreferencesEvent::LanguageChanged);
				}
			},
		)
		.detach();
		language.update(cx, |combo, cx| {
			combo.set_selected(Some(language_selected), cx)
		});

		let theme_options = vec![
			ComboBoxOption::new(0, i18n::tr("preferences.theme.dark")),
			ComboBoxOption::new(1, i18n::tr("preferences.theme.light")),
		];
		let theme = cx.new(|cx| ComboBox::new(3, theme_options, window, cx));
		cx.subscribe(&theme, |_this, _combo, event: &ComboBoxEvent, cx| {
			let ComboBoxEvent::Selected { value, .. } = event;
			let dark = *value == 0;
			set_theme_dark(dark);
			cx.emit(PreferencesEvent::ThemeChanged(dark));
		})
		.detach();
		theme.update(cx, |combo, cx| {
			combo.set_selected(Some(if theme_is_dark() { 0 } else { 1 }), cx)
		});

		// --- 常规 General: the startup update check -------------------------
		// On by default; off skips the startup request entirely (the config
		// key is read when the check would start, no restart needed).
		let check_updates = cx.new(|cx| {
			CheckBox::new(
				45,
				if crate::update::check_enabled() {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("preferences.check_updates"))
		});
		cx.subscribe(&check_updates, |_this, check, event: &CheckBoxEvent, cx| {
			let CheckBoxEvent::Toggled { state, .. } = event;
			crate::update::set_check_enabled(*state == CheckState::Checked);
			check.update(cx, |check, cx| check.set_state(*state, cx));
		})
		.detach();

		// --- 缓存 Cache: the disk cache directory --------------------------
		let cache_dir = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			PathField {
				editor,
				enabled: true,
			}
		});
		let configured_cache = config_get_string(CONFIG_KEY_DISK_CACHE_PATH);
		cache_dir.update(cx, |field, cx| field.set_path(configured_cache, window, cx));

		// --- 缓存 Cache: the playback pre-render window (cache ahead) ------
		// The number of frames the preview scheduler fills ahead of the
		// playhead during playback (`PlaybackPreRenderFrames`, consumed by
		// `update_preview_window` every playback tick — no restart needed).
		let cache_ahead = cx.new(|cx| {
			let current = config_get_int(CONFIG_KEY_PREVIEW_WINDOW, DEFAULT_PREVIEW_WINDOW_FORWARD)
				.clamp(8, 1200);
			SpinBox::new(
				10,
				SliderModel::new(ValueKind::Integer, 8.0, 1200.0, 8.0, current as f64),
				window,
				cx,
			)
		});
		cx.subscribe(&cache_ahead, |_this, _spin, event: &SpinBoxEvent, cx| {
			let value = match event {
				SpinBoxEvent::ValueChanged { value, .. }
				| SpinBoxEvent::EditCommitted { value, .. } => value.to_f64() as i64,
			};
			config_set_int(CONFIG_KEY_PREVIEW_WINDOW, value);
			let _ = cx;
		})
		.detach();

		// --- 代理 Proxy -----------------------------------------------------
		let use_proxy = cx.new(|cx| {
			CheckBox::new(
				7,
				if config_get_bool(CONFIG_KEY_USE_PROXY, true) {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("preferences.proxy.enable"))
		});
		cx.subscribe(&use_proxy, |_this, check, event: &CheckBoxEvent, cx| {
			let CheckBoxEvent::Toggled { state, .. } = event;
			let enabled = *state == CheckState::Checked;
			config_set_bool(CONFIG_KEY_USE_PROXY, enabled);
			check.update(cx, |check, cx| check.set_state(*state, cx));
		})
		.detach();

		let dividers = proxy_dividers();
		let divider_options = dividers
			.iter()
			.enumerate()
			.map(|(i, d)| {
				if *d <= 1 {
					ComboBoxOption::new(i, i18n::tr("preferences.proxy.full"))
				} else {
					ComboBoxOption::new(i, format!("1/{d}"))
				}
			})
			.collect();
		let proxy_divider = cx.new(|cx| ComboBox::new(4, divider_options, window, cx));
		cx.subscribe(&proxy_divider, |this, _combo, event: &ComboBoxEvent, cx| {
			let ComboBoxEvent::Selected { value, .. } = event;
			if let Some(divider) = this.dividers.get(*value) {
				config_set_int(CONFIG_KEY_PROXY_DIVIDER, *divider);
			}
			let _ = cx;
		})
		.detach();
		let current_divider = config_get_int(CONFIG_KEY_PROXY_DIVIDER, 1);
		let divider_selected = dividers
			.iter()
			.position(|d| *d == current_divider)
			.unwrap_or(0);
		proxy_divider.update(cx, |combo, cx| {
			combo.set_selected(Some(divider_selected), cx)
		});

		// --- 渲染 Rendering: hardware decoding switch -------------------
		// Default ON (user mandate); off forces software decoding.
		let hw_decode = cx.new(|cx| {
			CheckBox::new(
				9,
				if config_get_bool("HardwareDecoding", true) {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("preferences.hwdecode.enable"))
		});
		cx.subscribe(&hw_decode, |_this, check, event: &CheckBoxEvent, cx| {
			let CheckBoxEvent::Toggled { state, .. } = event;
			let enabled = *state == CheckState::Checked;
			config_set_bool("HardwareDecoding", enabled);
			check.update(cx, |check, cx| check.set_state(*state, cx));
		})
		.detach();

		// --- 色彩 Color: display ICC color management -----------------------
		// On by default: the viewer frames are transformed through the
		// display's ICC profile (system profile, or a custom file below).
		// A mode change re-evaluates the platform display policy and
		// retags the windows immediately (no restart).
		use crate::oakui::displaycolor::{CONFIG_KEY_COLOR_MODE, CONFIG_KEY_CUSTOM_ICC};
		let display_icc = cx.new(|cx| {
			let mode = config_get_string(CONFIG_KEY_COLOR_MODE);
			CheckBox::new(
				13,
				if mode != "off" {
					CheckState::Checked
				} else {
					CheckState::Unchecked
				},
				window,
				cx,
			)
			.with_label(i18n::tr("preferences.color.enable"))
		});
		cx.subscribe(&display_icc, |_this, check, event: &CheckBoxEvent, cx| {
			let CheckBoxEvent::Toggled { state, .. } = event;
			let enabled = *state == CheckState::Checked;
			config_set_string(CONFIG_KEY_COLOR_MODE, if enabled { "icc" } else { "off" });
			check.update(cx, |check, cx| check.set_state(*state, cx));
			// Drop the cached processors and tell the host to retag the
			// windows for the new policy.
			crate::oakui::displaycolor::invalidate();
			cx.emit(PreferencesEvent::DisplayColorChanged);
		})
		.detach();
		let display_icc_path = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			PathField {
				editor,
				enabled: true,
			}
		});
		let configured_icc = config_get_string(CONFIG_KEY_CUSTOM_ICC);
		display_icc_path.update(cx, |field, cx| field.set_path(configured_icc, window, cx));

		// --- 项目 Project: snapshot interval + default transition ----------
		let snapshot_interval = cx.new(|cx| {
			let current = config_get_int(
				CONFIG_KEY_SNAPSHOT_INTERVAL_SEC,
				DEFAULT_SNAPSHOT_INTERVAL_SEC,
			);
			SpinBox::new(
				8,
				SliderModel::new(ValueKind::Integer, 0.0, 86400.0, 10.0, current as f64),
				window,
				cx,
			)
		});
		cx.subscribe(
			&snapshot_interval,
			|_this, _spin, event: &SpinBoxEvent, cx| {
				let value = match event {
					SpinBoxEvent::ValueChanged { value, .. }
					| SpinBoxEvent::EditCommitted { value, .. } => value.to_f64() as i64,
				};
				config_set_int(CONFIG_KEY_SNAPSHOT_INTERVAL_SEC, value);
				let _ = cx;
			},
		)
		.detach();

		let transition_length = cx.new(|cx| {
			let current = config_get_string(CONFIG_KEY_DEFAULT_TRANSITION_SEC)
				.parse::<f64>()
				.ok()
				.filter(|v| *v >= 0.0)
				.unwrap_or_else(|| DEFAULT_TRANSITION_SEC.parse().unwrap());
			SpinBox::new(
				9,
				SliderModel::new(ValueKind::Float, 0.0, 60.0, 0.5, current),
				window,
				cx,
			)
		});
		cx.subscribe(
			&transition_length,
			|_this, _spin, event: &SpinBoxEvent, cx| {
				let value = match event {
					SpinBoxEvent::ValueChanged { value, .. }
					| SpinBoxEvent::EditCommitted { value, .. } => value.to_f64(),
				};
				config_set_string(CONFIG_KEY_DEFAULT_TRANSITION_SEC, &format!("{value}"));
				let _ = cx;
			},
		)
		.detach();

		// --- 音频 Audio: output / input devices -----------------------------
		// The enumeration reads the oakaudio manager even on the mock
		// engine; the config choice applies the moment the dropdown changes.
		let (audio_output, output_devices) = device_combo(5, true, window, cx);
		let (audio_input, input_devices) = device_combo(6, false, window, cx);
		cx.subscribe(&audio_output, |this, _combo, event: &ComboBoxEvent, cx| {
			let ComboBoxEvent::Selected { value, .. } = event;
			// Option 0 is the system default; the devices start at 1.
			let name = value
				.checked_sub(1)
				.and_then(|i| this.output_devices.get(i))
				.cloned()
				.unwrap_or_default();
			set_audio_output_device(&name);
			let _ = cx;
		})
		.detach();
		cx.subscribe(&audio_input, |this, _combo, event: &ComboBoxEvent, cx| {
			let ComboBoxEvent::Selected { value, .. } = event;
			let name = value
				.checked_sub(1)
				.and_then(|i| this.input_devices.get(i))
				.cloned()
				.unwrap_or_default();
			set_audio_input_device(&name);
			let _ = cx;
		})
		.detach();

		// --- 存储 Storage: the write-through library backend ----------------
		// `Storage/Backend` + `Storage/PgUrl` are read by oakstorage's
		// write-through when a project binds (per-project library session),
		// so a change applies to projects opened afterwards.
		let storage_is_pg = config_get_string(CONFIG_KEY_STORAGE_BACKEND) == "pg";
		let storage_backend = cx.new(|cx| {
			let options = vec![
				ComboBoxOption::new(0, "SQLite"),
				ComboBoxOption::new(1, "PostgreSQL"),
			];
			ComboBox::new(11, options, window, cx)
		});
		cx.subscribe(
			&storage_backend,
			|this, _combo, event: &ComboBoxEvent, cx| {
				match *event {
					ComboBoxEvent::Selected { value } => {
						let backend = if value == 1 { "pg" } else { "sqlite" };
						this.storage_is_pg = backend == "pg";
						config_set_string(CONFIG_KEY_STORAGE_BACKEND, backend);
						// Re-render so the connection-string row shows/hides with
						// the selection.
						cx.notify();
					}
				}
			},
		)
		.detach();
		storage_backend.update(cx, |combo, cx| {
			combo.set_selected(Some(if storage_is_pg { 1 } else { 0 }), cx)
		});
		let storage_pg_url = cx.new(|cx| {
			let editor = cx.new(|cx| InputState::new(window, cx));
			PathField {
				editor,
				enabled: true,
			}
		});
		let configured_pg_url = config_get_string(CONFIG_KEY_PG_URL);
		storage_pg_url.update(cx, |field, cx| {
			field.set_path(configured_pg_url, window, cx)
		});

		Self {
			backend,
			display_bit_depth,
			language,
			theme,
			check_updates,
			cache_dir,
			cache_ahead,
			use_proxy,
			hw_decode,
			proxy_divider,
			display_icc,
			display_icc_path,
			snapshot_interval,
			transition_length,
			audio_output,
			audio_input,
			storage_backend,
			storage_pg_url,
			storage_is_pg,
			backends,
			dividers,
			output_devices,
			input_devices,
		}
	}

	/// Commits the custom ICC path field to the config (called by the host
	/// when the dialog closes, like the cache directory).
	pub fn commit_display_icc_path(&self, cx: &App) {
		let path = self.display_icc_path.read(cx).path(cx).trim().to_string();
		config_set_string(crate::oakui::displaycolor::CONFIG_KEY_CUSTOM_ICC, &path);
	}

	/// Commits the PostgreSQL connection-string field to the config (called
	/// by the host when the dialog closes, like the cache directory).
	pub fn commit_storage_pg_url(&self, cx: &App) {
		let url = self.storage_pg_url.read(cx).path(cx).trim().to_string();
		config_set_string(CONFIG_KEY_PG_URL, &url);
	}

	/// Opens the platform file picker to choose a custom display ICC profile.
	///
	/// The picker accepts one existing file and is titled with the localized
	/// "browse" text. The chosen path is written into the custom-profile field;
	/// it reaches the config store only through `commit_display_icc_path` when the
	/// dialog closes. Cancelling or picking nothing leaves the field unchanged.
	/// The prompt resolves asynchronously, so the field update happens in a
	/// spawned task and `cx` is notified when it lands.
	pub(crate) fn browse_display_icc(&mut self, cx: &mut Context<Self>) {
		let receiver = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
			files: true,
			directories: false,
			multiple: false,
			prompt: Some(i18n::tr("preferences.color.browse").into()),
		});
		cx.spawn(async move |this, cx| {
			let Ok(Ok(Some(paths))) = receiver.await else {
				return;
			};
			let Some(path) = paths.first() else {
				return;
			};
			let _ = this.update_in(cx, |this, window, cx| {
				this.display_icc_path.update(cx, |field, cx| {
					field.set_path(path.to_string_lossy().into_owned(), window, cx)
				});
				cx.notify();
			});
		})
		.detach();
	}

	/// Returns the disk-cache directory currently shown in its field.
	///
	/// A [`SharedString`] snapshot of the path text. It is not yet persisted —
	/// [`Self::commit_cache_dir`] writes it to the config store when the dialog
	/// closes, so a typed-but-unbrowsed path still lands.
	pub fn cache_dir(&self, cx: &App) -> SharedString {
		self.cache_dir.read(cx).path(cx)
	}

	/// Commits the cache directory field to the config (called by the host
	/// when the dialog closes, so a typed-but-unbrowsed path still lands).
	pub fn commit_cache_dir(&self, cx: &App) {
		let path = self.cache_dir(cx).trim().to_string();
		config_set_string(CONFIG_KEY_DISK_CACHE_PATH, &path);
	}

	/// Opens the platform directory picker and lands the choice in the cache
	/// directory field (committed with the field, on dialog close).
	pub(crate) fn browse_cache_dir(&mut self, cx: &mut Context<Self>) {
		let receiver = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
			files: false,
			directories: true,
			multiple: false,
			prompt: Some(i18n::tr("preferences.cache.browse").into()),
		});
		cx.spawn(async move |this, cx| {
			let Ok(Ok(Some(paths))) = receiver.await else {
				return;
			};
			let Some(path) = paths.first() else {
				return;
			};
			let _ = this.update_in(cx, |this, window, cx| {
				this.cache_dir.update(cx, |field, cx| {
					field.set_path(path.to_string_lossy().into_owned(), window, cx)
				});
				cx.notify();
			});
		})
		.detach();
	}
}

/// Builds a device dropdown for the output (`output = true`) or input side:
/// option 0 is the system default, the rest are the enumerated devices, and
/// the current config value (validated against the enumeration) is
/// preselected. Returns the combo and the option→device-name list.
fn device_combo(
	control: usize,
	output: bool,
	window: &mut Window,
	cx: &mut Context<PreferencesContent>,
) -> (Entity<ComboBox>, Vec<String>) {
	let devices = if output {
		audio_output_devices()
	} else {
		audio_input_devices()
	};
	let current = if output {
		audio_output_device()
	} else {
		audio_input_device()
	};
	let mut options = vec![ComboBoxOption::new(
		0,
		i18n::tr("preferences.audio.default"),
	)];
	for (i, name) in devices.iter().enumerate() {
		options.push(ComboBoxOption::new(i + 1, name.clone()));
	}
	let selected = devices
		.iter()
		.position(|n| *n == current)
		.map(|i| i + 1)
		.unwrap_or(0);
	let placeholder = if output {
		i18n::tr("preferences.audio.output.placeholder")
	} else {
		i18n::tr("preferences.audio.input.placeholder")
	};
	let combo =
		cx.new(|cx| ComboBox::new(control, options, window, cx).with_placeholder(placeholder));
	combo.update(cx, |combo, cx| combo.set_selected(Some(selected), cx));
	(combo, devices)
}

/// Maps a renderer backend id from the config store to a display label.
///
/// Known ids become `"OpenGL"`, `"Metal"`, `"Vulkan"` or `"None (off)"`; any
/// other id is returned unchanged as an owned `String`, so an unrecognized or
/// plugin backend still appears in the dropdown instead of being dropped.
pub(crate) fn backend_label(name: &str) -> String {
	match name {
		"opengl" => "OpenGL",
		"metal" => "Metal",
		"vulkan" => "Vulkan",
		"none" => "None (off)",
		other => other,
	}
	.to_string()
}

/// Builds a labeled form row: a small caption above its widget.
///
/// `label` is the caption text and `widget` is any element; they are stacked in a
/// one-column flex layout, caption first, with `colors` supplying the caption
/// color. The dialogs in this module use it so rows share spacing and alignment.
/// The returned [`gpui_kit::Div`] is not attached to a parent.
pub(crate) fn form_row(
	colors: &gpui_kit::colors::Colors,
	label: SharedString,
	widget: impl IntoElement,
) -> gpui_kit::Div {
	div()
		.flex()
		.flex_col()
		.gap_1()
		.child(div().text_color(colors.text).child(label))
		.child(widget)
}

/// Builds a group header that separates the preference sections.
///
/// `label` is the section title, rendered in the theme's disabled text color at
/// small size with a little top padding; `colors` supplies that color. Purely
/// presentational: it holds no state and adds no element id.
pub(crate) fn section_header(
	colors: &gpui_kit::colors::Colors,
	label: SharedString,
) -> gpui_kit::Div {
	div()
		.pt_2()
		.text_color(colors.disabled)
		.text_xs()
		.child(label)
}

impl Render for PreferencesContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		// The full settings list is taller than a 900px window at the design
		// density, so the content scrolls inside the modal card.
		div()
			.id("preferences-content")
			.flex()
			.flex_col()
			.gap_3()
			.w_full()
			.max_h(px(720.0))
			.overflow_y_scroll()
			// 常规 General
			.child(section_header(
				&colors,
				i18n::tr("preferences.section.general").into(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.language").into(),
				self.language.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.theme").into(),
				self.theme.clone(),
			))
			.child(self.check_updates.clone())
			// 渲染 Rendering
			.child(section_header(
				&colors,
				i18n::tr("preferences.section.render").into(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.backend").into(),
				self.backend.clone(),
			))
			.child(self.hw_decode.clone())
			// 缓存 Cache
			.child(section_header(
				&colors,
				i18n::tr("preferences.section.cache").into(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.cache.dir").into(),
				div()
					.flex()
					.gap_2()
					.child(div().flex_1().child(self.cache_dir.clone()))
					.child(
						div()
							.id("preferences-cache-browse")
							.px_3()
							.py_1()
							.rounded_md()
							.bg(colors.background)
							.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
							.border_1()
							.border_color(colors.border)
							.text_color(colors.text)
							.cursor_pointer()
							.child(i18n::tr("preferences.cache.browse"))
							.on_click(cx.listener(|this, _event, _window, cx| {
								this.browse_cache_dir(cx);
							})),
					),
			))
			// 缓存 Cache: cache ahead (playback pre-render window)
			.child(form_row(
				&colors,
				i18n::tr("preferences.cache.ahead").into(),
				div()
					.debug_selector(|| "preferences-cache-ahead".into())
					.child(self.cache_ahead.clone()),
			))
			// 存储 Storage
			.child(section_header(
				&colors,
				i18n::tr("preferences.section.storage").into(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.storage.backend").into(),
				div()
					.debug_selector(|| "preferences-storage-backend".into())
					.child(self.storage_backend.clone()),
			))
			.child(if self.storage_is_pg {
				form_row(
					&colors,
					i18n::tr("preferences.storage.pg_url").into(),
					div()
						.debug_selector(|| "preferences-storage-pg-url".into())
						.child(self.storage_pg_url.clone()),
				)
			} else {
				div()
			})
			.child(
				div()
					.text_color(colors.disabled)
					.text_xs()
					.child(i18n::tr("preferences.storage.restart_hint")),
			)
			// 代理 Proxy
			.child(section_header(
				&colors,
				i18n::tr("preferences.section.proxy").into(),
			))
			.child(self.use_proxy.clone())
			.child(form_row(
				&colors,
				i18n::tr("preferences.proxy.resolution").into(),
				self.proxy_divider.clone(),
			))
			// 色彩 Color
			.child(section_header(
				&colors,
				i18n::tr("preferences.section.color").into(),
			))
			.child(self.display_icc.clone())
			.child(form_row(
				&colors,
				i18n::tr("preferences.color.custom").into(),
				div()
					.flex()
					.gap_2()
					.child(div().flex_1().child(self.display_icc_path.clone()))
					.child(
						div()
							.id("preferences-icc-browse")
							.px_3()
							.py_1()
							.rounded_md()
							.bg(colors.background)
							.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
							.border_1()
							.border_color(colors.border)
							.text_color(colors.text)
							.cursor_pointer()
							.child(i18n::tr("preferences.color.browse"))
							.on_click(cx.listener(|this, _event, _window, cx| {
								this.browse_display_icc(cx);
							})),
					),
			))
			.child(
				div()
					.text_color(colors.disabled)
					.text_xs()
					.child(i18n::tr("preferences.color.restart_hint")),
			)
			// 项目 Project
			.child(section_header(
				&colors,
				i18n::tr("preferences.section.project").into(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.snapshot.interval").into(),
				self.snapshot_interval.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.transition.default").into(),
				self.transition_length.clone(),
			))
			// 音频 Audio
			.child(section_header(
				&colors,
				i18n::tr("preferences.section.audio").into(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.audio.output").into(),
				self.audio_output.clone(),
			))
			.child(form_row(
				&colors,
				i18n::tr("preferences.audio.input").into(),
				self.audio_input.clone(),
			))
			// 渲染 Rendering: display bit depth (kept at the bottom of the list
			// so the rows above stay inside the card's scroll viewport).
			.child(form_row(
				&colors,
				i18n::tr("preferences.bit_depth.label").into(),
				self.display_bit_depth.clone(),
			))
			.child(
				div()
					.text_color(colors.disabled)
					.text_xs()
					.child(i18n::tr("preferences.bit_depth.restart_hint")),
			)
			.child(
				div()
					.text_color(colors.disabled)
					.text_xs()
					.child(i18n::tr("preferences.hint")),
			)
	}
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------
