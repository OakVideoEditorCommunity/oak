//! The timeline view: layout, interaction handling, and edit-request events.
//!
//! [`TimelineView`] is the top-level widget. It is generic over the app's
//! [`TimelineDataSource`] implementation and owns a [`TimelineState`] for
//! view-local state (zoom, scroll, playhead, selection).
//!
//! # Layout
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │ ruler (timecode ticks, work-area band, markers)      │
//! ├─────────┬────────────────────────────────────────────┤
//! │ track   │  clip area (per-track rows)                │
//! │ headers │   ┌──────┐   ┌─────────┐                   │
//! │  V1     │   │ clip │   │ clip    │        ┆ playhead │
//! │  V2     │        ┌──────────┐            ┆          │
//! │  A1     │   ┌─────────┐                 ┆          │
//! ├─────────┴────────────────────────────────────────────┤
//! │ horizontal scrollbar                                 │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! The ruler is painted by [`TimelineRuler`](super::TimelineRuler), headers
//! by [`TrackHeader`](super::TrackHeader), clips by
//! [`ClipElement`](super::ClipElement), and the playhead by
//! [`PlayheadElement`](super::PlayheadElement). The clip area lays out one
//! row per track from the data source, sized and stacked purely from
//! [`TrackData::height`](super::TrackData::height).
//!
//! # Interactions
//!
//! * **Ruler click** — seek: moves the playhead to the pointed frame
//!   (through [`TimelineState::set_playhead`]) and emits
//!   [`TimelineEvent::PlayheadChanged`].
//! * **Clip drag** — move: horizontal motion shifts the clip in time;
//!   vertical motion across a track boundary requests a cross-track move
//!   (never onto a locked track). While dragging, the clip snaps to nearby
//!   snap points (clip edges, playhead, work-area edges, markers) when
//!   [`TimelineState::snap_enabled`]. On mouse-up the widget emits
//!   [`TimelineEvent::ClipMoveRequested`]; the host expands the request to
//!   the clip's transitive [`ClipData::linked_ids`](super::ClipData::linked_ids)
//!   group — and the widget does not change anything itself.
//! * **Clip edge drag** — trim: the ~6 px hit zones at each clip edge start
//!   a trim gesture. Emits [`TimelineEvent::ClipTrimRequested`]; the host
//!   expands the request to linked clips and adjusts `media_in` for
//!   [`TrimEdge::Start`].
//! * **Marquee** — rubber-band selection: dragging on empty clip-area space
//!   draws a selection rect; on release the hit clips go through
//!   [`TimelineState::select_range`] and [`TimelineEvent::SelectionChanged`]
//!   is emitted.
//! * **Scroll wheel** — horizontal pan; **ctrl-scroll / pinch** — zoom
//!   anchored at the cursor via [`TimelineState::set_zoom`], emitting
//!   [`TimelineEvent::ZoomChanged`].
//! * **Header separator drag** — track height resize, emitting
//!   [`TimelineEvent::TrackHeightChanged`].
//!
//! # Edit model (read this before wiring events)
//!
//! The widget **never mutates the model**. Every gesture ends in a
//! [`TimelineEvent`] describing the requested edit. The host applies it
//! through its engine and undo stack — keeping the engine the single source
//! of truth — and then calls `cx.notify()` on the data-source entity, which
//! makes the view re-read and repaint. If the engine rejects the edit
//! (locked track, collision, …), simply don't notify and the gesture has no
//! visible effect.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use gpui_kit::{
	canvas, colors::DefaultColors, div, hsla, prelude::*, px, AnyElement, App, Context,
	DragMoveEvent, ElementId, Entity, EventEmitter, FocusHandle, Focusable, Hsla, MouseButton,
	MouseDownEvent, PinchEvent, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString,
	Window,
};

use super::{
	clip::{ClipContent, ClipDecorator, ClipElement, NoopClipDecorator, TRIM_HANDLE_WIDTH},
	data::{ClipData, ClipId, TimelineDataSource, TrackData, TrackKind},
	playhead::PlayheadElement,
	ruler::{RulerMarker, TimelineRuler},
	state::TimelineState,
	time::{snap, Frame, FrameRange, SnapKind, SnapPoint},
	track_header::{TrackHeader, TrackHeaderEvent, TrackToggleHandler},
};

/// Which edge of a clip a trim gesture grabbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrimEdge {
	/// The clip's left (in) edge. Trimming it changes both the clip's start
	/// and its [`ClipData::media_in`](super::ClipData::media_in).
	Start,
	/// The clip's right (out) edge.
	End,
}

/// The timeline's editing tools. The toolbar buttons and the host's Tools
/// menu map onto these; the view dispatches its gestures by the active tool.
///
/// Order is significant: [`TimelineTool::from_index`] resolves the toolbar
/// button index (the panel's `TOOLS` array uses exactly this order), and
/// [`TimelineTool::ALL`] lists every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimelineTool {
	/// Default pointer: select, move, trim, marquee.
	Select,
	/// Razor: a press on a clip splits it at the pointed frame.
	Razor,
	/// Ripple trim: trimming one edge also slides every following clip on the
	/// track so no gap opens (the host composes the command).
	Ripple,
	/// Slip: dragging a clip slides its media-in while the range stays put.
	Slip,
	/// Roll: dragging the boundary between two adjacent clips extends one
	/// edge while the other retracts by the same amount.
	Roll,
	/// Zoom: a press zooms in around the pointer; secondary (ctrl/cmd) zooms
	/// out.
	Zoom,
	/// Slide: dragging a clip moves it and trims the neighbors to compensate.
	Slide,
	/// Track select: a press selects every clip under or to the right of the
	/// pointer on the pointed track (secondary selects the whole track).
	TrackSelect,
}

impl TimelineTool {
	/// Every tool, in toolbar order (index = button order).
	pub const ALL: [TimelineTool; 8] = [
		TimelineTool::Select,
		TimelineTool::Razor,
		TimelineTool::Ripple,
		TimelineTool::Slip,
		TimelineTool::Roll,
		TimelineTool::Zoom,
		TimelineTool::Slide,
		TimelineTool::TrackSelect,
	];

	/// The tool at toolbar `index`, or `None` when out of range.
	pub fn from_index(index: usize) -> Option<TimelineTool> {
		Self::ALL.get(index).copied()
	}

	/// This tool's toolbar index (the inverse of [`Self::from_index`]).
	pub const fn index(self) -> usize {
		match self {
			TimelineTool::Select => 0,
			TimelineTool::Razor => 1,
			TimelineTool::Ripple => 2,
			TimelineTool::Slip => 3,
			TimelineTool::Roll => 4,
			TimelineTool::Zoom => 5,
			TimelineTool::Slide => 6,
			TimelineTool::TrackSelect => 7,
		}
	}
}

/// An edit or view-state change requested by the timeline widget.
///
/// The widget emits these and then **does nothing itself**. The host
/// application applies the request through its engine and undo stack, then
/// calls `cx.notify()` on the data-source entity so the view re-reads.
///
/// # Wiring into Oak
///
/// Each variant maps onto an undoable facade call in `oakengine`:
///
/// * `ClipMoveRequested` → `engine.move_clip(clip, new_track, new_start)`
///   (ripple off), wrapped in an undo command together with the clip's
///   linked group.
/// * `ClipTrimRequested` → `engine.trim_clip(clip, edge, new_frame)`,
///   likewise grouped with linked clips.
/// * `PlayheadChanged` → `engine.set_playhead(frame)` — not undoable.
/// * `SelectionChanged` → update the app's selection state; drives the
///   inspector/effect stack panels.
/// * `TrackHeightChanged` / `ZoomChanged` → persist as per-sequence view
///   state; not undoable.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineEvent {
	/// The user asked to move a clip (and, per the gesture docs on
	/// [`TimelineView`], its linked group) to a new position.
	///
	/// `new_track` is an index into
	/// [`TimelineDataSource::track`];
	/// `new_start` is the requested [`FrameRange`](super::FrameRange) start
	/// after snapping. The engine must validate collisions and track
	/// compatibility; the widget only guarantees the source track was not
	/// locked.
	ClipMoveRequested {
		/// The clip the gesture grabbed. The host expands this to the full
		/// linked group.
		clip: ClipId,
		/// Target track index.
		new_track: usize,
		/// Requested new start frame.
		new_start: Frame,
	},

	/// The user asked to trim one edge of a clip.
	///
	/// `new_frame` is the requested new position of the grabbed `edge`,
	/// after snapping and after clamping to the neighboring clips and to
	/// zero minimum length. For [`TrimEdge::Start`] the host must also
	/// adjust `media_in` by the same delta.
	ClipTrimRequested {
		/// The trimmed clip (expand to the linked group, as with moves).
		clip: ClipId,
		/// Which edge was grabbed.
		edge: TrimEdge,
		/// Requested new frame position of that edge.
		new_frame: Frame,
	},

	/// The razor tool pressed inside a clip: split it at `time`.
	///
	/// `time` is guaranteed to lie strictly inside the clip's range (a press
	/// on an edge bubbles to the clip area instead). The host must split the
	/// clip into two — one undoable entry — and re-notify.
	ClipSplitRequested {
		/// The clip to split.
		clip: ClipId,
		/// The requested split frame (inside the clip).
		time: Frame,
	},

	/// The ripple tool finished a trim: trim the clip and slide every later
	/// clip on the track so no gap opens between the trimmed edge and its
	/// follower. The host composes this as ONE undoable entry.
	ClipRippleTrimRequested {
		/// The trimmed clip.
		clip: ClipId,
		/// Which edge was grabbed.
		edge: TrimEdge,
		/// Requested new frame position of that edge (the ripple ripple point).
		new_frame: Frame,
	},

	/// The roll tool dragged the boundary between two adjacent clips.
	///
	/// `clip_a` is the left clip, `clip_b` the right one, and `new_frame` is
	/// the requested new boundary position (strictly between `clip_a`'s start
	/// and `clip_b`'s end). The host extends `clip_a`'s out edge while
	/// retracting `clip_b`'s in edge by the same delta — one undoable entry.
	ClipRollRequested {
		/// The left clip of the pair.
		clip_a: ClipId,
		/// The right clip of the pair.
		clip_b: ClipId,
		/// Requested new boundary frame.
		new_frame: Frame,
	},

	/// The slide tool dragged a clip to a new position; the host moves it and
	/// trims the neighbors to compensate — one undoable entry.
	ClipSlideRequested {
		/// The slid clip.
		clip: ClipId,
		/// Requested new start frame.
		new_start: Frame,
	},

	/// The slip tool dragged a clip's content: `new_media_in` is the
	/// requested source offset (in media frames) while the clip's range stays
	/// put. The host updates only the media-in — one undoable entry.
	ClipSlipRequested {
		/// The slipped clip.
		clip: ClipId,
		/// Requested new media-in frame (>= 0).
		new_media_in: Frame,
	},

	/// The playhead moved, by any means (ruler seek, keyboard, playback
	/// ticker). Carries the new position. Not undoable.
	PlayheadChanged(Frame),

	/// The selection changed. The new set is readable from
	/// [`TimelineView::selection`]. Not undoable.
	SelectionChanged,

	/// A track header was clicked, toggling the track's selection state.
	///
	/// The widget keeps the selected-track set locally (readable from
	/// [`TimelineView::selected_tracks`]); the host may persist it. Not
	/// undoable.
	TrackSelected {
		/// Index of the clicked track.
		track: usize,
		/// Whether the track is now selected.
		selected: bool,
	},

	/// A track header's toggle glyph was clicked. The widget changes nothing
	/// itself; the host applies the toggle through its engine (Oak: the
	/// undoable track flag setters) and notifies the data source.
	TrackToggleRequested {
		/// Index of the track whose toggle was clicked.
		track: usize,
		/// The requested toggle.
		toggle: TrackHeaderEvent,
	},

	/// The user dragged a transition wedge's edge to change its length.
	///
	/// `new_length` is the requested transition duration in frames, clamped
	/// to the clip length and to a one-frame minimum. The host validates
	/// against the actual transition implementation and re-notifies.
	TransitionChanged {
		/// The clip whose transition was grabbed.
		clip: ClipId,
		/// Which edge the transition sits on.
		edge: TrimEdge,
		/// Requested new transition length.
		new_length: Frame,
	},

	/// A track's height changed via header-separator drag. The host should
	/// write this back so [`TrackData::height`](super::TrackData::height)
	/// returns it on the next read.
	TrackHeightChanged {
		/// Index of the resized track.
		track: usize,
		/// New row height.
		height: Pixels,
	},

	/// The user is dragging on the ruler, reshaping the work area. `start` /
	/// `end` are the NEW in/out frames; the widget already updated its local
	/// band (`[`TimelineState::work_area`]`), and the host should apply the
	/// new range **live** (not undoable), mirroring Olive's drag behavior.
	WorkAreaPreview {
		/// The new work-area in point.
		start: Frame,
		/// The new work-area out point.
		end: Frame,
	},

	/// The ruler work-area drag finished. The host commits the new range as
	/// ONE undoable entry whose "old" side is `old_start`/`old_end` (the
	/// range at drag start, before any live preview was applied).
	WorkAreaCommitted {
		/// The committed work-area in point.
		start: Frame,
		/// The committed work-area out point.
		end: Frame,
		/// The in point at drag start (the undo side of the command).
		old_start: Frame,
		/// The out point at drag start (the undo side of the command).
		old_end: Frame,
	},

	/// The zoom (pixels per frame) changed via ctrl-scroll or pinch.
	/// Persist as view state if desired.
	ZoomChanged(f32),

	/// The user right-clicked somewhere in the widget. The widget opens no
	/// menu itself; `hit` tells the host what was under the pointer so it
	/// can assemble the matching context menu at `position`. Not undoable.
	ContextMenuRequested {
		/// The click position in window coordinates (suitable for placing a
		/// popup).
		position: Point<Pixels>,
		/// What the click hit.
		hit: TimelineHit,
	},
}

