use super::*;
use crate::oakui::MockEngine;
use crate::oakui::engine::EngineGateway;
use crate::widgets::effect_stack::LibraryEffectDrag;
use crate::widgets::timeline::{FrameRange, TimelineDataSource};
use gpui_kit::StatefulInteractiveElement;

/// The empty ghost for the test drag sources.
struct DropGhost;

impl Render for DropGhost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size(px(10.0))
	}
}

/// A test host with draggable footage / effect sources above a real
/// timeline panel, so the panel's `on_drag_move` / `on_drop` handlers can
/// be exercised end to end.
struct DropHost<E: AppEngine> {
	panel: Entity<TimelinePanel<E>>,
}

impl<E: AppEngine> Render for DropHost<E> {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		let source = |id: &'static str| {
			div()
				.id(id)
				.debug_selector(move || id.into())
				.h(px(24.0))
				.w_full()
				.flex_shrink_0()
		};
		div()
			.size_full()
			.flex()
			.flex_col()
			.child(
				source("footage-drag-source")
					.on_drag(FootageDrag(vec![3]), |_drag, _offset, _window, cx| {
						cx.new(|_| DropGhost)
					}),
			)
			.child(
				source("footage-drag-source-missing")
					.on_drag(FootageDrag(vec![9999]), |_drag, _offset, _window, cx| {
						cx.new(|_| DropGhost)
					}),
			)
			.child(source("effect-drag-source").on_drag(
				LibraryEffectDrag {
					type_id: "org.olivevideoeditor.Olive.solidgenerator".into(),
					name: "Solid".into(),
				},
				|_drag, _offset, _window, cx| cx.new(|_| DropGhost),
			))
			.child(self.panel.clone())
	}
}
use gpui_kit::{Hsla, TestAppContext, VisualTestContext, px, size};

/// Builds a `TimelinePanel` in a window of the given logical size and
/// returns a `VisualTestContext` for bounds assertions.
fn panel_window(
	cx: &mut TestAppContext,
	width: f32,
	height: f32,
) -> (
	&'static mut VisualTestContext,
	Entity<TimelinePanel<MockEngine>>,
) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(width), px(height)), |window, cx| {
		let engine = cx.new(crate::oakui::MockEngine::demo);
		let timeline = cx.new(|cx| TimelineView::new(engine.clone(), window, cx).zoom(2.0));
		TimelinePanel::new(engine, timeline, window, cx)
	});
	cx.run_until_parked();
	let panel = window.root(cx).expect("timeline panel root");
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	(cx, panel)
}

/// The Edit-menu operations that used to be stubs now reach the engine:
/// nudge moves the selection, enable/disable flips the flag, duplicate
/// copies past the selection's end, trims land at the playhead, and
/// rename/speed request their shell dialogs.
#[gpui_kit::test]
async fn edit_menu_actions_are_real(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1600.0, 900.0);
	// Demo clip 11 sits at [0, 240) on its track.
	let (engine, clip_id) = cx.read(|app| {
		let engine = panel.read(app).engine.clone();
		let track = engine.read(app).track(1).expect("second track");
		let clip = track.clips()[0].id();
		(engine, clip)
	});
	let range_of = |app: &App| {
		let mut found = None;
		let engine = engine.read(app);
		for t in 0..engine.track_count() {
			if let Some(track) = engine.track(t) {
				if let Some(c) = track.clips().iter().find(|c| c.id() == clip_id) {
					found = Some((c.range().start.0, c.range().end.0));
				}
			}
		}
		found
	};
	let enabled_of = |app: &App| {
		let mut found = None;
		let engine = engine.read(app);
		for t in 0..engine.track_count() {
			if let Some(track) = engine.track(t) {
				if let Some(c) = track.clips().iter().find(|c| c.id() == clip_id) {
					found = Some(c.is_enabled());
				}
			}
		}
		found
	};
	let select = |cx: &mut TestAppContext| {
		cx.update(|app| {
			panel.update(app, |p, cx| {
				p.timeline.update(cx, |t, cx| {
					t.state.clear_selection();
					t.state.select(clip_id);
					cx.notify();
				});
			});
		});
	};
	select(cx);

	// Nudge right/left.
	cx.update(|_window, app| panel.update(app, |p, cx| p.nudge_right(cx)));
	assert_eq!(cx.read(|app| range_of(app)), Some((1, 241)));
	cx.update(|_window, app| panel.update(app, |p, cx| p.nudge_left(cx)));
	assert_eq!(cx.read(|app| range_of(app)), Some((0, 240)));

	// Enable/disable toggles.
	assert_eq!(cx.read(|app| enabled_of(app)), Some(true));
	cx.update(|_window, app| panel.update(app, |p, cx| p.toggle_selected_enabled(cx)));
	assert_eq!(cx.read(|app| enabled_of(app)), Some(false));
	cx.update(|_window, app| panel.update(app, |p, cx| p.toggle_selected_enabled(cx)));
	assert_eq!(cx.read(|app| enabled_of(app)), Some(true));

	// Duplicate pastes right after the selection's end (240).
	cx.update(|_window, app| panel.update(app, |p, cx| p.duplicate(cx)));
	let dup_found = cx.read(|app| {
		let engine = engine.read(app);
		let track = engine.track(1).expect("second track");
		track
			.clips()
			.iter()
			.any(|c| c.range().start.0 == 240 && c.range().end.0 == 480 && c.id() != clip_id)
	});
	assert!(dup_found, "the duplicate lands at [240, 480)");

	// Move in-to-playhead: playhead at 100 trims the clip's head.
	cx.update(|_window, app| {
		engine.update(app, |engine, cx| {
			engine.request_frame(crate::oakui::engine::Monitor::Program, Frame(100), cx)
		});
	});
	cx.update(|_window, app| panel.update(app, |p, cx| p.move_in_to_playhead(cx)));
	assert_eq!(cx.read(|app| range_of(app)), Some((100, 240)));

	// The rename/speed requests emit their dialog events.
	let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
	let requests_sub = requests.clone();
	cx.update(|_window, app| {
		panel.update(app, |_p, cx| {
			cx.subscribe(
				&cx.entity(),
				move |_p, _e, event: &TimelineDialogRequest, _cx| {
					requests_sub.lock().unwrap().push(event.clone());
				},
			)
			.detach();
		});
	});
	cx.update(|_window, app| panel.update(app, |p, cx| p.rename_selected(cx)));
	cx.update(|_window, app| panel.update(app, |p, cx| p.speed_duration(cx)));
	let got = requests.lock().unwrap().clone();
	assert!(
		matches!(
			got.as_slice(),
			[
				TimelineDialogRequest::Rename { .. },
				TimelineDialogRequest::Speed { .. }
			]
		),
		"rename and speed request their dialogs: {got:?}"
	);
}

