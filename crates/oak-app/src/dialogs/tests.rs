use super::*;
use crate::oakui::MockEngine;
use crate::oakui::engine::AppEngine;

/// A minimal host view for tests that need a window (gpui-kit's input
/// states are built against one).
struct TestHost<V: gpui_kit::Render> {
	content: gpui_kit::Entity<V>,
}

impl<V: gpui_kit::Render> gpui_kit::Render for TestHost<V> {
	fn render(
		&mut self,
		_window: &mut gpui_kit::Window,
		_cx: &mut gpui_kit::Context<Self>,
	) -> impl gpui_kit::IntoElement {
		div().size_full().child(self.content.clone())
	}
}

/// Opens a window hosting the content built by `build`, returning the
/// window handle and the content entity.
fn windowed_content<V: gpui_kit::Render>(
	cx: &mut gpui_kit::TestAppContext,
	build: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::Context<V>) -> V,
) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<V>) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let mut captured = None;
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(800.0), gpui_kit::px(600.0)),
		|window, cx| {
			let content = cx.new(|cx| build(window, cx));
			captured = Some(content.clone());
			TestHost { content }
		},
	);
	cx.run_until_parked();
	(window.into(), captured.expect("the content was built"))
}

/// Runs `f` against the content inside its window (the windowed setters
/// need `&mut Window`).
fn update_content<V: gpui_kit::Render>(
	cx: &mut gpui_kit::TestAppContext,
	window: gpui_kit::AnyWindowHandle,
	content: &gpui_kit::Entity<V>,
	f: impl FnOnce(&mut V, &mut gpui_kit::Window, &mut gpui_kit::Context<V>),
) {
	cx.update_window(window, |_, window, cx| {
		content.update(cx, |content, cx| f(content, window, cx));
	})
	.expect("window open");
}

fn keystroke(key: &str) -> Keystroke {
	gpui_kit::Keystroke::parse(key).unwrap()
}

