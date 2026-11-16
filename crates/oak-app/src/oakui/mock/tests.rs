use super::*;
use crate::widgets::timeline::TrimEdge;
use gpui_kit::TestAppContext;

fn demo_engine(app: &mut gpui_kit::App) -> Entity<MockEngine> {
	app.new(MockEngine::demo)
}

#[gpui_kit::test]
async fn demo_project_has_a_sequence_and_four_tracks(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		let engine = engine.read(app);
		let sequence = engine.current_sequence().expect("demo sequence");
		assert_eq!(sequence.name, "第一稿");
		assert_eq!(sequence.length, Frame(SEQUENCE_LENGTH));
		assert_eq!(sequence.format.width, 1920);
		assert_eq!(sequence.format.height, 1080);
		assert_eq!(engine.track_count(), 4);
		assert_eq!(engine.track(0).expect("V2").kind(), TrackKind::Video);
		assert_eq!(engine.track(2).expect("A1").kind(), TrackKind::Audio);
	});
}

#[gpui_kit::test]
async fn gateway_play_starts_the_program_clock(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |engine, cx| {
			EngineGateway::play(engine, Monitor::Program, cx);
		});
		let clock = engine.read(app).program_clock.read(app);
		assert!(clock.transport.is_playing());
	});
}

#[gpui_kit::test]
async fn gateway_step_moves_the_clock_within_the_sequence(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |engine, cx| {
			engine.step(Monitor::Program, 25, cx);
		});
		let frame = engine.read(app).clock_frame(Monitor::Program, app);
		assert_eq!(frame, Frame(25));

		// Stepping far past the end clamps to the last frame.
		engine.update(app, |engine, cx| {
			engine.step(Monitor::Program, 1_000_000, cx);
		});
		let frame = engine.read(app).clock_frame(Monitor::Program, app);
		assert_eq!(frame, Frame(SEQUENCE_LENGTH - 1));
	});
}

#[gpui_kit::test]
async fn gateway_request_frame_seeks_and_pauses_stays(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |engine, cx| {
			engine.request_frame(Monitor::Source, Frame(42), cx);
		});
		let frame = engine.read(app).clock_frame(Monitor::Source, app);
		assert_eq!(frame, Frame(42));
	});
}

/// The stop-on-last tick pauses on the final frame instead of wrapping.
#[test]
fn clock_tick_stops_on_the_last_frame_when_asked() {
	use std::time::{Duration, Instant};
	let mut clock = MockClock::new(FrameRate::new(30, 1));
	clock.play();
	// 10 s at 30 fps = 300 frames into a 5-frame sequence: wrapped 60×.
	clock.started = Some((Instant::now() - Duration::from_secs(10), Frame(0)));

	clock.tick(Frame(5), true);
	assert_eq!(
		clock.transport.frame(),
		Frame(4),
		"pinned to the last frame"
	);
	assert!(!clock.transport.is_playing(), "playback stopped");
	// A later tick is a no-op: the anchor is cleared.
	clock.tick(Frame(5), true);
	assert_eq!(clock.transport.frame(), Frame(4));
}

/// Without stop-on-last the tick wraps modulo the sequence length.
#[test]
fn clock_tick_loops_when_not_stopping() {
	use std::time::{Duration, Instant};
	let mut clock = MockClock::new(FrameRate::new(30, 1));
	clock.play();
	clock.started = Some((Instant::now() - Duration::from_secs(10), Frame(0)));

	clock.tick(Frame(5), false);
	assert_eq!(clock.transport.frame(), Frame(0), "300 % 5 = 0");
	assert!(clock.transport.is_playing(), "still playing while looping");
}

#[gpui_kit::test]
async fn effect_stack_edit_applies_to_the_model(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		// Remove the OCIO LUT card (id 2).
		engine.update(app, |engine, cx| {
			engine.apply_effect_event(&EffectStackEvent::RemoveRequested(EffectId(2)), cx);
		});
		let stack = engine.read(app).effects();
		assert_eq!(stack.len(), 3);
		let titles: Vec<_> = stack.iter().map(|e| e.title().to_string()).collect();
		assert!(!titles.contains(&"OCIO LUT".to_string()));
		// Source and output cards are pinned and not removable.
		assert_eq!(titles.first().map(String::as_str), Some("媒体"));
		assert_eq!(titles.last().map(String::as_str), Some("输出"));
	});
}

/// The card-header click round trip: expansion toggles collapse then
/// re-expand. The header click emits BOTH `ExpansionToggled` and
/// `CardSelected`; the card selection mirrors its node into the node
/// editor, whose `SelectionChanged` echo must not fight the toggle (the
/// demo's 变换 card maps to graph node 2).
#[gpui_kit::test]
async fn effect_card_expansion_survives_the_card_selection_echo(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		let toggle = |app: &mut gpui_kit::App, expanded: bool| {
			engine.update(app, |engine, cx| {
				engine.apply_effect_event(
					&EffectStackEvent::ExpansionToggled {
						effect: EffectId(1),
						expanded,
					},
					cx,
				);
			});
		};
		let is_expanded = |app: &gpui_kit::App| -> bool {
			engine
				.read(app)
				.effects()
				.iter()
				.find(|e| e.id() == EffectId(1))
				.map(|e| e.is_expanded())
				.unwrap_or(false)
		};

		// The demo's 变换 card starts expanded; the collapse click also
		// selects the card (CardSelected → node_selection = {2}), and the
		// node-editor panel then mirrors that into the graph widget, whose
		// SelectionChanged echo arrives through apply_node_graph_event.
		assert!(is_expanded(app), "the demo card starts expanded");
		toggle(app, false);
		engine.update(app, |engine, cx| {
			engine.apply_effect_event(
				&EffectStackEvent::CardSelected {
					effect: EffectId(1),
				},
				cx,
			);
		});
		engine.update(app, |engine, cx| {
			engine.apply_node_graph_event(
				&NodeGraphEvent::SelectionChanged {
					nodes: BTreeSet::from([NodeId(2)]),
				},
				cx,
			);
		});
		assert!(
			!is_expanded(app),
			"the selection echo must not re-expand the collapsed card"
		);

		// Expand → collapse round trip stays clean.
		toggle(app, true);
		assert!(is_expanded(app));
		toggle(app, false);
		assert!(!is_expanded(app));
	});
}