/// The toolbar's zoom slider tracks the pointer: press+drag right raises
/// the zoom (regression: the track needs a debug selector for the test
/// to find it).
#[gpui_kit::test]
async fn zoom_slider_drag_tracks_the_cursor(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1600.0, 900.0);
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	let zoom = cx.read(|app| panel.read(app).zoom.clone());
	let before = cx.read(|app| zoom.read(app).value().to_f64());
	let bounds = cx
		.debug_bounds("oak-slider-10")
		.expect("zoom slider track rendered");
	let y = bounds.origin.y + px(9.0);
	cx.simulate_mouse_down(
		gpui_kit::point(bounds.left() + px(40.0), y),
		MouseButton::Left,
		gpui_kit::Modifiers::none(),
	);
	cx.simulate_mouse_move(
		gpui_kit::point(bounds.left() + px(120.0), y),
		MouseButton::Left,
		gpui_kit::Modifiers::none(),
	);
	cx.simulate_mouse_move(
		gpui_kit::point(bounds.left() + px(200.0), y),
		MouseButton::Left,
		gpui_kit::Modifiers::none(),
	);
	cx.simulate_mouse_up(
		gpui_kit::point(bounds.left() + px(200.0), y),
		MouseButton::Left,
		gpui_kit::Modifiers::none(),
	);
	cx.run_until_parked();
	let after = cx.read(|app| zoom.read(app).value().to_f64());
	assert!(
		after > before,
		"the drag moves the zoom: {before} -> {after}"
	);
}

/// The toolbar row must sit entirely above the timeline body, the right
/// controls must sit to the right of the timeline canvas (never
/// overlapping it), and the controls slot must keep its fixed width — at
/// the default 1600×900 and down to ~1100px wide.
#[gpui_kit::test]
async fn toolbar_ruler_and_right_controls_never_overlap(cx: &mut TestAppContext) {
	for width in [1600.0, 1280.0, 1100.0] {
		let (cx, _panel) = panel_window(cx, width, 900.0);

		let toolbar = cx
			.debug_bounds("timeline-toolbar")
			.expect("toolbar row rendered");
		let body = cx
			.debug_bounds("timeline-body")
			.expect("timeline body row rendered");
		let canvas = cx
			.debug_bounds("timeline-canvas")
			.expect("timeline canvas rendered");
		let right = cx
			.debug_bounds("timeline-right-controls")
			.expect("right controls slot rendered");

		// The toolbar is exactly 31px tall and ends where the body starts.
		assert!(
			(f32::from(toolbar.size.height) - TOOLBAR_HEIGHT).abs() < 0.5,
			"toolbar height {width}: {} != {TOOLBAR_HEIGHT}",
			toolbar.size.height
		);
		assert!(
			toolbar.bottom() <= body.top(),
			"toolbar overlaps the body at width {width}"
		);

		// The controls slot is fixed-width and never overlaps the canvas.
		assert!(
			(f32::from(right.size.width) - RIGHT_CONTROLS_WIDTH).abs() < 0.5,
			"right slot width {width}: {} != {RIGHT_CONTROLS_WIDTH}",
			right.size.width
		);
		assert!(
			canvas.right() <= right.left(),
			"right controls overlap the timeline canvas at width {width}"
		);

		// The timeline (ruler) keeps the remaining width: canvas right
		// edge equals the slot's left edge exactly.
		assert!(
			(f32::from(canvas.right()) - f32::from(right.left())).abs() < 0.5,
			"canvas and controls slot are not flush at width {width}"
		);

		// The right controls are inside the body's vertical bounds.
		assert!(
			right.top() >= body.top() && right.bottom() <= body.bottom(),
			"right controls escape the body at width {width}"
		);
	}
}

/// Resizing a window keeps the same invariants (the timeline body shrinks
/// while the toolbar and the right slot stay fixed).
#[gpui_kit::test]
async fn resizing_keeps_toolbar_and_right_slot_fixed(cx: &mut TestAppContext) {
	let (cx, _panel) = panel_window(cx, 1600.0, 900.0);

	let before = cx
		.debug_bounds("timeline-right-controls")
		.expect("right controls rendered");
	assert!((f32::from(before.size.width) - RIGHT_CONTROLS_WIDTH).abs() < 0.5);

	cx.simulate_resize(size(px(1100.0), px(900.0)));
	cx.run_until_parked();

	let after = cx
		.debug_bounds("timeline-right-controls")
		.expect("right controls rendered after resize");
	let canvas = cx
		.debug_bounds("timeline-canvas")
		.expect("timeline canvas after resize");
	assert!((f32::from(after.size.width) - RIGHT_CONTROLS_WIDTH).abs() < 0.5);
	assert!(canvas.right() <= after.left());
}

/// Right-clicking inside the timeline body opens the popup: the view
/// emits `ContextMenuRequested`, the panel assembles the matching menu
/// and the `ContextMenu` popup renders.
#[gpui_kit::test]
async fn right_click_opens_the_context_menu(cx: &mut TestAppContext) {
	let (cx, _panel) = panel_window(cx, 1600.0, 900.0);
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(
		cx.debug_bounds("menu-popup").is_none(),
		"menu starts hidden"
	);

	let canvas = cx
		.debug_bounds("timeline-canvas")
		.expect("timeline canvas rendered");
	let click = gpui_kit::point(
		canvas.origin.x + canvas.size.width * 0.5,
		canvas.origin.y + canvas.size.height * 0.5,
	);
	cx.simulate_mouse_down(
		click,
		gpui_kit::MouseButton::Right,
		gpui_kit::Modifiers::none(),
	);
	cx.run_until_parked();
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	let popup = cx
		.debug_bounds("menu-popup")
		.expect("context menu opened on right-click");
	assert!(popup.size.height > px(20.0), "popup lists the items");
}

