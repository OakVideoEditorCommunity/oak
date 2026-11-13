//! Pure transport state machine for the [`ViewerWidget`](super::ViewerWidget):
//! playback position, play/pause, in/out points and loop-range stepping.
//! No gpui coupling, unit-tested.

use crate::widgets::timeline::Frame;

/// The playback state of a viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportState {
	/// The current playhead frame.
	pub frame: Frame,
	/// Whether playback is running (the engine drives the position; the
	/// view only reflects it).
	pub playing: bool,
	/// The loop-in point, if set.
	pub in_point: Option<Frame>,
	/// The loop-out point, if set.
	pub out_point: Option<Frame>,
	/// Whether playback loops between the in/out points.
	pub loop_range: bool,
}

impl Default for TransportState {
	fn default() -> Self {
		Self::new()
	}
}

impl TransportState {
	/// Create a stopped transport at frame 0.
	pub fn new() -> Self {
		Self {
			frame: Frame(0),
			playing: false,
			in_point: None,
			out_point: None,
			loop_range: false,
		}
	}

	/// Toggle play/pause; returns the new state.
	pub fn toggle_play(&mut self) -> bool {
		self.playing = !self.playing;
		self.playing
	}

	/// Advance one frame. Clamps at `length` (exclusive); when a loop range
	/// is active the position wraps to `in_point` instead of stopping at
	/// `out_point`.
	pub fn advance(&mut self, length: Frame) -> Frame {
		let next = self.frame.0 + 1;
		self.frame = if let Some(out) = self.out_point {
			if self.loop_range && next >= out.0 {
				Frame(self.in_point.unwrap_or(Frame(0)).0)
			} else {
				Frame(next.min(length.0 - 1))
			}
		} else {
			Frame(next.min(length.0 - 1))
		};
		self.frame
	}

	/// Step the playhead by `delta` frames, clamped to `[0, length)`.
	pub fn step(&mut self, delta: i64, length: Frame) -> Frame {
		self.frame = Frame((self.frame.0 + delta).clamp(0, length.0 - 1));
		self.frame
	}

	/// Set the loop-in point at the current frame and enable the loop range.
	///
	/// `length` is the sequence length in frames; the stored frame is clamped
	/// to `[0, length - 1]`, so an empty sequence (`length.0 == 0`) stores
	/// `-1` and callers must guard that case. With `loop_range` enabled,
	/// [`TransportState::advance`] wraps back to this point after the
	/// out-point.
	pub fn set_in_point(&mut self, length: Frame) {
		self.in_point = Some(Frame(self.frame.0.min(length.0 - 1)));
		self.loop_range = true;
	}

	/// Set the loop-out point at the current frame and enable the loop range.
	///
	/// `length` is the sequence length in frames; the stored frame is clamped
	/// to `[0, length - 1]`, so an empty sequence (`length.0 == 0`) stores
	/// `-1` and callers must guard that case. Setting a point enables
	/// `loop_range` but does not validate the in/out ordering — the wrap in
	/// [`TransportState::advance`] uses whatever points are set.
	pub fn set_out_point(&mut self, length: Frame) {
		self.out_point = Some(Frame(self.frame.0.min(length.0 - 1)));
		self.loop_range = true;
	}

	/// Clear the loop range.
	///
	/// Drops both loop points (back to `None`) and disables `loop_range`, so
	/// playback stops at the sequence length again instead of wrapping. Does
	/// not move the playhead.
	pub fn clear_range(&mut self) {
		self.in_point = None;
		self.out_point = None;
		self.loop_range = false;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn toggle_play_flips() {
		let mut t = TransportState::new();
		assert!(!t.playing);
		assert!(t.toggle_play());
		assert!(!t.toggle_play());
	}

	#[test]
	fn advance_clamps_at_length() {
		let mut t = TransportState::new();
		t.frame = Frame(9);
		assert_eq!(t.advance(Frame(10)), Frame(9));
		assert_eq!(t.advance(Frame(10)), Frame(9));
	}

	#[test]
	fn advance_loops_within_range() {
		let mut t = TransportState::new();
		t.frame = Frame(5);
		t.in_point = Some(Frame(4));
		t.out_point = Some(Frame(8));
		t.loop_range = true;
		assert_eq!(t.advance(Frame(100)), Frame(6));
		t.frame = Frame(7);
		assert_eq!(t.advance(Frame(100)), Frame(4));
	}

	#[test]
	fn advance_ignores_out_point_without_loop() {
		let mut t = TransportState::new();
		t.out_point = Some(Frame(8));
		t.loop_range = false;
		t.frame = Frame(7);
		assert_eq!(t.advance(Frame(100)), Frame(8));
	}

	#[test]
	fn step_clamps_to_range() {
		let mut t = TransportState::new();
		t.frame = Frame(2);
		assert_eq!(t.step(-5, Frame(10)), Frame(0));
		assert_eq!(t.step(100, Frame(10)), Frame(9));
		// 9 + 1 -> 10, clamped to the last index 9.
		assert_eq!(t.step(1, Frame(10)), Frame(9));
	}

	#[test]
	fn in_out_points_respect_length() {
		let mut t = TransportState::new();
		t.frame = Frame(50);
		t.set_in_point(Frame(10));
		assert_eq!(t.in_point, Some(Frame(9)));
		t.frame = Frame(50);
		t.set_out_point(Frame(10));
		assert_eq!(t.out_point, Some(Frame(9)));
		assert!(t.loop_range);
		t.clear_range();
		assert_eq!(t.in_point, None);
		assert_eq!(t.out_point, None);
		assert!(!t.loop_range);
	}
}