/// Restores a config key when dropped: the app's config store is
/// process-wide, so tests that write preferences must restore them.
struct ConfigRestore(&'static str, String);

impl ConfigRestore {
	fn of(key: &'static str) -> Self {
		ConfigRestore(key, config_get_string(key))
	}
}

impl Drop for ConfigRestore {
	fn drop(&mut self) {
		config_set_string(self.0, &self.1);
	}
}

/// Restores `QUERCUS_CONFIG_DIR` when dropped (the shortcut-capture commits
/// write `<config>/shortcuts`, so those tests point it at a temp dir).
struct ConfigDirRestore(Option<std::ffi::OsString>);

impl Drop for ConfigDirRestore {
	fn drop(&mut self) {
		match &self.0 {
			Some(value) => unsafe { std::env::set_var("QUERCUS_CONFIG_DIR", value) },
			None => unsafe { std::env::remove_var("QUERCUS_CONFIG_DIR") },
		}
	}
}

/// A path field on its own (the dialogs' shared path widget): enabling
/// is idempotent and the path round-trips.
#[gpui_kit::test]
async fn path_field_enable_toggles_and_with_enabled(cx: &mut gpui_kit::TestAppContext) {
	let (window, field) = windowed_content(cx, |window, cx| {
		let editor = cx.new(|cx| InputState::new(window, cx));
		PathField {
			editor,
			enabled: true,
		}
	});
	update_content(cx, window, &field, |field, window, cx| {
		// Re-enabling an already-enabled field is the no-op path.
		field.set_enabled(true, cx);
		assert!(field.enabled);
		field.set_path("  /tmp/oak/profile.icc  ", window, cx);
		assert_eq!(field.path(cx).as_ref(), "  /tmp/oak/profile.icc  ");
		field.set_enabled(false, cx);
		assert!(!field.enabled);
	});

	let (_window2, disabled) = windowed_content(cx, |window, cx| {
		let editor = cx.new(|cx| InputState::new(window, cx));
		PathField {
			editor,
			enabled: true,
		}
		.with_enabled(false)
	});
	cx.update(|cx| assert!(!disabled.read(cx).enabled));
}

/// The label helpers and codec-compatibility fallbacks: every arm of the
/// display tables, including the catch-all arms the dropdowns cannot
/// reach.
#[test]
fn helper_fallbacks_and_labels() {
	// Renderer backend display names (the pass-through arm covers an
	// unknown backend id).
	assert_eq!(backend_label("opengl"), "OpenGL");
	assert_eq!(backend_label("metal"), "Metal");
	assert_eq!(backend_label("vulkan"), "Vulkan");
	assert_eq!(backend_label("none"), "None (off)");
	assert_eq!(backend_label("software"), "software");

	// Proxy resolution divider labels (the generic arm formats 1/N).
	assert_eq!(divider_label(1), i18n::tr("proxydialog.resolution.custom"));
	assert_eq!(divider_label(2), i18n::tr("proxydialog.resolution.half"));
	assert_eq!(divider_label(4), i18n::tr("proxydialog.resolution.quarter"));
	assert_eq!(divider_label(8), i18n::tr("proxydialog.resolution.eighth"));
	assert_eq!(divider_label(16), "1/16");

	// Proxy lifecycle labels for every state.
	use crate::oakui::engine::ProxyMediaState;
	assert_eq!(
		proxy_state_label(ProxyMediaState::Missing),
		i18n::tr("proxydialog.state.missing")
	);
	assert_eq!(
		proxy_state_label(ProxyMediaState::Generating),
		i18n::tr("proxydialog.state.generating")
	);
	assert_eq!(
		proxy_state_label(ProxyMediaState::Ready),
		i18n::tr("proxydialog.state.ready")
	);
	assert_eq!(
		proxy_state_label(ProxyMediaState::Failed),
		i18n::tr("proxydialog.state.failed")
	);

	// Codec compatibility tables: an unknown container falls back to
	// H.264 / AAC, a known one lists at least one codec.
	assert_eq!(compatible_video_codecs(9999), vec![1]);
	assert_eq!(compatible_audio_codecs(9999), vec![12]);
	assert!(!compatible_video_codecs(EXPORT_FORMAT_MP4).is_empty());
	assert!(!compatible_audio_codecs(EXPORT_FORMAT_MP4).is_empty());
}

/// The export dialog's container picker drives `format()` /
/// `extension()` (with MP4 fallbacks for a stale selection) and the
/// settings the engine consumes reflect the codec / color / range /
/// size controls.
#[gpui_kit::test]
async fn export_dialog_format_extension_and_settings(cx: &mut gpui_kit::TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(440.0), gpui_kit::px(400.0)),
		ExportDialogContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("dialog content root");

	// The default resolves to the MP4 entry and its extension.
	assert_eq!(cx.read(|cx| content.read(cx).format(cx)), EXPORT_FORMAT_MP4);
	let formats = cx.read(|cx| content.read(cx).formats.clone());
	let mp4_ext = formats
		.iter()
		.find(|(id, _, _)| *id == EXPORT_FORMAT_MP4)
		.map(|(_, _, ext)| ext.clone())
		.unwrap_or_else(|| "mp4".to_string());
	assert_eq!(cx.read(|cx| content.read(cx).extension(cx)), mp4_ext);

	// Every listed container resolves to its id and extension.
	for (index, (id, _, ext)) in formats.iter().enumerate() {
		cx.update(|cx| {
			content.update(cx, |content, cx| {
				content
					.format
					.update(cx, |combo, cx| combo.set_selected(Some(index), cx))
			})
		});
		assert_eq!(cx.read(|cx| content.read(cx).format(cx)), *id);
		assert_eq!(cx.read(|cx| content.read(cx).extension(cx)), *ext);
	}

	// No selection and a stale selection both fall back to MP4.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.format
				.update(cx, |combo, cx| combo.set_selected(None, cx))
		})
	});
	assert_eq!(cx.read(|cx| content.read(cx).format(cx)), EXPORT_FORMAT_MP4);
	assert_eq!(cx.read(|cx| content.read(cx).extension(cx)), "mp4");
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.format
				.update(cx, |combo, cx| combo.set_selected(Some(99), cx))
		})
	});
	assert_eq!(cx.read(|cx| content.read(cx).format(cx)), EXPORT_FORMAT_MP4);
	assert_eq!(cx.read(|cx| content.read(cx).extension(cx)), "mp4");

	// Rebuilding for another container re-selects its first codecs and
	// makes subsequent settings resolve against that container.
	let other = formats.last().expect("at least one container").0;
	cx.update(|cx| content.update(cx, |content, cx| content.apply_format(other, cx)));
	assert_eq!(cx.read(|cx| content.read(cx).active_format), other);
	assert_eq!(
		cx.read(|cx| content.read(cx).video_codec.read(cx).selected()),
		Some(0)
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).audio_codec.read(cx).selected()),
		Some(0)
	);

	// The size / rate / bitrate boxes are honoured.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.resolution_w.update(cx, |spin, cx| {
				spin.set_value(SliderValue::Integer(1920), cx)
			});
			content.resolution_h.update(cx, |spin, cx| {
				spin.set_value(SliderValue::Integer(1080), cx)
			});
			content
				.frame_rate
				.update(cx, |spin, cx| spin.set_value(SliderValue::Float(24.0), cx));
			content.bitrate.update(cx, |spin, cx| {
				spin.set_value(SliderValue::Integer(8_000_000), cx)
			});
		});
	});
	let settings = cx.read(|cx| content.read(cx).settings(cx));
	assert_eq!(settings.format, other);
	assert_eq!(settings.size, (1920, 1080));
	assert!((settings.frame_rate - 24.0).abs() < 1e-9);
	assert_eq!(settings.video_bitrate, 8_000_000);
	assert_eq!(settings.bit_depth, 8);
	assert_eq!(settings.range, None);
	assert_eq!(
		settings.video_codec,
		compatible_video_codecs(other).first().copied().unwrap_or(1)
	);
	assert_eq!(
		settings.audio_codec,
		compatible_audio_codecs(other)
			.first()
			.copied()
			.unwrap_or(12)
	);

	// HDR + in/out range flip the color and range fields.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.color
				.update(cx, |combo, cx| combo.set_selected(Some(1), cx));
			content
				.range
				.update(cx, |combo, cx| combo.set_selected(Some(1), cx));
		});
	});
	let settings = cx.read(|cx| content.read(cx).settings(cx));
	assert_eq!(settings.bit_depth, 10);
	assert_eq!(settings.color_primaries, 9);
	assert_eq!(settings.color_transfer, 16);
	assert_eq!(settings.color_space, 9);
	assert_eq!(settings.range, Some((0.0, 0.0)));

	// A zero width/height means "keep the sequence size"; an unselected
	// codec falls back to the first compatible entry.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.resolution_w
				.update(cx, |spin, cx| spin.set_value(SliderValue::Integer(0), cx));
			content
				.video_codec
				.update(cx, |combo, cx| combo.set_selected(None, cx));
			content
				.audio_codec
				.update(cx, |combo, cx| combo.set_selected(None, cx));
		});
	});
	let settings = cx.read(|cx| content.read(cx).settings(cx));
	assert_eq!(settings.size, (0, 0));
	assert_eq!(
		settings.video_codec,
		compatible_video_codecs(other).first().copied().unwrap_or(1)
	);
	assert_eq!(
		settings.audio_codec,
		compatible_audio_codecs(other)
			.first()
			.copied()
			.unwrap_or(12)
	);

	// A stale codec selection falls back to the H.264 / AAC defaults.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.video_codec
				.update(cx, |combo, cx| combo.set_selected(Some(99), cx));
			content
				.audio_codec
				.update(cx, |combo, cx| combo.set_selected(Some(99), cx));
		});
	});
	let settings = cx.read(|cx| content.read(cx).settings(cx));
	assert_eq!(settings.video_codec, 1);
	assert_eq!(settings.audio_codec, 12);

	// The full form renders (all rows + hint text).
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// Every general-tab row writes its choice into the config store: the
/// backend / bit-depth / theme / proxy / decode / color / storage combos
/// and checkboxes, and the numeric spin boxes (value changed and
/// committed edits).
#[gpui_kit::test]
async fn preferences_general_rows_write_the_config(cx: &mut gpui_kit::TestAppContext) {
	// Lock order: language → config (the other app test modules nest
	// their process-wide locks in this order too, so parallel tests
	// cannot deadlock).
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::oakui::graphops::test_lock();
	let previous_language = crate::i18n::language_code();

	let _backend = ConfigRestore::of(CONFIG_KEY_RENDERER_BACKEND);
	let _bit_depth = ConfigRestore::of(CONFIG_KEY_DISPLAY_BIT_DEPTH);
	let _theme = ConfigRestore::of(crate::oakui::real::CONFIG_KEY_THEME);
	let _language = ConfigRestore::of("Language");
	let _proxy = ConfigRestore::of(CONFIG_KEY_USE_PROXY);
	let _hw = ConfigRestore::of("HardwareDecoding");
	let _divider = ConfigRestore::of(CONFIG_KEY_PROXY_DIVIDER);
	let _color = ConfigRestore::of(crate::oakui::displaycolor::CONFIG_KEY_COLOR_MODE);
	let _audio_out = ConfigRestore::of(crate::oakui::real::CONFIG_KEY_AUDIO_OUTPUT);
	let _audio_in = ConfigRestore::of(crate::oakui::real::CONFIG_KEY_AUDIO_INPUT);
	let _ahead = ConfigRestore::of(CONFIG_KEY_PREVIEW_WINDOW);
	let _snapshot = ConfigRestore::of(CONFIG_KEY_SNAPSHOT_INTERVAL_SEC);
	let _transition = ConfigRestore::of(CONFIG_KEY_DEFAULT_TRANSITION_SEC);
	let _storage = ConfigRestore::of(CONFIG_KEY_STORAGE_BACKEND);
	let _updates = ConfigRestore::of(crate::update::CONFIG_KEY_CHECK_UPDATES);

	// Pin known starting values so the seeded rows are deterministic.
	config_set_string(crate::update::CONFIG_KEY_CHECK_UPDATES, "true");
	config_set_string(CONFIG_KEY_RENDERER_BACKEND, "opengl");
	config_set_string(CONFIG_KEY_DISPLAY_BIT_DEPTH, "10");
	config_set_string(crate::oakui::real::CONFIG_KEY_THEME, "dark");
	config_set_string(CONFIG_KEY_USE_PROXY, "true");
	config_set_string("HardwareDecoding", "true");
	config_set_int(CONFIG_KEY_PROXY_DIVIDER, 1);
	config_set_string(crate::oakui::displaycolor::CONFIG_KEY_COLOR_MODE, "icc");
	config_set_int(CONFIG_KEY_PREVIEW_WINDOW, 120);
	config_set_string(CONFIG_KEY_STORAGE_BACKEND, "sqlite");
	crate::i18n::set_language_code("en-US");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(800.0), gpui_kit::px(900.0)),
		PreferencesContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("preferences content");
	// The initial render (SQLite backend hides the connection row).
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// Renderer backend: the selected entry's name is persisted.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.backend.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 2 })
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_RENDERER_BACKEND), "vulkan");
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.backend.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 3 })
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_RENDERER_BACKEND), "none");
	// A value outside the backend list is ignored.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.backend.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 42 })
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_RENDERER_BACKEND), "none");

	// Display bit depth: value 1 means the 8-bit compatibility mode.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.display_bit_depth.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 1 })
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_DISPLAY_BIT_DEPTH), "8");
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.display_bit_depth.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 0 })
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_DISPLAY_BIT_DEPTH), "10");

	// Language: a known index switches and persists; an unknown index
	// is ignored.
	let languages = crate::i18n::available_languages();
	assert!(!languages.is_empty());
	config_set_string("Language", "");
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.language.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 0 })
			})
		})
	});
	assert_eq!(crate::i18n::language_code(), languages[0]);
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.language.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected {
					value: languages.len(),
				})
			})
		})
	});
	assert_eq!(crate::i18n::language_code(), languages[0]);

	// Theme: index 0 is dark, index 1 light.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.theme.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 1 })
			})
		})
	});
	assert!(!theme_is_dark());
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.theme.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 0 })
			})
		})
	});
	assert!(theme_is_dark());

	// Cache ahead: both the value-changed and the committed-edit events
	// land in the config the playback window reads.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.cache_ahead.update(cx, |_spin, cx| {
				cx.emit(SpinBoxEvent::ValueChanged {
					control: 10,
					value: SliderValue::Integer(300),
				})
			})
		})
	});
	assert_eq!(config_get_int(CONFIG_KEY_PREVIEW_WINDOW, 0), 300);
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.cache_ahead.update(cx, |_spin, cx| {
				cx.emit(SpinBoxEvent::EditCommitted {
					control: 10,
					value: SliderValue::Integer(400),
				})
			})
		})
	});
	assert_eq!(config_get_int(CONFIG_KEY_PREVIEW_WINDOW, 0), 400);

	// Use-proxy and hardware decoding are request-only checkboxes: the
	// row's subscription applies the toggled state back.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.use_proxy.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 7,
					state: CheckState::Unchecked,
				})
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_USE_PROXY), "false");
	assert_eq!(
		cx.read(|cx| content.read(cx).use_proxy.read(cx).state()),
		CheckState::Unchecked
	);
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.use_proxy.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 7,
					state: CheckState::Checked,
				})
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_USE_PROXY), "true");

	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.hw_decode.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 9,
					state: CheckState::Unchecked,
				})
			})
		})
	});
	assert_eq!(config_get_string("HardwareDecoding"), "false");

	// The startup update check: the toggle writes the config key the
	// startup path reads.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.check_updates.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 45,
					state: CheckState::Unchecked,
				})
			})
		})
	});
	assert_eq!(
		config_get_string(crate::update::CONFIG_KEY_CHECK_UPDATES),
		"false"
	);
	assert!(!crate::update::check_enabled());
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.check_updates.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 45,
					state: CheckState::Checked,
				})
			})
		})
	});
	assert!(crate::update::check_enabled());

	// Proxy divider: the selected option's divider value is persisted.
	let dividers = cx.read(|cx| content.read(cx).dividers.clone());
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.proxy_divider.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 3 })
			})
		})
	});
	assert_eq!(config_get_int(CONFIG_KEY_PROXY_DIVIDER, 0), dividers[3]);

	// Display color management: off writes "off", on writes "icc".
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.display_icc.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 13,
					state: CheckState::Unchecked,
				})
			})
		})
	});
	assert_eq!(
		config_get_string(crate::oakui::displaycolor::CONFIG_KEY_COLOR_MODE),
		"off"
	);
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.display_icc.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 13,
					state: CheckState::Checked,
				})
			})
		})
	});
	assert_eq!(
		config_get_string(crate::oakui::displaycolor::CONFIG_KEY_COLOR_MODE),
		"icc"
	);

	// Snapshot interval and default transition length.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.snapshot_interval.update(cx, |_spin, cx| {
				cx.emit(SpinBoxEvent::ValueChanged {
					control: 8,
					value: SliderValue::Integer(300),
				})
			})
		})
	});
	assert_eq!(config_get_int(CONFIG_KEY_SNAPSHOT_INTERVAL_SEC, 0), 300);
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.transition_length.update(cx, |_spin, cx| {
				cx.emit(SpinBoxEvent::ValueChanged {
					control: 9,
					value: SliderValue::Float(1.25),
				})
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_DEFAULT_TRANSITION_SEC), "1.25");

	// Audio device combos: option 0 is the system default (the empty
	// name), the rest map back onto the enumerated device list.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.audio_output.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 0 })
			})
		})
	});
	assert_eq!(
		config_get_string(crate::oakui::real::CONFIG_KEY_AUDIO_OUTPUT),
		""
	);
	let first_output = cx.read(|cx| content.read(cx).output_devices.first().cloned());
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.audio_output.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 1 })
			})
		})
	});
	assert_eq!(
		config_get_string(crate::oakui::real::CONFIG_KEY_AUDIO_OUTPUT),
		first_output.unwrap_or_default()
	);
	let first_input = cx.read(|cx| content.read(cx).input_devices.first().cloned());
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.audio_input.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 0 })
			})
		})
	});
	assert_eq!(
		config_get_string(crate::oakui::real::CONFIG_KEY_AUDIO_INPUT),
		""
	);
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.audio_input.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 1 })
			})
		})
	});
	assert_eq!(
		config_get_string(crate::oakui::real::CONFIG_KEY_AUDIO_INPUT),
		first_input.unwrap_or_default()
	);

	// Storage backend: PostgreSQL selects "pg" and reveals the URL row.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.storage_backend.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 1 })
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_STORAGE_BACKEND), "pg");
	assert!(cx.read(|cx| content.read(cx).storage_is_pg));
	// The PostgreSQL selection reveals the connection-string row.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.storage_backend.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 0 })
			})
		})
	});
	assert_eq!(config_get_string(CONFIG_KEY_STORAGE_BACKEND), "sqlite");
	assert!(!cx.read(|cx| content.read(cx).storage_is_pg));

	crate::i18n::set_language_code(&previous_language);
}