/// The clip menu keeps the C++ shape: edit section, color labels, the
/// three synchronize entries (registry actions; disabled without ≥ 2
/// eligible clips), cache and proxy submenus, the reveal/multi-cam
/// entries and a registry-backed "Properties".
#[test]
fn clip_menu_keeps_the_cpp_shape() {
	let menu = clip_menu(crate::oakui::engine::SyncEligibility::default(), &[], None);
	// Color label item sits right after the edit section and carries a
	// submenu of all 16 labels.
	let color = menu
		.items
		.iter()
		.find(|item| item.submenu.is_some() && item.label == i18n::tr("menu.color.label"))
		.expect("color label item");
	assert_eq!(
		color.submenu.as_ref().unwrap().items.len(),
		menu::COLOR_LABEL_COUNT
	);

	// The synchronize entries are the registry actions and stay
	// disabled while fewer than 2 clips are eligible.
	for action in [
		ActionId::SyncBySourceTime,
		ActionId::SyncByWaveform,
		ActionId::SyncByWaveformSpeed,
	] {
		let id = action.entry().menu_id();
		let item = menu
			.items
			.iter()
			.find(|item| item.id == id)
			.unwrap_or_else(|| panic!("clip menu missing synchronize entry {id}"));
		assert!(!item.enabled, "synchronize {id} should be disabled");
	}

	for id in [
		LOCAL_REVEAL_FOOTAGE_VIEWER,
		LOCAL_REVEAL_PROJECT,
		LOCAL_MULTICAM,
	] {
		let item = menu
			.items
			.iter()
			.find(|item| item.id == id)
			.unwrap_or_else(|| panic!("clip menu missing disabled placeholder id {id}"));
		assert!(!item.enabled, "placeholder {id} should be disabled");
	}

	// Cache and proxy are submenus; with no footage selected every
	// proxy entry but the settings action is disabled.
	let cache = menu
		.items
		.iter()
		.find(|item| item.label == i18n::tr("timeline.context.cache"))
		.expect("cache submenu");
	assert_eq!(cache.submenu.as_ref().unwrap().items.len(), 4);
	let proxy = menu
		.items
		.iter()
		.find(|item| item.label == i18n::tr("timeline.context.proxy"))
		.expect("proxy submenu");
	let proxy_items = &proxy.submenu.as_ref().unwrap().items;
	assert_eq!(proxy_items.len(), 5);
	assert!(proxy_items[..4].iter().all(|item| !item.enabled));
	assert!(proxy_items[4].enabled);

	// "Properties" dispatches through the speed/duration registry entry.
	let properties = menu.items.last().expect("properties is the clip menu tail");
	assert_eq!(properties.id, ActionId::SpeedDuration.entry().menu_id());
}

/// The synchronize / proxy enable state follows the selection (the C++
/// `get_selected_*_sync_clips` counts and the proxy-footage flags).
#[test]
fn clip_menu_enables_sync_and_proxy_from_selection() {
	use crate::oakui::engine::{ProxyFootageRow, ProxyMediaState, SyncEligibility};
	let rows = vec![
		ProxyFootageRow {
			id: 1,
			name: "a.mp4".into(),
			state: ProxyMediaState::Ready,
			enabled: true,
			has_custom: false,
			can_generate: true,
			has_proxy: true,
		},
		ProxyFootageRow {
			id: 2,
			name: "b.mp4".into(),
			state: ProxyMediaState::Missing,
			enabled: false,
			has_custom: false,
			can_generate: true,
			has_proxy: false,
		},
	];
	let menu = clip_menu(
		SyncEligibility {
			source_time: 2,
			waveform: 1,
		},
		&rows,
		None,
	);
	let find = |id: usize| {
		menu.items
			.iter()
			.find(|item| item.id == id)
			.unwrap_or_else(|| panic!("missing item {id}"))
	};
	assert!(
		find(ActionId::SyncBySourceTime.entry().menu_id()).enabled,
		"2 eligible clips enable source-time sync"
	);
	assert!(
		!find(ActionId::SyncByWaveform.entry().menu_id()).enabled,
		"1 eligible clip keeps waveform sync disabled"
	);
	let proxy = menu
		.items
		.iter()
		.find(|item| item.label == i18n::tr("timeline.context.proxy"))
		.expect("proxy submenu");
	let proxy_items = &proxy.submenu.as_ref().unwrap().items;
	assert!(proxy_items[0].enabled, "generate: footage can generate");
	assert!(proxy_items[1].enabled, "use: footage present");
	assert!(
		!proxy_items[1].checked.unwrap_or(false),
		"use: not every footage has its proxy enabled"
	);
	assert!(proxy_items[2].enabled, "reveal: one footage has a proxy");
	assert!(proxy_items[3].enabled, "delete: one footage has a proxy");
}

/// The Multi-Cam item follows the selection's multicam state: it enables
/// when a selected clip's connected viewer is a sequence and is checked
/// when that clip already has a multicam (the C++ conditions).
#[test]
fn clip_menu_multicam_item_follows_the_state() {
	// No eligible clip: disabled and unchecked.
	let menu = clip_menu(
		crate::oakui::engine::SyncEligibility::default(),
		&[],
		Some(MulticamMenuState {
			eligible: false,
			enabled: false,
		}),
	);
	let item = menu
		.items
		.iter()
		.find(|item| item.id == LOCAL_MULTICAM)
		.expect("multi-cam item");
	assert!(!item.enabled, "ineligible clips keep Multi-Cam disabled");
	assert!(!item.checked.unwrap_or(false));

	// Eligible + enabled: enabled and checked.
	let menu = clip_menu(
		crate::oakui::engine::SyncEligibility::default(),
		&[],
		Some(MulticamMenuState {
			eligible: true,
			enabled: true,
		}),
	);
	let item = menu
		.items
		.iter()
		.find(|item| item.id == LOCAL_MULTICAM)
		.expect("multi-cam item");
	assert!(item.enabled, "a sequence-fed clip enables Multi-Cam");
	assert!(
		item.checked.unwrap_or(false),
		"checked when multicam present"
	);

	// Eligible but not enabled: enabled, unchecked.
	let menu = clip_menu(
		crate::oakui::engine::SyncEligibility::default(),
		&[],
		Some(MulticamMenuState {
			eligible: true,
			enabled: false,
		}),
	);
	let item = menu
		.items
		.iter()
		.find(|item| item.id == LOCAL_MULTICAM)
		.expect("multi-cam item");
	assert!(item.enabled);
	assert!(!item.checked.unwrap_or(false));
}

/// The empty-area menu exposes the view toggles, the "Add Adjustment
/// Layer" creation entry (§3.5) and the sequence settings "Properties"
/// entry.
#[test]
fn empty_area_menu_toggles_and_properties() {
	let menu = empty_area_menu();
	let thumbnails = menu
		.items
		.iter()
		.find(|item| item.label == i18n::tr("timeline.context.show_thumbnails"))
		.expect("thumbnails submenu");
	let sub = &thumbnails.submenu.as_ref().unwrap().items;
	let ids: Vec<usize> = sub.iter().map(|item| item.id).collect();
	assert_eq!(
		ids,
		vec![
			LOCAL_THUMBNAIL_OFF,
			LOCAL_THUMBNAIL_IN_OUT,
			LOCAL_THUMBNAIL_ON
		]
	);
	assert!(sub.iter().all(|item| item.checked == Some(false)));

	// "Add Adjustment Layer" sits between the view toggles and
	// Properties, carries the i18n label and closes its section so the
	// tail reads as the properties block.
	let adjustment = menu
		.items
		.iter()
		.position(|item| item.id == LOCAL_ADD_ADJUSTMENT_LAYER)
		.expect("add adjustment layer item");
	let item = &menu.items[adjustment];
	assert!(item.label == i18n::tr("timeline.context.add_adjustment_layer"));
	assert!(item.enabled);
	assert!(item.separator_after);
	assert_eq!(
		menu.items[adjustment + 1].id,
		ActionId::SequenceSettings.entry().menu_id(),
		"Properties follows the adjustment item"
	);

	let properties = menu.items.last().expect("properties tail");
	assert_eq!(properties.id, ActionId::SequenceSettings.entry().menu_id());
}

