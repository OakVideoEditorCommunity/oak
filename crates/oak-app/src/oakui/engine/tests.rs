use super::*;
use crate::oakui::MockClock;
use crate::widgets::effect_stack::EffectData;
use crate::widgets::node_graph::PortId;
use crate::widgets::project_explorer::ProjectEntry;
use gpui_kit::{AppContext as _, TestAppContext};

#[test]
fn node_category_key_covers_every_category() {
	use NodeCategory as C;
	for category in [
		C::Output,
		C::Effect,
		C::Generator,
		C::Input,
		C::Math,
		C::Color,
		C::Distort,
		C::Filter,
		C::Keying,
		C::OpenFx,
		C::Group,
	] {
		let key = node_category_key(category).expect("category has an i18n key");
		assert!(key.starts_with("node.category."), "{category:?} -> {key}");
	}
	// Timeline-structural nodes never show in the Add menu.
	assert_eq!(node_category_key(C::Timeline), None);
}

/// A minimal engine that implements only the required methods, so every
/// `AppEngine` default body (the "not supported" / no-op fallbacks)
/// executes in the tests below.
struct NullEngine {
	source_clock: Entity<MockClock>,
	program_clock: Entity<MockClock>,
}

impl NullEngine {
	fn new(cx: &mut Context<Self>) -> Self {
		let rate = FrameRate { num: 25, den: 1 };
		NullEngine {
			source_clock: cx.new(|_| MockClock::new(rate)),
			program_clock: cx.new(|_| MockClock::new(rate)),
		}
	}
}

impl EngineGateway for NullEngine {
	fn project(&self) -> Option<&Project> {
		None
	}
	fn current_sequence(&self) -> Option<&Sequence> {
		None
	}
	fn open_project(&mut self, _path: PathBuf, _cx: &mut Context<Self>) {}
	fn request_frame(&mut self, _m: Monitor, _f: Frame, _cx: &mut Context<Self>) {}
	fn play(&mut self, _m: Monitor, _cx: &mut Context<Self>) {}
	fn pause(&mut self, _m: Monitor, _cx: &mut Context<Self>) {}
	fn step(&mut self, _m: Monitor, _d: i64, _cx: &mut Context<Self>) {}
	fn tick(&mut self, _cx: &mut Context<Self>) {}
}

impl TimelineDataSource for NullEngine {
	type Track = <crate::oakui::MockEngine as TimelineDataSource>::Track;
	fn frame_rate(&self) -> FrameRate {
		FrameRate { num: 25, den: 1 }
	}
	fn sequence_length(&self) -> Frame {
		Frame::ZERO
	}
	fn track_count(&self) -> usize {
		1
	}
	fn track(&self, _index: usize) -> Option<Self::Track> {
		None
	}
}

impl EffectStackDataSource for NullEngine {
	fn effects(&self) -> Vec<Arc<dyn EffectData>> {
		Vec::new()
	}
	fn target_label(&self) -> Option<SharedString> {
		None
	}
}

impl NodeGraphDataSource for NullEngine {
	type Node = <crate::oakui::MockEngine as NodeGraphDataSource>::Node;
	type Edge = <crate::oakui::MockEngine as NodeGraphDataSource>::Edge;
	fn nodes(&self) -> Vec<Self::Node> {
		Vec::new()
	}
	fn edges(&self) -> Vec<Self::Edge> {
		Vec::new()
	}
	fn can_connect(&self, _from: PortId, _to: PortId) -> bool {
		false
	}
}

impl ProjectDataSource for NullEngine {
	fn roots(&self) -> Vec<ProjectEntry> {
		Vec::new()
	}
	fn children(&self, _parent: u64) -> Vec<ProjectEntry> {
		Vec::new()
	}
}

impl AudioMeterDataSource for NullEngine {
	fn levels(&self) -> Vec<f32> {
		Vec::new()
	}
}

impl AppEngine for NullEngine {
	type Clock = MockClock;

