// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! The grading color wheel (三向色轮的单个轮子): a disk where the angle
//! picks the tint hue and the radius picks its intensity. The neutral
//! value (0.5 gray, the three-way corrector's no-op point) sits at the
//! center.
//!
//! The mapping is the wheel's contract:
//!
//! - `wheel_to_color(angle, radius)` — hue at full saturation is
//!   projected onto the neutral-centered deviation cube and scaled by
//!   the radius, so the rim is the strongest tint and the center is
//!   neutral;
//! - `color_to_wheel(color)` — the exact inverse: the hue of the color
//!   and `max|channel - 0.5| * 2` as the radius.
//!
//! Both directions are pinned by tests below.
//!
//! A drag emits [`ColorWheelEvent::DragStarted`] once, [`ColorWheelEvent::Changed`]
//! per move and [`ColorWheelEvent::DragFinished`] at the end, so the host
//! groups the whole gesture into one undo entry (the same contract the
//! parameter sliders follow).

use std::sync::{Arc, RwLock};

use gpui_kit::{
	App, Bounds, Context, EventEmitter, MouseButton, MouseDownEvent, MouseUpEvent, PathBuilder,
	Pixels, Point, Render, Rgba, Window, canvas, colors::DefaultColors, div, point, prelude::*, px,
};

/// The wheel's painted diameter, in logical pixels.
pub const WHEEL_SIZE: f32 = 84.0;

/// Polar resolution of the painted disk (segments around × rings out).
const SEGMENTS: usize = 48;
const RINGS: usize = 8;

/// The neutral channel value (the three-way corrector's no-op point).
const NEUTRAL: f32 = 0.5;

/// A drag payload tagging the wheel it belongs to (a drag over another
/// wheel is not ours).
struct WheelDrag {
	/// The owning control's id.
	control: usize,
}

/// The invisible drag ghost (the wheel itself shows the indicator).
struct WheelDragGhost;

impl Render for WheelDragGhost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().size(px(1.0))
	}
}

/// A wheel gesture event.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorWheelEvent {
	/// A drag started (the host opens an undo group).
	DragStarted {
		/// The control's stable id.
		control: usize,
	},
	/// The value changed (click or drag move).
	Changed {
		/// The control's stable id.
		control: usize,
		/// The new param color.
		color: Rgba,
	},
	/// The drag ended (the host closes the undo group).
	DragFinished {
		/// The control's stable id.
		control: usize,
	},
}

/// The hue (degrees, 0 = red) of an RGB color; 0 for grays.
///
/// `r`, `g`, `b` are the color's channels (any equal scale works: the result is
/// scale-invariant). Returns `0..360` degrees, or `0` when the channels are
/// equal within `1e-6` — a gray has no hue, so it folds to the red axis. Pairs
/// with `hsv_to_rgb`.
fn hue_of(r: f32, g: f32, b: f32) -> f32 {
	let max = r.max(g).max(b);
	let min = r.min(g).min(b);
	let d = max - min;
	if d < 1e-6 {
		return 0.0;
	}
	let h = if (max - r).abs() < 1e-6 {
		((g - b) / d).rem_euclid(6.0)
	} else if (max - g).abs() < 1e-6 {
		(b - r) / d + 2.0
	} else {
		(r - g) / d + 4.0
	};
	h * 60.0
}

/// HSV (degrees, 0..1, 0..1) → RGB (0..1 channels).
///
/// `h` is the hue in degrees and is wrapped into `0..360` (negative and
/// over-range values are accepted); `s` and `v` are saturation and value in
/// `0..1` and are not clamped. Returns `(r, g, b)` with alpha implied at 1.
/// Pairs with `hue_of`; `wheel_to_color` calls it at full saturation/value.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
	let h = h.rem_euclid(360.0) / 60.0;
	let c = v * s;
	let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
	let m = v - c;
	let (r, g, b) = match h as u32 {
		0 => (c, x, 0.0),
		1 => (x, c, 0.0),
		2 => (0.0, c, x),
		3 => (0.0, x, c),
		4 => (x, 0.0, c),
		_ => (c, 0.0, x),
	};
	((r + m) as f32, (g + m) as f32, (b + m) as f32)
}