#[gpui_kit::test]
async fn timeline_edits_are_applied_to_the_mock_model(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);

		// Trim the V1 "B-roll.mp4" clip (id 12, 240–600) in by 40 frames.
		engine.update(app, |engine, cx| {
			engine.apply_timeline_event(
				&TimelineEvent::ClipTrimRequested {
					clip: ClipId(12),
					edge: TrimEdge::Start,
					new_frame: Frame(280),
				},
				cx,
			);
		});
		let v1_track = engine.read(app).track(1).expect("V1");
		let b_roll = v1_track
			.clips()
			.iter()
			.find(|c| c.id() == ClipId(12))
			.expect("B-roll clip");
		assert_eq!(b_roll.range(), FrameRange::new(Frame(280), Frame(600)));
		assert_eq!(b_roll.media_in(), Frame(140), "media-in follows the trim");

		// Move the 开场 clip (id 11) onto the V2 track at frame 300.
		engine.update(app, |engine, cx| {
			engine.apply_timeline_event(
				&TimelineEvent::ClipMoveRequested {
					clip: ClipId(11),
					new_track: 0,
					new_start: Frame(300),
				},
				cx,
			);
		});
		let v2 = engine.read(app).track(0).expect("V2");
		assert!(
			v2.clips()
				.iter()
				.any(|c| c.id() == ClipId(11) && c.range().start == Frame(300)),
			"开场 moved to V2 at frame 300"
		);
		assert!(
			!engine
				.read(app)
				.track(1)
				.expect("V1")
				.clips()
				.iter()
				.any(|c| c.id() == ClipId(11)),
			"开场 left V1"
		);
	});
}

#[gpui_kit::test]
async fn track_toggle_requests_flip_the_track_flags(cx: &mut TestAppContext) {
	use crate::widgets::timeline::TrackHeaderEvent;
	cx.update(|app| {
		let engine = demo_engine(app);

		// V1 (display index 1) starts unlocked + visible.
		let v1 = engine.read(app).track(1).expect("V1");
		assert!(!v1.is_locked());
		assert!(v1.is_visible());

		engine.update(app, |engine, cx| {
			engine.apply_timeline_event(
				&TimelineEvent::TrackToggleRequested {
					track: 1,
					toggle: TrackHeaderEvent::ToggleLock,
				},
				cx,
			);
		});
		assert!(engine.read(app).track(1).expect("V1").is_locked());

		engine.update(app, |engine, cx| {
			engine.apply_timeline_event(
				&TimelineEvent::TrackToggleRequested {
					track: 1,
					toggle: TrackHeaderEvent::ToggleVisibility,
				},
				cx,
			);
		});
		let v1 = engine.read(app).track(1).expect("V1");
		assert!(!v1.is_visible(), "the visibility toggle hides the track");

		// A1 (display index 2) mutes.
		engine.update(app, |engine, cx| {
			engine.apply_timeline_event(
				&TimelineEvent::TrackToggleRequested {
					track: 2,
					toggle: TrackHeaderEvent::ToggleMute,
				},
				cx,
			);
		});
		assert!(engine.read(app).track(2).expect("A1").is_muted());
	});
}

#[gpui_kit::test]
async fn locked_tracks_reject_clip_edits(cx: &mut TestAppContext) {
	use crate::widgets::timeline::TrackHeaderEvent;
	cx.update(|app| {
		let engine = demo_engine(app);

		// Lock V1 (the B-roll track, display index 1).
		engine.update(app, |engine, cx| {
			engine.apply_timeline_event(
				&TimelineEvent::TrackToggleRequested {
					track: 1,
					toggle: TrackHeaderEvent::ToggleLock,
				},
				cx,
			);
		});

		// Trim, move and delete are all refused.
		engine.update(app, |engine, cx| {
			engine.apply_timeline_event(
				&TimelineEvent::ClipTrimRequested {
					clip: ClipId(12),
					edge: TrimEdge::Start,
					new_frame: Frame(280),
				},
				cx,
			);
			engine.apply_timeline_event(
				&TimelineEvent::ClipMoveRequested {
					clip: ClipId(12),
					new_track: 0,
					new_start: Frame(300),
				},
				cx,
			);
			engine.delete_clip(ClipId(12), false, cx);
		});
		let v1 = engine.read(app).track(1).expect("V1");
		let b_roll = v1
			.clips()
			.iter()
			.find(|c| c.id() == ClipId(12))
			.expect("B-roll survives the rejected delete");
		assert_eq!(
			b_roll.range(),
			FrameRange::new(Frame(240), Frame(600)),
			"trim and move were rejected"
		);
	});
}

#[gpui_kit::test]
async fn add_effect_appends_a_named_card(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		let effects = engine.read(app).addable_effects();
		assert!(
			!effects.is_empty(),
			"the demo has an app-local effect table"
		);
		let first = effects[0].clone();
		let (type_id, name) = (first.type_id, first.name);
		let before = engine.read(app).effects().len();
		engine.update(app, |engine, cx| {
			engine
				.add_effect(usize::MAX, &type_id, cx)
				.expect("add the effect");
		});
		let stack = engine.read(app).effects();
		assert_eq!(stack.len(), before + 1);
		assert_eq!(
			stack.last().map(|e| e.title().to_string()).as_deref(),
			Some(name.as_str()),
			"the card was appended at the chain end"
		);
	});
}

#[gpui_kit::test]
async fn mock_split_at_playhead_and_ripple_delete(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);

		// Park the program playhead inside 开场 (0–240) and split there.
		engine.update(app, |engine, cx| {
			engine.request_frame(Monitor::Program, Frame(120), cx);
			engine.split_at_playhead(cx);
		});
		let v1 = engine.read(app).track(1).expect("V1");
		assert_eq!(v1.clips().len(), 3, "开场 split into two");
		let spanning = v1
			.clips()
			.iter()
			.filter(|c| c.range().start.0 < 120 && 120 < c.range().end.0)
			.count();
		assert_eq!(spanning, 0, "no clip spans the split point");

		// Ripple-delete the second half of 开场 (id 11's split tail).
		let tail_id = v1
			.clips()
			.iter()
			.find(|c| c.range().start == Frame(120))
			.map(|c| c.id())
			.expect("split tail");
		engine.update(app, |engine, cx| {
			engine.delete_clip(tail_id, true, cx);
		});
		let v1 = engine.read(app).track(1).expect("V1");
		assert_eq!(v1.clips().len(), 2);
		// The following B-roll (was 240–600) shifted left by the removed
		// 120-frame tail: now starts at 120.
		let b_roll = v1
			.clips()
			.iter()
			.find(|c| c.id() == ClipId(12))
			.expect("B-roll clip");
		assert_eq!(
			b_roll.range().start,
			Frame(120),
			"ripple shifted B-roll left"
		);
	});
}