/// Choosing "Add Adjustment Layer" from the empty-area menu lands the
/// demo's five-second layer on the video track under the cursor, at the
/// clicked frame and in a color of its own (§3.5).
#[gpui_kit::test]
async fn empty_area_menu_adds_an_adjustment_layer_at_the_clicked_frame(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1600.0, 900.0);
	// The demo's track 0 is a video track with one clip (标题.mov, frames
	// 120..300); the created layer must not borrow that clip's color.
	let demo_colors: Vec<Hsla> = cx.read(|app| {
		let engine = panel.read(app).engine.clone();
		let engine = engine.read(app);
		engine
			.track(0)
			.expect("the demo's first track")
			.clips()
			.iter()
			.filter_map(|clip| clip.color())
			.collect()
	});
	assert_eq!(demo_colors.len(), 1, "the demo track has one colored clip");

	// The right-click anchors the menu on (track 0, frame 50); choosing
	// the item creates the layer there.
	cx.update(|_window, cx| {
		panel.update(cx, |panel, cx| {
			panel.open_context_menu(
				gpui_kit::point(px(10.0), px(10.0)),
				TimelineHit::Empty {
					track: 0,
					frame: Frame(50),
				},
				cx,
			);
			panel.on_local_menu_item(LOCAL_ADD_ADJUSTMENT_LAYER, cx);
		});
	});

	cx.read(|app| {
		let engine = panel.read(app).engine.clone();
		let engine = engine.read(app);
		let track = engine.track(0).expect("the demo's first track");
		let clips = track.clips();
		assert_eq!(clips.len(), 2, "the layer joins the track's demo clip");
		let layer = &clips[0];
		assert_eq!(
			layer.range(),
			FrameRange::new(Frame(50), Frame(175)),
			"five seconds (125 frames at 25fps) from the clicked frame"
		);
		assert_eq!(layer.label(), SharedString::from("调整图层"));
		assert_eq!(layer.media_in(), Frame(0));
		let color = layer.color().expect("the layer carries its own color");
		assert!(
			demo_colors.iter().all(|demo| *demo != color),
			"the layer color is distinct from the demo clips"
		);
	});
}

/// The track-header menu is exactly the two delete entries.
#[test]
fn track_head_menu_offers_add_then_delete_entries() {
	let ids: Vec<usize> = track_head_menu().items.iter().map(|item| item.id).collect();
	assert_eq!(
		ids,
		vec![
			LOCAL_ADD_VIDEO_TRACK,
			LOCAL_ADD_AUDIO_TRACK,
			LOCAL_DELETE_TRACK,
			LOCAL_DELETE_ALL_EMPTY
		]
	);
}

/// The marker menu pairs the color labels with the plain edit section
/// and a local marker-properties entry.
#[test]
fn marker_menu_pairs_color_labels_with_edit_section() {
	let menu = marker_menu();
	assert!(menu.items[0].label == i18n::tr("menu.color.label"));
	assert!(menu.items[0].separator_after);
	let last = menu.items.last().expect("marker properties tail");
	assert_eq!(last.id, LOCAL_MARKER_PROPERTIES);
	assert_eq!(last.label, i18n::tr("menu.context.properties"));
}

/// The ruler menu is the timecode-display radio group, all unchecked by
/// default.
#[test]
fn ruler_menu_is_the_timecode_radio_group() {
	let menu = ruler_menu();
	let ids: Vec<usize> = menu.items.iter().map(|item| item.id).collect();
	assert_eq!(
		ids,
		vec![
			LOCAL_TIMECODE_DROP_FRAME,
			LOCAL_TIMECODE_NON_DROP_FRAME,
			LOCAL_TIMECODE_SECONDS,
			LOCAL_TIMECODE_FRAMES,
			LOCAL_TIMECODE_MILLISECONDS,
		]
	);
	assert!(menu.items.iter().all(|item| item.checked == Some(false)));
}

// -----------------------------------------------------------------------
// Panel behavior: subscriptions, context menus, commands, drops, render.
// -----------------------------------------------------------------------

/// The zoom/height sliders and the snap checkbox drive the view/engine
/// through the panel's subscriptions.
#[gpui_kit::test]
async fn slider_and_snap_events_update_the_view_and_engine(cx: &mut TestAppContext) {
	use crate::oakui::component::controls::SliderValue;
	let (cx, panel) = panel_window(cx, 1280.0, 720.0);

	// Zoom slider -> timeline zoom.
	cx.update(|_window, app| {
		panel.read(app).zoom.clone().update(app, |_slider, cx| {
			cx.emit(SliderEvent::ValueChanged {
				control: 10,
				value: SliderValue::Float(3.5),
			});
		});
	});
	assert_eq!(
		cx.read(|app| panel.read(app).timeline.read(app).state.zoom),
		3.5
	);

	// Track-height slider -> every engine track.
	cx.update(|_window, app| {
		panel.read(app).height.clone().update(app, |_slider, cx| {
			cx.emit(SliderEvent::ValueChanged {
				control: 11,
				value: SliderValue::Float(80.0),
			});
		});
	});
	cx.read(|app| {
		let engine = panel.read(app).engine.clone();
		let engine = engine.read(app);
		assert!(engine.track_count() > 0, "the demo has tracks");
		for index in 0..engine.track_count() {
			assert_eq!(engine.track(index).unwrap().height(), px(80.0));
		}
	});

	// Snap checkbox -> view state.
	for (state, expected) in [(CheckState::Unchecked, false), (CheckState::Checked, true)] {
		cx.update(|_window, app| {
			panel.read(app).snap.clone().update(app, |_snap, cx| {
				cx.emit(CheckBoxEvent::Toggled { control: 12, state });
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).timeline.read(app).state.snap_enabled),
			expected
		);
	}
}

/// Every `TimelineHit` variant opens its matching context menu; an
/// unselected right-clicked clip is selected first.
#[gpui_kit::test]
async fn context_menus_open_for_every_hit_kind(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1600.0, 900.0);
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			let position = gpui_kit::point(px(20.0), px(20.0));

			panel.open_context_menu(position, TimelineHit::Clip(ClipId(10)), cx);
			assert!(
				panel.timeline.read(cx).selection().contains(&ClipId(10)),
				"an unselected right-clicked clip is selected first"
			);
			assert_eq!(panel.context_track, None);
			assert_eq!(panel.context_empty, None);

			// A second hit on the already-selected clip takes the
			// no-reselect path.
			panel.open_context_menu(position, TimelineHit::Clip(ClipId(10)), cx);

			panel.open_context_menu(
				position,
				TimelineHit::Empty {
					track: 2,
					frame: Frame(42),
				},
				cx,
			);
			assert_eq!(panel.context_empty, Some((2, Frame(42))));
			assert_eq!(panel.context_track, None);

			panel.open_context_menu(position, TimelineHit::TrackHead(3), cx);
			assert_eq!(panel.context_track, Some(3));
			assert_eq!(panel.context_empty, None);

			panel.open_context_menu(position, TimelineHit::RulerMarker(Frame(7)), cx);
			assert_eq!(panel.context_track, None);
			assert_eq!(panel.context_empty, None);

			panel.open_context_menu(position, TimelineHit::Ruler(Frame(9)), cx);
			assert_eq!(panel.context_track, None);
			assert_eq!(panel.context_empty, None);
		});
	});
}