/// What a right-click hit in the timeline widget (the
/// [`TimelineEvent::ContextMenuRequested`] payload).
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineHit {
	/// A clip.
	Clip(ClipId),
	/// The empty track area: the track under the pointer plus the frame at
	/// the pointer's x position.
	Empty {
		/// Index of the track under the pointer.
		track: usize,
		/// The frame at the pointer's x position.
		frame: Frame,
	},
	/// A track header.
	TrackHead(usize),
	/// A ruler marker (the marker's frame).
	RulerMarker(Frame),
	/// The ruler, away from any marker (the frame at the pointer).
	Ruler(Frame),
}

/// The video-editing timeline widget.
///
/// Construct with [`TimelineView::new`] over an [`Entity`] of your
/// [`TimelineDataSource`] implementation, place it in your layout like any
/// other view, and subscribe to its [`TimelineEvent`]s to apply edits:
///
/// ```ignore
/// let timeline = cx.new(|cx| TimelineView::new(model.clone(), window, cx));
/// cx.subscribe(&timeline, |this, timeline, event: &TimelineEvent, cx| {
///     match event {
///         TimelineEvent::ClipMoveRequested { clip, new_track, new_start } => {
///             this.engine.move_clip(*clip, *new_track, *new_start, cx); // undoable
///             this.model.update(cx, |_, cx| cx.notify());
///         }
///         // ...
///     }
/// }).detach();
/// ```
///
/// See the module-level docs for the layout, the full interaction list, and
/// the edit-request contract.
pub struct TimelineView<D: TimelineDataSource> {
	source: Entity<D>,
	/// View-local state (zoom, scroll, playhead, selection). Public for
	/// read access; mutate through [`TimelineState`]'s methods to preserve
	/// invariants.
	pub state: TimelineState,
	/// The active editing tool; the view dispatches its gestures by it.
	/// Set through [`TimelineView::set_tool`].
	pub tool: TimelineTool,
	/// The set of tracks selected via their headers.
	selected_tracks: BTreeSet<usize>,
	/// Rich clip content (thumbnails / waveforms), replaced by the host.
	decorator: std::sync::Arc<std::sync::RwLock<dyn ClipDecorator>>,
	focus_handle: FocusHandle,
	/// The clip area's window-space origin, captured on every layout by a
	/// canvas child (the right-click hit test converts window positions
	/// through it).
	clip_area_origin: Point<Pixels>,
	/// The clip area's width, captured on the same layout (Show-All fits
	/// the sequence into it).
	clip_area_width: f32,
	/// The in-progress clip-move drag, recorded by the drag constructor:
	/// the move ghost's geometry follows it during render. gpui-kit keeps
	/// `App::active_drag` private, so the timeline tracks its own.
	active_clip_drag: Option<Arc<RwLock<ClipDrag>>>,
}

/// Width of the track-headers column, in pixels. Public so host panels can
/// convert drop coordinates into clip-area space.
pub const HEADER_WIDTH: f32 = 160.0;

/// Height of the ruler row, in pixels. Public so host panels can convert
/// drop coordinates into clip-area space.
pub const RULER_HEIGHT: f32 = 32.0;

/// Minimum row height enforced by the height-resize drag, in pixels.
/// Public so host panels can reproduce the track row layout for drops.
pub const MIN_TRACK_HEIGHT: f32 = 24.0;

/// Snap engagement threshold, in pixels. Public: the transition drop's
/// snap window derives from it (8px at the current zoom, in frames).
pub const SNAP_THRESHOLD_PX: f32 = 8.0;

impl<D: TimelineDataSource> TimelineView<D> {
	/// Creates a timeline view over `source`.
	///
	/// Subscribes to `source`'s notifications: after the host applies an
	/// edit and calls `cx.notify()` on the source entity, the view re-reads
	/// all data and repaints.
	pub fn new(source: Entity<D>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
		let focus_handle = cx.focus_handle();
		cx.observe(&source, |_this, _source, cx| cx.notify())
			.detach();
		TimelineView {
			source,
			state: TimelineState::new(),
			tool: TimelineTool::Select,
			selected_tracks: BTreeSet::new(),
			decorator: std::sync::Arc::new(std::sync::RwLock::new(NoopClipDecorator)),
			focus_handle,
			clip_area_origin: Point::default(),
			clip_area_width: 0.0,
			active_clip_drag: None,
		}
	}

	/// The active editing tool.
	pub fn tool(&self) -> TimelineTool {
		self.tool
	}

	/// Sets the active editing tool, repainting if it changed.
	pub fn set_tool(&mut self, tool: TimelineTool, cx: &mut Context<Self>) {
		if self.tool != tool {
			self.tool = tool;
			cx.notify();
		}
	}

	/// Builder: installs the host's rich-clip decorator (M12 P4 — Oak's
	/// waveform decorator). The default is the no-op decorator.
	pub fn clip_decorator(
		mut self,
		decorator: std::sync::Arc<std::sync::RwLock<dyn ClipDecorator>>,
	) -> Self {
		self.decorator = decorator;
		self
	}

	/// Replaces the clip decorator after construction (the app wires its
	/// waveform cache once the engine is up).
	pub fn set_clip_decorator(
		&mut self,
		decorator: std::sync::Arc<std::sync::RwLock<dyn ClipDecorator>>,
	) {
		self.decorator = decorator;
	}

	/// The set of tracks selected via header clicks.
	pub fn selected_tracks(&self) -> &BTreeSet<usize> {
		&self.selected_tracks
	}

	/// Builder: sets the initial zoom (pixels per frame), clamped to
	/// [`MIN_ZOOM`](super::MIN_ZOOM)..=[`MAX_ZOOM`](super::MAX_ZOOM).
	pub fn zoom(mut self, zoom: f32) -> Self {
		self.state.set_zoom(zoom, gpui_kit::px(0.));
		self
	}

	/// Builder: enables or disables snapping initially.
	pub fn snap_enabled(mut self, enabled: bool) -> Self {
		self.state.snap_enabled = enabled;
		self
	}

	/// The data source entity this view was created over.
	pub fn source(&self) -> &Entity<D> {
		&self.source
	}

	/// The current selection, in deterministic (id) order.
	pub fn selection(&self) -> &BTreeSet<ClipId> {
		&self.state.selection
	}

	/// Zoom to fit the whole sequence (View > Show All): the sequence's
	/// full length lands in the clip area's current width, scrolled back
	/// to the start.
	pub fn show_all(&mut self, sequence_length: Frame, cx: &mut Context<Self>) {
		if self.clip_area_width <= 0.0 || sequence_length.0 <= 0 {
			return;
		}
		let fit = self.clip_area_width / sequence_length.0 as f32;
		self.state.set_zoom(fit, px(0.0));
		self.state.scroll_offset.x = px(0.0);
		cx.emit(TimelineEvent::ZoomChanged(self.state.zoom));
		cx.notify();
	}