#[gpui_kit::test]
async fn demo_graph_has_eight_nodes_and_six_edges(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		let engine = engine.read(app);
		let nodes = engine.nodes();
		let edges = engine.edges();

		assert_eq!(
			nodes.len(),
			8,
			"media, transform, blur, mixer, viewer, and the endpoint pair"
		);
		assert_eq!(edges.len(), 6);

		// The chain ends at the viewer node, fed by the mixer.
		let viewer = nodes
			.iter()
			.find(|n| n.title() == "输出")
			.expect("viewer node");
		assert_eq!(viewer.inputs().len(), 1);
		assert!(viewer.outputs().is_empty());

		// Every edge endpoint references an existing port, and every port
		// connectivity flag matches the edge list.
		for edge in &edges {
			assert!(
				nodes.iter().any(|n| n.id() == edge.from_node()),
				"edge {} from-node exists",
				edge.id().0
			);
			assert!(
				nodes.iter().any(|n| n.id() == edge.to_node()),
				"edge {} to-node exists",
				edge.id().0
			);
		}
		for node in nodes {
			for port in node.inputs().into_iter().chain(node.outputs()) {
				let expected = engine
					.edges()
					.iter()
					.any(|e| e.from_port() == port.id() || e.to_port() == port.id());
				assert_eq!(port.is_connected(), expected, "port {}", port.id().0);
			}
		}
	});
}

#[gpui_kit::test]
async fn can_connect_enforces_direction_type_duplicates_and_cycles(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		let engine = engine.read(app);

		// A valid, still-free connection: Clip1.video → Transform.in (the
		// transform already takes the clip-0 path, so this would be the
		// second input).
		assert!(engine.can_connect(PortId(3), PortId(20)));
		// The existing connection is not offered again.
		assert!(
			!engine.can_connect(PortId(21), PortId(40)),
			"duplicate edge"
		);
		// Input → output is rejected (wrong direction).
		assert!(
			!engine.can_connect(PortId(20), PortId(1)),
			"wrong direction"
		);
		// A port cannot connect to itself.
		assert!(
			!engine.can_connect(PortId(42), PortId(40)),
			"self connection"
		);
		// Type mismatch: the clip's audio output is not a video signal.
		assert!(
			!engine.can_connect(PortId(4), PortId(20)),
			"audio cannot feed a video input"
		);
		// Unknown ports are rejected.
		assert!(!engine.can_connect(PortId(999), PortId(20)));
		assert!(!engine.can_connect(PortId(1), PortId(999)));

		// Cycle rule: the graph flows left-to-right (Clip → Transform →
		// Mixer → Viewer), so no connection can close a loop — but the
		// reachability helper behind the rule is exercised directly.
		assert!(engine.reaches(NodeId(0), NodeId(5)), "main chain path");
		assert!(!engine.reaches(NodeId(5), NodeId(0)), "no backward path");
		assert!(engine.reaches(NodeId(4), NodeId(4)));
	});
}

#[gpui_kit::test]
async fn node_edits_apply_to_the_model(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		// Move the viewer node.
		engine.update(app, |engine, cx| {
			engine.apply_node_graph_event(
				&NodeGraphEvent::NodeMoveRequested {
					nodes: vec![NodeId(5)],
					delta: point(px(100.0), px(-20.0)),
				},
				cx,
			);
		});
		let moved = engine.read(app).node(NodeId(5)).expect("viewer node");
		assert_eq!(moved.position(), point(px(980.0), px(110.0)));

		// Connect the B-roll clip's video to the transform's unused mask
		// input, then disconnect the transform → mixer edge.
		engine.update(app, |engine, cx| {
			engine.apply_node_graph_event(
				&NodeGraphEvent::ConnectionRequested {
					from: PortId(3),
					to: PortId(22),
				},
				cx,
			);
		});
		let engine_read = engine.read(app);
		assert_eq!(engine_read.edges().len(), 7);
		assert!(
			engine_read
				.port(PortId(22))
				.is_some_and(|p| p.is_connected())
		);
		let edges = engine_read.edges();
		let extra = edges
			.iter()
			.find(|e| e.to_port() == PortId(22))
			.expect("the new connection");

		engine.update(app, |engine, cx| {
			engine.apply_node_graph_event(
				&NodeGraphEvent::DisconnectionRequested { edge: extra.id() },
				cx,
			);
		});
		assert_eq!(engine.read(app).edges().len(), 6);
		assert!(
			!engine
				.read(app)
				.port(PortId(22))
				.is_some_and(|p| p.is_connected()),
			"mask input freed again"
		);

		// Deleting the blur node takes its incident edges with it.
		engine.update(app, |engine, cx| {
			engine.apply_node_graph_event(
				&NodeGraphEvent::DeleteRequested {
					nodes: vec![NodeId(3)],
					edges: vec![],
				},
				cx,
			);
		});
		let engine_read = engine.read(app);
		assert_eq!(engine_read.nodes().len(), 7);
		assert!(
			!engine_read
				.edges()
				.iter()
				.any(|e| e.to_node() == NodeId(3) || e.from_node() == NodeId(3)),
			"edges incident to the deleted node are removed"
		);
	});
}

#[gpui_kit::test]
async fn cpu_frame_is_cached_per_playhead_and_advances(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);

		// Same playhead → same cached image (Arc identity), so a paused
		// viewer never regenerates its picture.
		let a = engine.read(app).cpu_frame(Monitor::Program, app);
		let b = engine.read(app).cpu_frame(Monitor::Program, app);
		assert!(Arc::ptr_eq(&a, &b), "paused frame must be cached");

		// Advancing the playhead produces a different image: the sweep
		// moved, so playback is visible.
		engine.update(app, |engine, cx| {
			engine.step(Monitor::Program, 25, cx);
		});
		let c = engine.read(app).cpu_frame(Monitor::Program, app);
		assert!(!Arc::ptr_eq(&a, &c), "a new playhead frame regenerates");

		// The frame has the documented proxy size and opaque BGRA8 bytes.
		let size = c.size(0);
		assert_eq!(size.width, crate::oakui::frames::SYNTH_FRAME_WIDTH.into());
		assert_eq!(size.height, crate::oakui::frames::SYNTH_FRAME_HEIGHT.into());
		let bytes = c.as_bytes(0).expect("single frame");
		assert_eq!(
			bytes.len(),
			(crate::oakui::frames::SYNTH_FRAME_WIDTH * crate::oakui::frames::SYNTH_FRAME_HEIGHT * 4)
				as usize
		);
		assert!(bytes.chunks_exact(4).all(|px| px[3] == 255), "opaque alpha");
	});
}