/// The local (non-registry) menu items route to the engine; missing
/// targets and unknown ids are benign no-ops.
#[gpui_kit::test]
async fn local_menu_items_route_to_the_engine(cx: &mut TestAppContext) {
	// The Multi-Cam item runs real undoable commands; serialize with the
	// graph tests sharing the global undo stack.
	let _guard = crate::oakui::graphops::test_lock();
	let (cx, panel) = panel_window(cx, 1600.0, 900.0);

	let tracks_before = cx.read(|app| panel.read(app).engine.read(app).track_count());
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.on_local_menu_item(LOCAL_ADD_VIDEO_TRACK, cx);
			panel.on_local_menu_item(LOCAL_ADD_AUDIO_TRACK, cx);
		});
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).track_count()),
		tracks_before + 2
	);

	// Delete one track (with a recorded track-head target). Without a
	// target the click is ignored.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.on_local_menu_item(LOCAL_DELETE_TRACK, cx);
			panel.context_track = Some(2);
			panel.on_local_menu_item(LOCAL_DELETE_TRACK, cx);
		});
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).track_count()),
		tracks_before + 1
	);
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.on_local_menu_item(LOCAL_DELETE_ALL_EMPTY, cx);
		});
	});

	// A color label only logs (the engine has no clip-color surface).
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.on_local_menu_item(menu::COLOR_LABEL_BASE + 3, cx);
		});
	});

	// Add adjustment layer: without a recorded empty-area click it is a
	// no-op; with one the demo layer lands on the pointed track.
	let clips_before = cx.read(|app| {
		panel
			.read(app)
			.engine
			.read(app)
			.track(0)
			.unwrap()
			.clips()
			.len()
	});
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.on_local_menu_item(LOCAL_ADD_ADJUSTMENT_LAYER, cx);
			panel.context_empty = Some((0, Frame(10)));
			panel.on_local_menu_item(LOCAL_ADD_ADJUSTMENT_LAYER, cx);
		});
	});
	cx.read(|app| {
		assert_eq!(
			panel
				.read(app)
				.engine
				.read(app)
				.track(0)
				.unwrap()
				.clips()
				.len(),
			clips_before + 1,
			"the adjustment layer joined the video track"
		);
	});
	// An audio target is rejected (the error branch logs).
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.context_empty = Some((2, Frame(10)));
			panel.on_local_menu_item(LOCAL_ADD_ADJUSTMENT_LAYER, cx);
		});
	});

	// The cache / timecode / thumbnail placeholders and an unknown id.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			for item in [
				LOCAL_CACHE_ALL,
				LOCAL_CACHE_IN_OUT,
				LOCAL_CACHE_DISCARD,
				LOCAL_CACHE_AUTO,
				LOCAL_TIMECODE_DROP_FRAME,
				LOCAL_TIMECODE_NON_DROP_FRAME,
				LOCAL_TIMECODE_SECONDS,
				LOCAL_TIMECODE_FRAMES,
				LOCAL_TIMECODE_MILLISECONDS,
				LOCAL_USE_AUDIO_TIME_UNITS,
				LOCAL_SHOW_WAVEFORMS,
				LOCAL_THUMBNAIL_OFF,
				LOCAL_THUMBNAIL_IN_OUT,
				LOCAL_THUMBNAIL_ON,
				LOCAL_MARKER_PROPERTIES,
				LOCAL_REVEAL_FOOTAGE_VIEWER,
				LOCAL_REVEAL_PROJECT,
			] {
				panel.on_local_menu_item(item, cx);
			}
			panel.on_local_menu_item(999_999, cx);
		});
	});

	// The proxy actions over the selected demo clip (id 10 maps to the
	// explorer's intro.mov): generate first, then use / reveal / delete.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.timeline.update(cx, |view, cx| {
				view.state.selection.clear();
				view.state.selection.insert(ClipId(10));
				cx.notify();
			});
			panel.on_local_menu_item(LOCAL_PROXY_GENERATE, cx);
			panel.on_local_menu_item(LOCAL_PROXY_USE, cx);
			panel.on_local_menu_item(LOCAL_PROXY_REVEAL, cx);
			panel.on_local_menu_item(LOCAL_PROXY_DELETE, cx);
		});
	});
	cx.read(|app| {
		let engine = panel.read(app).engine.clone();
		let engine = engine.read(app);
		assert!(engine.proxy_row(10).is_some(), "the demo row exists");
	});

	// Multi-Cam flips through the real graph commands.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.timeline.update(cx, |view, cx| {
				view.state.selection.insert(ClipId(10));
				cx.notify();
			});
			panel.on_local_menu_item(LOCAL_MULTICAM, cx);
		});
	});
}