/// Map a wheel position to the param color: the hue at full saturation
/// projected onto the neutral-centered deviation cube (the dominant
/// channel rails at the rim), scaled by `radius`. Radius 0 is neutral.
pub fn wheel_to_color(angle_deg: f32, radius: f32) -> Rgba {
	let radius = radius.clamp(0.0, 1.0);
	let (r, g, b) = hsv_to_rgb(angle_deg, 1.0, 1.0);
	let (dr, dg, db) = (r - NEUTRAL, g - NEUTRAL, b - NEUTRAL);
	let m = dr.abs().max(dg.abs()).max(db.abs());
	let scale = if m > 1e-6 { NEUTRAL / m } else { 0.0 };
	Rgba {
		r: (NEUTRAL + radius * dr * scale).clamp(0.0, 1.0),
		g: (NEUTRAL + radius * dg * scale).clamp(0.0, 1.0),
		b: (NEUTRAL + radius * db * scale).clamp(0.0, 1.0),
		a: 1.0,
	}
}

/// Map a param color back to the wheel: the color's hue and
/// `max|channel - 0.5| * 2` as the radius. Grays sit at the center.
pub fn color_to_wheel(color: Rgba) -> (f32, f32) {
	let (dr, dg, db) = (
		color.r - NEUTRAL,
		color.g - NEUTRAL,
		color.b - NEUTRAL,
	);
	let intensity = (dr.abs().max(dg.abs()).max(db.abs()) * 2.0).clamp(0.0, 1.0);
	if intensity < 1e-4 {
		return (0.0, 0.0);
	}
	(hue_of(color.r, color.g, color.b), intensity)
}

/// A grading color wheel: one tint control of a three-way row.
pub struct ColorWheel {
	/// The control's stable id (element ids, drag routing, events).
	control: usize,
	/// The current param color.
	value: Rgba,
	/// Whether a drag is in flight (a re-sync must not fight the pointer).
	dragging: bool,
	/// The disk's window-space bounds, captured each frame by the canvas
	/// prepaint (hit mapping).
	bounds: Arc<RwLock<Option<Bounds<Pixels>>>>,
}

impl ColorWheel {
	/// A wheel showing `value`.
	///
	/// `control` is the control's stable id (embedded in every emitted event and
	/// used to route drags to the right wheel); `value` is the initial param color.
	/// `_cx` is accepted for the widget-constructor convention but unused: no state
	/// is registered. The painted bounds are captured by the first canvas prepaint,
	/// so pointer mapping before that frame is a no-op.
	pub fn new(control: usize, value: Rgba, _cx: &mut Context<Self>) -> Self {
		Self {
			control,
			value,
			dragging: false,
			bounds: Arc::new(RwLock::new(None)),
		}
	}

	/// The current value.
	pub fn value(&self) -> Rgba {
		self.value
	}

	/// Whether a drag is in flight.
	pub fn is_dragging(&self) -> bool {
		self.dragging
	}

	/// Set the value from outside (engine re-sync); ignored mid-drag.
	pub fn set_value(&mut self, value: Rgba, cx: &mut Context<Self>) {
		if self.dragging {
			return;
		}
		if self.value != value {
			self.value = value;
			cx.notify();
		}
	}

	/// Map a window-space point to a wheel position (angle degrees,
	/// radius 0..1 clamped): the y axis points down on screen, so up is
	/// positive hue direction.
	fn position_to_wheel(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
		let bounds = *self.bounds.read().unwrap();
		let bounds = bounds?;
		let center = bounds.center();
		let radius_px = f32::from(bounds.size.width) * 0.5;
		if radius_px <= 0.0 {
			return None;
		}
		let dx = f32::from(position.x - center.x);
		let dy = f32::from(position.y - center.y);
		let radius = (dx.hypot(dy) / radius_px).clamp(0.0, 1.0);
		let angle = (-dy).atan2(dx).to_degrees().rem_euclid(360.0);
		Some((angle, radius))
	}

	/// Apply a pointer position as the new value (click or drag move).
	///
	/// `position` is window-space; it is mapped by `position_to_wheel` to the
	/// wheel's angle/radius and then by `wheel_to_color`. Positions before the first
	/// painted bounds are ignored. Value, `ColorWheelEvent::Changed` and a notify
	/// are only produced when the resulting color differs, so a move inside the same
	/// painted cell stays quiet.
	fn apply_pointer(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
		let Some((angle, radius)) = self.position_to_wheel(position) else {
			return;
		};
		let color = wheel_to_color(angle, radius);
		if color != self.value {
			self.value = color;
			cx.emit(ColorWheelEvent::Changed {
				control: self.control,
				color,
			});
			cx.notify();
		}
	}