// -----------------------------------------------------------------------
// Sequence markers & work area (M12 P4): the mock state the 序列 menu
// actions and the timeline ruler drag drive.
// -----------------------------------------------------------------------

#[gpui_kit::test]
async fn marker_menu_actions_add_and_remove_at_playhead(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		// Park the playhead at frame 40 (the program monitor's clock).
		engine.update(app, |engine, cx| {
			engine.request_frame(Monitor::Program, Frame(40), cx);
		});

		// 序列 → 添加标记: a marker appears at the playhead.
		engine.update(app, |engine, cx| engine.add_marker_at_playhead(cx));
		let markers = engine.read(app).markers();
		assert_eq!(markers.len(), 1);
		assert_eq!(markers[0].frame, Frame(40));

		// Adding again at the same frame is idempotent for the menu.
		engine.update(app, |engine, cx| engine.add_marker_at_playhead(cx));
		assert_eq!(engine.read(app).markers().len(), 1);

		// 序列 → 清除标记 removes it.
		engine.update(app, |engine, cx| engine.remove_marker_at_playhead(cx));
		assert!(engine.read(app).markers().is_empty());
	});
}

#[gpui_kit::test]
async fn workarea_menu_actions_set_and_clear(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		assert_eq!(engine.read(app).workarea(), None);

		// The menu's commit path (with an explicit old range).
		engine.update(app, |engine, cx| {
			engine.commit_workarea(Frame(0), Frame(100), Frame(20), Frame(80), cx);
		});
		assert_eq!(engine.read(app).workarea(), Some((Frame(20), Frame(80))));

		// The ruler-drag preview path (live, non-undoable).
		engine.update(app, |engine, cx| {
			engine.set_workarea_preview(Frame(25), Frame(75), cx);
		});
		assert_eq!(engine.read(app).workarea(), Some((Frame(25), Frame(75))));

		// 序列 → 清除工作区 disables it.
		engine.update(app, |engine, cx| engine.clear_workarea(cx));
		assert_eq!(engine.read(app).workarea(), None);
	});
}

#[gpui_kit::test]
async fn timeline_workarea_events_drive_the_mock_engine(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		// The timeline widget's events land on the engine through
		// apply_timeline_event (the app shell's subscription).
		engine.update(app, |engine, cx| {
			engine.apply_timeline_event(
				&TimelineEvent::WorkAreaPreview {
					start: Frame(30),
					end: Frame(90),
				},
				cx,
			);
			engine.apply_timeline_event(
				&TimelineEvent::WorkAreaCommitted {
					start: Frame(30),
					end: Frame(90),
					old_start: Frame::ZERO,
					old_end: Frame(100),
				},
				cx,
			);
		});
		assert_eq!(engine.read(app).workarea(), Some((Frame(30), Frame(90))));
	});
}

/// 添加文本素材 on the mock: every call gets a fresh entry id, the ids
/// are listed by the bin as leaves labelled with the shared `文本`
/// node label (the drop path resolves them by name).
#[gpui_kit::test]
async fn create_text_footage_lists_a_text_entry(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		let first = engine.update(app, |engine, cx| {
			engine.create_text_footage(cx).expect("create text footage")
		});
		let second = engine.update(app, |engine, cx| {
			engine.create_text_footage(cx).expect("create text footage")
		});
		assert_ne!(first, second, "each action gets a fresh entry id");
		assert_eq!(engine.read(app).text_footages(), &[first, second]);
		let listed: Vec<_> = engine
			.read(app)
			.roots()
			.into_iter()
			.filter(|e| e.id == first || e.id == second)
			.collect();
		assert_eq!(listed.len(), 2, "both text entries are listed in the bin");
		assert!(
			listed
				.iter()
				.all(|e| e.name == TEXT_FOOTAGE_LABEL && !e.is_dir),
			"the entries carry the shared 文本 label as leaves"
		);
	});
}

// ---- settings / proxy / sync / multicam paths ------------------------

use crate::oakui::engine::ProxyMediaState;

/// Settings round trips: proxy toggle, OCIO config validation and the
/// cache-location clamp/trim rules.
#[gpui_kit::test]
async fn settings_and_cache_location_round_trip(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |e, cx| {
			let use_proxy = e.use_proxy_media();
			e.set_use_proxy_media(!use_proxy, cx);
			assert_eq!(e.use_proxy_media(), !use_proxy);

			// An empty path clears the override; a bogus path is
			// rejected like the real engine's OCIO validation.
			assert!(e.set_project_ocio_config(String::new(), cx).is_ok());
			assert_eq!(e.project_ocio_config(), "");
			assert!(
				e.set_project_ocio_config("/definitely/not/a/config".into(), cx)
					.is_err()
			);

			// Setting 2 keeps the trimmed custom path; other settings
			// clear it, and out-of-range codes clamp to 0..=2.
			e.set_project_cache_location(2, "  /tmp/oak-cache  ".into(), cx);
			assert_eq!(
				e.project_cache_location(),
				(2, "/tmp/oak-cache".to_string())
			);
			// 9 clamps to 2 (custom) and keeps the trimmed path...
			e.set_project_cache_location(9, "  /ignored  ".into(), cx);
			assert_eq!(e.project_cache_location(), (2, "/ignored".to_string()));
			// ...while a negative code clamps to 0 and clears it.
			e.set_project_cache_location(-5, "/ignored".into(), cx);
			assert_eq!(e.project_cache_location(), (0, String::new()));
		});
	});
}