/// The panel command trait routes transport, in/out points, selection and
/// editing commands to the engine without panicking.
#[gpui_kit::test]
async fn panel_commands_cover_transport_edit_and_view(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1280.0, 720.0);

	// Transport: play/pause toggles the program clock.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			assert!(panel.play_pause(cx));
		});
	});
	assert!(
		cx.read(|app| panel
			.read(app)
			.engine
			.read(app)
			.program_clock()
			.read(app)
			.is_playing()),
		"play_pause starts the program monitor"
	);
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			assert!(panel.play_pause(cx));
			assert!(panel.prev_frame(cx));
			assert!(panel.next_frame(cx));
			assert!(panel.go_to_start(cx));
			assert!(panel.go_to_end(cx));
			assert!(panel.shuttle_left(cx));
			assert!(panel.shuttle_right(cx));
			assert!(panel.shuttle_stop(cx));
		});
	});
	assert!(
		!cx.read(|app| panel
			.read(app)
			.engine
			.read(app)
			.program_clock()
			.read(app)
			.is_playing()),
		"shuttle_stop pauses the program monitor"
	);

	// In/out points.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.engine.update(cx, |engine, cx| {
				engine.request_frame(Monitor::Program, Frame(90), cx)
			});
			assert!(panel.set_in(cx));
		});
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).workarea().map(|(s, _)| s)),
		Some(Frame(90))
	);
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			assert!(panel.set_out(cx));
			assert!(panel.reset_in(cx));
		});
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).workarea().map(|(s, _)| s)),
		Some(Frame::ZERO),
		"reset_in returns the in point to the sequence start"
	);
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			assert!(panel.reset_out(cx));
		});
	});
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			assert!(panel.clear_in_out(cx));
		});
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).workarea()),
		None
	);

	// Reset with no work area set (the unwrap_or fallbacks).
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			assert!(panel.reset_in(cx));
			assert!(panel.reset_out(cx));
		});
	});
	assert!(
		cx.read(|app| panel.read(app).engine.read(app).workarea())
			.is_some()
	);

	// Selection + editing commands (the mock's clipboard methods are
	// no-ops; delete/split touch its demo tracks).
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			assert!(panel.select_all(cx));
			assert!(panel.deselect_all(cx));
			assert!(panel.delete_selected(cx), "an empty selection is handled");
			assert!(panel.ripple_delete(cx));
			assert!(panel.select_all(cx));
			assert!(panel.copy_selected(cx));
			assert!(panel.cut_selected(cx));
			assert!(panel.paste(cx));
			assert!(panel.delete_selected(cx));
			assert!(panel.ripple_delete(cx));
			assert!(panel.split_at_playhead(cx));
			assert!(panel.default_transition(cx));
			assert!(panel.set_marker(cx));
			assert!(panel.sync_by_source_time(cx));
			assert!(panel.sync_by_waveform(cx));
			assert!(panel.sync_by_waveform_speed(cx));
			assert!(panel.toggle_links(cx));
			assert!(panel.zoom_in(cx));
			assert!(panel.zoom_out(cx));
			assert!(panel.increase_track_height(cx));
			assert!(panel.decrease_track_height(cx));
		});
	});
}

/// `delete_selection` ignores an empty selection and removes the selected
/// clips through the engine; the work-area commits follow the playhead.
#[gpui_kit::test]
async fn delete_selection_and_workarea_paths(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1280.0, 720.0);

	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.timeline.update(cx, |view, cx| {
				view.state.selection.clear();
				cx.notify();
			});
			panel.delete_selection(false, cx);
			panel.delete_selection(true, cx);
		});
	});

	let clips_before = cx.read(|app| {
		panel
			.read(app)
			.engine
			.read(app)
			.track(0)
			.unwrap()
			.clips()
			.len()
	});
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.timeline.update(cx, |view, cx| {
				view.state.selection.insert(ClipId(10));
				cx.notify();
			});
			panel.delete_selection(false, cx);
		});
	});
	assert_eq!(
		cx.read(|app| panel
			.read(app)
			.engine
			.read(app)
			.track(0)
			.unwrap()
			.clips()
			.len()),
		clips_before - 1,
		"the selected clip left its track"
	);

	// A playhead at frame zero cannot form an out point: the commit is
	// ignored (and the in point falls back to the sequence length).
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.engine.update(cx, |engine, cx| {
				engine.request_frame(Monitor::Program, Frame::ZERO, cx)
			});
			panel.set_point_at_playhead(false, cx);
			panel.set_point_at_playhead(true, cx);
		});
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).workarea().map(|(s, _)| s)),
		Some(Frame::ZERO)
	);
}

/// `finish_footage_drop` and `finish_effect_drop` route their pending
/// targets; a missing target and an unusable effect are benign no-ops.
#[gpui_kit::test]
async fn finished_drops_route_to_the_engine(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1280.0, 720.0);

	let target = || FootageDropTarget {
		track_kind: TrackKind::Video,
		track_index: 1,
		time: Frame(20),
		length: 250,
	};

	// No pending footage drop.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.finish_footage_drop(&FootageDrag(vec![3]), cx);
		});
	});
	assert!(cx.read(|app| panel.read(app).engine.read(app).footage_drops().is_empty()));

	// A pending drop with an open sequence reaches the engine.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.footage_drop = Some(target());
			panel.finish_footage_drop(&FootageDrag(vec![3]), cx);
		});
	});
	cx.read(|app| {
		let panel = panel.read(app);
		assert!(panel.footage_drop.is_none(), "the target is consumed");
		assert_eq!(panel.engine.read(app).footage_drops().len(), 1);
	});

	// Two selected entries are placed sequentially, not on top of
	// each other.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.footage_drop = Some(target());
			panel.finish_footage_drop(&FootageDrag(vec![3, 3]), cx);
		});
	});
	cx.read(|app| {
		let drops = panel.read(app).engine.read(app).footage_drops();
		assert_eq!(drops.len(), 3);
		assert!(drops[1].time.0 < drops[2].time.0);
	});

	let generator = crate::widgets::effect_stack::LibraryEffectDrag {
		type_id: "org.olivevideoeditor.Olive.solidgenerator".into(),
		name: "Solid".into(),
	};
	let transition = crate::widgets::effect_stack::LibraryEffectDrag {
		type_id: "org.olivevideoeditor.Olive.transition".into(),
		name: "Dissolve".into(),
	};
	let non_generator = crate::widgets::effect_stack::LibraryEffectDrag {
		type_id: "org.olivevideoeditor.Olive.blur".into(),
		name: "Blur".into(),
	};

	// No pending effect drop.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.finish_effect_drop(&generator, cx);
		});
	});

	// A transition type routes to the engine's transition drop.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.effect_drop = Some(target());
			panel.finish_effect_drop(&transition, cx);
		});
	});
	cx.read(|app| assert!(panel.read(app).effect_drop.is_none()));

	// A non-generator effect on a clip appends to that clip's chain
	// (no new timeline clip); the same drop on empty space is ignored.
	let clips_before = cx.read(|app| {
		panel
			.read(app)
			.engine
			.read(app)
			.track(1)
			.unwrap()
			.clips()
			.len()
	});
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.effect_drop = Some(target());
			panel.finish_effect_drop(&non_generator, cx);
		});
	});
	assert_eq!(
		cx.read(|app| panel
			.read(app)
			.engine
			.read(app)
			.track(1)
			.unwrap()
			.clips()
			.len()),
		clips_before,
		"a non-generator makes no clip"
	);
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).effect_clip_drops.len()),
		1,
		"the effect landed on the clip under the drop point"
	);

	// Empty space ignores the same effect.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.effect_drop = Some(FootageDropTarget {
				track_kind: TrackKind::Video,
				track_index: 1,
				time: Frame(10_000),
				length: 250,
			});
			panel.finish_effect_drop(&non_generator, cx);
		});
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).effect_clip_drops.len()),
		1,
		"empty space ignores the effect"
	);

	// A generator type creates a standalone clip on the pointed track.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.effect_drop = Some(target());
			panel.finish_effect_drop(&generator, cx);
		});
	});
	assert_eq!(
		cx.read(|app| panel
			.read(app)
			.engine
			.read(app)
			.track(1)
			.unwrap()
			.clips()
			.len()),
		clips_before + 1,
		"the generator clip landed on the pointed track"
	);
}