	/// Seeks the playhead to `frame`, clamped to the sequence, emitting
	/// [`TimelineEvent::PlayheadChanged`] if the position changed.
	pub fn seek(&mut self, frame: Frame, cx: &mut Context<Self>) {
		let seq_len = self.sequence_length(cx);
		let old = self.state.playhead;
		self.state.set_playhead(frame, seq_len);
		let old_scroll = self.state.scroll_offset.x;
		if self.clip_area_width > 0.0 {
			let margin = 24.0_f32.min(self.clip_area_width * 0.1);
			let playhead_x = self.state.point_at_frame(self.state.playhead).as_f32();
			let content_x = self.state.playhead.0 as f32 * self.state.zoom;
			if playhead_x < margin {
				self.state.scroll_offset.x = px((content_x - margin).max(0.0));
			} else if playhead_x > self.clip_area_width - margin {
				self.state.scroll_offset.x =
					px((content_x - (self.clip_area_width - margin)).max(0.0));
			}
		}
		if self.state.playhead != old {
			cx.emit(TimelineEvent::PlayheadChanged(self.state.playhead));
		}
		if self.state.playhead != old || self.state.scroll_offset.x != old_scroll {
			cx.notify();
		}
	}

	/// The sequence length from the data source.
	fn sequence_length(&self, cx: &mut Context<Self>) -> Frame {
		self.source.read(cx).sequence_length()
	}

	/// The current range of the clip with `id`, scanned from the data source.
	/// Falls back to an empty range when the clip no longer exists.
	fn clip_range(&self, id: ClipId, cx: &mut Context<Self>) -> FrameRange {
		let source = self.source.read(cx);
		for index in 0..source.track_count() {
			if let Some(track) = source.track(index) {
				for clip in track.clips() {
					if clip.id() == id {
						return clip.range();
					}
				}
			}
		}
		FrameRange::new(Frame::ZERO, Frame::ZERO)
	}

	/// Snap points gathered fresh from the data source: work-area edges,
	/// markers, the playhead, and every enabled clip edge. `dragged` (the
	/// clip being moved) is excluded so a clip never snaps to itself.
	fn snap_points(&self, dragged: Option<ClipId>, cx: &mut Context<Self>) -> Vec<SnapPoint> {
		let source = self.source.read(cx);
		let mut points = Vec::new();
		if let Some(area) = self.state.work_area {
			points.push(SnapPoint {
				frame: area.start,
				kind: SnapKind::WorkAreaEdge,
			});
			points.push(SnapPoint {
				frame: area.end,
				kind: SnapKind::WorkAreaEdge,
			});
		}
		for marker in source.markers() {
			points.push(SnapPoint {
				frame: marker.frame,
				kind: SnapKind::Marker,
			});
		}
		points.push(SnapPoint {
			frame: self.state.playhead,
			kind: SnapKind::Playhead,
		});
		for index in 0..source.track_count() {
			if let Some(track) = source.track(index) {
				for clip in track.clips() {
					if clip.is_enabled() && Some(clip.id()) != dragged {
						let range = clip.range();
						points.push(SnapPoint {
							frame: range.start,
							kind: SnapKind::ClipStart,
						});
						points.push(SnapPoint {
							frame: range.end,
							kind: SnapKind::ClipEnd,
						});
					}
				}
			}
		}
		points
	}

	/// Index of the track whose row contains screen `y` (relative to the
	/// clip area's top). Falls back to the last track when below all rows.
	fn track_at_y(&self, y: f32, cx: &mut Context<Self>) -> usize {
		let source = self.source.read(cx);
		let count = source.track_count();
		let mut acc = 0.0;
		for index in 0..count {
			if let Some(track) = source.track(index) {
				acc += track.height().as_f32().max(MIN_TRACK_HEIGHT);
				if y < acc {
					return index;
				}
			}
		}
		count.saturating_sub(1)
	}

	/// Whether the track at `index` is locked. Out-of-range tracks count as
	/// locked so drop requests onto them are rejected.
	fn track_locked(&self, index: usize, cx: &mut Context<Self>) -> bool {
		self.source
			.read(cx)
			.track(index)
			.map(|track| track.is_locked())
			.unwrap_or(true)
	}

	/// Updates a clip-move drag from the pointer position: computes the new
	/// start frame (with snapping) and the track under the cursor.
	fn update_clip_drag(
		&mut self,
		event: &DragMoveEvent<Arc<RwLock<ClipDrag>>>,
		cx: &mut Context<Self>,
	) {
		let drag = Arc::clone(event.drag(cx));
		let press = drag.read().expect("drag lock is not poisoned").press;
		let now = event.event.position - event.bounds.origin;
		let mut drag = drag.write().expect("clip drag lock is not poisoned");
		let wrapper_x = self.state.point_at_frame(drag.original_start).as_f32();
		let dx = now.x.as_f32() - (wrapper_x + press.x.as_f32());
		let mut new_start = Frame(drag.original_start.0 + (dx / self.state.zoom).round() as i64);
		let new_track = self.track_at_y(now.y.as_f32(), cx);
		if self.state.snap_enabled {
			if let Some(result) = snap(
				new_start,
				self.snap_points(Some(drag.clip), cx).into_iter(),
				px(SNAP_THRESHOLD_PX),
				self.state.zoom,
			) {
				new_start = result.frame;
			}
		}
		drag.new_start = new_start;
		drag.new_track = new_track;
		// Repaint so the move ghost follows the resolved target track/frame.
		cx.notify();
	}

	/// Updates a trim drag from the pointer position: computes the new
	/// position of the grabbed edge, clamped to keep the clip non-empty and
	/// inside the sequence, then snapped and re-clamped.
	fn update_trim_drag(
		&mut self,
		event: &DragMoveEvent<Arc<RwLock<TrimDrag>>>,
		cx: &mut Context<Self>,
	) {
		let drag = Arc::clone(event.drag(cx));
		let press = drag.read().expect("drag lock is not poisoned").press;
		let now = event.event.position - event.bounds.origin;
		let seq_len = self.sequence_length(cx);
		let mut drag = drag.write().expect("trim drag lock is not poisoned");
		let range = self.clip_range(drag.clip, cx);
		let handle_x = match drag.edge {
			TrimEdge::Start => now.x.as_f32() - press.x.as_f32(),
			TrimEdge::End => now.x.as_f32() - press.x.as_f32() + TRIM_HANDLE_WIDTH,
		};
		let mut new_frame = self.state.frame_at_point(px(handle_x));
		let clamp = |frame: Frame| match drag.edge {
			TrimEdge::Start => frame.max(Frame::ZERO).min(Frame((range.end.0 - 1).max(0))),
			TrimEdge::End => frame
				.max(Frame((range.start.0 + 1).min(seq_len.0)))
				.min(seq_len),
		};
		new_frame = clamp(new_frame);
		if self.state.snap_enabled {
			if let Some(result) = snap(
				new_frame,
				self.snap_points(Some(drag.clip), cx).into_iter(),
				px(SNAP_THRESHOLD_PX),
				self.state.zoom,
			) {
				new_frame = clamp(result.frame);
			}
		}
		drag.new_frame = new_frame;
	}

	/// Updates a track-height drag from the pointer position.
	fn update_height_drag(
		&mut self,
		event: &DragMoveEvent<Arc<RwLock<HeightDrag>>>,
		cx: &mut Context<Self>,
	) {
		let drag = Arc::clone(event.drag(cx));
		let press = drag.read().expect("drag lock is not poisoned").press;
		let now = event.event.position - event.bounds.origin;
		let mut drag = drag.write().expect("height drag lock is not poisoned");
		let dy = now.y.as_f32() - (press.y.as_f32() + drag.separator_y);
		drag.new_height = px((drag.start_height.as_f32() + dy).max(MIN_TRACK_HEIGHT));
	}

	/// Updates a marquee selection from the pointer position: hit-tests the
	/// rows intersecting the rubber-band rect and selects the clips inside.
	fn update_marquee(
		&mut self,
		event: &DragMoveEvent<MarqueeDrag>,
		rows: &[RowData],
		cx: &mut Context<Self>,
	) {
		let press = event.drag(cx).0.get();
		let now = event.event.position - event.bounds.origin;
		let mut frame_start = self.state.frame_at_point(px(press.x.as_f32()));
		let mut frame_end = self.state.frame_at_point(px(now.x.as_f32()));
		if frame_start > frame_end {
			std::mem::swap(&mut frame_start, &mut frame_end);
		}
		let y0 = press.y.min(now.y).as_f32();
		let y1 = press.y.max(now.y).as_f32();
		let mut ids = BTreeSet::new();
		for row in rows {
			if y1 >= row.y && y0 <= row.y + row.height {
				for clip in &row.clips {
					if clip.enabled
						&& clip.range.end.0 > frame_start.0
						&& clip.range.start.0 < frame_end.0
					{
						ids.insert(clip.id);
					}
				}
			}
		}
		self.state.select_range(ids);
	}

	/// Emits [`TimelineEvent::ClipMoveRequested`] for a finished clip move,
	/// unless the gesture didn't move the clip or a locked track was involved.
	/// Always repaints so the move ghost disappears after the drop.
	fn finish_clip_drag(&mut self, drag: &Arc<RwLock<ClipDrag>>, cx: &mut Context<Self>) {
		self.active_clip_drag = None;
		let (clip, original_start, original_track, new_start, new_track) = {
			let drag = drag.read().expect("clip drag lock is not poisoned");
			(
				drag.clip,
				drag.original_start,
				drag.original_track,
				drag.new_start,
				drag.new_track,
			)
		};
		if (new_start, new_track) != (original_start, original_track)
			&& !self.track_locked(original_track, cx)
			&& !self.track_locked(new_track, cx)
		{
			cx.emit(TimelineEvent::ClipMoveRequested {
				clip,
				new_track,
				new_start,
			});
		}
		cx.notify();
	}

	/// Emits [`TimelineEvent::ClipTrimRequested`] (select tool) or
	/// [`TimelineEvent::ClipRippleTrimRequested`] (ripple tool) for a finished
	/// trim — the ripple variant also slides the track's later clips, which
	/// the host composes as one undoable entry.
	fn finish_trim_drag(&mut self, drag: &Arc<RwLock<TrimDrag>>, cx: &mut Context<Self>) {
		let (clip, edge, original_frame, new_frame) = {
			let drag = drag.read().expect("trim drag lock is not poisoned");
			(drag.clip, drag.edge, drag.original_frame, drag.new_frame)
		};
		if new_frame != original_frame {
			if self.tool == TimelineTool::Ripple {
				cx.emit(TimelineEvent::ClipRippleTrimRequested {
					clip,
					edge,
					new_frame,
				});
			} else {
				cx.emit(TimelineEvent::ClipTrimRequested {
					clip,
					edge,
					new_frame,
				});
			}
			cx.notify();
		}
	}