/// Seeding the preferences from non-default config values: the
/// alternate checkbox / combo states, the fallbacks for unknown values
/// and the numeric clamps.
#[gpui_kit::test]
async fn preferences_seed_from_non_default_config(cx: &mut gpui_kit::TestAppContext) {
	let _guard = crate::oakui::graphops::test_lock();
	let _backend = ConfigRestore::of(CONFIG_KEY_RENDERER_BACKEND);
	let _bit_depth = ConfigRestore::of(CONFIG_KEY_DISPLAY_BIT_DEPTH);
	let _proxy = ConfigRestore::of(CONFIG_KEY_USE_PROXY);
	let _hw = ConfigRestore::of("HardwareDecoding");
	let _divider = ConfigRestore::of(CONFIG_KEY_PROXY_DIVIDER);
	let _color = ConfigRestore::of(crate::oakui::displaycolor::CONFIG_KEY_COLOR_MODE);
	let _storage = ConfigRestore::of(CONFIG_KEY_STORAGE_BACKEND);
	let _cache = ConfigRestore::of(CONFIG_KEY_DISK_CACHE_PATH);
	let _icc = ConfigRestore::of(crate::oakui::displaycolor::CONFIG_KEY_CUSTOM_ICC);
	let _pg = ConfigRestore::of(CONFIG_KEY_PG_URL);
	let _ahead = ConfigRestore::of(CONFIG_KEY_PREVIEW_WINDOW);
	let _transition = ConfigRestore::of(CONFIG_KEY_DEFAULT_TRANSITION_SEC);

	// An unknown backend id and an unknown divider fall back to the
	// first entry; the other rows seed their alternate states.
	config_set_string(CONFIG_KEY_RENDERER_BACKEND, "vaporware");
	config_set_string(CONFIG_KEY_DISPLAY_BIT_DEPTH, "8");
	config_set_string(CONFIG_KEY_USE_PROXY, "false");
	config_set_string("HardwareDecoding", "false");
	config_set_string(crate::oakui::displaycolor::CONFIG_KEY_COLOR_MODE, "off");
	config_set_string(CONFIG_KEY_STORAGE_BACKEND, "pg");
	config_set_int(CONFIG_KEY_PROXY_DIVIDER, 3);
	config_set_string(CONFIG_KEY_DISK_CACHE_PATH, "/tmp/oak-seeded-cache");
	config_set_string(
		crate::oakui::displaycolor::CONFIG_KEY_CUSTOM_ICC,
		"/tmp/oak-seeded.icc",
	);
	config_set_string(CONFIG_KEY_PG_URL, "user@db/oak");
	config_set_int(CONFIG_KEY_PREVIEW_WINDOW, 4);
	// A negative / unparsable transition length falls back to 0.5s.
	config_set_string(CONFIG_KEY_DEFAULT_TRANSITION_SEC, "-1.0");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(800.0), gpui_kit::px(900.0)),
		PreferencesContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("preferences content");

	assert_eq!(
		cx.read(|cx| content.read(cx).backend.read(cx).selected()),
		Some(0)
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).display_bit_depth.read(cx).selected()),
		Some(1)
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).use_proxy.read(cx).state()),
		CheckState::Unchecked
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).hw_decode.read(cx).state()),
		CheckState::Unchecked
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).display_icc.read(cx).state()),
		CheckState::Unchecked
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).storage_backend.read(cx).selected()),
		Some(1)
	);
	assert!(cx.read(|cx| content.read(cx).storage_is_pg));
	assert_eq!(
		cx.read(|cx| content.read(cx).proxy_divider.read(cx).selected()),
		Some(0)
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).cache_dir(cx).to_string()),
		"/tmp/oak-seeded-cache"
	);
	assert_eq!(
		cx.read(|cx| content
			.read(cx)
			.display_icc_path
			.read(cx)
			.path(cx)
			.to_string()),
		"/tmp/oak-seeded.icc"
	);
	assert_eq!(
		cx.read(|cx| content
			.read(cx)
			.storage_pg_url
			.read(cx)
			.path(cx)
			.to_string()),
		"user@db/oak"
	);
	// Cache ahead below the minimum clamps up to 8.
	assert_eq!(
		cx.read(|cx| content.read(cx).cache_ahead.read(cx).value().to_f64()),
		8.0
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).transition_length.read(cx).value().to_f64()),
		0.5
	);
	// The PostgreSQL row renders in this state.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// Known backend / divider values and an over-max cache-ahead clamp.
	config_set_string(CONFIG_KEY_RENDERER_BACKEND, "metal");
	config_set_string(CONFIG_KEY_DISPLAY_BIT_DEPTH, "10");
	config_set_int(CONFIG_KEY_PROXY_DIVIDER, 4);
	config_set_string(CONFIG_KEY_STORAGE_BACKEND, "sqlite");
	config_set_int(CONFIG_KEY_PREVIEW_WINDOW, 5000);
	config_set_string(CONFIG_KEY_DEFAULT_TRANSITION_SEC, "2.5");
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(800.0), gpui_kit::px(900.0)),
		PreferencesContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("preferences content");
	assert_eq!(
		cx.read(|cx| content.read(cx).backend.read(cx).selected()),
		Some(1)
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).display_bit_depth.read(cx).selected()),
		Some(0)
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).proxy_divider.read(cx).selected()),
		Some(2)
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).storage_backend.read(cx).selected()),
		Some(0)
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).cache_ahead.read(cx).value().to_f64()),
		1200.0
	);
	assert_eq!(
		cx.read(|cx| content.read(cx).transition_length.read(cx).value().to_f64()),
		2.5
	);
	// The SQLite state renders without the connection-string row.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// The preferences dialog's free-text fields commit trimmed values on
/// close, and the folder/file pickers land their choice in the fields
/// (with cancels and empty answers leaving everything untouched).
#[gpui_kit::test]
async fn preferences_commit_fields_and_browse_paths(cx: &mut gpui_kit::TestAppContext) {
	let _guard = crate::oakui::graphops::test_lock();
	let _cache = ConfigRestore::of(CONFIG_KEY_DISK_CACHE_PATH);
	let _icc = ConfigRestore::of(crate::oakui::displaycolor::CONFIG_KEY_CUSTOM_ICC);
	let _pg = ConfigRestore::of(CONFIG_KEY_PG_URL);

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(800.0), gpui_kit::px(900.0)),
		PreferencesDialogContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("preferences dialog content");
	let general = cx.read(|cx| content.read(cx).general.clone());

	// Typed (un-browsed) paths land trimmed when the dialog commits.
	cx.update_window(window.clone().into(), |_, window, cx| {
		general.update(cx, |general, cx| {
			general.cache_dir.update(cx, |field, cx| {
				field.set_path("  /tmp/oak-cache  ", window, cx)
			});
			general.display_icc_path.update(cx, |field, cx| {
				field.set_path("  /tmp/oak.icc  ", window, cx)
			});
			general.storage_pg_url.update(cx, |field, cx| {
				field.set_path("  user:pass@localhost/oak  ", window, cx)
			});
		});
	})
	.expect("preferences window");
	cx.update(|cx| content.update(cx, |dialog, cx| dialog.commit_cache_dir(cx)));
	assert_eq!(
		config_get_string(CONFIG_KEY_DISK_CACHE_PATH),
		"/tmp/oak-cache"
	);
	assert_eq!(
		config_get_string(crate::oakui::displaycolor::CONFIG_KEY_CUSTOM_ICC),
		"/tmp/oak.icc"
	);
	assert_eq!(
		config_get_string(CONFIG_KEY_PG_URL),
		"user:pass@localhost/oak"
	);

	// Cancelling the cache-folder picker leaves the typed path alone.
	cx.update(|cx| general.update(cx, |general, cx| general.browse_cache_dir(cx)));
	cx.simulate_path_prompt_response(|_options| None);
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| general.read(cx).cache_dir(cx).to_string()),
		"  /tmp/oak-cache  "
	);

	// An empty answer (no first path) is also ignored.
	cx.update(|cx| general.update(cx, |general, cx| general.browse_cache_dir(cx)));
	cx.simulate_path_prompt_response(|_options| Some(Vec::new()));
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| general.read(cx).cache_dir(cx).to_string()),
		"  /tmp/oak-cache  "
	);

	// Picking a folder fills the field through the async continuation.
	let picked = std::env::temp_dir().join("oak-dialogs-cache-browse");
	cx.update(|cx| general.update(cx, |general, cx| general.browse_cache_dir(cx)));
	cx.simulate_path_prompt_response({
		let picked = picked.clone();
		move |options| {
			assert!(options.directories && !options.files);
			Some(vec![picked])
		}
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| general.read(cx).cache_dir(cx).to_string()),
		picked.to_string_lossy()
	);

	// The ICC browse path behaves the same way.
	let icc = std::env::temp_dir().join("oak-dialogs-profile.icc");
	cx.update(|cx| general.update(cx, |general, cx| general.browse_display_icc(cx)));
	cx.simulate_path_prompt_response({
		let icc = icc.clone();
		move |options| {
			assert!(options.files && !options.directories);
			Some(vec![icc])
		}
	});
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| general
			.read(cx)
			.display_icc_path
			.read(cx)
			.path(cx)
			.to_string()),
		icc.to_string_lossy()
	);
	// Cancelling the ICC picker leaves the picked path in place.
	cx.update(|cx| general.update(cx, |general, cx| general.browse_display_icc(cx)));
	cx.simulate_path_prompt_response(|_options| None);
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| general
			.read(cx)
			.display_icc_path
			.read(cx)
			.path(cx)
			.to_string()),
		icc.to_string_lossy()
	);
}