/// `resolve_drop_point` maps the cursor to track + frame, clamps above
/// the ruler / below the tracks and falls back on an empty timeline.
#[gpui_kit::test]
async fn resolve_drop_point_maps_and_clamps(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1280.0, 720.0);

	assert_eq!(
		cx.read(|app| panel
			.read(app)
			.resolve_drop_point(gpui_kit::point(px(200.0), px(10.0)), app)),
		None,
		"above the ruler there is no clip area"
	);

	// Inside the header column the frame clamps to zero; the y walk finds
	// the second track (64px tall rows).
	let (kind, index, frame) = cx
		.read(|app| {
			panel
				.read(app)
				.resolve_drop_point(gpui_kit::point(px(10.0), px(100.0)), app)
		})
		.expect("below the ruler");
	assert_eq!((kind, index), (TrackKind::Video, 1));
	assert_eq!(frame, Frame::ZERO);

	// Below every track clamps to the last one.
	let (kind, index, _) = cx
		.read(|app| {
			panel
				.read(app)
				.resolve_drop_point(gpui_kit::point(px(600.0), px(4000.0)), app)
		})
		.expect("below the ruler");
	assert_eq!((kind, index), (TrackKind::Audio, 3));

	// An empty timeline falls back to (Video, 0).
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			let engine = panel.engine.clone();
			engine.update(cx, |engine, cx| {
				while engine.track_count() > 0 {
					engine.remove_track(0, cx);
				}
			});
			let (kind, index, _) = panel
				.resolve_drop_point(gpui_kit::point(px(600.0), px(100.0)), cx)
				.expect("still below the ruler");
			assert_eq!((kind, index), (TrackKind::Video, 0));
		});
	});
}

/// `zoom_timeline` and `nudge_track_height` scale the view and clamp the
/// engine's track heights to the slider range.
#[gpui_kit::test]
async fn zoom_and_track_height_nudges(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1280.0, 720.0);
	let before = cx.read(|app| panel.read(app).timeline.read(app).state.zoom);
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| panel.zoom_timeline(1.25, cx));
	});
	assert!(cx.read(|app| panel.read(app).timeline.read(app).state.zoom) > before);
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| panel.zoom_timeline(0.8, cx));
	});

	cx.update(|_window, app| {
		panel.update(app, |panel, cx| panel.nudge_track_height(1000.0, cx));
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).track(0).unwrap().height()),
		px(160.0),
		"the height clamps at the slider maximum"
	);
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| panel.nudge_track_height(-1000.0, cx));
	});
	assert_eq!(
		cx.read(|app| panel.read(app).engine.read(app).track(0).unwrap().height()),
		px(24.0),
		"the height clamps at the slider minimum"
	);

	// No tracks: the 64px default is used as the nudge base.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			let engine = panel.engine.clone();
			engine.update(cx, |engine, cx| {
				while engine.track_count() > 0 {
					engine.remove_track(0, cx);
				}
			});
			panel.nudge_track_height(0.0, cx);
		});
	});
}

/// The toolbar's tool buttons and trailing controls run their `on_click`
/// closures: tool selection, add-track buttons, zoom buttons and the
/// snap icon.
#[gpui_kit::test]
async fn toolbar_buttons_switch_tools_and_route_actions(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1600.0, 900.0);
	let toolbar = cx
		.debug_bounds("timeline-toolbar")
		.expect("toolbar rendered");
	let y = toolbar.origin.y + px(15.0);

	// The eight tool buttons sit at 24px + 8px-gap offsets after the 8px
	// toolbar padding.
	for index in 0..8 {
		let x = toolbar.origin.x + px(8.0 + 32.0 * index as f32 + 12.0);
		cx.simulate_click(gpui_kit::point(x, y), gpui_kit::Modifiers::none());
		assert_eq!(
			cx.read(|app| panel.read(app).timeline.read(app).tool()),
			TimelineTool::from_index(index).unwrap(),
			"toolbar button {index} selects its tool"
		);
	}
	// Clicking the already-selected tool again hits the no-op guard.
	cx.simulate_click(
		gpui_kit::point(toolbar.origin.x + px(8.0 + 7.0 * 32.0 + 12.0), y),
		gpui_kit::Modifiers::none(),
	);

	// Walk the trailing controls: each hit routes through an `on_click`
	// closure (add-track buttons, zoom buttons, the snap toggle).
	let mut saw_add_track = false;
	let mut saw_zoom = false;
	let mut saw_snap = false;
	let mut x = toolbar.origin.x + px(264.0);
	while x < toolbar.right() {
		let (tracks_before, zoom_before, snap_before) = cx.read(|app| {
			let panel = panel.read(app);
			let engine = panel.engine.read(app);
			(
				engine.track_count(),
				panel.timeline.read(app).state.zoom,
				panel.timeline.read(app).state.snap_enabled,
			)
		});
		cx.simulate_click(gpui_kit::point(x, y), gpui_kit::Modifiers::none());
		let (tracks_after, zoom_after, snap_after) = cx.read(|app| {
			let panel = panel.read(app);
			let engine = panel.engine.read(app);
			(
				engine.track_count(),
				panel.timeline.read(app).state.zoom,
				panel.timeline.read(app).state.snap_enabled,
			)
		});
		saw_add_track |= tracks_after != tracks_before;
		saw_zoom |= (zoom_after - zoom_before).abs() > f32::EPSILON;
		saw_snap |= snap_after != snap_before;
		x += px(4.0);
	}
	assert!(saw_add_track, "an add-track button was hit");
	assert!(saw_zoom, "a zoom button was hit");
	assert!(saw_snap, "the snap toggle was hit");
}

/// The drop ghost renders (including the out-of-range row break), the
/// dock metadata is populated and a left click focuses the panel.
#[gpui_kit::test]
async fn drop_ghost_and_dock_metadata(cx: &mut TestAppContext) {
	let (cx, panel) = panel_window(cx, 1280.0, 720.0);

	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.footage_drop = Some(FootageDropTarget {
				track_kind: TrackKind::Video,
				track_index: 1,
				time: Frame(50),
				length: 250,
			});
			cx.notify();
		});
	});
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	assert!(cx.debug_bounds("timeline-canvas").is_some());

	// A target beyond the last track breaks the row walk instead of
	// panicking; a zero-length ghost keeps its 4px minimum width.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.footage_drop = None;
			panel.effect_drop = Some(FootageDropTarget {
				track_kind: TrackKind::Audio,
				track_index: 999,
				time: Frame(10),
				length: 0,
			});
			cx.notify();
		});
	});
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});

	// Dock metadata.
	cx.update(|_window, app| {
		let panel = panel.read(app);
		assert_eq!(panel.panel_name(), crate::panels::ids::TIMELINE);
	});

	// A left click focuses the panel (the root's mouse-down handler).
	cx.simulate_mouse_down(
		gpui_kit::point(px(640.0), px(400.0)),
		gpui_kit::MouseButton::Left,
		gpui_kit::Modifiers::none(),
	);
}