	/// Complete an in-flight drag (no-op otherwise).
	///
	/// Clears `dragging`, emits `ColorWheelEvent::DragFinished` for the control and
	/// notifies, which lets the host close the undo group opened by `DragStarted`.
	/// Called on mouse-up; a mouse-up with no drag in flight changes and emits
	/// nothing.
	fn finish_drag(&mut self, cx: &mut Context<Self>) {
		if self.dragging {
			self.dragging = false;
			cx.emit(ColorWheelEvent::DragFinished {
				control: self.control,
			});
			cx.notify();
		}
	}

	/// Reset the value to neutral (the double-click handler).
	///
	/// Computes neutral through the same mapping (`wheel_to_color(0.0, 0.0)`, the
	/// three-way corrector's no-op gray) and, when the value actually changes, emits
	/// `DragStarted`, `Changed` and `DragFinished` in that order around the update,
	/// so the host records the reset as one undoable gesture, then notifies.
	///
	/// `cx` is the wheel's context. A double-click while already neutral emits
	/// nothing.
	fn reset(&mut self, cx: &mut Context<Self>) {
		let neutral = wheel_to_color(0.0, 0.0);
		if self.value == neutral {
			return;
		}
		cx.emit(ColorWheelEvent::DragStarted {
			control: self.control,
		});
		self.value = neutral;
		cx.emit(ColorWheelEvent::Changed {
			control: self.control,
			color: neutral,
		});
		cx.emit(ColorWheelEvent::DragFinished {
			control: self.control,
		});
		cx.notify();
	}

	/// Paint the disk: polar cells colored by the mapping itself (the
	/// wheel shows exactly the tint each position produces), plus the
	/// indicator ring at the value's position.
	fn paint(value: Rgba, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
		let colors = cx.default_colors().clone();
		let center = bounds.center();
		let radius_px = f32::from(bounds.size.width) * 0.5;
		for ring in 0..RINGS {
			let r0 = ring as f32 / RINGS as f32;
			let r1 = (ring + 1) as f32 / RINGS as f32;
			for segment in 0..SEGMENTS {
				let a0 = segment as f32 / SEGMENTS as f32 * 360.0;
				let a1 = (segment + 1) as f32 / SEGMENTS as f32 * 360.0;
				let cell = wheel_to_color((a0 + a1) * 0.5, (r0 + r1) * 0.5);
				let point_at = |angle: f32, radius: f32| {
					let rad = angle.to_radians();
					point(
						center.x + px(rad.cos() * radius * radius_px),
						center.y - px(rad.sin() * radius * radius_px),
					)
				};
				let mut path = PathBuilder::fill();
				path.move_to(point_at(a0, r0));
				path.line_to(point_at(a0, r1));
				path.line_to(point_at(a1, r1));
				path.line_to(point_at(a1, r0));
				path.close();
				window.paint_path(path.build().expect("the sector path builds"), cell);
			}
		}

		// The indicator: a dark/light double ring at the value's position
		// (readable on any tint).
		let (angle, radius) = color_to_wheel(value);
		let rad = angle.to_radians();
		let at = point(
			center.x + px(rad.cos() * radius * radius_px),
			center.y - px(rad.sin() * radius * radius_px),
		);
		for (ring_radius, color) in [
			(6.0f32, colors.background),
			(5.0, colors.text),
		] {
			let mut ring = PathBuilder::stroke(px(1.5));
			let steps = 16;
			for i in 0..=steps {
				let a = i as f32 / steps as f32 * std::f32::consts::TAU;
				let p = point(
					at.x + px(a.cos() * ring_radius),
					at.y + px(a.sin() * ring_radius),
				);
				if i == 0 {
					ring.move_to(p);
				} else {
					ring.line_to(p);
				}
			}
			window.paint_path(ring.build().expect("the indicator ring builds"), color);
		}
	}
}

impl EventEmitter<ColorWheelEvent> for ColorWheel {}

