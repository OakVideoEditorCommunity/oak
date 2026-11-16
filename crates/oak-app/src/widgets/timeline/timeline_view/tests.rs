use super::*;
use crate::widgets::timeline::FrameRate;

#[test]
fn reshape_work_area_moves_edges_within_bounds() {
	let band = FrameRange::new(Frame(20), Frame(80));
	// In edge: clamped so the band never collapses or inverts.
	let reshaped = reshape_work_area(Some(band), Frame(20), Frame(50), EdgeKind::In, 1000);
	assert_eq!(reshaped, FrameRange::new(Frame(50), Frame(80)));
	let reshaped = reshape_work_area(Some(band), Frame(20), Frame(95), EdgeKind::In, 1000);
	assert_eq!(reshaped, FrameRange::new(Frame(79), Frame(80)));
	// Out edge.
	let reshaped = reshape_work_area(Some(band), Frame(80), Frame(40), EdgeKind::Out, 1000);
	assert_eq!(reshaped, FrameRange::new(Frame(20), Frame(40)));
	let reshaped = reshape_work_area(Some(band), Frame(80), Frame(10), EdgeKind::Out, 1000);
	assert_eq!(reshaped, FrameRange::new(Frame(20), Frame(21)));
}

#[test]
fn reshape_work_area_whole_replaces_or_creates_the_band() {
	// Forward drag: press-to-cursor range.
	let reshaped = reshape_work_area(
		Some(FrameRange::new(Frame(0), Frame(10))),
		Frame(30),
		Frame(70),
		EdgeKind::Whole,
		1000,
	);
	assert_eq!(reshaped, FrameRange::new(Frame(30), Frame(70)));
	// Backward drag: the range is normalized (start <= end).
	let reshaped = reshape_work_area(
		Some(FrameRange::new(Frame(0), Frame(10))),
		Frame(70),
		Frame(30),
		EdgeKind::Whole,
		1000,
	);
	assert_eq!(reshaped, FrameRange::new(Frame(30), Frame(70)));
	// Creating a band from nothing uses the virtual 0..length band as the
	// base for edge moves.
	let reshaped = reshape_work_area(None, Frame(0), Frame(40), EdgeKind::Out, 1000);
	assert_eq!(reshaped, FrameRange::new(Frame(0), Frame(40)));
}

/// A minimal row snapshot for the ghost-rect math (the rect only reads
/// `index` and `height`; the rest stay at inert defaults).
fn ghost_row(index: usize, height: f32) -> RowData {
	RowData {
		index,
		name: SharedString::from(format!("V{}", index + 1)),
		kind: TrackKind::Video,
		height,
		y: 0.0,
		locked: false,
		muted: false,
		solo: false,
		visible: true,
		clips: Vec::new(),
	}
}

#[test]
fn clip_ghost_rect_maps_target_frame_and_track() {
	let rows = vec![ghost_row(0, 48.0), ghost_row(1, 64.0), ghost_row(2, 32.0)];
	// Target frame 100 at zoom 2, scrolled by 40 px: x = 200 - 40.
	let rect = clip_ghost_rect(Frame(100), 1, Frame(25), 2.0, px(40.0), &rows);
	assert_eq!(rect.x, px(160.0));
	// y accumulates the rows above track 1 (row 0's 48 px).
	assert_eq!(rect.y, px(48.0));
	// The ghost spans the clip's own length at the target row's height.
	assert_eq!(rect.width, px(50.0));
	assert_eq!(rect.height, px(64.0));
}

#[test]
fn clip_ghost_rect_follows_cross_track_drag() {
	let rows = vec![ghost_row(0, 48.0), ghost_row(1, 64.0), ghost_row(2, 32.0)];
	// Dragged to track 2: y = 48 + 64, height = row 2's height.
	let rect = clip_ghost_rect(Frame(30), 2, Frame(10), 1.0, px(0.0), &rows);
	assert_eq!(rect.y, px(112.0));
	assert_eq!(rect.height, px(32.0));
	assert_eq!(rect.x, px(30.0));
	assert_eq!(rect.width, px(10.0));
}