/// Proxy rows/states/generation/deletion and the custom-parameter
/// storage paths.
#[gpui_kit::test]
async fn proxy_paths(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |e, cx| {
			let rows = e.proxy_rows();
			assert!(!rows.is_empty(), "the demo library lists footage");
			let video = rows
				.iter()
				.find(|row| row.can_generate)
				.cloned()
				.expect("a video row");
			let audio = rows.iter().find(|row| !row.can_generate).cloned();

			assert_eq!(e.proxy_state(u64::MAX), None);
			assert!(e.proxy_generate(u64::MAX, cx).is_err());
			assert!(e.proxy_row(u64::MAX).is_none());
			if let Some(audio) = &audio {
				assert!(
					e.proxy_generate(audio.id, cx).is_err(),
					"audio-only footage cannot generate a proxy"
				);
			}

			// Generate → ready/enabled, then flip and delete.
			assert_eq!(e.proxy_state(video.id), Some(ProxyMediaState::Missing));
			e.proxy_generate(video.id, cx).expect("proxy generates");
			assert_eq!(e.proxy_state(video.id), Some(ProxyMediaState::Ready));
			let row = e.proxy_row(video.id).expect("the row reflects the state");
			assert!(row.has_proxy && row.enabled);

			let params = crate::oakui::engine::ProxyParamsUi {
				width: 320,
				height: 180,
				divider: 1,
				crf: 18,
				preset: "veryfast".into(),
				include_audio: true,
			};
			e.proxy_set_custom_params(video.id, params.clone(), cx);
			assert_eq!(e.proxy_custom_params(video.id), Some(params));
			assert!(e.proxy_row(video.id).unwrap().has_custom);
			e.proxy_clear_custom_params(video.id, cx);
			assert_eq!(e.proxy_custom_params(video.id), None);
			assert!(!e.proxy_row(video.id).unwrap().has_custom);

			e.proxy_set_enabled(video.id, false, cx);
			assert!(!e.proxy_row(video.id).unwrap().enabled);
			e.proxy_reveal(video.id);

			e.proxy_delete(video.id, cx);
			assert_eq!(e.proxy_state(video.id), Some(ProxyMediaState::Missing));
		});
	});
}

/// Project lifecycle mutators, generator/footage drops, text footage,
/// the work area and every effect-stack event arm.
#[gpui_kit::test]
async fn engine_mutator_and_effect_event_paths(cx: &mut TestAppContext) {
	let _lock = crate::oakui::graphops::test_lock();
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |e, cx| {
			// Project lifecycle.
			e.new_project(cx);
			assert!(
				e.open_project_path(std::path::PathBuf::from("/tmp/demo.ove"), cx)
					.is_ok()
			);
			assert_eq!(e.project.path, std::path::PathBuf::from("/tmp/demo.ove"));
			assert!(
				e.import_footage(std::path::PathBuf::from("/tmp/clip.mp4"), cx)
					.is_ok()
			);
			assert!(e.imported_footage.iter().any(|p| p.ends_with("clip.mp4")));

			let text = e.create_text_footage(cx).expect("text footage");
			assert!(e.text_footages.contains(&text));

			// Work area preview + clear.
			e.set_workarea_preview(Frame(1), Frame(9), cx);
			assert_eq!(e.workarea(), Some((Frame(1), Frame(9))));
			e.clear_workarea(cx);
			assert_eq!(e.workarea(), None);

			// Generator drop: the label resolves through the effect
			// table and an out-of-range track auto-selects video.
			let generator = MockEngine::demo_effect_library()
				.into_iter()
				.next()
				.expect("an addable generator");
			e.drop_generator_clip(&generator.type_id, usize::MAX, Frame(3), cx)
				.expect("auto-selects a video track");
			assert!(
				e.tracks
					.iter()
					.any(|t| t.kind == TrackKind::Video && !t.clips.is_empty())
			);
			assert!(e.drop_transition_at("x", 0, Frame(0), 25, cx).is_ok());

			// Footage drops: an unknown id is a no-op; audio footage
			// auto-selects an audio track even when pointed at video.
			let rows = e.proxy_rows();
			let video = rows
				.iter()
				.find(|row| row.can_generate)
				.cloned()
				.expect("a video entry");
			let audio = rows.iter().find(|row| !row.can_generate).cloned();
			e.drop_footage(u64::MAX, TrackKind::Video, 0, Frame(0), cx);
			e.drop_footage(video.id, TrackKind::Video, usize::MAX, Frame(0), cx);
			if let Some(audio) = audio {
				e.drop_footage(audio.id, TrackKind::Video, 0, Frame(0), cx);
			}

			// Effect-stack events.
			let before = e.effects.len();
			e.apply_effect_event(&EffectStackEvent::AddRequested { index: usize::MAX }, cx);
			assert_eq!(e.effects.len(), before + 1);
			let id = e.effects.last().unwrap().id;
			e.apply_effect_event(
				&EffectStackEvent::AddTypeRequested {
					index: 0,
					type_id: "Invert".into(),
				},
				cx,
			);
			assert_eq!(e.effects.first().unwrap().title, "Invert");

			e.apply_effect_event(
				&EffectStackEvent::EnableToggled {
					effect: id,
					enabled: false,
				},
				cx,
			);
			assert!(!e.effects.last().unwrap().enabled);
			// Unknown ids are ignored.
			e.apply_effect_event(
				&EffectStackEvent::EnableToggled {
					effect: EffectId(9999),
					enabled: true,
				},
				cx,
			);
			e.apply_effect_event(
				&EffectStackEvent::ExpansionToggled {
					effect: id,
					expanded: true,
				},
				cx,
			);
			assert!(e.effects.last().unwrap().expanded);
			e.apply_effect_event(
				&EffectStackEvent::ExpansionToggled {
					effect: EffectId(9999),
					expanded: false,
				},
				cx,
			);

			e.apply_effect_event(&EffectStackEvent::CardSelected { effect: id }, cx);
			e.apply_effect_event(
				&EffectStackEvent::ReorderRequested {
					effect: id,
					new_index: usize::MAX,
				},
				cx,
			);
			assert_eq!(e.effects.last().unwrap().id, id);
			e.apply_effect_event(
				&EffectStackEvent::ReorderRequested {
					effect: EffectId(9999),
					new_index: 0,
				},
				cx,
			);
			e.apply_effect_event(
				&EffectStackEvent::ContextMenuRequested {
					effect: id,
					position: gpui_kit::Point::new(px(0.0), px(0.0)),
				},
				cx,
			);
			e.apply_effect_event(&EffectStackEvent::ParameterChanged { effect: id }, cx);
			// Unknown ids are ignored; a known removable card goes.
			e.apply_effect_event(&EffectStackEvent::RemoveRequested(EffectId(9999)), cx);
			e.apply_effect_event(&EffectStackEvent::RemoveRequested(id), cx);
		});
	});
}