	/// The horizontal drag distance converted to a frame delta at the current
	/// zoom — the shared base of the slide, slip, and roll gestures.
	fn drag_frame_delta(&self, press: Point<Pixels>, now: Point<Pixels>) -> i64 {
		((now.x.as_f32() - press.x.as_f32()) / self.state.zoom).round() as i64
	}

	/// Updates a slide drag from the pointer position: the clip's start shifts
	/// by the mouse delta (snapped when snap is enabled) but never before frame
	/// zero.
	fn update_slide_drag(
		&mut self,
		event: &DragMoveEvent<Arc<RwLock<SlideDrag>>>,
		cx: &mut Context<Self>,
	) {
		let drag = Arc::clone(event.drag(cx));
		let press = drag.read().expect("drag lock is not poisoned").press;
		let now = event.event.position - event.bounds.origin;
		let delta = self.drag_frame_delta(press, now);
		let mut drag = drag.write().expect("slide drag lock is not poisoned");
		let mut new_start = Frame(drag.original_start.0 + delta);
		if self.state.snap_enabled {
			if let Some(result) = snap(
				new_start,
				self.snap_points(Some(drag.clip), cx).into_iter(),
				px(SNAP_THRESHOLD_PX),
				self.state.zoom,
			) {
				new_start = result.frame;
			}
		}
		drag.new_start = new_start.max(Frame::ZERO);
		cx.notify();
	}

	/// Emits [`TimelineEvent::ClipSlideRequested`] for a finished slide unless
	/// the clip didn't move.
	fn finish_slide_drag(&mut self, drag: &Arc<RwLock<SlideDrag>>, cx: &mut Context<Self>) {
		let (clip, original_start, new_start) = {
			let drag = drag.read().expect("slide drag lock is not poisoned");
			(drag.clip, drag.original_start, drag.new_start)
		};
		if new_start != original_start {
			cx.emit(TimelineEvent::ClipSlideRequested { clip, new_start });
			cx.notify();
		}
	}

	/// Updates a slip drag from the pointer position: the media in-point moves
	/// by the mouse delta, clamped to non-negative source frames.
	fn update_slip_drag(
		&mut self,
		event: &DragMoveEvent<Arc<RwLock<SlipDrag>>>,
		cx: &mut Context<Self>,
	) {
		let drag = Arc::clone(event.drag(cx));
		let press = drag.read().expect("drag lock is not poisoned").press;
		let now = event.event.position - event.bounds.origin;
		let delta = self.drag_frame_delta(press, now);
		let mut drag = drag.write().expect("slip drag lock is not poisoned");
		drag.new_media_in = Frame((drag.original_media_in.0 + delta).max(0));
		cx.notify();
	}

	/// Emits [`TimelineEvent::ClipSlipRequested`] for a finished slip unless
	/// the media in-point didn't move.
	fn finish_slip_drag(&mut self, drag: &Arc<RwLock<SlipDrag>>, cx: &mut Context<Self>) {
		let (clip, original_media_in, new_media_in) = {
			let drag = drag.read().expect("slip drag lock is not poisoned");
			(drag.clip, drag.original_media_in, drag.new_media_in)
		};
		if new_media_in != original_media_in {
			cx.emit(TimelineEvent::ClipSlipRequested { clip, new_media_in });
			cx.notify();
		}
	}

	/// Updates a roll drag from the pointer position: the shared boundary moves
	/// with the pointer, clamped to keep both clips non-empty (inside
	/// `min + 1 .. max - 1`), then snapped and re-clamped.
	fn update_roll_drag(
		&mut self,
		event: &DragMoveEvent<Arc<RwLock<RollDrag>>>,
		cx: &mut Context<Self>,
	) {
		let drag = Arc::clone(event.drag(cx));
		let press = drag.read().expect("drag lock is not poisoned").press;
		let now = event.event.position - event.bounds.origin;
		let mut drag = drag.write().expect("roll drag lock is not poisoned");
		let clamp = |frame: Frame| Frame(frame.0.clamp(drag.min.0 + 1, drag.max.0 - 1));
		let handle_x = now.x.as_f32() - press.x.as_f32();
		let mut new_frame = clamp(self.state.frame_at_point(px(handle_x)));
		if self.state.snap_enabled {
			if let Some(result) = snap(
				new_frame,
				self.snap_points(Some(drag.clip_a), cx).into_iter(),
				px(SNAP_THRESHOLD_PX),
				self.state.zoom,
			) {
				new_frame = clamp(result.frame);
			}
		}
		drag.new_frame = new_frame;
		cx.notify();
	}

	/// Emits [`TimelineEvent::ClipRollRequested`] for a finished roll unless
	/// the boundary didn't move.
	fn finish_roll_drag(&mut self, drag: &Arc<RwLock<RollDrag>>, cx: &mut Context<Self>) {
		let (clip_a, clip_b, boundary, new_frame) = {
			let drag = drag.read().expect("roll drag lock is not poisoned");
			(drag.clip_a, drag.clip_b, drag.boundary, drag.new_frame)
		};
		if new_frame != boundary {
			cx.emit(TimelineEvent::ClipRollRequested {
				clip_a,
				clip_b,
				new_frame,
			});
			cx.notify();
		}
	}

	/// Apply a transition-resize drag move: scale the requested length by the
	/// horizontal mouse delta and clamp to the clip length.
	fn update_transition_drag(
		&mut self,
		event: &DragMoveEvent<Arc<RwLock<TransitionDrag>>>,
		cx: &mut Context<Self>,
	) {
		let drag = Arc::clone(event.drag(cx));
		let press = drag.read().expect("drag lock is not poisoned").press;
		let now = event.event.position - event.bounds.origin;
		let dx = now.x - press.x;
		let clip_len = {
			let range = self.clip_range(drag.read().unwrap().clip, cx);
			range.end - range.start
		};
		let mut drag = drag.write().expect("transition drag lock is not poisoned");
		let frames_per_pixel = 1.0 / self.state.zoom as f64;
		let delta = (dx.as_f32() as f64 * frames_per_pixel).round() as i64;
		let requested = drag.original_length.0 + delta;
		drag.new_length = Frame(requested.clamp(1, clip_len.0.max(1)));
		cx.notify();
	}

	/// Emits [`TimelineEvent::TransitionChanged`] for a finished resize.
	fn finish_transition_drag(
		&mut self,
		drag: &Arc<RwLock<TransitionDrag>>,
		cx: &mut Context<Self>,
	) {
		let (clip, edge, original_length, new_length) = {
			let drag = drag.read().expect("transition drag lock is not poisoned");
			(drag.clip, drag.edge, drag.original_length, drag.new_length)
		};
		if new_length != original_length {
			cx.emit(TimelineEvent::TransitionChanged {
				clip,
				edge,
				new_length,
			});
			cx.notify();
		}
	}

	/// Emits [`TimelineEvent::TrackHeightChanged`] for a finished resize.
	fn finish_height_drag(&mut self, drag: &Arc<RwLock<HeightDrag>>, cx: &mut Context<Self>) {
		let (track, start_height, new_height) = {
			let drag = drag.read().expect("height drag lock is not poisoned");
			(drag.track, drag.start_height, drag.new_height)
		};
		if new_height.as_f32() != start_height.as_f32() {
			cx.emit(TimelineEvent::TrackHeightChanged {
				track,
				height: new_height,
			});
			cx.notify();
		}
	}

	/// How close (in pixels) a press must be to a work-area band edge for the
	/// drag to resize that edge instead of reshaping the whole band.
	const WORK_AREA_EDGE_THRESHOLD_PX: f32 = 6.0;

	/// Updates a ruler work-area drag from the pointer position.
	///
	/// The first drag that passes the click/drag threshold (see
	/// [`RulerDrag`]) reshapes the work area: pressing near an existing band
	/// edge moves that edge, any other press replaces the whole band with the
	/// range spanned from the press point. The widget updates its local band
	/// immediately and emits [`TimelineEvent::WorkAreaPreview`] so the host
	/// can apply the range live (not undoable). A plain click never reaches
	/// here — it only seeks (the ruler's `on_mouse_down`).
	fn update_ruler_drag(
		&mut self,
		event: &DragMoveEvent<Arc<RwLock<RulerDrag>>>,
		cx: &mut Context<Self>,
	) {
		let drag = Arc::clone(event.drag(cx));
		let press = drag.read().expect("drag lock is not poisoned").press;
		let now = event.event.position - event.bounds.origin;
		let mut drag = drag.write().expect("ruler drag lock is not poisoned");
		let dx = (now.x.as_f32() - press.x.as_f32()).abs();
		if dx < 3.0 {
			// Still within the click threshold; keep seeking-only behavior.
			return;
		}
		if !drag.resizing {
			// First real drag movement: pick what the gesture reshapes. A
			// press within `WORK_AREA_EDGE_THRESHOLD_PX` of an existing band
			// edge moves that edge; otherwise the whole band is replaced.
			let edge = match self.state.work_area {
				Some(band) => {
					let in_edge = self.state.point_at_frame(band.start).as_f32();
					let out_edge = self.state.point_at_frame(band.end).as_f32();
					if (press.x.as_f32() - in_edge).abs() <= Self::WORK_AREA_EDGE_THRESHOLD_PX {
						EdgeKind::In
					} else if (press.x.as_f32() - out_edge).abs() <= Self::WORK_AREA_EDGE_THRESHOLD_PX {
						EdgeKind::Out
					} else {
						EdgeKind::Whole
					}
				}
				None => EdgeKind::Whole,
			};
			drag.edge = Some(edge);
			drag.old_band = self.state.work_area;
			drag.resizing = true;
		}
		let Some(edge) = drag.edge else {
			return;
		};
		let seq_len = self.sequence_length(cx).0.max(1);
		let frame = self.state.frame_at_point(now.x).0.clamp(0, seq_len);
		let press_frame = self.state.frame_at_point(press.x).0.clamp(0, seq_len);
		let band = reshape_work_area(
			drag.old_band,
			Frame(press_frame),
			Frame(frame),
			edge,
			seq_len,
		);
		self.state.work_area = Some(band);
		drag.new_band = Some(band);
		cx.emit(TimelineEvent::WorkAreaPreview {
			start: band.start,
			end: band.end,
		});
		cx.notify();
	}