impl Render for ColorWheel {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let value = self.value;
		let control = self.control;
		let record = self.bounds.clone();
		div()
			.id(gpui_kit::ElementId::named_usize("color-wheel", control))
			.debug_selector(move || format!("color-wheel-{control}"))
			.size(px(WHEEL_SIZE))
			.cursor_pointer()
			.child(
				canvas(
					move |b, _window, _cx| {
						*record.write().unwrap() = Some(b);
						value
					},
					move |b, value, window, cx| {
						ColorWheel::paint(value, b, window, cx);
					},
				)
				.size_full(),
			)
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(|this, event: &MouseDownEvent, _window, cx| {
					if event.click_count >= 2 {
						this.reset(cx);
						return;
					}
					this.dragging = true;
					cx.emit(ColorWheelEvent::DragStarted {
						control: this.control,
					});
					this.apply_pointer(event.position, cx);
				}),
			)
			.on_drag(
				Arc::new(RwLock::new(WheelDrag { control })),
				|_payload, _offset, _window, cx| cx.new(|_| WheelDragGhost),
			)
			.on_drag_move(cx.listener(
				|this, event: &gpui_kit::DragMoveEvent<Arc<RwLock<WheelDrag>>>, _window, cx| {
					let ours = event.drag(cx).read().unwrap().control == this.control;
					if !ours {
						return;
					}
					this.apply_pointer(event.event.position, cx);
				},
			))
			// Finish the gesture on mouse up, wherever it lands (the same
			// in/out pair the sliders use).
			.on_mouse_up(
				MouseButton::Left,
				cx.listener(|this, _event: &MouseUpEvent, _window, cx| this.finish_drag(cx)),
			)
			.on_mouse_up_out(
				MouseButton::Left,
				cx.listener(|this, _event: &MouseUpEvent, _window, cx| this.finish_drag(cx)),
			)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, size};

	#[test]
	fn wheel_mapping_round_trips() {
		// Neutral sits at the center.
		let neutral = wheel_to_color(0.0, 0.0);
		assert!((neutral.r - 0.5).abs() < 1e-6);
		assert_eq!(color_to_wheel(neutral), (0.0, 0.0));

		// The rim is the strongest tint: red at full radius rails the red
		// channel up and the others down.
		let red = wheel_to_color(0.0, 1.0);
		assert!((red.r - 1.0).abs() < 1e-6 && red.g.abs() < 1e-6 && red.b.abs() < 1e-6);

		// A mid-tint round-trips (angle + radius survive the cube
		// projection within float tolerance).
		let (angle, radius) = color_to_wheel(wheel_to_color(200.0, 0.6));
		assert!((angle - 200.0).abs() < 1.0, "hue survives: {angle}");
		assert!((radius - 0.6).abs() < 1e-3, "intensity survives: {radius}");

		// Every rim color is a legal 0..1 param color.
		for deg in (0..360).step_by(15) {
			let c = wheel_to_color(deg as f32, 1.0);
			for ch in [c.r, c.g, c.b] {
				assert!((0.0..=1.0).contains(&ch));
			}
		}
	}

	/// A click drags the value to the pointed tint and the gesture wraps
	/// in one DragStarted/DragFinished pair; double-click resets to
	/// neutral.
	#[gpui_kit::test]
	async fn wheel_drag_and_double_click_reset(cx: &mut TestAppContext) {
		cx.update(|cx| {
			gpui_kit::init(cx);
			cx.init_colors();
		});
		let events = Arc::new(RwLock::new(Vec::new()));
		let events_sub = events.clone();
		struct Host {
			wheel: gpui_kit::Entity<ColorWheel>,
		}
		impl Render for Host {
			fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
				div().size_full().child(self.wheel.clone())
			}
		}
		let window = cx.open_window(size(px(200.0), px(200.0)), |_window, cx| {
			let wheel = cx.new(|cx| ColorWheel::new(1, wheel_to_color(0.0, 0.0), cx));
			cx.subscribe(&wheel, move |_w, _e, event: &ColorWheelEvent, _cx| {
				events_sub.write().unwrap().push(event.clone());
			})
			.detach();
			Host { wheel }
		});
		cx.run_until_parked();
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		cx.update(|window, cx| {
			window.draw(cx).clear(cx);
		});
		let bounds = cx.debug_bounds("color-wheel-1").expect("wheel painted");
		let center = bounds.center();

		// Click at the right rim (0° hue, full radius).
		let rim = point(center.x + px(f32::from(bounds.size.width) * 0.45), center.y);
		cx.simulate_click(rim, Modifiers::default());
		let got = events.read().unwrap().clone();
		assert!(
			matches!(got.first(), Some(ColorWheelEvent::DragStarted { .. })),
			"the gesture opens: {got:?}"
		);
		assert!(got
			.iter()
			.any(|e| matches!(e, ColorWheelEvent::Changed { color, .. } if color.r > 0.9)));
		assert!(
			matches!(got.last(), Some(ColorWheelEvent::DragFinished { .. })),
			"the gesture closes: {got:?}"
		);

		// Double-click resets to neutral.
		events.write().unwrap().clear();
		cx.simulate_click(center, Modifiers::default());
		cx.simulate_click(center, Modifiers::default());
		let got = events.read().unwrap().clone();
		assert!(got
			.iter()
			.any(|e| matches!(e, ColorWheelEvent::Changed { color, .. } if (color.r - 0.5).abs() < 1e-6)));
	}
}