/// Sync eligibility/commands, clip→footage mapping, the multicam
/// surface and the demo's unsupported default transition.
#[gpui_kit::test]
async fn sync_and_multicam_paths(cx: &mut TestAppContext) {
	let _lock = crate::oakui::graphops::test_lock();
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |e, cx| {
			let clips: Vec<ClipId> = e
				.track(0)
				.map(|track| track.clips().iter().map(|clip| clip.id()).collect())
				.unwrap_or_default();
			let eligibility = e.sync_eligibility(&clips);
			assert!(eligibility.source_time <= clips.len());
			e.sync_clips_by_source_time(clips.clone(), cx);
			e.sync_clips_by_waveform(clips.clone(), false, cx);
			e.sync_clips_by_waveform(clips.clone(), true, cx);
			let entries = e.clip_footage_entries(&clips);
			assert!(entries.len() <= clips.len());

			if let Some(state) = e.multicam_state() {
				assert!(state.source_count >= 1);
				let _ = e.multicam_angle_frame(0, cx);
				let _ = e.multicam_angle_frame(-1, cx);
				assert!(e.multicam_eligible(&clips));
				assert!(e.multicam_enabled_on_selection(&clips));
				e.multicam_switch_to(0, false, cx);
				e.multicam_switch_to(99, false, cx);
				e.multicam_switch_to(-1, true, cx);
				e.multicam_enable_selected(vec![], true, cx);
				e.multicam_enable_selected(vec![], false, cx);
			}
			assert!(e.add_default_transition(vec![], cx).is_err());
		});
	});
}

/// The node editor's ignored events, background click, unmapped
/// selections and the selection helpers.
#[gpui_kit::test]
async fn node_graph_ignored_events_selection_and_reach_cycles(cx: &mut TestAppContext) {
	let _lock = crate::oakui::graphops::test_lock();
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |e, cx| {
			// Fire-and-forget events: preview, viewport, context menu.
			e.apply_node_graph_event(
				&NodeGraphEvent::NodeMovePreview {
					nodes: vec![NodeId(5)],
					delta: point(px(4.0), px(4.0)),
				},
				cx,
			);
			e.apply_node_graph_event(
				&NodeGraphEvent::ViewChanged {
					offset: point(px(10.0), px(10.0)),
					zoom: 1.5,
				},
				cx,
			);
			e.apply_node_graph_event(
				&NodeGraphEvent::NodeContextMenuRequested {
					node: NodeId(5),
					position: point(px(0.0), px(0.0)),
				},
				cx,
			);
			// A background click is logged and ignored.
			e.apply_node_graph_event(
				&NodeGraphEvent::BackgroundClicked {
					position: point(px(3.0), px(3.0)),
				},
				cx,
			);

			// Selection helpers: no selection, then a node with no
			// matching effect card (NodeId 0 has no 第一稿.mp4 card).
			assert!(e.selected_node_ids().is_empty());
			assert_eq!(e.selected_effect(), None);
			assert_eq!(e.selected_graph_node(), None);
			e.apply_node_graph_event(
				&NodeGraphEvent::SelectionChanged {
					nodes: BTreeSet::from([NodeId(0)]),
				},
				cx,
			);
			assert_eq!(e.selected_node_ids(), &BTreeSet::from([NodeId(0)]));
			assert_eq!(e.selected_graph_node(), Some(0));
			assert_eq!(e.selected_effect(), None, "no card shares the title");

			// Two selected nodes have no single effect target.
			e.apply_node_graph_event(
				&NodeGraphEvent::SelectionChanged {
					nodes: BTreeSet::from([NodeId(2), NodeId(3)]),
				},
				cx,
			);
			assert_eq!(e.selected_effect(), None);
			assert_eq!(e.selected_graph_node(), None);
			e.apply_node_graph_event(
				&NodeGraphEvent::SelectionChanged {
					nodes: BTreeSet::from([NodeId(2)]),
				},
				cx,
			);
			assert_eq!(e.selected_effect(), Some(EffectId(1)), "变换 card");

			// An invalid connection is rejected without touching the
			// edge list.
			let before = e.edges.len();
			e.apply_node_graph_event(
				&NodeGraphEvent::ConnectionRequested {
					from: PortId(999),
					to: PortId(20),
				},
				cx,
			);
			assert_eq!(e.edges.len(), before, "unknown port rejected");

			// Reachability: a manually-added back edge makes the DFS
			// revisit a node (the visited guard).
			e.edges.push(MockEdge {
				id: EdgeId(99),
				from_node: NodeId(5),
				from_port: PortId(50),
				to_node: NodeId(2),
				to_port: PortId(20),
			});
			assert!(!e.reaches(NodeId(0), NodeId(7)), "no path into the sink");
			assert!(e.reaches(NodeId(0), NodeId(2)));
		});
	});
}