	/// Emits [`TimelineEvent::WorkAreaCommitted`] for a finished ruler drag:
	/// the host commits the new range as ONE undoable entry whose old side is
	/// the band at drag start (`0..0` when the drag created a new band).
	fn finish_ruler_drag(&mut self, drag: &Arc<RwLock<RulerDrag>>, cx: &mut Context<Self>) {
		let (resizing, old_band, new_band) = {
			let drag = drag.read().expect("ruler drag lock is not poisoned");
			(drag.resizing, drag.old_band, drag.new_band)
		};
		if !resizing {
			return;
		}
		let Some(new) = new_band else {
			return;
		};
		// A drag that created a new band (no old one) reports the empty
		// `0..0` range as its old side; the host's undoable commit restores
		// disabled + the empty range on undo.
		let old = old_band.unwrap_or(FrameRange::new(Frame::ZERO, Frame::ZERO));
		if old != new {
			cx.emit(TimelineEvent::WorkAreaCommitted {
				start: new.start,
				end: new.end,
				old_start: old.start,
				old_end: old.end,
			});
			cx.notify();
		}
	}

	/// Emits [`TimelineEvent::SelectionChanged`] for a finished marquee.
	fn finish_marquee(&mut self, cx: &mut Context<Self>) {
		cx.emit(TimelineEvent::SelectionChanged);
		cx.notify();
	}

	// --- interaction handlers (private) ---
	//
	// The gesture logic lives in the render method's inline listeners and
	// the `update_*` / `finish_*` helpers above; these signatures are
	// retained (with `#[allow(dead_code)]`) as documentation of the
	// gesture set the view wires up.