	fn create(cx: &mut Context<Self>) -> Self {
		NullEngine::new(cx)
	}
	fn source_clock(&self) -> &Entity<Self::Clock> {
		&self.source_clock
	}
	fn program_clock(&self) -> &Entity<Self::Clock> {
		&self.program_clock
	}
	fn clock_frame(&self, _m: Monitor, _cx: &App) -> Frame {
		Frame::ZERO
	}
	fn cpu_frame(&self, _m: Monitor, _cx: &App) -> Arc<RenderImage> {
		// A 2×2 transparent frame, so frame-consuming defaults
		// (save_frame) run instead of panicking.
		let samples = [0.0f32; 16];
		Arc::new(crate::oakui::frames::f32_rgba_to_bgra_image(2, 2, &samples))
	}
	fn scope_data(&self, _m: Monitor, _cx: &App) -> ScopeData {
		ScopeData::default()
	}
	fn add_track(&mut self, _kind: TrackKind, _cx: &mut Context<Self>) {}
	fn remove_track(&mut self, _index: usize, _cx: &mut Context<Self>) {}
	fn set_track_height(&mut self, _height: Pixels, _cx: &mut Context<Self>) {}
	fn select_item(&mut self, _id: u64, _cx: &mut Context<Self>) {}
	fn apply_effect_event(&mut self, _e: &EffectStackEvent, _cx: &mut Context<Self>) {}
	fn apply_node_graph_event(&mut self, _e: &NodeGraphEvent, _cx: &mut Context<Self>) {}
	fn apply_timeline_event(&mut self, _e: &TimelineEvent, _cx: &mut Context<Self>) {}
	fn split_clip(&mut self, _clip: ClipId, _time: Frame, _cx: &mut Context<Self>) {}
	fn split_at_playhead(&mut self, _cx: &mut Context<Self>) {}
	fn delete_clip(&mut self, _clip: ClipId, _ripple: bool, _cx: &mut Context<Self>) {}
	fn can_undo(&self) -> bool {
		false
	}
	fn can_redo(&self) -> bool {
		false
	}
	fn undo(&mut self, _cx: &mut Context<Self>) {}
	fn redo(&mut self, _cx: &mut Context<Self>) {}
	fn new_project(&mut self, _cx: &mut Context<Self>) {}
	fn open_project_path(&mut self, _path: PathBuf, _cx: &mut Context<Self>) -> Result<(), String> {
		Err("not supported".into())
	}
	fn export_project_path(
		&mut self,
		_path: PathBuf,
		_cx: &mut Context<Self>,
	) -> Result<(), String> {
		Err("not supported".into())
	}
	fn close_project(&mut self, _cx: &mut Context<Self>) {}
	fn start_export(&mut self, _format: i32, _path: PathBuf) -> Result<ExportSession, String> {
		Err("not supported".into())
	}
	fn start_export_with(
		&mut self,
		_settings: &ExportSettings,
		_path: PathBuf,
	) -> Result<ExportSession, String> {
		Err("not supported".into())
	}
	fn add_default_transition(
		&mut self,
		_clips: Vec<ClipId>,
		_cx: &mut Context<Self>,
	) -> Result<usize, String> {
		Err("not supported".into())
	}
	fn backend_name(&self) -> &'static str {
		"null"
	}
}