/// Dispatches one complete drag gesture against the current rendered
/// frame (a repaint between the events would consume the listeners).
fn dispatch_drag(
	cx: &mut VisualTestContext,
	start: gpui_kit::Point<Pixels>,
	hover: gpui_kit::Point<Pixels>,
	ruler: gpui_kit::Point<Pixels>,
) {
	cx.update(|window, cx| {
		window.dispatch_event(
			gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
				position: start,
				modifiers: gpui_kit::Modifiers::none(),
				button: gpui_kit::MouseButton::Left,
				click_count: 1,
				first_mouse: false,
			}),
			cx,
		);
		let mut move_to = |position| {
			window.dispatch_event(
				gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
					position,
					modifiers: gpui_kit::Modifiers::none(),
					pressed_button: Some(gpui_kit::MouseButton::Left),
				}),
				cx,
			);
		};
		// Past the drag threshold to start the drag.
		move_to(start + gpui_kit::point(px(6.0), px(0.0)));
		// Over the ruler first (no target yet, so the clear is a no-op),
		// then hover the drop point, slide up to the ruler (clearing the
		// target) and hover it again.
		move_to(ruler);
		move_to(hover);
		move_to(ruler);
		move_to(hover);
		window.dispatch_event(
			gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
				position: hover,
				modifiers: gpui_kit::Modifiers::none(),
				button: gpui_kit::MouseButton::Left,
				click_count: 1,
			}),
			cx,
		);
	});
}

/// A full drag onto the timeline canvas resolves the drop target (and
/// clears it over the ruler), then routes the payload to the engine.
#[gpui_kit::test]
async fn drag_gestures_resolve_targets_and_route_drops(cx: &mut TestAppContext) {
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(1280.0), px(720.0)), |window, cx| {
		let engine = cx.new(MockEngine::demo);
		let timeline = cx.new(|cx| TimelineView::new(engine.clone(), window, cx).zoom(2.0));
		let panel = cx.new(|cx| TimelinePanel::new(engine, timeline, window, cx));
		DropHost { panel }
	});
	cx.run_until_parked();
	let host = window.root(cx).expect("drop host root");
	let mut cx = VisualTestContext::from_window(window.into(), cx);
	let panel = cx.read(|app| host.read(app).panel.clone());

	let canvas = cx.debug_bounds("timeline-canvas").expect("canvas rendered");
	let hover = gpui_kit::point(
		canvas.left() + px(HEADER_WIDTH + 100.0),
		canvas.top() + px(RULER_HEIGHT + 20.0),
	);
	let ruler = gpui_kit::point(
		canvas.left() + px(HEADER_WIDTH + 100.0),
		canvas.top() + px(10.0),
	);

	// An unknown footage id falls back to a one-frame ghost and the
	// engine rejects the drop.
	let source = cx
		.debug_bounds("footage-drag-source-missing")
		.expect("missing-footage source");
	dispatch_drag(&mut cx, source.center(), hover, ruler);
	cx.run_until_parked();
	cx.read(|app| {
		let panel = panel.read(app);
		assert!(panel.footage_drop.is_none(), "the drop consumed the target");
		assert!(
			panel.engine.read(app).footage_drops().is_empty(),
			"the unknown entry never placed a clip"
		);
	});

	// The real footage entry places its clip.
	let source = cx
		.debug_bounds("footage-drag-source")
		.expect("footage source");
	dispatch_drag(&mut cx, source.center(), hover, ruler);
	cx.run_until_parked();
	cx.read(|app| {
		let panel = panel.read(app);
		let drops = panel.engine.read(app).footage_drops().to_vec();
		assert_eq!(drops.len(), 1, "the footage drop reached the engine");
		assert_eq!(drops[0].id, 3);
		assert_eq!(drops[0].track_kind, TrackKind::Video);
		assert_eq!(drops[0].track_index, 0);
		assert!(drops[0].time.0 >= 0);
	});

	// The generator effect becomes a standalone clip on the pointed
	// track (the drop point sits on the first row).
	let clips_before = cx.read(|app| {
		panel
			.read(app)
			.engine
			.read(app)
			.track(0)
			.unwrap()
			.clips()
			.len()
	});
	let source = cx
		.debug_bounds("effect-drag-source")
		.expect("effect source");
	dispatch_drag(&mut cx, source.center(), hover, ruler);
	cx.run_until_parked();
	cx.read(|app| {
		let panel = panel.read(app);
		assert!(panel.effect_drop.is_none(), "the effect drop is consumed");
		assert_eq!(
			panel.engine.read(app).track(0).unwrap().clips().len(),
			clips_before + 1,
			"the generator clip landed on the pointed track"
		);
	});
}

/// With no open sequence the footage drop is handed back to the shell and
/// the work-area helpers fall back to a one-frame sequence.
#[gpui_kit::test]
async fn no_sequence_paths_use_the_fallbacks(cx: &mut TestAppContext) {
	use std::cell::Cell;
	use std::rc::Rc;
	cx.update(|cx| {
		gpui_kit::init(cx);
		cx.init_colors();
	});
	let window = cx.open_window(size(px(1280.0), px(720.0)), |window, cx| {
		let engine = cx.new(crate::oakui::RealEngine::create);
		let timeline = cx.new(|cx| TimelineView::new(engine.clone(), window, cx).zoom(2.0));
		let panel = cx.new(|cx| TimelinePanel::new(engine, timeline, window, cx));
		DropHost { panel }
	});
	cx.run_until_parked();
	let host = window.root(cx).expect("host root");
	let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
	let panel = cx.read(|app| host.read(app).panel.clone());

	let emitted = Rc::new(Cell::new(false));
	cx.update(|_window, app| {
		let emitted = emitted.clone();
		app.subscribe(
			&panel,
			move |_panel: Entity<TimelinePanel<crate::oakui::RealEngine>>,
			      _event: &FootageDropNeedsSequence,
			      _cx| {
				emitted.set(true);
			},
		)
		.detach();
		panel.update(app, |panel, cx| {
			panel.footage_drop = Some(FootageDropTarget {
				track_kind: TrackKind::Video,
				track_index: 0,
				time: Frame(5),
				length: 1,
			});
			panel.finish_footage_drop(&FootageDrag(vec![3]), cx);
		});
	});
	assert!(
		emitted.get(),
		"the shell is asked to set up a sequence first"
	);
	cx.read(|app| assert!(panel.read(app).footage_drop.is_none()));

	// No sequence length is available: the fallbacks use the playhead.
	cx.update(|_window, app| {
		panel.update(app, |panel, cx| {
			panel.set_point_at_playhead(false, cx);
			panel.set_point_at_playhead(true, cx);
			assert!(panel.reset_in(cx));
			assert!(panel.reset_out(cx));
		});
	});
}