	#[allow(dead_code)] // gesture documentation; see the section comment above
	fn on_ruler_mouse_down(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

	#[allow(dead_code)] // gesture documentation; see the section comment above
	fn on_clip_drag_move(&mut self, _clip: ClipId, _window: &mut Window, _cx: &mut Context<Self>) {}

	#[allow(dead_code)] // gesture documentation; see the section comment above
	fn on_clip_drag_drop(&mut self, _clip: ClipId, _window: &mut Window, _cx: &mut Context<Self>) {}

	#[allow(dead_code)] // gesture documentation; see the section comment above
	fn on_trim_drag(
		&mut self,
		_clip: ClipId,
		_edge: TrimEdge,
		_window: &mut Window,
		_cx: &mut Context<Self>,
	) {
	}

	#[allow(dead_code)] // gesture documentation; see the section comment above
	fn on_marquee(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

	#[allow(dead_code)] // gesture documentation; see the section comment above
	fn on_scroll(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
}

impl<D: TimelineDataSource> Focusable for TimelineView<D> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<D: TimelineDataSource> EventEmitter<TimelineEvent> for TimelineView<D> {}

impl<D: TimelineDataSource> Render for TimelineView<D> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let state = self.state.clone();
		// The active tool shapes which interactions are wired onto the clip
		// area and the clip wrappers below.
		let tool = self.tool;
		let source = self.source.read(cx);
		let frame_rate = source.frame_rate();
		let seq_len = source.sequence_length();

		// Snapshot the rows: one per track, stacked from the top of the clip
		// area, with y positions accumulated from the model's track heights.
		let mut rows: Vec<RowData> = Vec::new();
		let mut y = 0.0;
		for index in 0..source.track_count() {
			if let Some(track) = source.track(index) {
				let height = track.height().as_f32().max(MIN_TRACK_HEIGHT);
				let clips = track
					.clips()
					.iter()
					.map(|clip| {
						let color = clip.color().unwrap_or_else(|| kind_color(track.kind()));
						let in_transition = clip
							.in_transition()
							.filter(|duration| duration.0 > 0)
							.map(|duration| FrameRange::new(Frame::ZERO, duration));
						let out_transition = clip
							.out_transition()
							.filter(|duration| duration.0 > 0)
							.map(|duration| FrameRange::new(Frame::ZERO, duration));
						ClipRenderData {
							id: clip.id(),
							range: clip.range(),
							label: clip.label(),
							color,
							enabled: clip.is_enabled(),
							multicam: clip.is_multicam(),
							media_in: clip.media_in(),
							in_transition,
							out_transition,
						}
					})
					.collect();
				rows.push(RowData {
					index,
					name: track.name(),
					kind: track.kind(),
					height,
					y,
					locked: track.is_locked(),
					muted: track.is_muted(),
					solo: track.is_solo(),
					visible: track.is_visible(),
					clips,
				});
				y += height;
			}
		}

		// The marquee handler needs the row geometry; keep a snapshot for it
		// (the clip-area child iterator consumes `rows` below).
		let marquee_rows = Arc::new(rows.clone());

		// The clip-move ghost geometry: while a clip drag is active, the
		// overlay shows the resolved target track + frame so the user sees
		// where the clip will land (including across tracks), matching the
		// footage-drop ghost in the host panel. `None` outside a clip drag.
		let clip_ghost = self.active_clip_drag.as_ref().and_then(|drag| {
			let drag = drag.read().ok()?;
			Some(clip_ghost_rect(
				drag.new_start,
				drag.new_track,
				drag.original_length,
				state.zoom,
				state.scroll_offset.x,
				&rows,
			))
		});
		let clip_ghost_element: AnyElement = match clip_ghost {
			Some(rect) => {
				let colors = cx.default_colors().clone();
				div()
					.absolute()
					.left(rect.x)
					.top(rect.y)
					.w(rect.width)
					.h(rect.height)
					.rounded_sm()
					.border_1()
					.border_color(colors.selected)
					.bg(gpui_kit::Rgba {
						a: 0.35,
						..colors.selected
					})
					.into_any_element()
			}
			None => div().into_any_element(),
		};

		let playhead_x = state.point_at_frame(state.playhead).as_f32();
		let decorator = self.decorator.clone();

		// Sequence markers → ruler paint data (the data source provides them
		// sorted; the ruler colors them, falling back to the accent).
		let marker_color = marker_accent_color();
		let markers: Vec<RulerMarker> = source
			.markers()
			.into_iter()
			.map(|m| RulerMarker {
				frame: m.frame,
				color: m.color.unwrap_or(marker_color),
			})
			.collect();

		let ruler = div()
			.flex()
			.flex_row()
			.h(px(RULER_HEIGHT))
			.flex_shrink_0()
			.child(div().w(px(HEADER_WIDTH)).flex_shrink_0())
			.child(
				div()
					.flex_1()
					.h_full()
					// Clip the ruler at the clip area's left edge: the canvas
					// paints translated ticks/labels when scrolled, and gpui's
					// canvas does not self-clip — without this the labels
					// paint over the track-headers column (the headers must
					// occlude the timeline, not the other way round).
					.overflow_hidden()
					.id("timeline-ruler")
					.on_mouse_down(
						MouseButton::Left,
						cx.listener(|this, event: &MouseDownEvent, _window, cx| {
							// The ruler's left edge aligns with the clip
							// area's — convert through the canvas-captured
							// origin (the widget does not necessarily sit at
							// window x = 0).
							let frame = this
								.state
								.frame_at_point(event.position.x - this.clip_area_origin.x);
							this.seek(frame, cx);
						}),
					)
					.on_mouse_down(
						MouseButton::Right,
						cx.listener(|this, event: &MouseDownEvent, _window, cx| {
							let x = event.position.x - this.clip_area_origin.x;
							// A press close to a marker addresses the marker
							// (the ruler paints them as thin glyphs); any
							// other press addresses the ruler itself.
							let marker =
								this.source.read(cx).markers().into_iter().find(|marker| {
									let marker_x = this.state.point_at_frame(marker.frame);
									(f32::from(marker_x - x)).abs() <= SNAP_THRESHOLD_PX
								});
							let hit = match marker {
								Some(marker) => TimelineHit::RulerMarker(marker.frame),
								None => TimelineHit::Ruler(this.state.frame_at_point(x)),
							};
							cx.emit(TimelineEvent::ContextMenuRequested {
								position: event.position,
								hit,
							});
							cx.stop_propagation();
						}),
					)
					.on_drag(
						Arc::new(RwLock::new(RulerDrag {
							press: Point::default(),
							old_band: None,
							new_band: None,
							edge: None,
							resizing: false,
						})),
						drag_ghost,
					)
					.on_drag_move(cx.listener(
						|this, event: &DragMoveEvent<Arc<RwLock<RulerDrag>>>, _window, cx| {
							this.update_ruler_drag(event, cx);
						},
					))
					.on_drop(
						cx.listener(|this, drag: &Arc<RwLock<RulerDrag>>, _window, cx| {
							this.finish_ruler_drag(drag, cx);
						}),
					)
					.child(TimelineRuler::new(state.clone(), frame_rate, seq_len).markers(markers)),
			);

		let headers = div()
			.w(px(HEADER_WIDTH))
			.flex_shrink_0()
			.flex()
			.flex_col()
			.id("timeline-track-headers")
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<HeightDrag>>>, _window, cx| {
					this.update_height_drag(event, cx);
				},
			))
			.on_drop(
				cx.listener(|this, drag: &Arc<RwLock<HeightDrag>>, _window, cx| {
					this.finish_height_drag(drag, cx);
				}),
			)
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<TransitionDrag>>>, _window, cx| {
					this.update_transition_drag(event, cx);
				},
			))
			.on_drop(
				cx.listener(|this, drag: &Arc<RwLock<TransitionDrag>>, _window, cx| {
					this.finish_transition_drag(drag, cx);
				}),
			)
			.children(rows.iter().map(|row| {
				let height = row.height;
				let separator_y = row.y + height - TrackHeader::SEPARATOR_HEIGHT;
				let header_hover = crate::widgets::theme::row_hover(cx.default_colors());
				// The toggle glyphs report through the view as
				// `TrackToggleRequested` edit requests (the view itself
				// never mutates the model).
				let view = cx.weak_entity();
				let on_toggle: TrackToggleHandler = Arc::new(move |track, toggle, _window, app| {
					if let Some(view) = view.upgrade() {
						view.update(app, |_this, cx| {
							cx.emit(TimelineEvent::TrackToggleRequested { track, toggle });
							cx.notify();
						});
					}
				});
				div()
					.h(px(height))
					.relative()
					.child(
						div()
							.id(ElementId::named_usize("timeline-track-header", row.index))
							.h(px(height - TrackHeader::SEPARATOR_HEIGHT))
							.cursor_pointer()
							.hover(move |style| style.bg(header_hover))
							.on_click({
								let track = row.index;
								cx.listener(move |this, _event: &gpui_kit::ClickEvent, _window, cx| {
									let selected = if this.selected_tracks.contains(&track) {
										this.selected_tracks.remove(&track);
										false
									} else {
										this.selected_tracks.insert(track);
										true
									};
									cx.emit(TimelineEvent::TrackSelected { track, selected });
									cx.notify();
								})
							})
							.on_mouse_down(
								MouseButton::Right,
								cx.listener({
									let track = row.index;
									move |_this, event: &MouseDownEvent, _window, cx| {
										cx.emit(TimelineEvent::ContextMenuRequested {
											position: event.position,
											hit: TimelineHit::TrackHead(track),
										});
										cx.stop_propagation();
									}
								}),
							)
							.child(
								TrackHeader::new(row.index, row.name.clone(), row.kind)
									.locked(row.locked)
									.muted(row.muted)
									.solo(row.solo)
									.visible(row.visible)
									.on_toggle(on_toggle),
							),
					)
					.child(
						div()
							.absolute()
							.left(px(0.))
							.right(px(0.))
							.bottom(px(0.))
							.h(px(TrackHeader::SEPARATOR_HEIGHT))
							.id(ElementId::named_usize("timeline-track-resize", row.index))
							.cursor_row_resize()
							.on_drag(
								Arc::new(RwLock::new(HeightDrag {
									press: Point::default(),
									track: row.index,
									start_height: px(height),
									new_height: px(height),
									separator_y,
								})),
								drag_ghost,
							),
					)
			}));

		// The clip area's canvas child records the element's window-space
		// origin into the view (see the canvas below).
		let view = cx.entity();
		// The per-clip right-click handlers live inside a `move` closure
		// (the rows are consumed), so they cannot borrow `cx` for
		// `cx.listener`; they go through this weak handle instead.
		let weak_view = cx.weak_entity();

		let mut clip_area = div()
			.flex_1()
			.h_full()
			.id("timeline-clip-area")
			.relative()
			.overflow_hidden()
			.flex()
			.flex_col()
			.on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
				if event.modifiers.control || event.modifiers.platform {
					let anchor = event.position.x - px(HEADER_WIDTH);
					let factor = match event.delta {
						ScrollDelta::Pixels(delta) => 1.0 + delta.y.as_f32() * 0.01,
						ScrollDelta::Lines(delta) => 1.0 + delta.y * 0.01,
					}
					.clamp(0.5, 2.0);
					let old = this.state.zoom;
					this.state.set_zoom(old * factor, anchor);
					if (this.state.zoom - old).abs() > f32::EPSILON {
						cx.emit(TimelineEvent::ZoomChanged(this.state.zoom));
					}
				} else {
					let dx = match event.delta {
						ScrollDelta::Pixels(delta) => delta.y.as_f32(),
						ScrollDelta::Lines(delta) => delta.y * 24.0,
					};
					this.state.scroll_offset.x = px((this.state.scroll_offset.x.as_f32() + dx).max(0.0));
				}
				cx.notify();
			}))
			.on_pinch(cx.listener(|this, event: &PinchEvent, _window, cx| {
				let anchor = event.position.x - px(HEADER_WIDTH);
				let old = this.state.zoom;
				this.state.set_zoom(old * (1.0 + event.delta), anchor);
				if (this.state.zoom - old).abs() > f32::EPSILON {
					cx.emit(TimelineEvent::ZoomChanged(this.state.zoom));
				}
				cx.notify();
			}))
			.on_mouse_down(
				MouseButton::Right,
				cx.listener(|this, event: &MouseDownEvent, _window, cx| {
					// A right-click that reaches the clip area itself hit no
					// clip (clip handlers stop propagation): report the empty
					// track area with the track and frame under the pointer.
					let local = event.position - this.clip_area_origin;
					let frame = this.state.frame_at_point(local.x);
					let track = this.track_at_y(f32::from(local.y), cx);
					cx.emit(TimelineEvent::ContextMenuRequested {
						position: event.position,
						hit: TimelineHit::Empty { track, frame },
					});
				}),
			)
			// The zoom and track-select tools act directly on a clip-area
			// press (a press on a clip bubbles here: the clip wrapper leaves
			// non-select tools alone).
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(|this, event: &MouseDownEvent, _window, cx| {
					let local = event.position - this.clip_area_origin;
					match this.tool {
						TimelineTool::Zoom => {
							// Zoom in on the pointed frame; secondary (ctrl/cmd)
							// zooms out instead. Anchored at the cursor, like
							// ctrl-scroll.
							let factor = if event.modifiers.secondary() {
								0.8
							} else {
								1.25
							};
							let old = this.state.zoom;
							this.state.set_zoom(old * factor, local.x);
							if (this.state.zoom - old).abs() > f32::EPSILON {
								cx.emit(TimelineEvent::ZoomChanged(this.state.zoom));
							}
							cx.notify();
						}
						TimelineTool::TrackSelect => {
							// Select every clip under or to the right of the
							// pointer on the pointed track; secondary selects
							// the whole track.
							let frame = this.state.frame_at_point(local.x);
							let track = this.track_at_y(f32::from(local.y), cx);
							let whole = event.modifiers.secondary();
							let ids: Vec<ClipId> = this
								.source
								.read(cx)
								.track(track)
								.map(|t| {
									t.clips()
										.iter()
										.filter(|clip| whole || clip.range().end.0 > frame.0)
										.map(|clip| clip.id())
										.collect()
								})
								.unwrap_or_default();
							this.state.select_range(ids);
							cx.emit(TimelineEvent::SelectionChanged);
							cx.notify();
						}
						_ => {}
					}
				}),
			)
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<ClipDrag>>>, _window, cx| {
					this.update_clip_drag(event, cx);
				},
			))
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<TrimDrag>>>, _window, cx| {
					this.update_trim_drag(event, cx);
				},
			))
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<SlideDrag>>>, _window, cx| {
					this.update_slide_drag(event, cx);
				},
			))
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<SlipDrag>>>, _window, cx| {
					this.update_slip_drag(event, cx);
				},
			))
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<RollDrag>>>, _window, cx| {
					this.update_roll_drag(event, cx);
				},
			))
			.on_drop(
				cx.listener(|this, drag: &Arc<RwLock<ClipDrag>>, _window, cx| {
					this.finish_clip_drag(drag, cx);
				}),
			)
			.on_drop(
				cx.listener(|this, drag: &Arc<RwLock<TrimDrag>>, _window, cx| {
					this.finish_trim_drag(drag, cx);
				}),
			)
			.on_drop(
				cx.listener(|this, drag: &Arc<RwLock<SlideDrag>>, _window, cx| {
					this.finish_slide_drag(drag, cx);
				}),
			)
			.on_drop(
				cx.listener(|this, drag: &Arc<RwLock<SlipDrag>>, _window, cx| {
					this.finish_slip_drag(drag, cx);
				}),
			)
			.on_drop(
				cx.listener(|this, drag: &Arc<RwLock<RollDrag>>, _window, cx| {
					this.finish_roll_drag(drag, cx);
				}),
			)
			.on_drop(
				cx.listener(|this, drag: &Arc<RwLock<HeightDrag>>, _window, cx| {
					this.finish_height_drag(drag, cx);
				}),
			)
			.child(
				// Records the clip area's window-space origin on every
				// layout (the right-click hit test converts window
				// positions through it); paints nothing.
				canvas(
					move |bounds, _window, cx| {
						view.update(cx, |this, _cx| {
							this.clip_area_origin = bounds.origin;
							this.clip_area_width = f32::from(bounds.size.width);
						});
					},
					|_bounds, (), _window, _cx| {},
				)
				.absolute()
				.size_full(),
			);
		// The marquee (rubber-band) selection only exists on the select
		// tool; the razor tool swaps the area cursor to a vertical text
		// (I-beam) cursor matching its blade, and the zoom tool to a
		// crosshair instead.
		if tool == TimelineTool::Select {
			clip_area = clip_area
				.on_drag(MarqueeDrag(Cell::new(Point::default())), drag_ghost)
				.on_drag_move({
					cx.listener(
						move |this, event: &DragMoveEvent<MarqueeDrag>, _window, cx| {
							this.update_marquee(event, marquee_rows.as_slice(), cx);
						},
					)
				})
				.on_drop(cx.listener(|this, _drag: &MarqueeDrag, _window, cx| {
					this.finish_marquee(cx);
				}));
		}
		if tool == TimelineTool::Razor {
			clip_area = clip_area.cursor_text();
		}
		if tool == TimelineTool::Zoom {
			clip_area = clip_area.cursor_crosshair();
		}
		let clip_area =
			clip_area
				.children(rows.into_iter().map(move |row| {
					let height = row.height;
					let row_locked = row.locked;
					let kind = row.kind;
					let row_index = row.index;
					let state = &state;
					let decorator = &decorator;
					// Per-row clone: the per-clip closures below are built once
					// per row and move this handle in.
					let weak_view = weak_view.clone();
					// Roll edits reshape the shared boundary between adjacent
					// clips: precompute each boundary's partners so the per-clip
					// closure can attach a roll handle to the left clip of each
					// adjacent pair.
					let roll_info: Vec<Option<RollPair>> = (0..row.clips.len())
						.map(|i| roll_pair(&row.clips, i))
						.collect();
					div()
						.h(px(height))
						.relative()
						.children(row.clips.into_iter().enumerate().map(
							move |(clip_index, clip)| {
								let x0 = state.point_at_frame(clip.range.start).as_f32();
								let x1 = state.point_at_frame(clip.range.end).as_f32();
								let width = (x1 - x0).max(1.0);
								let clip_height = (height - TrackHeader::SEPARATOR_HEIGHT).max(1.0);
								let mut children: Vec<AnyElement> = vec![ClipElement::new(
									clip.id,
									clip.label.clone(),
									clip.color,
									clip.in_transition,
									clip.out_transition,
									decorator.clone(),
								)
								.selected(state.is_selected(clip.id))
								.enabled(clip.enabled)
								.locked(row_locked)
								.multicam(clip.multicam)
								.content(match kind {
									TrackKind::Video => ClipContent::Thumbnails,
									TrackKind::Audio => ClipContent::Waveform,
									TrackKind::Subtitle => ClipContent::None,
								})
								.into_any_element()];
								if !row_locked
									&& (tool == TimelineTool::Select
										|| tool == TimelineTool::Ripple)
								{
									children.push(
										div()
											.absolute()
											.left(px(0.))
											.top(px(0.))
											.bottom(px(0.))
											.w(px(TRIM_HANDLE_WIDTH))
											.id(ElementId::named_usize(
												"timeline-trim-start",
												clip.id.0 as usize,
											))
											.cursor_ew_resize()
											.on_drag(
												Arc::new(RwLock::new(TrimDrag {
													press: Point::default(),
													clip: clip.id,
													edge: TrimEdge::Start,
													original_frame: clip.range.start,
													new_frame: clip.range.start,
												})),
												drag_ghost,
											)
											.into_any_element(),
									);
									children.push(
										div()
											.absolute()
											.right(px(0.))
											.top(px(0.))
											.bottom(px(0.))
											.w(px(TRIM_HANDLE_WIDTH))
											.id(ElementId::named_usize(
												"timeline-trim-end",
												clip.id.0 as usize,
											))
											.cursor_ew_resize()
											.on_drag(
												Arc::new(RwLock::new(TrimDrag {
													press: Point::default(),
													clip: clip.id,
													edge: TrimEdge::End,
													original_frame: clip.range.end,
													new_frame: clip.range.end,
												})),
												drag_ghost,
											)
											.into_any_element(),
									);
								}

								// The roll tool draws a handle on the right edge of the
								// left clip of each adjacent pair; dragging it reshapes
								// the shared boundary.
								if tool == TimelineTool::Roll {
									if let Some(pair) = roll_info[clip_index] {
										children.push(
											div()
												.absolute()
												.right(px(0.))
												.top(px(0.))
												.bottom(px(0.))
												.w(px(TRIM_HANDLE_WIDTH))
												.id(ElementId::named_usize(
													"timeline-roll-handle",
													clip.id.0 as usize,
												))
												.cursor_ew_resize()
												.on_drag(
													Arc::new(RwLock::new(RollDrag {
														press: Point::default(),
														clip_a: pair.a,
														clip_b: pair.b,
														boundary: pair.boundary,
														new_frame: pair.boundary,
														min: pair.min,
														max: pair.max,
													})),
													drag_ghost,
												)
												.into_any_element(),
										);
									}
								}

								// Transition-resize handles sit at the wedges' edges,
								// select tool only (they are selection affordances, not
								// trim handles).
								if tool == TimelineTool::Select {
									if let Some(transition) = clip.in_transition {
										children.push(
											div()
												.absolute()
												.left(px(0.))
												.top(px(0.))
												.bottom(px(0.))
												.w(px(TRIM_HANDLE_WIDTH))
												.id(ElementId::named_usize(
													"timeline-transition-handle",
													clip.id.0 as usize,
												))
												.cursor_ew_resize()
												.on_drag(
													Arc::new(RwLock::new(TransitionDrag {
														press: Point::default(),
														clip: clip.id,
														edge: TrimEdge::Start,
														original_length: transition.end,
														new_length: transition.end,
													})),
													drag_ghost,
												)
												.into_any_element(),
										);
									}
									if let Some(transition) = clip.out_transition {
										children.push(
											div()
												.absolute()
												.right(px(0.))
												.top(px(0.))
												.bottom(px(0.))
												.w(px(TRIM_HANDLE_WIDTH))
												.id(ElementId::named_usize(
													"timeline-transition-handle",
													clip.id.0 as usize,
												))
												.cursor_ew_resize()
												.on_drag(
													Arc::new(RwLock::new(TransitionDrag {
														press: Point::default(),
														clip: clip.id,
														edge: TrimEdge::End,
														original_length: transition.end,
														new_length: transition.end,
													})),
													drag_ghost,
												)
												.into_any_element(),
										);
									}
								}
								let mut wrapper = div()
									.absolute()
									.left(px(x0))
									// Slight vertical inset: the design's clips read
									// as rounded bars with a small gap between rows,
									// not full-height slabs. The corners are rounded
									// in ClipElement's painted quads (content masks
									// are rectangular only).
									.top(px(2.))
									.w(px(width))
									.h(px((clip_height - 4.0).max(1.0)))
									.overflow_hidden()
									.id(ElementId::named_usize("timeline-clip", clip.id.0 as usize))
									.on_mouse_down(MouseButton::Left, {
										let view = weak_view.clone();
										let id = clip.id;
										move |event: &MouseDownEvent, _window, cx| {
											if let Some(view) = view.upgrade() {
												view.update(cx, |this, cx| {
													// A press means different things per tool: select runs the
													// NLE selection logic, razor splits the clip at the pointer.
													// Zoom and track-select leave the press to the clip area; the
													// other tools act on their own handles.
													match this.tool {
														TimelineTool::Select => {
															// NLE selection on press: a plain press on an unselected
															// clip selects just it; a press on an already-selected clip
															// keeps the multi-selection (group drags work); Ctrl/Cmd
															// toggles membership.
															let changed = if event
																.modifiers
																.secondary()
															{
																!this.state.selection.remove(&id)
																	&& {
																		this.state
																			.selection
																			.insert(id);
																		true
																	}
															} else if !this
																.state
																.selection
																.contains(&id)
															{
																this.state.selection.clear();
																this.state.selection.insert(id)
															} else {
																false
															};
															if changed {
																cx.emit(
																	TimelineEvent::SelectionChanged,
																);
																cx.notify();
															}
														}
														TimelineTool::Razor => {
															// Split the clip at the clicked frame (only inside the clip;
															// the edges are trim territory).
															let local = event.position
																- this.clip_area_origin;
															let frame =
																this.state.frame_at_point(local.x);
															let range = this.clip_range(id, cx);
															if frame.0 > range.start.0
																&& frame.0 < range.end.0
															{
																cx.emit(TimelineEvent::ClipSplitRequested { clip: id, time: frame });
																cx.notify();
															}
														}
														_ => {}
													}
												});
											}
										}
									})
									.on_mouse_down(MouseButton::Right, {
										let view = weak_view.clone();
										let id = clip.id;
										move |event: &MouseDownEvent, _window, cx| {
											if let Some(view) = view.upgrade() {
												view.update(cx, |_this, cx| {
													cx.emit(TimelineEvent::ContextMenuRequested {
														position: event.position,
														hit: TimelineHit::Clip(id),
													});
													cx.stop_propagation();
												});
											}
										}
									});
								match tool {
									TimelineTool::Select => {
										wrapper = wrapper.on_drag(
											Arc::new(RwLock::new(ClipDrag {
												press: Point::default(),
												clip: clip.id,
												original_start: clip.range.start,
												original_track: row_index,
												new_start: clip.range.start,
												new_track: row_index,
												original_length: clip.range.len(),
											})),
											{
												// The clip-move ghost follows the view's own record of
												// the drag (gpui-kit keeps `App::active_drag` private).
												let view = weak_view.clone();
												move |drag: &Arc<RwLock<ClipDrag>>,
												      offset: Point<Pixels>,
												      window: &mut Window,
												      cx: &mut App| {
													drag.record_press(offset);
													if let Some(view) = view.upgrade() {
														view.update(cx, |this, _cx| {
															this.active_clip_drag = Some(drag.clone());
														});
													}
													drag_ghost(drag, offset, window, cx)
												}
											},
										);
									}
									TimelineTool::Slide => {
										wrapper = wrapper.on_drag(
											Arc::new(RwLock::new(SlideDrag {
												press: Point::default(),
												clip: clip.id,
												original_start: clip.range.start,
												new_start: clip.range.start,
											})),
											drag_ghost,
										);
									}
									TimelineTool::Slip => {
										wrapper = wrapper.on_drag(
											Arc::new(RwLock::new(SlipDrag {
												press: Point::default(),
												clip: clip.id,
												original_media_in: clip.media_in,
												new_media_in: clip.media_in,
											})),
											drag_ghost,
										);
									}
									_ => {}
								}
								wrapper.children(children)
							},
						))
				}))
				// The clip-move ghost overlays the rows (an absolute child of the
				// clip area; an empty div when no clip drag is active).
				.child(clip_ghost_element)
				// The playhead lives INSIDE the clip area (clip-area-local x): it
				// is clipped by the area's left edge when scrolled off-screen
				// instead of painting over the track-headers column, and stays on
				// top of the clips.
				.child(
					div()
						.absolute()
						.left(px(playhead_x))
						.top(px(0.))
						.bottom(px(0.))
						.w(px(1.))
						.child(PlayheadElement::new(px(0.), playhead_color())),
				);

		div().size_full().flex().flex_col().child(ruler).child(
			div()
				.flex()
				.flex_row()
				.flex_1()
				.relative()
				.child(headers)
				.child(clip_area),
		)
	}
}