#[test]
fn clip_ghost_rect_clamps_out_of_range_track_to_last_row() {
	let rows = vec![ghost_row(0, 48.0), ghost_row(1, 64.0)];
	// A track index beyond the last row (track removed mid-drag) lands
	// on the last row's band, mirroring `track_at_y`'s fallback.
	let rect = clip_ghost_rect(Frame(5), 7, Frame(3), 1.0, px(0.0), &rows);
	assert_eq!(rect.y, px(48.0));
	assert_eq!(rect.height, px(64.0));

	// With no rows at all the rect falls back to the minimum row height.
	let empty = clip_ghost_rect(Frame(5), 0, Frame(3), 1.0, px(0.0), &[]);
	assert_eq!(empty.height, px(MIN_TRACK_HEIGHT));
}

#[test]
fn clip_ghost_rect_never_collapses_below_four_pixels() {
	let rows = vec![ghost_row(0, 48.0)];
	let rect = clip_ghost_rect(Frame(0), 0, Frame(1), 0.01, px(0.0), &rows);
	assert_eq!(rect.width, px(4.0));
}

/// A minimal clip snapshot for the roll/track-select math.
fn clip_snapshot(id: u64, start: i64, end: i64) -> ClipRenderData {
	ClipRenderData {
		id: ClipId(id),
		range: FrameRange::new(Frame(start), Frame(end)),
		label: "clip".into(),
		color: hsla(0.4, 0.5, 0.5, 1.0),
		enabled: true,
		multicam: false,
		media_in: Frame::ZERO,
		in_transition: None,
		out_transition: None,
	}
}

#[test]
fn timeline_tool_index_roundtrips() {
	for (index, tool) in TimelineTool::ALL.iter().enumerate() {
		assert_eq!(tool.index(), index, "{tool:?} sits at toolbar slot {index}");
		assert_eq!(TimelineTool::from_index(index), Some(*tool));
	}
	assert_eq!(TimelineTool::from_index(TimelineTool::ALL.len()), None);
}

#[test]
fn roll_pair_detects_adjacent_clips() {
	// Back-to-back clips share a boundary; the pair's legal range is the
	// left clip's start .. the right clip's end.
	let clips = vec![clip_snapshot(0, 0, 10), clip_snapshot(1, 10, 20)];
	let pair = roll_pair(&clips, 0).expect("adjacent clips form a pair");
	assert_eq!(
		pair,
		RollPair {
			a: ClipId(0),
			b: ClipId(1),
			boundary: Frame(10),
			min: Frame(0),
			max: Frame(20),
		}
	);
	// The right clip of a pair has no following partner.
	assert_eq!(roll_pair(&clips, 1), None);
	// A gap (or overlap) between clips breaks the pair.
	let gapped = vec![clip_snapshot(0, 0, 10), clip_snapshot(1, 15, 25)];
	assert_eq!(roll_pair(&gapped, 0), None);
}

#[test]
fn track_select_clips_picks_under_and_right() {
	let clips = vec![clip_snapshot(0, 0, 10), clip_snapshot(1, 12, 30)];
	// Clicking inside clip 0 picks clip 0 and everything after it.
	assert_eq!(
		track_select_clips(&clips, Frame(5)),
		vec![ClipId(0), ClipId(1)]
	);
	// Clicking in the gap between clips picks only the clips right of the
	// pointer.
	assert_eq!(track_select_clips(&clips, Frame(11)), vec![ClipId(1)]);
	// Clicking at the very start of the sequence picks the whole track.
	assert_eq!(
		track_select_clips(&clips, Frame(0)),
		vec![ClipId(0), ClipId(1)]
	);
}

/// A minimal data source for the interaction tests: one video track, no
/// clips.
struct OneTrackSource;

/// Root view hosting the timeline, recording every emitted event.
struct Host {
	timeline: Entity<TimelineView<OneTrackSource>>,
	events: Vec<TimelineEvent>,
}

impl Render for Host {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size_full().child(self.timeline.clone())
	}
}

struct StubClip;

impl ClipData for StubClip {
	fn id(&self) -> ClipId {
		ClipId(0)
	}

	fn range(&self) -> FrameRange {
		FrameRange::new(Frame::ZERO, Frame(10))
	}

	fn media_in(&self) -> Frame {
		Frame::ZERO
	}

	fn label(&self) -> SharedString {
		"clip".into()
	}
}