#[gpui_kit::test]
async fn app_engine_defaults_degrade_gracefully(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = app.new(NullEngine::create);

		// Gateway defaults.
		assert_eq!(engine.read(app).source_media_name(), "");

		// Add-menu / inspector / OFX defaults.
		assert_eq!(engine.read(app).selected_graph_node(), None);
		assert!(engine.read(app).protected_graph_nodes().is_empty());
		assert!(engine.read(app).addable_effects().is_empty());
		assert!(engine.read(app).effect_params(EffectId(0)).is_none());
		assert_eq!(engine.read(app).ofx_interact_target(app), None);
		assert!(engine.read(app).addable_effects().is_empty());

		// History defaults.
		assert!(engine.read(app).history_entries().is_empty());
		assert_eq!(engine.read(app).history_index(), 0);
		assert!(!engine.read(app).storage_bound());
		assert_eq!(engine.read(app).storage_last_error(), None);
		assert_eq!(engine.read(app).waveform_cache().map(|_| ()), None);

		// Footage defaults.
		assert_eq!(engine.read(app).entry_path(1), None);
		assert_eq!(
			engine
				.read(app)
				.project_entry_name(1)
				.map(|s| s.to_string()),
			None
		);
		assert_eq!(engine.read(app).footage_length_frames(1), None);
		assert_eq!(engine.read(app).footage_video_params(1), None);
		assert_eq!(engine.read(app).workarea(), None);

		// Unsupported / no-op mutators return their error strings.
		engine.update(app, |engine, cx| {
			assert!(engine.add_adjustment_layer(0, Frame::ZERO, cx).is_err());
			assert!(engine.add_effect(0, "no.such.effect", cx).is_err());
			assert!(
				engine
					.set_effect_param(EffectId(0), "p", EffectValue::None, cx)
					.is_err()
			);
			assert!(engine.effect_push_button(EffectId(0), "p", cx).is_err());
			assert!(
				engine
					.add_node_at("no.such.node", Point::default(), cx)
					.is_err()
			);
			assert!(
				engine
					.replace_footage(1, PathBuf::from("/tmp/x.mp4"), cx)
					.is_err()
			);
			assert!(engine.library_projects().is_err());
			assert!(engine.library_create_project("x", cx).is_err());
			assert!(engine.library_open_project("u", cx).is_err());
			assert!(engine.library_delete_project("u").is_err());
			assert!(engine.library_rename_project("u", "n").is_err());
			assert!(engine.library_duplicate_project("u").is_err());
			assert!(engine.save_project_as("x", cx).is_err());
			assert!(
				engine
					.library_import_project(PathBuf::from("/tmp/x.ove"))
					.is_err()
			);
			assert!(
				engine
					.library_export_project("u", PathBuf::from("/tmp/x.ove"))
					.is_err()
			);
			assert!(
				engine
					.import_footage(PathBuf::from("/tmp/x.mp4"), cx)
					.is_ok()
			);

			// All panelless operations are silent no-ops.
			engine.set_selected_clips(vec![ClipId(1), ClipId(2)], cx);
			engine.rename_entry(1, "name".into(), cx);
			engine.delete_entry(1, cx);
			engine.add_marker_at_playhead(cx);
			engine.remove_marker_at_playhead(cx);
			engine.set_workarea_preview(Frame(1), Frame(2), cx);
			engine.commit_workarea(Frame(0), Frame(1), Frame(2), Frame(3), cx);
			engine.clear_workarea(cx);
			engine.clipboard_copy(vec![ClipId(1)], cx);
			engine.clipboard_cut(vec![ClipId(1)], cx);
			engine.clipboard_paste(cx);
			engine.jump_history(-1, cx);
			engine.drop_footage(1, TrackKind::Video, 0, Frame::ZERO, cx);
		});

		// `delete_empty_tracks` walks the (empty) track list and stops.
		engine.update(app, |engine, cx| engine.delete_empty_tracks(cx));

		assert!(engine.read(app).node_library().is_empty());
	});
}