/// Which edge of a work-area band a ruler drag reshapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeKind {
	/// The band's in (left) edge.
	In,
	/// The band's out (right) edge.
	Out,
	/// The whole band, replaced by the press-to-cursor range.
	Whole,
}

/// Shared state for a ruler work-area drag. The drag starts on every ruler
/// press (alongside the seek), but only starts reshaping once the pointer
/// moves past the click threshold.
struct RulerDrag {
	/// The grab offset at drag start (recorded by the ghost
	/// constructor; see [`RecordPress`]).
	press: Point<Pixels>,
	/// The work-area band at drag start; `None` when the drag creates a new
	/// band.
	old_band: Option<FrameRange>,
	/// The band as the pointer moved.
	new_band: Option<FrameRange>,
	/// What the gesture reshapes, decided on the first real movement.
	edge: Option<EdgeKind>,
	/// Whether the pointer has passed the click threshold.
	resizing: bool,
}

/// Shared state for a clip-move gesture (the clip wrapper starts it, the
/// clip area updates and finishes it).
struct ClipDrag {
	/// The grab offset at drag start (recorded by the ghost
	/// constructor; see [`RecordPress`]).
	press: Point<Pixels>,
	clip: ClipId,
	original_start: Frame,
	original_track: usize,
	new_start: Frame,
	new_track: usize,
	/// The clip's own length in frames at drag start (the move ghost's
	/// extent).
	original_length: Frame,
}

/// Shared state for a trim gesture; see [`TrimEdge`].
struct TrimDrag {
	/// The grab offset at drag start (recorded by the ghost
	/// constructor; see [`RecordPress`]).
	press: Point<Pixels>,
	clip: ClipId,
	edge: TrimEdge,
	original_frame: Frame,
	new_frame: Frame,
}

