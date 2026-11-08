//! Keying support for keyable form controls.
//!
//! Every keyable control (sliders, color swatches, curve editor points) shows
//! a small diamond button on its right edge. Clicking it emits a
//! [`KeyingRequest`] so the host can add/remove/inspect the keyframe for that
//! control; the widget itself never touches the engine.

use gpui_kit::{
	App, Bounds, Div, ElementId, Hsla, Pixels, Stateful, Window, canvas, colors::DefaultColors,
	div, fill, point, prelude::*, px, size,
};

/// The keying state of a control at the current playhead frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyingState {
	/// The control has no keyframe.
	NoKey,
	/// The control has a keyframe somewhere on the timeline.
	HasKey,
	/// The control has a keyframe exactly at the current frame.
	AtCurrentFrame,
}

/// A request emitted when a keying diamond is clicked.
///
/// Widgets embed this in their own event enums, e.g.
/// `SliderEvent::Keying(KeyingRequest)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyingRequest {
	/// The stable id of the control that owns the diamond.
	pub control: usize,
	/// The keying state the control reported when the diamond was painted.
	pub state: KeyingState,
}

impl KeyingRequest {
	/// Creates a keying request pairing a control id with a reported keying
	/// state.
	///
	/// # Arguments
	///
	/// * `control` - Stable id of the control that owns the diamond; the widget
	///   uses it to route the request back to its edited value.
	/// * `state` - The keying state captured when the diamond was painted; the
	///   host uses it to decide whether to add, remove or inspect the keyframe.
	///
	/// The request is plain data — widgets never touch the engine themselves.
	pub fn new(control: usize, state: KeyingState) -> Self {
		Self { control, state }
	}
}

/// Accent used for "has a key" diamonds.
const KEY_COLOR: Hsla = Hsla {
	h: 0.09,
	s: 0.75,
	l: 0.55,
	a: 1.0,
};
/// Brighter accent for "key at current frame".
const KEY_ACTIVE_COLOR: Hsla = Hsla {
	h: 0.09,
	s: 0.9,
	l: 0.68,
	a: 1.0,
};
/// Base opacity of an empty diamond, so missing keys read dimmer.
const EMPTY_OPACITY: f32 = 0.45;
/// The painted size of the diamond glyph itself.
const DIAMOND_SIZE: f32 = 14.0;

/// Build the keying diamond element for `control`.
///
/// The returned element is interactive: chain `.on_click(...)` (or
/// `cx.listener(...)`) to receive clicks, then emit a [`KeyingRequest`].
pub fn keying_diamond(control: usize, state: KeyingState) -> Stateful<Div> {
	div()
		.size(px(20.0))
		.flex()
		.items_center()
		.justify_center()
		.opacity(if state == KeyingState::NoKey {
			EMPTY_OPACITY
		} else {
			1.0
		})
		.hover(|style| style.opacity(1.0))
		.child(canvas(
			move |_bounds, _window, _cx| (),
			move |bounds, (), window, cx| paint_diamond(bounds, state, window, cx),
		))
		.id(ElementId::named_usize("gpui-widgets-keying", control))
}

/// Convenience constructor for the [`KeyingRequest`] a diamond click should
/// emit: it pairs the owning control id with the state the diamond was painted
/// with.
///
/// Equivalent to [`KeyingRequest::new`]; click handlers can call it directly,
/// e.g. `keying_request(control, state)` inside `.on_click(...)`, so the host
/// can add/remove/inspect the keyframe.
pub fn keying_request(control: usize, state: KeyingState) -> KeyingRequest {
	KeyingRequest::new(control, state)
}

/// Paints the 14 px keying diamond into `bounds` for the given keying state.
///
/// The body is filled with the key accent when a key exists and left hollow
/// (outline only) for [`KeyingState::NoKey`]; an
/// [`KeyingState::AtCurrentFrame`] key additionally gets a small center dot
/// painted in the current background color. Colors come from the live theme via
/// `cx` (the disabled color for the empty outline), so a theme change is picked
/// up on the next paint. Called as the canvas paint callback of
/// [`keying_diamond`].
fn paint_diamond(bounds: Bounds<Pixels>, state: KeyingState, window: &mut Window, cx: &mut App) {
	use gpui_kit::PathBuilder;
	let colors = cx.default_colors().clone();
	let center = bounds.center();
	let half = px(DIAMOND_SIZE / 2.0 - 2.0);
	let (fill_color, border_color) = match state {
		KeyingState::NoKey => (Hsla::from(colors.disabled), Hsla::from(colors.disabled)),
		KeyingState::HasKey => (KEY_COLOR, KEY_COLOR),
		KeyingState::AtCurrentFrame => (KEY_ACTIVE_COLOR, KEY_ACTIVE_COLOR),
	};

	// The diamond body is filled when a key exists, hollow otherwise.
	if state != KeyingState::NoKey {
		let mut body = PathBuilder::fill();
		body.move_to(point(center.x, center.y - half));
		body.line_to(point(center.x + half, center.y));
		body.line_to(point(center.x, center.y + half));
		body.line_to(point(center.x - half, center.y));
		body.close();
		if let Ok(path) = body.build() {
			window.paint_path(path, fill_color);
		}
	}

	// The outline is always painted so the diamond is visible when empty.
	let mut outline = PathBuilder::stroke(px(1.0));
	outline.move_to(point(center.x, center.y - half));
	outline.line_to(point(center.x + half, center.y));
	outline.line_to(point(center.x, center.y + half));
	outline.line_to(point(center.x - half, center.y));
	outline.close();
	if let Ok(path) = outline.build() {
		window.paint_path(path, border_color);
	}

	// A center dot marks a key exactly at the current frame.
	if state == KeyingState::AtCurrentFrame {
		let origin = point(center.x - px(2.0), center.y - px(2.0));
		window.paint_quad(fill(
			Bounds::new(origin, size(px(4.0), px(4.0))),
			Hsla::from(colors.background),
		));
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use gpui_kit::{Context, Render, TestAppContext, Window, div, px, size};

	struct Host {
		control: usize,
		state: KeyingState,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div()
				.size_full()
				.child(keying_diamond(self.control, self.state))
		}
	}

	#[gpui_kit::test]
	async fn diamond_renders_in_a_window(cx: &mut TestAppContext) {
		use gpui_kit::VisualTestContext;
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(60.0), px(60.0)), |_window, _cx| Host {
			control: 3,
			state: KeyingState::AtCurrentFrame,
		});
		cx.run_until_parked();
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		cx.update(|window, cx| {
			window.draw(cx).clear(cx);
		});
	}

	#[test]
	fn request_round_trips() {
		let request = KeyingRequest::new(7, KeyingState::AtCurrentFrame);
		assert_eq!(request.control, 7);
		assert_eq!(request.state, KeyingState::AtCurrentFrame);
		assert_ne!(request, KeyingRequest::new(7, KeyingState::HasKey));
	}

	#[test]
	fn states_are_distinct() {
		assert_ne!(KeyingState::NoKey, KeyingState::HasKey);
		assert_ne!(KeyingState::HasKey, KeyingState::AtCurrentFrame);
		assert_ne!(KeyingState::NoKey, KeyingState::AtCurrentFrame);
	}

	#[test]
	fn keying_request_helper_matches() {
		assert_eq!(
			keying_request(3, KeyingState::NoKey),
			KeyingRequest::new(3, KeyingState::NoKey)
		);
	}
}