/// The tabbed preferences host forwards the general tab's events, turns
/// the keyboard tab's Changed into ShortcutsChanged, counts the action
/// rows and switches tabs from the debug-selector buttons.
#[gpui_kit::test]
async fn preferences_dialog_tabs_and_event_forwarding(cx: &mut gpui_kit::TestAppContext) {
	// Lock order: language → config (see the general-rows test).
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _guard = crate::oakui::graphops::test_lock();
	let previous_language = crate::i18n::language_code();
	let _theme = ConfigRestore::of(crate::oakui::real::CONFIG_KEY_THEME);
	let _mode = ConfigRestore::of(crate::oakui::displaycolor::CONFIG_KEY_COLOR_MODE);
	let _language = ConfigRestore::of("Language");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(800.0), gpui_kit::px(900.0)),
		PreferencesDialogContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("preferences dialog content");

	let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
	let _subscription = cx.update(|cx| {
		let seen = seen.clone();
		cx.subscribe(&content, move |_dialog, event: &PreferencesEvent, _cx| {
			seen.borrow_mut().push(*event);
		})
	});

	let general = cx.read(|cx| content.read(cx).general.clone());
	let keyboard = cx.read(|cx| content.read(cx).keyboard.clone());

	// Theme / language / display-color changes are re-emitted by the host.
	cx.update(|cx| {
		general.update(cx, |general, cx| {
			general.theme.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 1 })
			})
		})
	});
	cx.update(|cx| {
		general.update(cx, |general, cx| {
			general.display_icc.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 13,
					state: CheckState::Unchecked,
				})
			})
		})
	});
	cx.update(|cx| {
		general.update(cx, |general, cx| {
			general.language.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 0 })
			})
		})
	});
	// The keyboard tab's Changed becomes ShortcutsChanged.
	cx.update(|cx| keyboard.update(cx, |_keyboard, cx| cx.emit(KeyboardEvent::Changed)));

	let events = seen.borrow().clone();
	assert!(events.contains(&PreferencesEvent::ThemeChanged(false)));
	assert!(events.contains(&PreferencesEvent::DisplayColorChanged));
	assert!(events.contains(&PreferencesEvent::LanguageChanged));
	assert!(events.contains(&PreferencesEvent::ShortcutsChanged));

	// The keyboard tab lists the menu-bar actions.
	assert!(cx.read(|cx| content.read(cx).keyboard_tab_row_count(cx)) > 0);

	// The tab buttons switch the active tab.
	let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let general_tab = visual
		.debug_bounds("prefs-tab-general")
		.expect("the general tab button is painted");
	let keyboard_tab = visual
		.debug_bounds("prefs-tab-keyboard")
		.expect("the keyboard tab button is painted");
	visual.simulate_click(keyboard_tab.center(), gpui_kit::Modifiers::none());
	assert_eq!(
		visual.read(|cx| content.read(cx).active),
		PreferencesTab::Keyboard
	);
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	visual.simulate_click(general_tab.center(), gpui_kit::Modifiers::none());
	assert_eq!(
		visual.read(|cx| content.read(cx).active),
		PreferencesTab::General
	);

	crate::i18n::set_language_code(&previous_language);
}

/// The proxy dialog seeds from the config, edits the global params,
/// generates / deletes proxies through the engine and applies the
/// custom-params checkbox on accept.
#[gpui_kit::test]
async fn proxy_dialog_edits_generates_deletes_and_applies(cx: &mut gpui_kit::TestAppContext) {
	let _guard = crate::oakui::graphops::test_lock();
	use crate::oakui::engine::ProxyMediaState;
	let _width = ConfigRestore::of("ProxyWidth");
	let _height = ConfigRestore::of("ProxyHeight");
	let _divider = ConfigRestore::of("ProxyDivider");
	let _crf = ConfigRestore::of("ProxyCRF");
	let _preset = ConfigRestore::of("ProxyPreset");
	let _audio = ConfigRestore::of("ProxyIncludeAudio");
	let _max = ConfigRestore::of("ProxyMaxConcurrent");
	let _ffmpeg = ConfigRestore::of(CONFIG_KEY_FFMPEG_PATH);

	// A preset name outside the table falls back to "veryfast"; a
	// divider outside [1,2,4,8] falls back to the first entry.
	config_set_string("ProxyPreset", "turbo");
	config_set_int("ProxyDivider", 3);
	config_set_int("ProxyWidth", 640);
	config_set_int("ProxyHeight", 360);
	config_set_int("ProxyCRF", 30);
	config_set_bool("ProxyIncludeAudio", false);
	config_set_int("ProxyMaxConcurrent", 4);
	config_set_string(CONFIG_KEY_FFMPEG_PATH, "/usr/bin/ffmpeg");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let engine = cx.update(|cx| cx.new(MockEngine::demo));
	let window = cx.open_window(gpui_kit::size(gpui_kit::px(600.0), gpui_kit::px(760.0)), {
		let engine = engine.clone();
		move |window, cx| ProxyDialogContent::new(engine, window, cx)
	});
	cx.run_until_parked();
	let content = window.root(cx).expect("proxy dialog content");

	// Seeded params: the unknown preset and divider fall back.
	let params = cx.read(|cx| content.read(cx).current_params(cx));
	assert_eq!(params.width, 640);
	assert_eq!(params.height, 360);
	assert_eq!(params.divider, 1, "an unknown divider falls back to full");
	assert_eq!(params.crf, 30);
	assert_eq!(params.preset, "veryfast");
	assert!(!params.include_audio);
	assert!(cx.read(|cx| !content.read(cx).rows.is_empty()));

	// Selections and edited values flow into `current_params` (the proxy
	// dialog reads the widgets directly, so the selection is set on the
	// combo itself).
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.divider
				.update(cx, |combo, cx| combo.set_selected(Some(3), cx));
			content
				.preset
				.update(cx, |combo, cx| combo.set_selected(Some(8), cx));
			content
				.width
				.update(cx, |spin, cx| spin.set_value(SliderValue::Integer(320), cx));
			content
				.height
				.update(cx, |spin, cx| spin.set_value(SliderValue::Integer(180), cx));
			content
				.crf
				.update(cx, |spin, cx| spin.set_value(SliderValue::Integer(18), cx));
			content
				.include_audio
				.update(cx, |check, cx| check.set_state(CheckState::Checked, cx));
			content
				.max_concurrent
				.update(cx, |spin, cx| spin.set_value(SliderValue::Integer(8), cx));
		});
	});
	cx.update_window(window.clone().into(), |_, window, cx| {
		content.update(cx, |content, cx| {
			content.ffmpeg_path.update(cx, |field, cx| {
				field.set_path("  /opt/ffmpeg  ", window, cx)
			});
		});
	})
	.expect("proxy dialog window");
	let params = cx.read(|cx| content.read(cx).current_params(cx));
	assert_eq!(params.divider, 8);
	assert_eq!(params.width, 320);
	// The height spin snaps to its 8px grid from the 120 minimum:
	// 180 → 184.
	assert_eq!(params.height, 184);
	assert_eq!(params.crf, 18);
	assert_eq!(params.preset, "veryslow");
	assert!(params.include_audio);

	// Saving the global settings writes every edited field.
	cx.update(|cx| content.update(cx, |content, cx| content.save_global_settings(cx)));
	assert_eq!(config_get_int("ProxyWidth", 0), 320);
	assert_eq!(config_get_int("ProxyHeight", 0), 184);
	assert_eq!(config_get_int("ProxyDivider", 0), 8);
	assert_eq!(config_get_int("ProxyCRF", 0), 18);
	assert_eq!(config_get_string("ProxyPreset"), "veryslow");
	assert_eq!(config_get_string("ProxyIncludeAudio"), "true");
	assert_eq!(config_get_int("ProxyMaxConcurrent", 0), 8);
	assert_eq!(config_get_string(CONFIG_KEY_FFMPEG_PATH), "/opt/ffmpeg");

	// Generating with the custom checkbox set: every video row gets the
	// edited params and a (mock: instantly ready) proxy; the audio-only
	// rows are skipped.
	let rows = cx.read(|cx| content.read(cx).rows.clone());
	assert!(rows.iter().any(|row| row.can_generate));
	assert!(rows.iter().any(|row| !row.can_generate));
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.custom_params
				.update(cx, |check, cx| check.set_state(CheckState::Checked, cx));
			content.generate(cx);
		});
	});
	for row in &rows {
		if row.can_generate {
			assert_eq!(
				cx.read(|cx| engine.read(cx).proxy_state(row.id)),
				Some(ProxyMediaState::Ready),
				"row {} generated",
				row.name
			);
			assert_eq!(
				cx.read(|cx| engine.read(cx).proxy_custom_params(row.id))
					.map(|params| params.width),
				Some(320)
			);
		} else {
			assert_eq!(
				cx.read(|cx| engine.read(cx).proxy_state(row.id)),
				Some(ProxyMediaState::Missing),
				"audio-only row {} is skipped",
				row.name
			);
			assert!(
				cx.read(|cx| engine.read(cx).proxy_custom_params(row.id))
					.is_none()
			);
		}
	}

	// A row the engine rejects (a stale entry id) only logs the failure
	// and does not stop the remaining rows.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.rows.push(crate::oakui::engine::ProxyFootageRow {
				id: 9999,
				name: "stale.mp4".into(),
				state: ProxyMediaState::Missing,
				enabled: false,
				has_custom: false,
				can_generate: true,
				has_proxy: false,
			});
			content.generate(cx);
		});
	});

	// Deleting removes the generated proxies.
	cx.update(|cx| content.update(cx, |content, cx| content.delete(cx)));
	for row in rows.iter().filter(|row| row.can_generate) {
		assert_eq!(
			cx.read(|cx| engine.read(cx).proxy_state(row.id)),
			Some(ProxyMediaState::Missing)
		);
	}

	// Accept with the checkbox off clears every row's custom params.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.custom_params
				.update(cx, |check, cx| check.set_state(CheckState::Unchecked, cx));
			content.accept(cx);
		});
	});
	for row in &rows {
		assert!(
			cx.read(|cx| engine.read(cx).proxy_custom_params(row.id))
				.is_none()
		);
	}

	// Accept with it on stores the edited params on every row.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.custom_params
				.update(cx, |check, cx| check.set_state(CheckState::Checked, cx));
			content.accept(cx);
		});
	});
	for row in &rows {
		assert_eq!(
			cx.read(|cx| engine.read(cx).proxy_custom_params(row.id))
				.map(|params| params.crf),
			Some(18)
		);
	}

	// Unset combo selections fall back to the compiled-in divider /
	// preset defaults.
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content
				.divider
				.update(cx, |combo, cx| combo.set_selected(None, cx));
			content
				.preset
				.update(cx, |combo, cx| combo.set_selected(None, cx));
		});
	});
	let params = cx.read(|cx| content.read(cx).current_params(cx));
	assert_eq!(params.divider, 1);
	assert_eq!(params.preset, "veryfast");

	// Render with the populated footage list, then with no footage (the
	// empty hint row).
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.rows.clear();
			cx.notify();
		})
	});
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// Project properties: the cache-location combo drives the custom-path
/// field, an invalid OCIO config rejects the commit, a valid one applies
/// the cache + color settings, and every color-combo index maps to its
/// canonical setting string.
#[gpui_kit::test]
async fn project_properties_commit_and_color_settings(cx: &mut gpui_kit::TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let engine = cx.update(|cx| cx.new(MockEngine::demo));
	let window = cx.open_window(gpui_kit::size(gpui_kit::px(560.0), gpui_kit::px(600.0)), {
		let engine = engine.clone();
		move |window, cx| ProjectPropertiesContent::new(engine, window, cx)
	});
	cx.run_until_parked();
	let content = window.root(cx).expect("project properties content");

	// A fresh mock project carries no OCIO override / custom cache path.
	assert!(
		cx.read(|cx| content.read(cx).ocio_config_path(cx))
			.is_empty()
	);
	assert!(
		cx.read(|cx| content.read(cx).custom_cache_path(cx))
			.is_empty()
	);
	assert_eq!(cx.read(|cx| content.read(cx).cache_setting), 0);

	// Selecting 自定义位置 through the combo's own event enables the
	// custom-path field live.
	cx.update(|cx| {
		content.update(cx, |dialog, cx| {
			dialog.cache_location.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 2 })
			})
		})
	});
	assert_eq!(cx.read(|cx| content.read(cx).cache_setting), 2);
	assert!(cx.read(|cx| content.read(cx).custom_cache_path.read(cx).enabled));

	// `select_cache_setting` clamps and syncs the field's enable state.
	cx.update(|cx| content.update(cx, |dialog, cx| dialog.select_cache_setting(7, cx)));
	assert_eq!(cx.read(|cx| content.read(cx).cache_setting), 2);
	cx.update(|cx| content.update(cx, |dialog, cx| dialog.select_cache_setting(1, cx)));
	assert!(!cx.read(|cx| content.read(cx).custom_cache_path.read(cx).enabled));

	// An invalid OCIO config path rejects the commit and is not applied.
	cx.update_window(window.clone().into(), |_, window, cx| {
		content.update(cx, |dialog, cx| {
			dialog.set_ocio_config_path("/nonexistent/oak-dialogs.ocio", window, cx)
		});
	})
	.expect("preferences window");
	let result = cx.update(|cx| content.update(cx, |dialog, cx| dialog.commit(cx)));
	assert!(result.is_err());
	assert!(
		cx.read(|cx| engine.read(cx).project_ocio_config())
			.is_empty()
	);

	// An empty path restores the app default; the cache location and
	// color settings then apply (custom path trimmed).
	let _ = cx.update_window(window.clone().into(), |_, window, cx| {
		content.update(cx, |dialog, cx| {
			dialog.set_ocio_config_path("", window, cx);
			dialog.select_cache_setting(2, cx);
			dialog.set_custom_cache_path("  /tmp/oak-proj-cache  ", window, cx);
			dialog
				.working_space
				.update(cx, |combo, cx| combo.set_selected(Some(1), cx));
			dialog
				.output_gamut
				.update(cx, |combo, cx| combo.set_selected(Some(2), cx));
			dialog
				.output_transfer
				.update(cx, |combo, cx| combo.set_selected(Some(3), cx));
		});
	});
	let result = cx.update(|cx| content.update(cx, |dialog, cx| dialog.commit(cx)));
	assert!(result.is_ok());
	assert!(cx.read(|cx| content.read(cx).error().is_none()));
	assert_eq!(
		cx.read(|cx| engine.read(cx).project_cache_location()),
		(2, "/tmp/oak-proj-cache".to_string())
	);
	let (working, gamut, transfer) = cx.update(|cx| {
		content.update(cx, |dialog, cx| {
			dialog
				.working_space
				.update(cx, |combo, cx| combo.set_selected(Some(0), cx));
			dialog
				.output_gamut
				.update(cx, |combo, cx| combo.set_selected(Some(1), cx));
			dialog
				.output_transfer
				.update(cx, |combo, cx| combo.set_selected(Some(2), cx));
			dialog.color_settings(cx)
		})
	});
	assert_eq!(working, "acescg");
	assert_eq!(gamut, "displayp3");
	assert_eq!(transfer, "pq");

	// Index → setting mapping for every combo value, and the default
	// when nothing is selected.
	let cases = [(0, "acescg"), (1, "srgb_legacy")];
	for (index, expected) in cases {
		let got = cx.update(|cx| {
			content.update(cx, |dialog, cx| {
				dialog
					.working_space
					.update(cx, |combo, cx| combo.set_selected(Some(index), cx));
				dialog.color_settings(cx).0
			})
		});
		assert_eq!(got, expected);
	}
	for (index, expected) in [(0, "srgb"), (1, "displayp3"), (2, "bt2020")] {
		let got = cx.update(|cx| {
			content.update(cx, |dialog, cx| {
				dialog
					.output_gamut
					.update(cx, |combo, cx| combo.set_selected(Some(index), cx));
				dialog.color_settings(cx).1
			})
		});
		assert_eq!(got, expected);
	}
	for (index, expected) in [(0, "srgb"), (1, "gamma22"), (2, "pq"), (3, "hlg")] {
		let got = cx.update(|cx| {
			content.update(cx, |dialog, cx| {
				dialog
					.output_transfer
					.update(cx, |combo, cx| combo.set_selected(Some(index), cx));
				dialog.color_settings(cx).2
			})
		});
		assert_eq!(got, expected);
	}
	let (working, gamut, transfer) = cx.update(|cx| {
		content.update(cx, |dialog, cx| {
			dialog
				.working_space
				.update(cx, |combo, cx| combo.set_selected(None, cx));
			dialog
				.output_gamut
				.update(cx, |combo, cx| combo.set_selected(None, cx));
			dialog
				.output_transfer
				.update(cx, |combo, cx| combo.set_selected(None, cx));
			dialog.color_settings(cx)
		})
	});
	assert_eq!(working, "acescg");
	assert_eq!(gamut, "srgb");
	assert_eq!(transfer, "srgb");

	// The clean form renders without the error row, then with it when a
	// rejected commit is reported.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	cx.update(|cx| {
		content.update(cx, |dialog, cx| {
			dialog.set_error(Some("bad config".into()), cx)
		})
	});
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	assert_eq!(
		cx.read(|cx| content.read(cx).error().cloned()),
		Some("bad config".to_string())
	);
}