/// Shared state for a clip-slide gesture: the clip keeps its media offsets
/// while its start (and with it the whole range) shifts by the horizontal
/// mouse delta; the host recomposes the neighboring clips as one undo entry.
struct SlideDrag {
	/// The grab offset at drag start (recorded by the ghost
	/// constructor; see [`RecordPress`]).
	press: Point<Pixels>,
	clip: ClipId,
	original_start: Frame,
	new_start: Frame,
}

/// Shared state for a clip-slip gesture: the clip's range stays fixed while
/// its media in-point slides under the pointer, revealing different source
/// frames.
struct SlipDrag {
	/// The grab offset at drag start (recorded by the ghost
	/// constructor; see [`RecordPress`]).
	press: Point<Pixels>,
	clip: ClipId,
	original_media_in: Frame,
	new_media_in: Frame,
}

/// Shared state for a roll gesture: the shared boundary between two adjacent
/// clips moves, extending the left clip's out edge while the right clip's in
/// edge retracts by the same amount (the pair's total length is preserved).
struct RollDrag {
	/// The grab offset at drag start (recorded by the ghost
	/// constructor; see [`RecordPress`]).
	press: Point<Pixels>,
	clip_a: ClipId,
	clip_b: ClipId,
	boundary: Frame,
	new_frame: Frame,
	/// The left clip's start — the boundary's exclusive lower bound.
	min: Frame,
	/// The right clip's end — the boundary's exclusive upper bound.
	max: Frame,
}

/// The two clips sharing a boundary and the boundary's legal range — the
/// precomputed input a roll handle is built from.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RollPair {
	a: ClipId,
	b: ClipId,
	boundary: Frame,
	min: Frame,
	max: Frame,
}

/// In-flight payload of a transition-resize drag.
struct TransitionDrag {
	/// The grab offset at drag start (recorded by the ghost
	/// constructor; see [`RecordPress`]).
	press: Point<Pixels>,
	clip: ClipId,
	edge: TrimEdge,
	original_length: Frame,
	new_length: Frame,
}

/// Shared state for a track-height resize gesture.
struct HeightDrag {
	/// The grab offset at drag start (recorded by the ghost
	/// constructor; see [`RecordPress`]).
	press: Point<Pixels>,
	track: usize,
	start_height: Pixels,
	new_height: Pixels,
	/// The separator strip's top edge, relative to the headers column's top.
	separator_y: f32,
}

/// The marquee (rubber-band) selection gesture's payload: the press
/// point (recorded by the ghost constructor; see [`RecordPress`]).
struct MarqueeDrag(Cell<Point<Pixels>>);

/// Layout snapshot of one track row, captured during render.
#[derive(Clone)]
struct RowData {
	index: usize,
	name: SharedString,
	kind: TrackKind,
	height: f32,
	y: f32,
	locked: bool,
	muted: bool,
	solo: bool,
	visible: bool,
	clips: Vec<ClipRenderData>,
}

/// Layout snapshot of one clip, captured during render.
#[derive(Clone)]
struct ClipRenderData {
	id: ClipId,
	range: FrameRange,
	label: SharedString,
	color: Hsla,
	enabled: bool,
	multicam: bool,
	media_in: Frame,
	in_transition: Option<FrameRange>,
	out_transition: Option<FrameRange>,
}

/// The on-screen geometry of a clip-move ghost: the translucent overlay shown
/// on the target track while a clip is dragged (the same visual language as
/// the footage-drop ghost the host paints in its timeline panel).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipGhostRect {
	/// Left edge in clip-area pixels.
	x: Pixels,
	/// Top edge relative to the clip area's top.
	y: Pixels,
	/// Width spanning the clip's own length.
	width: Pixels,
	/// Height of the target track's row.
	height: Pixels,
}

/// Resolves the clip-move ghost rect from a drag's target frame + track and
/// the clip's own length: `x` is `new_start` in clip-area pixels (zoomed and
/// scroll-compensated, matching [`TimelineState::point_at_frame`]), `y`
/// accumulates the row heights above `new_track`, and the rect spans `length`
/// frames at the target row's height.
///
/// Pure — the frame/track → rect conversion is unit-tested directly.
fn clip_ghost_rect(
	new_start: Frame,
	new_track: usize,
	length: Frame,
	zoom: f32,
	scroll_x: Pixels,
	rows: &[RowData],
) -> ClipGhostRect {
	let mut y = 0.0f32;
	let mut last_top = 0.0f32;
	for row in rows {
		if row.index == new_track {
			return ClipGhostRect {
				x: px(new_start.0 as f32 * zoom) - scroll_x,
				y: px(y),
				width: px(length.0 as f32 * zoom).max(px(4.0)),
				height: px(row.height),
			};
		}
		last_top = y;
		y += row.height;
	}
	// Target index beyond the last row (a track vanished mid-drag): clamp to
	// the last row's band, mirroring `track_at_y`'s fallback.
	match rows.last() {
		Some(last) => ClipGhostRect {
			x: px(new_start.0 as f32 * zoom) - scroll_x,
			y: px(last_top),
			width: px(length.0 as f32 * zoom).max(px(4.0)),
			height: px(last.height),
		},
		None => ClipGhostRect {
			x: px(new_start.0 as f32 * zoom) - scroll_x,
			y: px(0.0),
			width: px(length.0 as f32 * zoom).max(px(4.0)),
			height: px(MIN_TRACK_HEIGHT),
		},
	}
}

/// Resolves the roll handle for clip `index` in a row: `Some` only when the
/// next clip starts exactly where this one ends, with the pair's legal
/// boundary range (`min` = the left clip's start, `max` = the right clip's
/// end).
///
/// Pure — the adjacency test is unit-tested directly.
fn roll_pair(clips: &[ClipRenderData], index: usize) -> Option<RollPair> {
	let a = clips.get(index)?;
	let b = clips.get(index + 1)?;
	if b.range.start != a.range.end {
		return None;
	}
	Some(RollPair {
		a: a.id,
		b: b.id,
		boundary: a.range.end,
		min: a.range.start,
		max: b.range.end,
	})
}

/// The clips a track-select press picks: everything at or right of the clicked
/// frame (`end > frame`), mirroring the pointer's "from here onward"
/// semantics.
///
/// Only used by tests — the live press reads the source's trait objects
/// directly instead.
#[cfg(test)]
fn track_select_clips(clips: &[ClipRenderData], frame: Frame) -> Vec<ClipId> {
	clips
		.iter()
		.filter(|clip| clip.range.end.0 > frame.0)
		.map(|clip| clip.id)
		.collect()
}

/// The ghost rendered under the cursor during any timeline drag.
struct DragPreview;

impl Render for DragPreview {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size_full().bg(hsla(0.6, 0.7, 0.9, 0.35))
	}
}

/// The grab offset of an in-progress drag (the pointer's offset within
/// the dragged element at drag start), recorded into the payload by the
/// ghost constructor. gpui-kit keeps `App::active_drag` private, so the
/// timeline tracks its own press points.
trait RecordPress {
	fn record_press(&self, offset: Point<Pixels>);
}

macro_rules! impl_record_press {
	($($t:ty),* $(,)?) => {
		$(
			impl RecordPress for Arc<RwLock<$t>> {
				fn record_press(&self, offset: Point<Pixels>) {
					self.write().expect("drag lock is not poisoned").press = offset;
				}
			}
		)*
	};
}

impl_record_press!(
	RulerDrag, ClipDrag, TrimDrag, SlideDrag, SlipDrag, RollDrag, TransitionDrag, HeightDrag,
);

impl RecordPress for MarqueeDrag {
	fn record_press(&self, offset: Point<Pixels>) {
		self.0.set(offset);
	}
}

/// Builds the drag ghost view for a drag of any value type, recording the
/// grab offset into the payload (see [`RecordPress`]).
fn drag_ghost<T: RecordPress>(
	drag: &T,
	offset: Point<Pixels>,
	_window: &mut Window,
	cx: &mut App,
) -> Entity<DragPreview> {
	drag.record_press(offset);
	cx.new(|_cx| DragPreview)
}

/// The playhead line color: the design's accent-blue vertical line.
fn playhead_color() -> Hsla {
	hsla(0.60, 0.90, 0.60, 1.0)
}

/// Reshapes the work-area band for a ruler drag move.
///
/// * `band` — the band at drag start (`None` = the drag creates a new one).
/// * `press_frame` / `current_frame` — the clamped press and pointer frames.
/// * `edge` — what the gesture reshapes (see [`EdgeKind`]).
///
/// `In`/`Out` keep at least one frame of band length; `Whole` replaces the
/// band with the press-to-pointer range, always `[start, end)` with
/// `end > start`.
fn reshape_work_area(
	band: Option<FrameRange>,
	press_frame: Frame,
	current_frame: Frame,
	edge: EdgeKind,
	seq_len: i64,
) -> FrameRange {
	let mut band = band.unwrap_or_else(|| FrameRange::new(Frame::ZERO, Frame(seq_len)));
	match edge {
		EdgeKind::In => band.start = Frame(current_frame.0.min((band.end.0 - 1).max(0))),
		EdgeKind::Out => band.end = Frame(current_frame.0.max(band.start.0 + 1)),
		EdgeKind::Whole => {
			band = if press_frame.0 <= current_frame.0 {
				FrameRange::new(press_frame, Frame(current_frame.0.max(press_frame.0 + 1)))
			} else {
				FrameRange::new(current_frame, Frame(press_frame.0.max(current_frame.0 + 1)))
			};
		}
	}
	band
}

/// The default marker color (used when a marker carries no explicit color):
/// the theme-agnostic amber accent, distinct from the playhead and the
/// work-area band.
fn marker_accent_color() -> Hsla {
	hsla(0.10, 0.85, 0.55, 1.0)
}

#[cfg(test)]
mod tests {
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
		use gpui_kit::{size, Modifiers, TestAppContext, VisualTestContext};
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
}

/// The default body color for clips on a track of `kind`.
///
/// Per the design both media kinds read as green bars (the audio waveform
/// supplies the contrast); subtitles keep the amber family.
fn kind_color(kind: TrackKind) -> Hsla {
	match kind {
		TrackKind::Video => hsla(0.40, 0.44, 0.43, 1.0),
		TrackKind::Audio => hsla(0.38, 0.45, 0.36, 1.0),
		TrackKind::Subtitle => hsla(0.10, 0.45, 0.35, 1.0),
	}
}