/// Every timeline edit event the widget can emit applies to the mock
/// model: playhead, both trim edges, split, ripple trim, roll (valid and
/// rejected), slide, slip, moves, track height and toggles.
#[gpui_kit::test]
async fn timeline_edit_events_apply_to_the_mock_model(cx: &mut TestAppContext) {
	use crate::widgets::timeline::TrackHeaderEvent;
	cx.update(|app| {
		let engine = demo_engine(app);
		let clip = |app: &App, track: usize, id: u64| -> Option<MockClip> {
			engine
				.read(app)
				.track(track)?
				.clips()
				.iter()
				.find(|c| c.id() == ClipId(id))
				.cloned()
		};

		// PlayheadChanged: an equal frame is a no-op, a new one seeks.
		engine.update(app, |e, cx| {
			e.apply_timeline_event(&TimelineEvent::PlayheadChanged(Frame(0)), cx);
			assert_eq!(e.clock_frame(Monitor::Program, cx), Frame(0));
			e.apply_timeline_event(&TimelineEvent::PlayheadChanged(Frame(77)), cx);
			assert_eq!(e.clock_frame(Monitor::Program, cx), Frame(77));

			// Trim the END edge of B-roll (id 12, 240–600).
			e.apply_timeline_event(
				&TimelineEvent::ClipTrimRequested {
					clip: ClipId(12),
					edge: TrimEdge::End,
					new_frame: Frame(700),
				},
				cx,
			);
		});
		assert_eq!(
			clip(app, 1, 12).expect("B-roll").range(),
			FrameRange::new(Frame(240), Frame(700))
		);

		// Ripple-trim 开场 (id 11) in by 20: the following B-roll shifts
		// with the edge.
		engine.update(app, |e, cx| {
			e.apply_timeline_event(
				&TimelineEvent::ClipRippleTrimRequested {
					clip: ClipId(11),
					edge: TrimEdge::Start,
					new_frame: Frame(20),
				},
				cx,
			);
		});
		let clip11 = clip(app, 1, 11).expect("开场");
		assert_eq!(clip11.range().start, Frame(20));
		assert_eq!(clip11.media_in(), Frame(20), "media-in follows");
		assert_eq!(
			clip(app, 1, 12).expect("B-roll").range(),
			FrameRange::new(Frame(260), Frame(720)),
			"the later clip rippled with the edge"
		);

		// Ripple-trim the B-roll's end out to 800: nothing follows it.
		engine.update(app, |e, cx| {
			e.apply_timeline_event(
				&TimelineEvent::ClipRippleTrimRequested {
					clip: ClipId(12),
					edge: TrimEdge::End,
					new_frame: Frame(800),
				},
				cx,
			);
		});
		assert_eq!(
			clip(app, 1, 12).expect("B-roll").range(),
			FrameRange::new(Frame(260), Frame(800))
		);

		// Roll the shared boundary of 开场/B-roll to 300.
		engine.update(app, |e, cx| {
			e.apply_timeline_event(
				&TimelineEvent::ClipRollRequested {
					clip_a: ClipId(11),
					clip_b: ClipId(12),
					new_frame: Frame(300),
				},
				cx,
			);
			// Rejected rolls: unknown clip, boundary outside the span.
			e.apply_timeline_event(
				&TimelineEvent::ClipRollRequested {
					clip_a: ClipId(999),
					clip_b: ClipId(12),
					new_frame: Frame(300),
				},
				cx,
			);
			e.apply_timeline_event(
				&TimelineEvent::ClipRollRequested {
					clip_a: ClipId(11),
					clip_b: ClipId(12),
					new_frame: Frame(10),
				},
				cx,
			);
		});
		assert_eq!(clip(app, 1, 11).expect("开场").range().end, Frame(300));
		assert_eq!(clip(app, 1, 12).expect("B-roll").range().start, Frame(300));

		// A roll across two different tracks is rejected.
		engine.update(app, |e, cx| {
			e.apply_timeline_event(
				&TimelineEvent::ClipRollRequested {
					clip_a: ClipId(11),
					clip_b: ClipId(13),
					new_frame: Frame(100),
				},
				cx,
			);
			// Split request: split 开场 at 150.
			e.apply_timeline_event(
				&TimelineEvent::ClipSplitRequested {
					clip: ClipId(11),
					time: Frame(150),
				},
				cx,
			);
		});
		let v1_clips = engine.read(app).track(1).expect("V1").clips().to_vec();
		assert_eq!(v1_clips.len(), 3, "开场 split in two");
		assert!(
			v1_clips
				.iter()
				.any(|c| c.range() == FrameRange::new(Frame(150), Frame(300)))
		);

		// Slide: the clip keeps its length at the new start.
		engine.update(app, |e, cx| {
			e.apply_timeline_event(
				&TimelineEvent::ClipSlideRequested {
					clip: ClipId(12),
					new_start: Frame(500),
				},
				cx,
			);
			// Unknown clip is ignored.
			e.apply_timeline_event(
				&TimelineEvent::ClipSlideRequested {
					clip: ClipId(999),
					new_start: Frame(0),
				},
				cx,
			);
			// Slip: position stays, media offset moves.
			e.apply_timeline_event(
				&TimelineEvent::ClipSlipRequested {
					clip: ClipId(12),
					new_media_in: Frame(42),
				},
				cx,
			);
			// Unknown clips are ignored by move too.
			e.apply_timeline_event(
				&TimelineEvent::ClipMoveRequested {
					clip: ClipId(999),
					new_track: 0,
					new_start: Frame(0),
				},
				cx,
			);
		});
		let b_roll = clip(app, 1, 12).expect("B-roll");
		assert_eq!(b_roll.range().start, Frame(500), "slid in time");
		assert_eq!(b_roll.media_in(), Frame(42), "slipped the media offset");

		// Track height (valid + unknown index) and toggles (solo, and
		// the unknown-track guard).
		engine.update(app, |e, cx| {
			e.apply_timeline_event(
				&TimelineEvent::TrackHeightChanged {
					track: 1,
					height: px(80.0),
				},
				cx,
			);
			e.apply_timeline_event(
				&TimelineEvent::TrackHeightChanged {
					track: 99,
					height: px(10.0),
				},
				cx,
			);
			e.apply_timeline_event(
				&TimelineEvent::TrackToggleRequested {
					track: 99,
					toggle: TrackHeaderEvent::ToggleLock,
				},
				cx,
			);
			e.apply_timeline_event(
				&TimelineEvent::TrackToggleRequested {
					track: 3,
					toggle: TrackHeaderEvent::ToggleSolo,
				},
				cx,
			);
		});
		assert_eq!(engine.read(app).track(1).expect("V1").height(), px(80.0));
		assert!(engine.read(app).track(3).expect("A2").is_solo());

		// The direct split_clip API: unknown clip, locked track, an
		// out-of-range time and a real split.
		engine.update(app, |e, cx| {
			e.split_clip(ClipId(999), Frame(10), cx);
			e.apply_timeline_event(
				&TimelineEvent::TrackToggleRequested {
					track: 1,
					toggle: TrackHeaderEvent::ToggleLock,
				},
				cx,
			);
			e.split_clip(ClipId(11), Frame(250), cx);
			assert_eq!(
				e.tracks[1].clips.len(),
				3,
				"a locked track rejects the split"
			);
			e.apply_timeline_event(
				&TimelineEvent::TrackToggleRequested {
					track: 1,
					toggle: TrackHeaderEvent::ToggleLock,
				},
				cx,
			);
			e.split_clip(ClipId(11), Frame(0), cx);
			assert_eq!(
				e.tracks[1].clips.len(),
				3,
				"a split on the clip edge is a no-op"
			);
			e.split_clip(ClipId(11), Frame(100), cx);
			assert_eq!(e.tracks[1].clips.len(), 4, "the split landed");
		});
	});
}