/// The shared sequence format fields: picking a preset fills the
/// dimensions and rate, editing any field snaps the preset back to
/// custom, and the interlaced checkbox accepts the toggled state.
#[gpui_kit::test]
async fn sequence_format_fields_presets_and_custom(cx: &mut gpui_kit::TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let engine = cx.update(|cx| cx.new(MockEngine::demo));
	let window = cx.open_window(gpui_kit::size(gpui_kit::px(520.0), gpui_kit::px(560.0)), {
		let engine = engine.clone();
		move |window, cx| NewSequenceContent::new(engine, window, cx)
	});
	cx.run_until_parked();
	let content = window.root(cx).expect("new sequence content");
	let fields = cx.read(|cx| content.read(cx).format.clone());

	// The default seed is HD 1080p25 with a non-empty localized name.
	assert_eq!(
		cx.read(|cx| content.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat {
			width: 1920,
			height: 1080,
			rate: FrameRate::new(25, 1),
		}
	);
	assert!(!cx.read(|cx| content.read(cx).interlaced(cx)));
	assert!(!cx.read(|cx| content.read(cx).name(cx)).is_empty());

	// Picking the 4K DCI preset fills the numeric fields and rate.
	cx.update(|cx| {
		fields.update(cx, |fields, cx| {
			fields.preset.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 6 })
			})
		})
	});
	assert_eq!(
		cx.read(|cx| fields.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat {
			width: 4096,
			height: 2160,
			rate: FrameRate::new(24, 1),
		}
	);

	// The custom entry itself leaves the fields untouched.
	cx.update(|cx| {
		fields.update(cx, |fields, cx| {
			fields.preset.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 0 })
			})
		})
	});
	assert_eq!(
		cx.read(|cx| fields.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat {
			width: 4096,
			height: 2160,
			rate: FrameRate::new(24, 1),
		}
	);

	// Editing the width snaps the preset back to custom.
	cx.update(|cx| {
		fields.update(cx, |fields, cx| {
			fields.width.update(cx, |spin, cx| {
				spin.set_value(SliderValue::Integer(1280), cx);
				cx.emit(SpinBoxEvent::ValueChanged {
					control: 41,
					value: SliderValue::Integer(1280),
				});
			});
		})
	});
	assert_eq!(
		cx.read(|cx| fields.read(cx).preset.read(cx).selected()),
		Some(0)
	);
	// …and so does the height.
	cx.update(|cx| {
		fields.update(cx, |fields, cx| {
			fields.height.update(cx, |spin, cx| {
				spin.set_value(SliderValue::Integer(720), cx);
				cx.emit(SpinBoxEvent::ValueChanged {
					control: 42,
					value: SliderValue::Integer(720),
				});
			});
		})
	});
	// …and the frame rate.
	cx.update(|cx| {
		fields.update(cx, |fields, cx| {
			fields.rate.update(cx, |combo, cx| {
				combo.set_selected(Some(7), cx);
				cx.emit(ComboBoxEvent::Selected { value: 7 });
			})
		})
	});
	assert_eq!(
		cx.read(|cx| fields.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat {
			width: 1280,
			height: 720,
			rate: FrameRate::new(60, 1),
		}
	);
	assert_eq!(
		cx.read(|cx| fields.read(cx).preset.read(cx).selected()),
		Some(0)
	);

	// The interlaced checkbox is request-only: the host accepts the
	// toggled state back.
	cx.update(|cx| {
		fields.update(cx, |fields, cx| {
			fields.interlaced.update(cx, |_check, cx| {
				cx.emit(CheckBoxEvent::Toggled {
					control: 44,
					state: CheckState::Checked,
				})
			})
		})
	});
	assert!(cx.read(|cx| fields.read(cx).interlaced(cx)));
	assert!(cx.read(|cx| content.read(cx).interlaced(cx)));

	// Editing the name flows through the TextValue wrapper.
	cx.update_window(window.clone().into(), |_, window, cx| {
		content.update(cx, |content, cx| {
			content
				.name
				.update(cx, |name, cx| name.set_value("Opening", window, cx))
		});
	})
	.expect("window open");
	assert_eq!(cx.read(|cx| content.read(cx).name(cx)).as_ref(), "Opening");

	// The mock engine has no project surface: the commit fails and the
	// host-reported error row clears on success.
	let result = cx.update(|cx| content.update(cx, |content, cx| content.commit(cx)));
	assert!(result.is_err());
	cx.update(|cx| content.update(cx, |content, cx| content.set_error(Some("boom".into()), cx)));
	assert_eq!(
		cx.read(|cx| content.read(cx).error().cloned()),
		Some("boom".to_string())
	);
	cx.update(|cx| content.update(cx, |content, cx| content.set_error(None, cx)));
	assert!(cx.read(|cx| content.read(cx).error().is_none()));

	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// The new-sequence dialog seeds from a probed footage format (custom
/// dimensions + interlaced) and its properties sibling falls back to the
/// HD defaults for a missing sequence; both report commit failures and
/// render.
#[gpui_kit::test]
async fn new_sequence_and_properties_commit_paths(cx: &mut gpui_kit::TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let engine = cx.update(|cx| cx.new(MockEngine::demo));

	// A probed 512×512 @ 12.34 fps interlaced source seeds the dialog.
	let seed = SequenceFormatSeed::from_format(
		&crate::oakui::engine::VideoFormat {
			width: 512,
			height: 512,
			rate: FrameRate::new(1234, 100),
		},
		true,
	);
	let seeded = cx.open_window(gpui_kit::size(gpui_kit::px(520.0), gpui_kit::px(560.0)), {
		let engine = engine.clone();
		move |window, cx| NewSequenceContent::new_seeded(engine, seed, window, cx)
	});
	cx.run_until_parked();
	let seeded_content = seeded.root(cx).expect("seeded new sequence content");
	// The custom rate is not one of the dropdown choices, so the dialog
	// falls back to the default 25 fps for it.
	assert_eq!(
		cx.read(|cx| seeded_content.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat {
			width: 512,
			height: 512,
			rate: FrameRate::new(25, 1),
		}
	);
	assert!(cx.read(|cx| seeded_content.read(cx).interlaced(cx)));
	let result = cx.update(|cx| seeded_content.update(cx, |content, cx| content.commit(cx)));
	assert!(result.is_err(), "the mock has no project to create into");
	cx.update_window(seeded.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// The default constructor seeds HD 1080p25.
	let fresh = cx.open_window(gpui_kit::size(gpui_kit::px(520.0), gpui_kit::px(560.0)), {
		let engine = engine.clone();
		move |window, cx| NewSequenceContent::new(engine, window, cx)
	});
	cx.run_until_parked();
	let fresh_content = fresh.root(cx).expect("new sequence content");
	assert_eq!(
		cx.read(|cx| fresh_content.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat::hd_1080p25()
	);
	cx.update_window(fresh.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// A missing sequence falls back to the custom HD seed with a blank
	// name; committing reports the engine's failure.
	let properties = cx.open_window(gpui_kit::size(gpui_kit::px(520.0), gpui_kit::px(560.0)), {
		let engine = engine.clone();
		move |window, cx| SequencePropertiesContent::new(engine, 42, window, cx)
	});
	cx.run_until_parked();
	let props = properties.root(cx).expect("sequence properties content");
	assert!(cx.read(|cx| props.read(cx).name(cx)).is_empty());
	assert_eq!(
		cx.read(|cx| props.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat {
			width: 1920,
			height: 1080,
			rate: FrameRate::new(25, 1),
		}
	);
	assert!(!cx.read(|cx| props.read(cx).interlaced(cx)));
	let result = cx.update(|cx| props.update(cx, |content, cx| content.commit(cx)));
	assert!(result.is_err());
	cx.update(|cx| props.update(cx, |content, cx| content.set_error(Some("boom".into()), cx)));
	assert!(cx.read(|cx| props.read(cx).error().is_some()));
	cx.update_window(properties.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// The multicam wizard lists the engine's angles, toggles row checks and
/// reports the selection / sync mode / name; the empty case renders the
/// no-footage hint.
#[gpui_kit::test]
async fn multicam_wizard_selection_and_render(cx: &mut gpui_kit::TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let engine = cx.update(|cx| cx.new(MockEngine::demo));
	let window = cx.open_window(gpui_kit::size(gpui_kit::px(520.0), gpui_kit::px(560.0)), {
		let engine = engine.clone();
		move |window, cx| MulticamWizardContent::new(engine, window, cx)
	});
	cx.run_until_parked();
	let content = window.root(cx).expect("multicam wizard content");

	assert!(cx.read(|cx| content.read(cx).has_rows()));
	assert_eq!(cx.read(|cx| content.read(cx).sync_mode(cx)), 0);
	assert!(!cx.read(|cx| content.read(cx).name(cx)).is_empty());
	assert!(cx.read(|cx| content.read(cx).selection()).is_empty());
	assert_eq!(
		cx.read(|cx| content.read(cx).engine().entity_id()),
		engine.entity_id()
	);

	// Toggling an out-of-range row is a no-op; toggling a real row
	// selects it, and toggling it again deselects it.
	cx.update(|cx| content.update(cx, |wizard, cx| wizard.toggle_row(99, cx)));
	assert!(cx.read(|cx| content.read(cx).selection()).is_empty());
	cx.update(|cx| content.update(cx, |wizard, cx| wizard.toggle_row(1, cx)));
	let selection = cx.read(|cx| content.read(cx).selection());
	assert_eq!(selection.len(), 1);
	assert_eq!(selection[0].name.as_ref(), "intro.mov");
	cx.update(|cx| content.update(cx, |wizard, cx| wizard.toggle_row(1, cx)));
	assert!(cx.read(|cx| content.read(cx).selection()).is_empty());

	// The selected sync mode tracks the combo.
	cx.update(|cx| {
		content.update(cx, |wizard, cx| {
			wizard
				.sync
				.update(cx, |combo, cx| combo.set_selected(Some(2), cx))
		})
	});
	assert_eq!(cx.read(|cx| content.read(cx).sync_mode(cx)), 2);

	// A row the engine could not probe carries no duration (the render
	// uses an empty label for it).
	cx.update(|cx| {
		content.update(cx, |wizard, cx| {
			wizard.rows.push((
				crate::oakui::engine::WizardFootage {
					id: 99,
					name: "unprobed.mov".into(),
					source_timecode: None,
					duration_s: None,
					has_audio: None,
				},
				false,
			));
			cx.notify();
		})
	});

	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// No wizard footage: the empty hint branch renders.
	cx.update(|cx| {
		content.update(cx, |wizard, cx| {
			wizard.rows.clear();
			cx.notify();
		})
	});
	assert!(!cx.read(|cx| content.read(cx).has_rows()));
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// The lightweight contents render without an engine: About, the
/// export-project format picker, the drop-sequence choice and the rename
/// field.
#[gpui_kit::test]
async fn lightweight_dialog_contents_render(cx: &mut gpui_kit::TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let size = gpui_kit::size(gpui_kit::px(460.0), gpui_kit::px(320.0));

	// About: static text.
	let about = cx.open_window(size, |_window, _cx| AboutContent::new());
	cx.run_until_parked();
	cx.update_window(about.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// Export project: OTIO default, per-format extension, stale
	// selection falls back to OTIO.
	let export = cx.open_window(size, ExportProjectDialogContent::new);
	cx.run_until_parked();
	let export_content = export.root(cx).expect("export project content");
	assert_eq!(
		cx.read(|cx| export_content.read(cx).format(cx)),
		PROJECT_FORMAT_OTIO
	);
	assert_eq!(cx.read(|cx| export_content.read(cx).extension(cx)), "otio");
	cx.update(|cx| {
		export_content.update(cx, |content, cx| {
			content
				.format
				.update(cx, |combo, cx| combo.set_selected(Some(1), cx))
		})
	});
	assert_eq!(
		cx.read(|cx| export_content.read(cx).format(cx)),
		PROJECT_FORMAT_OVE
	);
	assert_eq!(cx.read(|cx| export_content.read(cx).extension(cx)), "ove");
	cx.update(|cx| {
		export_content.update(cx, |content, cx| {
			content
				.format
				.update(cx, |combo, cx| combo.set_selected(Some(2), cx))
		})
	});
	assert_eq!(
		cx.read(|cx| export_content.read(cx).extension(cx)),
		"fcpxml"
	);
	cx.update(|cx| {
		export_content.update(cx, |content, cx| {
			content
				.format
				.update(cx, |combo, cx| combo.set_selected(Some(99), cx))
		})
	});
	assert_eq!(
		cx.read(|cx| export_content.read(cx).format(cx)),
		PROJECT_FORMAT_OTIO
	);
	assert_eq!(cx.read(|cx| export_content.read(cx).extension(cx)), "otio");
	cx.update_window(export.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// The drop-onto-empty-timeline choice: the probed video parameters
	// and the pure-audio note.
	let probed = cx.open_window(size, |_window, _cx| {
		DropSequenceChoiceContent::new(Some((1920, 1080, FrameRate::new(30000, 1001), true)))
	});
	cx.run_until_parked();
	let probed_content = probed.root(cx).expect("drop choice content");
	assert_eq!(
		cx.read(|cx| probed_content.read(cx).probed()),
		Some((1920, 1080, FrameRate::new(30000, 1001), true))
	);
	cx.update_window(probed.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	let progressive = cx.open_window(size, |_window, _cx| {
		DropSequenceChoiceContent::new(Some((1280, 720, FrameRate::new(25, 1), false)))
	});
	cx.run_until_parked();
	cx.update_window(progressive.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	let audio_only = cx.open_window(size, |_window, _cx| DropSequenceChoiceContent::new(None));
	cx.run_until_parked();
	let audio_content = audio_only.root(cx).expect("drop choice content");
	assert!(cx.read(|cx| audio_content.read(cx).probed()).is_none());
	cx.update_window(audio_only.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// Rename: the field is prefilled with the current name.
	let rename = cx.open_window(size, {
		let current = SharedString::from("Old Name");
		move |window, cx| RenameContent::new(current, window, cx)
	});
	cx.run_until_parked();
	let rename_content = rename.root(cx).expect("rename content");
	assert_eq!(
		cx.read(|cx| rename_content.read(cx).value(cx)).as_ref(),
		"Old Name"
	);
	cx.update_window(rename.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// The new-project dialog: the name field round-trips and the format
/// rows follow the shared preset / custom behavior.
#[gpui_kit::test]
async fn new_project_dialog_edits_and_renders(cx: &mut gpui_kit::TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(520.0), gpui_kit::px(560.0)),
		NewProjectContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("new project content");

	assert!(!cx.read(|cx| content.read(cx).name(cx)).is_empty());
	assert_eq!(
		cx.read(|cx| content.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat::hd_1080p25()
	);
	assert!(!cx.read(|cx| content.read(cx).interlaced(cx)));

	// The preset combo and the spin boxes are the shared
	// SequenceFormatFields; pick the 4K UHD entry.
	let fields = cx.read(|cx| content.read(cx).format.clone());
	cx.update(|cx| {
		fields.update(cx, |fields, cx| {
			fields.preset.update(cx, |_combo, cx| {
				cx.emit(ComboBoxEvent::Selected { value: 5 })
			})
		})
	});
	assert_eq!(
		cx.read(|cx| content.read(cx).format(cx)),
		crate::oakui::engine::VideoFormat {
			width: 3840,
			height: 2160,
			rate: FrameRate::new(25, 1),
		}
	);
	cx.update_window(window.clone().into(), |_, window, cx| {
		content.update(cx, |content, cx| {
			content
				.name
				.update(cx, |name, cx| name.set_value("My Project", window, cx))
		});
	})
	.expect("window open");
	assert_eq!(
		cx.read(|cx| content.read(cx).name(cx)).as_ref(),
		"My Project"
	);

	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
}

/// The export dialog's sequence picker: the host populates it from
/// the project, a known preselection wins, an unknown one falls back
/// to the first entry, and an empty project leaves nothing to export.
#[gpui_kit::test]
async fn export_dialog_sequence_picker(cx: &mut gpui_kit::TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(440.0), gpui_kit::px(400.0)),
		ExportDialogContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("dialog content root");

	let sequences = vec![
		(7u64, SharedString::from("Opening")),
		(9u64, SharedString::from("Finale")),
	];
	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.set_sequences(sequences.clone(), Some(9), cx)
		});
	});
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_sequence(cx)),
		Some(9),
		"the known preselection wins"
	);

	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.set_sequences(sequences.clone(), Some(42), cx)
		});
	});
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_sequence(cx)),
		Some(7),
		"an unknown preselection falls back to the first entry"
	);

	cx.update(|cx| {
		content.update(cx, |content, cx| {
			content.set_sequences(Vec::new(), None, cx)
		});
	});
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_sequence(cx)),
		None,
		"an empty project leaves nothing to export"
	);
}

/// The 4K presets are in the dropdown and resolve to their formats
/// (4K UHD 3840×2160@25, 4K DCI 4096×2160@24).
#[test]
fn four_k_presets_are_offered_and_resolve() {
	let options = sequence_preset_options();
	let labels: Vec<&str> = options.iter().map(|o| o.label.as_ref()).collect();
	assert_eq!(options.len(), 7);
	assert_eq!(sequence_preset_format(5), Some((3840, 2160, 25, 1)));
	assert_eq!(sequence_preset_format(6), Some((4096, 2160, 24, 1)));
	assert_eq!(sequence_preset_format(7), None);
	// The preset labels come from the language packs (default English
	// here): the 4K entries say what they set.
	assert!(
		labels[5].contains("3840×2160"),
		"4K UHD label: {}",
		labels[5]
	);
	assert!(
		labels[6].contains("4096×2160"),
		"4K DCI label: {}",
		labels[6]
	);
}

/// A probed footage format seeds the new-sequence fields: an exact
/// preset match selects the preset, otherwise the custom entry with the
/// probed dimensions/rate already filled in.
#[test]
fn seed_from_format_matches_presets_and_falls_back_to_custom() {
	let hd = crate::oakui::engine::VideoFormat {
		width: 1920,
		height: 1080,
		rate: FrameRate::new(25, 1),
	};
	let seed = SequenceFormatSeed::from_format(&hd, false);
	assert_eq!(seed.preset, 3);
	assert_eq!((seed.width, seed.height), (1920, 1080));
	assert_eq!(seed.rate, Some(2));

	let custom = crate::oakui::engine::VideoFormat {
		width: 512,
		height: 512,
		rate: FrameRate::new(1234, 100),
	};
	let seed = SequenceFormatSeed::from_format(&custom, true);
	assert_eq!(seed.preset, 0);
	assert_eq!((seed.width, seed.height), (512, 512));
	assert_eq!(seed.rate, None);
	assert!(seed.interlaced);

	let dci = crate::oakui::engine::VideoFormat {
		width: 4096,
		height: 2160,
		rate: FrameRate::new(24, 1),
	};
	let seed = SequenceFormatSeed::from_format(&dci, false);
	assert_eq!(seed.preset, 6, "4K DCI resolves to its preset");
}

/// The sequence properties dialog seeds the preset dropdown correctly for
/// every preset value, so reopening properties on a 4K sequence keeps
/// 4K selected (not "custom").
#[test]
fn properties_seed_matches_all_presets_including_4k() {
	for index in 1..=SEQUENCE_PRESET_COUNT {
		let (w, h, num, den) = sequence_preset_format(index).unwrap();
		let params = crate::oakui::engine::SequenceParameters {
			name: "4K".into(),
			format: crate::oakui::engine::VideoFormat {
				width: w,
				height: h,
				rate: FrameRate::new(num, den),
			},
			interlaced: false,
		};
		let seed = SequenceFormatSeed::from_format(&params.format, params.interlaced);
		assert_eq!(seed.preset, index, "preset {index} reselects itself");
	}
}

#[test]
fn capture_decision_handles_all_shapes() {
	assert!(matches!(
		capture_decision(&keystroke("escape")),
		CaptureDecision::Cancel
	));
	assert!(matches!(
		capture_decision(&keystroke("backspace")),
		CaptureDecision::Clear
	));
	assert!(matches!(
		capture_decision(&keystroke("delete")),
		CaptureDecision::Clear
	));
	// Bare modifiers never bind.
	assert!(matches!(
		capture_decision(&keystroke("shift")),
		CaptureDecision::Ignore
	));
	assert!(matches!(
		capture_decision(&keystroke("control")),
		CaptureDecision::Ignore
	));
	assert!(matches!(
		capture_decision(&keystroke("alt")),
		CaptureDecision::Ignore
	));
	// A real key (with or without modifiers) becomes the canonical binding.
	match capture_decision(&keystroke("secondary-s")) {
		CaptureDecision::Assign(canon) => {
			assert_eq!(
				canon,
				gpui_kit::Keystroke::parse("secondary-s").unwrap().unparse()
			);
		}
		other => panic!("expected assign, got {other:?}"),
	}
}

#[test]
fn keyboard_filter_matches_name_path_and_shortcut() {
	assert!(keyboard_filter_matches(
		"Save Project",
		"File",
		Some("⌘S"),
		"save"
	));
	assert!(keyboard_filter_matches(
		"Save Project",
		"File",
		Some("⌘S"),
		"file > save"
	));
	// Shortcut matching is case-insensitive.
	assert!(keyboard_filter_matches(
		"Save Project",
		"File",
		Some("⌘S"),
		"⌘s"
	));
	assert!(!keyboard_filter_matches(
		"Save Project",
		"File",
		Some("⌘S"),
		"undo"
	));
	// Empty query matches everything; rows without a shortcut match only
	// by name/path.
	assert!(keyboard_filter_matches("About Oak…", "Help", None, ""));
	assert!(keyboard_filter_matches("About Oak…", "Help", None, "help"));
	assert!(!keyboard_filter_matches("About Oak…", "Help", None, "⌘"));
}

#[test]
fn search_filter_matches_label_or_path() {
	let _guard = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");
	assert!(search_filter_matches(ActionId::NewProject, "File", "new"));
	assert!(search_filter_matches(
		ActionId::NewProject,
		"File",
		"file > new"
	));
	assert!(!search_filter_matches(ActionId::NewProject, "File", "undo"));
	assert!(search_filter_matches(ActionId::NewProject, "File", ""));
}

#[test]
fn selection_step_wraps_and_respects_visibility() {
	assert_eq!(selection_step(&[2, 5, 7], None, 1), Some(2));
	assert_eq!(selection_step(&[2, 5, 7], None, -1), Some(7));
	assert_eq!(selection_step(&[2, 5, 7], Some(2), 1), Some(5));
	assert_eq!(selection_step(&[2, 5, 7], Some(7), 1), Some(2));
	assert_eq!(selection_step(&[2, 5, 7], Some(2), -1), Some(7));
	assert_eq!(selection_step(&[], None, 1), None);
}

#[test]
fn keyboard_rows_cover_the_menu_bar() {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	crate::i18n::set_language_code("en-US");
	let rows = crate::app::menu_action_paths();
	assert!(!rows.is_empty());
	// Every listed action resolves to a registry entry with a menu item.
	for (action, path) in &rows {
		assert_ne!(action.menu_id(), crate::actions::HIDDEN_MENU_ID);
		assert!(!path.is_empty(), "action {action:?} has an empty path");
	}
}

/// The keyboard tab: capture ignore / cancel / clear / assign (with the
/// conflict steal), reset selected / all, and the import / export file
/// round trips (including the cancel and failure paths).
#[gpui_kit::test]
async fn keyboard_tab_capture_reset_and_file_round_trip(cx: &mut gpui_kit::TestAppContext) {
	// Lock order: shortcuts → language → config (the same nesting the
	// other app test modules use, so parallel tests cannot deadlock).
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _config = crate::oakui::graphops::test_lock();
	let previous_language = crate::i18n::language_code();
	crate::i18n::set_language_code("en-US");

	// Point the configuration at an isolated temp dir: capture commits
	// call `save_custom_shortcuts()` against `<config>/shortcuts`.
	let config_dir =
		std::env::temp_dir().join(format!("oak-dialogs-shortcuts-{}", std::process::id()));
	std::fs::create_dir_all(&config_dir).expect("temp config dir");
	let _env = ConfigDirRestore(std::env::var_os("QUERCUS_CONFIG_DIR"));
	unsafe { std::env::set_var("QUERCUS_CONFIG_DIR", &config_dir) };

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(700.0), gpui_kit::px(600.0)),
		KeyboardTabContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("keyboard tab content");

	assert!(cx.read(|cx| content.read(cx).capturing.is_none()));
	assert!(cx.read(|cx| content.read(cx).row_count()) > 0);
	let first_action = cx.read(|cx| content.read(cx).rows[0].action);
	let second_action = cx.read(|cx| content.read(cx).rows[1].action);

	// Escape cancels; a bare modifier is ignored (capture continues); a
	// real key binds the canonical form.
	cx.update(|cx| {
		content.update(cx, |tab, cx| {
			tab.begin_capture(0, cx);
			assert_eq!(tab.capturing, Some(0));
			tab.handle_capture_key(&keystroke("shift"), cx);
			assert_eq!(tab.capturing, Some(0), "modifier-only keys keep capturing");
			tab.handle_capture_key(&keystroke("escape"), cx);
			assert!(tab.capturing.is_none(), "escape cancels the capture");
		});
	});
	// handle_capture_key outside capture mode is a no-op.
	cx.update(|cx| content.update(cx, |tab, cx| tab.handle_capture_key(&keystroke("a"), cx)));

	// Backspace unbinds the action and saves the diff.
	cx.update(|cx| {
		content.update(cx, |tab, cx| {
			tab.begin_capture(0, cx);
			tab.handle_capture_key(&keystroke("backspace"), cx);
		})
	});
	assert!(cx.read(|cx| content.read(cx).capturing.is_none()));
	assert!(crate::actions::effective_keys(first_action.entry()).is_empty());
	assert!(cx.read(|cx| content.read(cx).status.is_some()));
	// The unbound row renders its "No shortcut"-style placeholder.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// Reset Selected restores the registry defaults.
	cx.update(|cx| {
		content.update(cx, |tab, cx| {
			tab.selected = Some(0);
			tab.reset_selected(cx);
		})
	});
	let defaults: Vec<String> = first_action
		.entry()
		.default_keys
		.iter()
		.map(|key| key.to_string())
		.collect();
	assert_eq!(
		crate::actions::effective_keys(first_action.entry()),
		defaults
	);

	// Reset Selected with nothing selected is a no-op.
	cx.update(|cx| {
		content.update(cx, |tab, cx| {
			tab.selected = None;
			tab.reset_selected(cx);
		})
	});

	// Assigning a key to one action, then the same key to another action,
	// moves the binding: the displaced action loses it and the status
	// reports the conflict.
	let canon = keystroke("secondary-alt-p").unparse();
	cx.update(|cx| {
		content.update(cx, |tab, cx| {
			tab.begin_capture(0, cx);
			tab.handle_capture_key(&keystroke("secondary-alt-p"), cx);
		})
	});
	assert_eq!(
		crate::actions::effective_keys(first_action.entry()),
		vec![canon.clone()]
	);
	cx.update(|cx| {
		content.update(cx, |tab, cx| {
			tab.begin_capture(1, cx);
			tab.handle_capture_key(&keystroke("secondary-alt-p"), cx);
		})
	});
	assert_eq!(
		crate::actions::effective_keys(second_action.entry()),
		vec![canon.clone()]
	);
	assert!(
		crate::actions::effective_keys(first_action.entry()).is_empty(),
		"the displaced action loses the key"
	);
	assert!(cx.read(|cx| content.read(cx).status.is_some()));

	// Reset All is two-step: the first call arms the inline confirmation
	// (and cancels any capture), the second applies it.
	cx.update(|cx| {
		content.update(cx, |tab, cx| {
			tab.capturing = Some(0);
			tab.reset_all(cx);
		})
	});
	assert!(cx.read(|cx| content.read(cx).confirm_reset_all));
	assert!(cx.read(|cx| content.read(cx).capturing.is_none()));
	cx.update(|cx| content.update(cx, |tab, cx| tab.reset_all(cx)));
	assert!(!cx.read(|cx| content.read(cx).confirm_reset_all));
	assert!(!crate::actions::has_custom_shortcuts());
	assert!(cx.read(|cx| content.read(cx).status.is_some()));

	// Import: a picked file replaces the overrides and the effective
	// state is saved back to the configured location.
	let import_path = config_dir.join("import.shortcuts");
	std::fs::write(
		&import_path,
		format!("{}\talt-q\n", first_action.entry().cpp_id),
	)
	.expect("write import file");
	cx.update(|cx| content.update(cx, |tab, cx| tab.import_shortcuts(cx)));
	cx.simulate_path_prompt_response({
		let import_path = import_path.clone();
		move |_options| Some(vec![import_path])
	});
	cx.run_until_parked();
	assert_eq!(
		crate::actions::effective_keys(first_action.entry()),
		vec![keystroke("alt-q").unparse()]
	);
	let saved = std::fs::read_to_string(config_dir.join("shortcuts")).expect("saved shortcuts");
	assert!(saved.contains(first_action.entry().cpp_id));

	// Import failure / cancel paths leave the overrides alone.
	cx.update(|cx| content.update(cx, |tab, cx| tab.import_shortcuts(cx)));
	cx.simulate_path_prompt_response(|_options| Some(Vec::new()));
	cx.run_until_parked();
	cx.update(|cx| content.update(cx, |tab, cx| tab.import_shortcuts(cx)));
	cx.simulate_path_prompt_response(|_options| None);
	cx.run_until_parked();
	cx.update(|cx| content.update(cx, |tab, cx| tab.import_shortcuts(cx)));
	cx.simulate_path_prompt_response({
		let missing = config_dir.join("missing.shortcuts");
		move |_options| Some(vec![missing])
	});
	cx.run_until_parked();
	assert!(cx.read(|cx| content.read(cx).status.is_some()));

	// Export writes the current diff to the picked file.
	let export_path = config_dir.join("exported.shortcuts");
	cx.update(|cx| content.update(cx, |tab, cx| tab.export_shortcuts(cx)));
	cx.simulate_new_path_selection({
		let export_path = export_path.clone();
		move |_base| Some(export_path)
	});
	cx.run_until_parked();
	assert!(export_path.exists(), "export wrote the picked file");

	// Export failure / cancel paths report the failure and return early.
	cx.update(|cx| content.update(cx, |tab, cx| tab.export_shortcuts(cx)));
	cx.simulate_new_path_selection(|_base| None);
	cx.run_until_parked();
	cx.update(|cx| content.update(cx, |tab, cx| tab.export_shortcuts(cx)));
	cx.simulate_new_path_selection(|_base| {
		Some(std::path::PathBuf::from("/nonexistent-oak-dir/shortcuts"))
	});
	cx.run_until_parked();
	assert!(cx.read(|cx| content.read(cx).status.is_some()));

	// The keystroke interceptor routes real keys to the capture logic:
	// dispatching them through the window is enough (the field does not
	// need focus).
	crate::actions::reset_all_custom_shortcuts();
	cx.update(|cx| content.update(cx, |tab, cx| tab.begin_capture(0, cx)));
	cx.dispatch_keystroke(window.into(), keystroke("secondary-alt-p"));
	cx.run_until_parked();
	assert!(cx.read(|cx| content.read(cx).capturing.is_none()));
	assert_eq!(
		crate::actions::effective_keys(first_action.entry()),
		vec![keystroke("secondary-alt-p").unparse()]
	);

	// The search field's subscription updates the filter and clears the
	// selection; the render then only lists matching rows. Type for
	// real: the Change event is what drives the filter.
	cx.update_window(window.clone().into(), |_, window, cx| {
		content.update(cx, |tab, cx| tab.selected = Some(0));
		let focus = content.read(cx).query.read(cx).focus_handle(cx);
		window.focus(&focus, cx);
	})
	.expect("window open");
	cx.simulate_input(window.clone().into(), "save");
	assert_eq!(cx.read(|cx| content.read(cx).filter.clone()), "save");
	assert!(cx.read(|cx| content.read(cx).selected.is_none()));
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	// Platform select-all (cmd-a on macOS, ctrl-a elsewhere — the
	// input keymap binds ctrl-a to MoveHome on macOS) + backspace
	// clears the query.
	cx.simulate_keystrokes(window.clone().into(), "secondary-a backspace");

	// The rendered tab: clicking a capture field starts a capture, and
	// the armed reset-all confirmation footer renders.
	crate::actions::reset_all_custom_shortcuts();
	let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let capture0 = visual
		.debug_bounds("keyboard-capture-0")
		.expect("the first capture field is painted");
	visual.simulate_click(capture0.center(), gpui_kit::Modifiers::none());
	assert_eq!(visual.read(|cx| content.read(cx).capturing), Some(0));
	visual.update(|_window, cx| {
		content.update(cx, |tab, _cx| {
			tab.confirm_reset_all = true;
		})
	});
	visual.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	crate::actions::reset_all_custom_shortcuts();
	crate::i18n::set_language_code(&previous_language);
	let _ = std::fs::remove_dir_all(&config_dir);
}

/// Action search: live filtering (with its empty result case), arrow /
/// enter key handling through the interceptor, execution events and the
/// list rendering.
#[gpui_kit::test]
async fn action_search_filters_navigates_and_executes(cx: &mut gpui_kit::TestAppContext) {
	let _guard = crate::actions::shortcuts_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let _lang = crate::i18n::lang_test_lock()
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let previous_language = crate::i18n::language_code();
	crate::i18n::set_language_code("en-US");

	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(
		gpui_kit::size(gpui_kit::px(600.0), gpui_kit::px(500.0)),
		ActionSearchContent::new,
	);
	cx.run_until_parked();
	let content = window.root(cx).expect("action search content");

	let items: Vec<(ActionId, String)> = cx.read(|cx| {
		content
			.read(cx)
			.items
			.iter()
			.map(|item| (item.action, item.path.clone()))
			.collect()
	});
	assert!(!items.is_empty());
	assert!(
		cx.read(|cx| content.read(cx).filter().to_string())
			.is_empty()
	);
	assert!(cx.read(|cx| content.read(cx).selected_action()).is_none());
	let _focus = cx.read(|cx| content.read(cx).search_focus(cx));

	// The search field filters and selects the first matching action.
	// Type for real: the filter subscription runs on Change events.
	let filter = i18n::tr(ActionId::NewProject.entry().i18n_key).to_lowercase();
	cx.update_window(window.clone().into(), |_, window, cx| {
		let focus = content.read(cx).search_focus(cx);
		window.focus(&focus, cx);
	})
	.expect("window open");
	cx.simulate_input(window.clone().into(), &filter);
	assert_eq!(cx.read(|cx| content.read(cx).filter().to_string()), filter);
	let first_match = items
		.iter()
		.position(|(action, path)| search_filter_matches(*action, path, &filter))
		.expect("the filter matches the action");
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_action()),
		Some(items[first_match].0)
	);

	// A query matching nothing clears the selection (the empty-list
	// render branch).
	cx.simulate_keystrokes(window.clone().into(), "secondary-a");
	cx.simulate_input(window.clone().into(), "zzz-no-such-action-zzz");
	assert!(cx.read(|cx| content.read(cx).selected_action()).is_none());
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	// Clearing the query re-selects the first row; arrow moves wrap
	// through every item.
	cx.simulate_keystrokes(window.clone().into(), "secondary-a backspace");
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_action()),
		Some(items[0].0)
	);
	cx.update(|cx| content.update(cx, |search, cx| search.move_selection(-1, cx)));
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_action()),
		Some(items[items.len() - 1].0)
	);
	cx.update(|cx| content.update(cx, |search, cx| search.move_selection(1, cx)));
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_action()),
		Some(items[0].0)
	);

	// The interceptor handles Down / Up / Enter; other keys pass
	// through. Enter executes the current selection exactly once.
	let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
	let _subscription = cx.update(|cx| {
		let events = events.clone();
		cx.subscribe(&content, move |_search, event: &ActionSearchEvent, _cx| {
			events.borrow_mut().push(*event);
		})
	});
	// A non-text key passes the interceptor through to the field
	// (gpui-kit inserts printable keys as text, so a letter would
	// change the filter; F1 is inert here).
	cx.dispatch_keystroke(window.into(), keystroke("f1"));
	cx.run_until_parked();
	cx.dispatch_keystroke(window.into(), keystroke("down"));
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_action()),
		Some(items[1].0)
	);
	cx.dispatch_keystroke(window.into(), keystroke("up"));
	cx.run_until_parked();
	assert_eq!(
		cx.read(|cx| content.read(cx).selected_action()),
		Some(items[0].0)
	);
	cx.dispatch_keystroke(window.into(), keystroke("enter"));
	cx.run_until_parked();
	assert_eq!(
		events.borrow().as_slice(),
		&[ActionSearchEvent::Execute(items[0].0)]
	);

	// Executing with no selection emits nothing.
	cx.update(|cx| {
		content.update(cx, |search, cx| {
			search.selection = None;
			search.execute(cx);
		})
	});
	assert_eq!(events.borrow().len(), 1);

	// The populated list renders.
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");
	// With no actions at all, the no-actions note renders.
	cx.update(|cx| {
		content.update(cx, |search, cx| {
			search.items.clear();
			search.selection = None;
			cx.notify();
		})
	});
	cx.update_window(window.into(), |_root, window, cx| {
		window.draw(cx).clear(cx);
	})
	.expect("window is still open");

	crate::i18n::set_language_code(&previous_language);
}