/// The remaining `AppEngine` defaults: project/library/proxy/sync/
/// multicam/eyedropper fallbacks that a minimal engine must survive.
#[gpui_kit::test]
async fn app_engine_remaining_defaults_are_safe(cx: &mut TestAppContext) {
	let _lock = crate::oakui::graphops::test_lock();
	cx.update(|app| {
		let engine = app.new(NullEngine::create);

		// Read-only fallbacks.
		assert!(engine.read(app).sequence_entries().is_empty());
		assert_eq!(engine.read(app).current_sequence_id(), None);
		assert_eq!(engine.read(app).sequence_parameters(1), None);
		assert!(!engine.read(app).entry_is_sequence(1));
		assert!(engine.read(app).proxy_rows().is_empty());
		assert_eq!(engine.read(app).proxy_state(1), None);
		assert!(engine.read(app).proxy_row(1).is_none());
		assert_eq!(engine.read(app).proxy_task_progress(), None);
		assert_eq!(engine.read(app).proxy_custom_params(1), None);
		assert!(engine.read(app).clip_footage_entries(&[]).is_empty());
		assert_eq!(engine.read(app).multicam_state(), None);
		assert!(!engine.read(app).multicam_eligible(&[]));
		assert!(!engine.read(app).multicam_enabled_on_selection(&[]));
		assert_eq!(engine.read(app).multicam_wizard_footage(), None);
		assert!(engine.read(app).multicam_wizard_sync_offsets(&[]).is_err());
		assert!(!engine.read(app).eyedropper_armed(app));
		let _ =
			engine
				.read(app)
				.save_frame(Monitor::Program, PathBuf::from("/tmp/oak-null.png"), app);

		// Config-backed getters; the setters write their compiled-in
		// defaults back so the process config stays neutral.
		let _ = engine.read(app).use_proxy_media();
		let _ = engine.read(app).playback_divider();
		let _ = engine.read(app).stop_on_last();
		let _ = engine.read(app).waveform_mode();
		let _ = engine.read(app).project_ocio_config();
		let _ = engine.read(app).project_cache_location();
		let _ = engine.read(app).project_color_settings();
		let _ = engine.read(app).sync_eligibility(&[]);
		let _ = engine.read(app).proxy_effective_params(1);

		engine.update(app, |engine, cx| {
			engine.set_use_proxy_media(false, cx);
			engine.set_playback_divider(1, cx);
			engine.set_stop_on_last(false, cx);
			engine.set_waveform_mode(0, cx);
			assert!(engine.set_project_ocio_config(String::new(), cx).is_ok());
			engine.set_project_cache_location(0, String::new(), cx);
			engine.set_project_color_settings(String::new(), String::new(), String::new(), cx);

			// Unsupported operations report errors instead of panicking.
			assert!(
				engine
					.start_export_of(1, &ExportSettings::default(), PathBuf::from("/tmp/x.mp4"))
					.is_err()
			);
			assert!(
				engine
					.create_sequence_with_params("s".into(), VideoFormat::hd_1080p25(), false, cx,)
					.is_err()
			);
			assert!(engine.create_folder("f".into(), cx).is_err());
			assert!(engine.create_text_footage(cx).is_err());
			assert!(engine.drop_generator_clip("t", 0, Frame::ZERO, cx).is_err());
			assert!(
				engine
					.drop_transition_at("t", 0, Frame::ZERO, 25, cx)
					.is_err()
			);
			assert!(
				engine
					.update_sequence_parameters(1, "n".into(), VideoFormat::hd_1080p25(), false, cx,)
					.is_err()
			);
			engine.open_sequence_id(1, cx);

			assert!(engine.proxy_generate(1, cx).is_err());
			engine.proxy_delete(1, cx);
			engine.proxy_set_enabled(1, true, cx);
			engine.proxy_reveal(1);
			engine.proxy_set_custom_params(
				1,
				ProxyParamsUi {
					width: 0,
					height: 0,
					divider: 1,
					crf: 0,
					preset: String::new(),
					include_audio: false,
				},
				cx,
			);
			engine.proxy_clear_custom_params(1, cx);

			engine.sync_clips_by_source_time(Vec::new(), cx);
			engine.sync_clips_by_waveform(Vec::new(), false, cx);
			engine.toggle_clip_links(Vec::new(), cx);
			engine.multicam_enable_selected(Vec::new(), true, cx);
			engine.multicam_enable_selected(Vec::new(), false, cx);
			engine.multicam_switch_to(0, false, cx);
			assert!(engine.multicam_angle_frame(0, cx).is_none());
			assert!(
				engine
					.multicam_create_sequence(Vec::new(), Vec::new(), "m".into(), cx)
					.is_err()
			);

			// Eyedropper mailbox round trip.
			engine.set_eyedropper_armed(true, cx);
			assert!(engine.eyedropper_armed(cx));
			engine.eyedropper_picked(gpui_kit::rgba(0x11223344), cx);
			assert!(engine.take_eyedropper_result(cx).is_some());
			engine.set_eyedropper_armed(false, cx);
			assert!(engine.take_eyedropper_result(cx).is_none());
		});
	});
}
