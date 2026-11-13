//! Pure view-state for the timeline: zoom, scroll, playhead, selection.
//!
//! [`TimelineState`] holds no [`Entity`](gpui_kit::Entity) handles and no model
//! references — it is plain data with pure methods, which makes it the
//! unit-testable core of the widget. [`TimelineView`](super::TimelineView)
//! owns one of these and funnels every interaction through it.
//!
//! # Coordinate model
//!
//! The clip area uses a single affine mapping between sequence frames and
//! pixels, parameterized by `zoom` (pixels per frame) and `scroll_offset`
//! (the content position currently at the top-left of the viewport):
//!
//! ```text
//! screen_x(frame) = frame * zoom - scroll_offset.x
//! frame_at(x)     = (x + scroll_offset.x) / zoom
//! ```
//!
//! [`TimelineState::frame_at_point`] and [`TimelineState::point_at_frame`]
//! are exact inverses (up to float rounding and frame truncation) and both
//! are implemented here — they are the contract the painter and the mouse
//! handlers share.

use std::collections::BTreeSet;

use gpui_kit::{Pixels, Point, point, px};

use super::data::ClipId;
use super::time::{Frame, FrameRange};

/// Minimum zoom: 0.001 pixels per frame (a full day of 24 fps fits in ~86 px).
pub const MIN_ZOOM: f32 = 0.001;

/// Maximum zoom: 1000 pixels per frame (one frame fills a large display).
pub const MAX_ZOOM: f32 = 1000.0;

/// View-local state of the timeline: zoom, scroll, playhead, selection.
///
/// All fields are public so tests can construct states directly, but the
/// invariants below are only maintained if you go through the methods:
///
/// * `zoom` stays within `[MIN_ZOOM, MAX_ZOOM]` (use [`Self::set_zoom`]).
/// * `scroll_offset` components stay `>= px(0.)`.
/// * `playhead` stays within `[Frame::ZERO, sequence_length]` (use
///   [`Self::set_playhead`]).
#[derive(Debug, Clone)]
pub struct TimelineState {
	/// Horizontal scale, in **pixels per frame**. Drives both the clip area
	/// and the ruler. Clamped to `[MIN_ZOOM, MAX_ZOOM]` by
	/// [`Self::set_zoom`].
	pub zoom: f32,

	/// Content coordinate currently at the top-left of the viewport. `x`
	/// scrolls the whole timeline; `y` scrolls the stacked tracks.
	pub scroll_offset: Point<Pixels>,

	/// Current playhead position, in sequence frames.
	pub playhead: Frame,

	/// The current clip selection.
	///
	/// A `BTreeSet` so iteration order is deterministic (paint z-order of
	/// selection outlines, test assertions) and membership tests are cheap.
	pub selection: BTreeSet<ClipId>,

	/// Whether drag operations snap to clip edges, the playhead, work-area
	/// edges and markers. Toggled by the user (Oak: the magnet toolbar
	/// button); checked by every drag handler before calling
	/// [`snap`](super::snap).
	pub snap_enabled: bool,

	/// The work area (render/export in-out range), if set. Shown as a band
	/// on the ruler and offered as snap points.
	pub work_area: Option<FrameRange>,
}

impl Default for TimelineState {
	fn default() -> Self {
		TimelineState {
			zoom: 1.0,
			scroll_offset: point(px(0.), px(0.)),
			playhead: Frame::ZERO,
			selection: BTreeSet::new(),
			snap_enabled: true,
			work_area: None,
		}
	}
}

impl TimelineState {
	/// Creates a default state: zoom 1 px/frame, no scroll, playhead at
	/// zero, empty selection, snapping on, no work area.
	pub fn new() -> Self {
		Self::default()
	}

	/// Maps a horizontal screen position in the clip area to a sequence
	/// frame: `(x + scroll_offset.x) / zoom`, rounded **toward zero** to the
	/// nearest whole frame.
	///
	/// Positions left of the content start yield negative frames; callers
	/// clamp to the sequence as appropriate. This is the exact inverse of
	/// [`Self::point_at_frame`] — see the module docs for the mapping.
	///
	/// # Panics
	///
	/// Never panics in release; in debug it asserts that `zoom > 0`.
	pub fn frame_at_point(&self, x: Pixels) -> Frame {
		debug_assert!(self.zoom > 0.0, "zoom must be positive");
		let content_x = x + self.scroll_offset.x;
		Frame((content_x / px(self.zoom)) as i64)
	}

	/// Maps a sequence frame to its horizontal screen position in the clip
	/// area: `frame * zoom - scroll_offset.x`.
	///
	/// Frames scrolled off-screen yield negative or beyond-viewport values;
	/// that is expected — painters clip to their bounds.
	pub fn point_at_frame(&self, frame: Frame) -> Pixels {
		px(frame.0 as f32 * self.zoom) - self.scroll_offset.x
	}