/// Track helpers, clip linking, the sync guard/commands and eligibility.
#[gpui_kit::test]
async fn track_link_and_sync_helpers(cx: &mut TestAppContext) {
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |e, cx| {
			// add_track: every kind, including subtitles.
			let before = e.track_count();
			e.add_track(TrackKind::Subtitle, cx);
			assert_eq!(e.track_count(), before + 1);
			assert_eq!(e.tracks.last().unwrap().kind, TrackKind::Subtitle);
			assert_eq!(e.tracks.last().unwrap().height, px(32.0));

			// set_track_height applies to every track.
			e.set_track_height(px(72.0), cx);
			assert!(e.tracks.iter().all(|t| t.height == px(72.0)));

			// Clip links: a single clip is ignored; a pair links
			// (normalized both ways) and a fully-linked pair unlinks.
			e.toggle_clip_links(vec![ClipId(1)], cx);
			assert!(!e.clips_linked(1, 2));
			e.toggle_clip_links(vec![ClipId(1), ClipId(2)], cx);
			assert!(e.clips_linked(1, 2) && e.clips_linked(2, 1));
			e.toggle_clip_links(vec![ClipId(2), ClipId(1)], cx);
			assert!(!e.clips_linked(1, 2), "a linked pair unlinks");

			// Sync: a locked track's clips are skipped; the unlocked
			// ones align to the earliest in point (id 13 is audio-only
			// and id 12 starts later).
			e.tracks[2].locked = true;
			let eligibility = e.sync_eligibility(&[ClipId(11), ClipId(13), ClipId(999)]);
			assert_eq!(
				eligibility.source_time, 1,
				"audio and unknown clips do not count"
			);
			e.sync_clips_by_source_time(vec![ClipId(11), ClipId(13), ClipId(12)], cx);
			assert_eq!(e.tracks[1].clips[1].range.start, Frame(0), "B-roll moved");
			e.tracks[2].locked = false;

			// Sync eligibility for an audio clip alone.
			let eligibility = e.sync_eligibility(&[ClipId(13)]);
			assert_eq!(eligibility.source_time, 0);
		});
	});
}

/// Adjustment layers, generator/footage drops without a target track,
/// `add_effect`'s unknown type, `add_node_at`, the legacy `start_export`
/// wrapper and the demo multicam's create/enable/disable paths.
#[gpui_kit::test]
async fn drop_add_and_legacy_export_paths(cx: &mut TestAppContext) {
	let _lock = crate::oakui::graphops::test_lock();
	cx.update(|app| {
		let engine = demo_engine(app);
		engine.update(app, |e, cx| {
			// Adjustment layers: unknown track, non-video track, then a
			// video track (inserted in ascending start order).
			assert!(e.add_adjustment_layer(99, Frame(0), cx).is_err());
			assert!(e.add_adjustment_layer(2, Frame(0), cx).is_err());
			e.add_adjustment_layer(0, Frame(200), cx)
				.expect("video track accepts");
			assert!(e.tracks[0].clips.iter().any(|c| c.label == "调整图层"));

			// Unknown effect type keeps the stack unchanged.
			let before = e.effects.len();
			assert!(e.add_effect(0, "no-such-effect", cx).is_err());
			assert_eq!(e.effects.len(), before);

			// A generator drop with the factory table unknown falls back
			// to the raw type id as the label.
			e.drop_generator_clip("not-in-the-table", 0, Frame(0), cx)
				.expect("video track exists");
			assert!(
				e.tracks
					.iter()
					.any(|t| t.clips.iter().any(|c| c.label == "not-in-the-table"))
			);

			// No video track at all: both generator and video footage
			// drops are rejected.
			let keep: Vec<MockTrack> = e
				.tracks
				.iter()
				.filter(|t| t.kind != TrackKind::Video)
				.cloned()
				.collect();
			e.tracks = keep;
			assert!(e.drop_generator_clip("x", 0, Frame(0), cx).is_err());
			e.drop_footage(3, TrackKind::Video, 0, Frame(0), cx);
			assert!(e.footage_drops.is_empty());
		});

		// Audio footage with no audio track reports and drops nothing.
		let engine2 = demo_engine(app);
		engine2.update(app, |e, cx| {
			e.tracks.retain(|t| t.kind != TrackKind::Audio);
			let before = e.footage_drops.len();
			e.drop_footage(20, TrackKind::Audio, 0, Frame(0), cx);
			assert_eq!(e.footage_drops.len(), before, "no audio track");
			// An unknown entry id is rejected too.
			e.drop_footage(u64::MAX, TrackKind::Video, 0, Frame(0), cx);
		});

		// add_node_at: unknown type errors; a known one gets one video
		// input and one video output.
		engine.update(app, |e, cx| {
			assert!(
				e.add_node_at("no-such-node", point(px(0.0), px(0.0)), cx)
					.is_err()
			);
			let entry = e.node_library().into_iter().next().expect("a node type");
			e.add_node_at(&entry.type_id, point(px(12.0), px(34.0)), cx)
				.expect("known node type");
			let added = e.node(NodeId(100)).expect("the created node");
			assert_eq!(added.title().as_ref(), entry.name.as_str());
			assert_eq!(added.position(), point(px(12.0), px(34.0)));
			assert_eq!(added.inputs().len(), 1);
			assert_eq!(added.outputs().len(), 1);
			assert!(added.endpoint().is_none());

			// The legacy start_export wrapper delegates and succeeds.
			let session = e.start_export(0, std::path::PathBuf::from("/tmp/x.mp4"));
			assert!(session.is_ok());
			drop(session);

			// The multicam sequence create only renames an unnamed demo
			// sequence.
			e.sequence.name = String::new();
			assert_eq!(
				e.multicam_create_sequence(vec![], vec![], "Cam".into(), cx),
				Ok(1)
			);
			assert_eq!(e.sequence.name, "Cam");
			assert_eq!(
				e.multicam_create_sequence(vec![], vec![], "Other".into(), cx),
				Ok(1)
			);
			assert_eq!(e.sequence.name, "Cam", "a named sequence is kept");
		});

		// Demo multicam state starts enabled; disable and re-enable
		// cover both local transitions without touching real undo.
		let engine3 = demo_engine(app);
		engine3.update(app, |e, cx| {
			assert!(e.multicam_state().is_some());
			e.multicam_enable_selected(vec![], true, cx);
			assert!(e.multicam_state().is_some(), "enable is a no-op");
			e.multicam_switch_to(2, false, cx);
			assert_eq!(e.multicam_state().map(|s| s.current_source), Some(2));
			e.multicam_enable_selected(vec![], false, cx);
			assert!(e.multicam_state().is_none(), "disabled");
			e.multicam_enable_selected(vec![], true, cx);
			assert!(e.multicam_state().is_some(), "enabled again");
		});
	});
}