struct StubTrack;

impl TrackData for StubTrack {
	type Clip = StubClip;

	fn kind(&self) -> TrackKind {
		TrackKind::Video
	}

	fn name(&self) -> SharedString {
		"V1".into()
	}

	fn height(&self) -> Pixels {
		px(48.0)
	}

	fn clips(&self) -> &[Self::Clip] {
		&[]
	}
}

impl TimelineDataSource for OneTrackSource {
	type Track = StubTrack;

	fn frame_rate(&self) -> FrameRate {
		FrameRate::new(25, 1)
	}

	fn sequence_length(&self) -> Frame {
		Frame(1000)
	}

	fn track_count(&self) -> usize {
		1
	}

	fn track(&self, index: usize) -> Option<Self::Track> {
		(index == 0).then_some(StubTrack)
	}
}

/// Clicking a track header's toggle glyph emits a
/// `TrackToggleRequested` edit request (and does NOT toggle the header's
/// track selection).
#[test]
fn seek_scrolls_only_when_the_playhead_leaves_the_viewport() {
	use gpui_kit::{TestAppContext, VisualTestContext, size};
	use std::ops::Deref;

	let mut test_app = TestAppContext::single();
	test_app.update(|cx| cx.init_colors());
	let window = test_app.open_window(size(px(800.), px(200.)), |window, cx| {
		let source = cx.new(|_cx| OneTrackSource);
		let timeline = cx.new(|cx| TimelineView::new(source, window, cx));
		Host {
			events: Vec::new(),
			timeline,
		}
	});
	let any_window = *window.deref();
	let host = window.root(&mut test_app).expect("host root");
	let cx = VisualTestContext::from_window(any_window, &test_app).into_mut();
	let timeline = cx.read(|app| host.read(app).timeline.clone());
	cx.update(|_window, cx| {
		timeline.update(cx, |timeline, cx| {
			timeline.clip_area_width = 100.0;
			timeline.seek(Frame(50), cx);
			assert_eq!(timeline.state.scroll_offset.x, px(0.0));
			timeline.seek(Frame(95), cx);
			assert_eq!(timeline.state.scroll_offset.x, px(5.0));
			timeline.seek(Frame(70), cx);
			assert_eq!(timeline.state.scroll_offset.x, px(5.0));
			timeline.seek(Frame(0), cx);
			assert_eq!(timeline.state.scroll_offset.x, px(0.0));
			timeline.seek(Frame(500), cx);
			assert_eq!(timeline.state.scroll_offset.x, px(410.0));
		});
	});
}

#[test]
fn track_toggle_click_emits_request() {
	use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, size};
	use std::ops::Deref;

	let mut test_app = TestAppContext::single();
	test_app.update(|cx| cx.init_colors());
	let window = test_app.open_window(size(px(800.), px(200.)), |window, cx| {
		let source = cx.new(|_cx| OneTrackSource);
		let timeline = cx.new(|cx| TimelineView::new(source, window, cx));
		let host = Host {
			events: Vec::new(),
			timeline,
		};
		cx.subscribe(
			&host.timeline,
			|host: &mut Host,
			 _t: Entity<TimelineView<OneTrackSource>>,
			 event: &TimelineEvent,
			 _cx: &mut Context<Host>| {
				host.events.push(event.clone());
			},
		)
		.detach();
		host
	});
	let any_window = *window.deref();
	let host = window.root(&mut test_app).expect("host root");
	let cx = VisualTestContext::from_window(any_window, &test_app).into_mut();

	let toggle = cx
		.debug_bounds("track-toggle-0-L")
		.expect("the lock glyph rendered");
	cx.simulate_click(toggle.center(), Modifiers::none());
	cx.run_until_parked();

	let events = cx.read(|app| host.read(app).events.clone());
	assert!(
		events.contains(&TimelineEvent::TrackToggleRequested {
			track: 0,
			toggle: TrackHeaderEvent::ToggleLock,
		}),
		"the lock toggle click emitted its request, got {events:?}"
	);
	assert!(
		!events
			.iter()
			.any(|e| matches!(e, TimelineEvent::TrackSelected { .. })),
		"the toggle click must not bubble into a track selection"
	);
}