	/// Sets the zoom, clamped to `[MIN_ZOOM, MAX_ZOOM]`, while keeping the
	/// frame under `anchor` (a screen x position, typically the cursor)
	/// stationary on screen.
	///
	/// # Math contract
	///
	/// Let `f = frame_at_point(anchor)` (fractional, before truncation).
	/// After zooming, `scroll_offset.x` is adjusted so that
	/// `f * new_zoom - new_scroll_x == anchor`, i.e.:
	///
	/// ```text
	/// new_scroll_x = (anchor + old_scroll_x) * (new_zoom / old_zoom) - anchor
	/// ```
	///
	/// clamped to `>= px(0.)`. This is the standard "zoom to cursor"
	/// behavior of every NLE.
	pub fn set_zoom(&mut self, zoom: f32, anchor: Pixels) {
		let new_zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
		let old_zoom = self.zoom.max(MIN_ZOOM);
		let content_at_anchor = anchor + self.scroll_offset.x;
		let new_scroll = content_at_anchor * (new_zoom / old_zoom) - anchor;
		self.zoom = new_zoom;
		self.scroll_offset.x = if new_scroll < px(0.) {
			px(0.)
		} else {
			new_scroll
		};
	}

	/// Sets the playhead, clamped to `[Frame::ZERO, sequence_length]`.
	///
	/// `sequence_length` comes from
	/// [`TimelineDataSource::sequence_length`](super::TimelineDataSource::sequence_length);
	/// passing the inclusive end is legal — the playhead may rest one frame
	/// past the last content frame.
	pub fn set_playhead(&mut self, frame: Frame, sequence_length: Frame) {
		self.playhead = frame.clamp(Frame::ZERO, sequence_length);
	}

	/// Replaces the selection with exactly `id`.
	///
	/// Any previous selection is cleared first, so this models a plain
	/// click (not shift/ctrl-click, which use [`Self::add_to_selection`] and
	/// [`Self::toggle`]). Pure view-state: it neither moves the playhead nor
	/// informs the engine.
	pub fn select(&mut self, id: ClipId) {
		self.selection.clear();
		self.selection.insert(id);
	}

	/// Adds `id` to the selection without disturbing the rest (shift-click).
	pub fn add_to_selection(&mut self, id: ClipId) {
		self.selection.insert(id);
	}

	/// Toggles `id`'s membership in the selection (ctrl/cmd-click).
	pub fn toggle(&mut self, id: ClipId) {
		if !self.selection.remove(&id) {
			self.selection.insert(id);
		}
	}

	/// Empties the selection.
	///
	/// Drops every clip id from the view state (and therefore every selection
	/// outline), without touching the engine's selection or emitting a
	/// [`TimelineEvent::SelectionChanged`](super::TimelineEvent) — callers
	/// that need those do it themselves, as
	/// [`TimelinePanel`](crate::panels::timeline::TimelinePanel) does when it
	/// mirrors an engine selection into the view.
	pub fn clear_selection(&mut self) {
		self.selection.clear();
	}

	/// Selects exactly the given clips (marquee/rubber-band result).
	///
	/// The hit-testing that produces `ids` lives in
	/// [`TimelineView`](super::TimelineView); this method only stores the
	/// outcome, replacing any previous selection.
	pub fn select_range(&mut self, ids: impl IntoIterator<Item = ClipId>) {
		self.selection = ids.into_iter().collect();
	}

	/// Whether `id` is currently selected.
	///
	/// A `ClipId` membership test against [`Self::selection`]; it does not
	/// consult the engine, so a clip deleted behind the view's back can still
	/// test as selected until the view resyncs. `O(log n)` on the ordered set.
	pub fn is_selected(&self, id: ClipId) -> bool {
		self.selection.contains(&id)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn point_and_frame_are_inverse() {
		let mut state = TimelineState::new();
		state.zoom = 2.5;
		state.scroll_offset.x = px(40.);
		let x = state.point_at_frame(Frame(100));
		assert_eq!(state.frame_at_point(x), Frame(100));
	}

	#[test]
	fn zoom_keeps_anchor_frame_stationary() {
		let mut state = TimelineState::new();
		state.scroll_offset.x = px(100.);
		let anchor = px(200.);
		// At zoom 1, frame 300 sits at screen x = 300 - 100 = 200 = anchor.
		state.set_zoom(4.0, anchor);
		// After zooming, frame 300 must still sit exactly under the anchor.
		assert_eq!(state.point_at_frame(Frame(300)), anchor);
	}

	#[test]
	fn playhead_is_clamped() {
		let mut state = TimelineState::new();
		state.set_playhead(Frame(-5), Frame(100));
		assert_eq!(state.playhead, Frame::ZERO);
		state.set_playhead(Frame(500), Frame(100));
		assert_eq!(state.playhead, Frame(100));
	}
}
