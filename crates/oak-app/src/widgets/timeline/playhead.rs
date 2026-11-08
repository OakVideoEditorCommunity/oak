//! The playhead: its on-screen element and the playback ticker.
//!
//! [`PlayheadElement`] draws the current-position line across ruler and
//! tracks plus a grab handle on the ruler. [`PlayheadTicker`] advances the
//! playhead during playback by converting wall-clock elapsed time into
//! frames at the sequence rate — the only sanctioned wall-clock→frames
//! conversion in the widget, going through
//! [`seconds_to_frame`](super::seconds_to_frame).

use std::time::Instant;

use gpui_kit::{
	App, Bounds, Hsla, PathBuilder, Pixels, Window, canvas, fill, point, prelude::*, px, size,
};

use super::time::{Frame, FrameRate};

/// The vertical playhead line and its ruler grab handle.
///
/// Geometry is supplied pre-computed: `x` is the playhead's screen position
/// ([`TimelineState::point_at_frame`](super::TimelineState::point_at_frame)),
/// so the element itself does no time math.
#[derive(IntoElement)]
pub struct PlayheadElement {
	x: Pixels,
	color: Hsla,
}

impl PlayheadElement {
	/// Creates the playhead element at screen position `x`.
	///
	/// `x` is the pre-computed screen-space position in pixels (from
	/// [`TimelineState::point_at_frame`](super::TimelineState::point_at_frame)); the
	/// element adds its own canvas origin and does no time math. `color` fills both
	/// the 1 px vertical line and the ruler grab handle.
	pub fn new(x: Pixels, color: Hsla) -> Self {
		PlayheadElement { x, color }
	}

	/// Screen x of the line.
	pub fn x(&self) -> Pixels {
		self.x
	}
}

impl RenderOnce for PlayheadElement {
	fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
		let x = self.x;
		let color = self.color;

		canvas(
			move |_bounds, _window, _cx| (),
			move |bounds, (), window, cx| {
				let _ = cx;
				let x = bounds.left() + x;

				// The playhead line, full height of the element. The width is
				// 1 px and the line is centered on `x`.
				window.paint_quad(fill(
					Bounds {
						origin: point(x, bounds.top()),
						size: size(px(1.0), bounds.size.height),
					},
					color,
				));

				// A small downward-pointing grab handle at the top of the
				// line, marking where the user can drag to seek.
				let handle_height = 8.0;
				let mut path = PathBuilder::fill();
				path.move_to(point(px(x.as_f32() - 5.0), bounds.top()));
				path.line_to(point(px(x.as_f32() + 6.0), bounds.top()));
				path.line_to(point(px(x.as_f32() + 0.5), bounds.top() + px(handle_height)));
				path.close();
				let path = path.build().expect("playhead handle path always builds");
				window.paint_path(path, color);
			},
		)
		.size_full()
	}
}

/// Drives playhead advancement while the sequence is playing.
///
/// # Drift-free accumulation
///
/// The ticker records the wall-clock [`Instant`] and the exact [`Frame`] at
/// which playback started, and on every animation frame computes:
///
/// ```text
/// playhead = start_frame + seconds_to_frame(rate, now - start_instant)
/// ```
///
/// It never adds a per-tick delta to the previous playhead — that would
/// accumulate rounding error and drift against the audio clock. Because both
/// anchors are fixed, total error stays under half a frame no matter how
/// long playback runs.
///
/// The ticker is driven by [`Window::request_animation_frame`](gpui_kit::Window::request_animation_frame) and stops
/// re-scheduling itself when [`PlayheadTicker::stop`] is called or the view
/// is released. Every computed position goes through
/// [`TimelineState::set_playhead`](super::TimelineState::set_playhead)
/// (clamped to the sequence) and emits [`TimelineEvent::PlayheadChanged`](super::TimelineEvent::PlayheadChanged)
/// when it changes.
pub struct PlayheadTicker {
	rate: FrameRate,
	start_frame: Frame,
	start_instant: Option<Instant>,
}

impl PlayheadTicker {
	/// Creates a stopped ticker for sequences running at `rate`.
	///
	/// `rate` is the sequence frame rate used to convert elapsed wall-clock seconds
	/// into frames (see [`PlayheadTicker::current_frame`]). The anchor frame starts at
	/// [`Frame::ZERO`] and no instant is recorded until [`PlayheadTicker::start`], so
	/// [`PlayheadTicker::is_playing`] is initially `false` and `current_frame()`
	/// returns frame 0.
	pub fn new(rate: FrameRate) -> Self {
		PlayheadTicker {
			rate,
			start_frame: Frame::ZERO,
			start_instant: None,
		}
	}

	/// Whether playback is currently running.
	pub fn is_playing(&self) -> bool {
		self.start_instant.is_some()
	}

	/// Starts playback from `start_frame`.
	///
	/// Re-anchors both the wall-clock and frame anchors (see the type docs),
	/// so pausing and resuming never accumulates error. If already playing,
	/// this restarts the anchor — useful for jog/shuttle seeks mid-playback.
	///
	/// The owning [`TimelineView`](super::TimelineView) is responsible for
	/// scheduling the animation-frame loop
	/// ([`Window::request_animation_frame`](gpui_kit::Window::request_animation_frame)) and polling
	/// [`Self::current_frame`] each tick, pushing the result through
	/// [`TimelineState::set_playhead`](super::TimelineState::set_playhead)
	/// and emitting [`TimelineEvent::PlayheadChanged`](super::TimelineEvent::PlayheadChanged) on change.
	pub fn start(&mut self, start_frame: Frame) {
		self.start_frame = start_frame;
		self.start_instant = Some(Instant::now());
	}

	/// The playhead position right now, per the drift-free formula in the
	/// type docs. When stopped, returns the last anchor frame.
	pub fn current_frame(&self) -> Frame {
		match self.start_instant {
			Some(instant) => {
				self.start_frame
					+ super::time::seconds_to_frame(self.rate, instant.elapsed().as_secs_f64())
			}
			None => self.start_frame,
		}
	}

	/// Stops playback. Returns the frame playback stopped at, so the caller
	/// can make it the new playhead rest position.
	pub fn stop(&mut self) -> Frame {
		let frame = self.current_frame();
		self.start_instant = None;
		self.start_frame = frame;
		frame
	}
}
