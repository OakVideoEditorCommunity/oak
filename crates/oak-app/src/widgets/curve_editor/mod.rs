//! A keyframe curve editor: edit a cubic-bezier curve by dragging points and
//! their control handles.
//!
//! The curve is a normalized `x in 0..1` -> `y in 0..1` mapping (e.g. a time
//! remap). The widget keeps a local working copy of the points for fluid
//! dragging and emits [`CurveEditorEvent`] requests; the host applies them
//! through its model and calls [`CurveEditor::set_points`] to reconcile.
//! Pure geometry lives in [`curve`] and is unit-tested.

mod curve;

use std::sync::{Arc, RwLock};

use gpui_kit::{
	App, Bounds, ClickEvent, Context, DragMoveEvent, ElementId, Entity, EventEmitter, FocusHandle,
	Focusable, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, Render, Window,
	canvas, colors::DefaultColors, div, fill, point, prelude::*, px, quad, size,
};
use gpui_kit::{BorderStyle, Corners, Edges, PathBuilder};

pub use curve::{CurvePoint, CurveVec2, HandleSide, hit_test_handle, hit_test_point, sample_curve};

/// The default editor height.
const EDITOR_HEIGHT: f32 = 120.0;
/// The paint threshold (normalized units) for grabbing a point or handle.
const HIT_THRESHOLD: f64 = 0.06;

/// A request emitted by a curve editor.
#[derive(Debug, Clone, PartialEq)]
pub enum CurveEditorEvent {
	/// A keyframe point was dragged to a new position.
	PointMoved {
		/// The control's stable id.
		control: usize,
		/// The point's index in the (sorted) point list.
		index: usize,
		/// The point's new position/handles.
		point: CurvePoint,
	},
	/// A bezier control handle was dragged.
	HandleMoved {
		/// The control's stable id.
		control: usize,
		/// The owning point's index.
		index: usize,
		/// Which handle was moved.
		side: HandleSide,
		/// The handle's new offset from the point (normalized).
		handle: CurveVec2,
	},
	/// A new keyframe point was added (double-click on the canvas).
	PointAdded {
		/// The control's stable id.
		control: usize,
		/// The index the point was inserted at.
		index: usize,
		/// The new point.
		point: CurvePoint,
	},
}

/// What a drag gesture is editing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DragTarget {
	None,
	Point(usize),
	Handle { index: usize, side: HandleSide },
}

/// Transient payload carried by an in-flight drag.
#[derive(Clone, Copy, Debug)]
struct CurveDrag {
	target: DragTarget,
}

/// Invisible ghost view for drags.
#[derive(Clone, Copy, Debug)]
struct CurveGhost;

impl Render for CurveGhost {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		div().w(px(0.0)).h(px(0.0))
	}
}

/// A keyframe curve editor.
pub struct CurveEditor {
	control: usize,
	points: Vec<CurvePoint>,
	focus_handle: FocusHandle,
	/// Canvas bounds, refreshed each frame for position conversion.
	bounds: Bounds<Pixels>,
	/// Hit target recorded on mouse-down, consumed when the drag starts.
	pending_drag: Option<DragTarget>,
}

impl CurveEditor {
	/// Create an editor for `control` over `points` (already sorted by x).
	pub fn new(
		control: usize,
		points: Vec<CurvePoint>,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			control,
			points,
			focus_handle: cx.focus_handle(),
			bounds: Bounds::default(),
			pending_drag: None,
		}
	}

	/// The current working copy of the curve.
	pub fn points(&self) -> &[CurvePoint] {
		&self.points
	}

	/// Whether a point/handle drag is in progress (hosts must not push a
	/// `set_points` re-sync mid-drag — it would steal the gesture).
	pub fn is_dragging(&self) -> bool {
		self.pending_drag.is_some()
	}

	/// Apply the host's reconciled curve and repaint.
	///
	/// `points` replaces the working copy as-is (the host is expected to hand over
	/// a sorted list; nothing is clamped or re-sorted), and `cx` receives the
	/// notify. Hosts must not call this mid-drag — see [`Self::is_dragging`] — since
	/// it would steal the gesture.
	pub fn set_points(&mut self, points: Vec<CurvePoint>, cx: &mut Context<Self>) {
		self.points = points;
		cx.notify();
	}

	/// Convert a window position into normalized curve coordinates.
	///
	/// `position` is window-space; the result is `0..=1` on both axes with y
	/// flipped (curve origin at the bottom-left). Falls back to the center
	/// `(0.5, 0.5)` when the editor has not been laid out yet (zero width or
	/// height), so callers never divide by zero.
	fn normalize(&self, position: Point<Pixels>) -> CurveVec2 {
		let w = f32::from(self.bounds.size.width);
		let h = f32::from(self.bounds.size.height);
		if w <= 0.0 || h <= 0.0 {
			return CurveVec2::new(0.5, 0.5);
		}
		CurveVec2::new(
			((f32::from(position.x) - f32::from(self.bounds.left())) / w).clamp(0.0, 1.0) as f64,
			1.0 - ((f32::from(position.y) - f32::from(self.bounds.top())) / h).clamp(0.0, 1.0)
				as f64,
		)
	}

	fn hit_test(&self, position: Point<Pixels>) -> DragTarget {
		let pos = self.normalize(position);
		if let Some((index, side)) = hit_test_handle(&self.points, pos, HIT_THRESHOLD) {
			return DragTarget::Handle { index, side };
		}
		if let Some(index) = hit_test_point(&self.points, pos, HIT_THRESHOLD) {
			return DragTarget::Point(index);
		}
		DragTarget::None
	}

	/// Move point `index` to `pos`, keeping the list sorted.
	///
	/// `pos` is in normalized curve coordinates: x is clamped so the point cannot
	/// pass its neighbours (a 0.001 gutter inside the neighbour's x, or the `0..=1`
	/// edge at the ends) and y is clamped to `0..=1`. `index` must be valid (it
	/// panics otherwise; callers take it from `hit_test`). Emits `PointMoved` with
	/// the clamped point and repaints.
	fn move_point(&mut self, index: usize, pos: CurveVec2, cx: &mut Context<Self>) {
		let (prev_x, next_x) = if self.points.len() == 1 {
			(0.0, 1.0)
		} else if index == 0 {
			(0.0, self.points[1].x)
		} else if index == self.points.len() - 1 {
			(self.points[index - 1].x, 1.0)
		} else {
			(self.points[index - 1].x, self.points[index + 1].x)
		};
		let min_x = (prev_x + 0.001).min(1.0);
		let max_x = (next_x - 0.001).max(0.0);
		let point = self.points.get_mut(index).expect("point index in range");
		point.x = pos.x.clamp(min_x, max_x);
		point.y = pos.y.clamp(0.0, 1.0);
		let moved = *point;
		cx.emit(CurveEditorEvent::PointMoved {
			control: self.control,
			index,
			point: moved,
		});
		cx.notify();
	}

	/// Move one side of point `index`'s control handle to `pos`.
	///
	/// `pos` is in normalized curve coordinates; what is stored is the offset from
	/// the point (`pos - point`), so the tangent follows the pointer. `side` picks
	/// the incoming or outgoing handle, `index` must exist (an out-of-range index is
	/// ignored). `HandleMoved` is emitted with the new offset, then the widget
	/// repaints.
	fn move_handle(
		&mut self,
		index: usize,
		side: HandleSide,
		pos: CurveVec2,
		cx: &mut Context<Self>,
	) {
		let Some(point) = self.points.get_mut(index) else {
			return;
		};
		let offset = CurveVec2::new(pos.x - point.x, pos.y - point.y);
		match side {
			HandleSide::In => point.handle_in = Some(offset),
			HandleSide::Out => point.handle_out = Some(offset),
		}
		cx.emit(CurveEditorEvent::HandleMoved {
			control: self.control,
			index,
			side,
			handle: offset,
		});
		cx.notify();
	}

	/// Insert a new point at `pos` (double-click), keeping the list sorted.
	///
	/// `pos` is in normalized curve coordinates and is clamped to `0..=1` per axis.
	/// The point is inserted at its sorted x position (binary search) with no
	/// control handles, so the segments either side of it are linear until a handle
	/// is dragged. `PointAdded` is emitted with the insertion index and the widget
	/// repaints. A double-click always adds a point, even on top of an existing one.
	///
	/// `cx` is the editor context.
	fn add_point(&mut self, pos: CurveVec2, cx: &mut Context<Self>) {
		let x = pos.x.clamp(0.0, 1.0);
		let y = pos.y.clamp(0.0, 1.0);
		let insert_at = self.points.partition_point(|p| p.x < x);
		self.points.insert(insert_at, CurvePoint::new(x, y));
		cx.emit(CurveEditorEvent::PointAdded {
			control: self.control,
			index: insert_at,
			point: CurvePoint::new(x, y),
		});
		cx.notify();
	}
}

impl EventEmitter<CurveEditorEvent> for CurveEditor {}

impl Focusable for CurveEditor {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl Render for CurveEditor {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let entity = cx.entity();
		let canvas_entity = entity.clone();
		let points = self.points.clone();
		let control = self.control;

		div()
			.id(ElementId::named_usize("gpui-widgets-curve", control))
			.h(px(EDITOR_HEIGHT))
			.rounded_md()
			.bg(colors.background)
			.border_1()
			.border_color(colors.border)
			.overflow_hidden()
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
					this.pending_drag = Some(this.hit_test(event.position));
				}),
			)
			.on_drag(
				Arc::new(RwLock::new(CurveDrag {
					target: DragTarget::None,
				})),
				move |drag, offset, window, cx| {
					curve_ghost(drag, offset, window, cx, entity.clone())
				},
			)
			.on_drag_move(cx.listener(
				|this, event: &DragMoveEvent<Arc<RwLock<CurveDrag>>>, _window, cx| {
					let drag = event.drag(cx).clone();
					let target = drag.read().unwrap().target;
					let pos = this.normalize(event.event.position);
					match target {
						DragTarget::Point(index) => this.move_point(index, pos, cx),
						DragTarget::Handle { index, side } => {
							this.move_handle(index, side, pos, cx);
						}
						DragTarget::None => {}
					}
				},
			))
			.on_click(cx.listener(|this, event: &ClickEvent, _window, cx| {
				if event.click_count() >= 2 {
					this.add_point(this.normalize(event.position()), cx);
				}
			}))
			.on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
				if matches!(event.keystroke.key.as_str(), "escape") {
					this.pending_drag = None;
				}
				cx.notify();
			}))
			.child(
				canvas(
					move |bounds, _window, cx| {
						canvas_entity.update(cx, |this, _| this.bounds = bounds);
						bounds
					},
					move |bounds, content, window, cx| {
						paint_curve(bounds, content, &points, &colors, window, cx);
					},
				)
				.size_full(),
			)
	}
}

/// Initialize an in-flight drag with the hit target recorded at mouse-down.
fn curve_ghost(
	drag: &Arc<RwLock<CurveDrag>>,
	_offset: Point<Pixels>,
	_window: &mut Window,
	cx: &mut App,
	entity: Entity<CurveEditor>,
) -> Entity<CurveGhost> {
	entity.update(cx, |this, _| {
		let target = this.pending_drag.take().unwrap_or(DragTarget::None);
		if let Ok(mut drag) = drag.write() {
			drag.target = target;
		}
	});
	cx.new(|_| CurveGhost)
}

/// Paint the editor: the grid, the interpolated curve polyline, then every point
/// with its in/out handle dots and tangent lines.
///
/// `bounds` is the canvas area in window space (a zero size paints nothing);
/// `_content` is the canvas content bounds (unused: the curve fills `bounds`);
/// `points` is the sorted working copy of the curve; `colors` supplies the grid,
/// curve, point and handle colors from the host theme. `window` receives the
/// quads and paths, `_cx` is unused, and the mapping flips y so values grow
/// upward on screen.
///
/// Called from the canvas paint callback in `render`, after the prepaint stored
/// the bounds for hit testing.
fn paint_curve(
	bounds: Bounds<Pixels>,
	_content: Bounds<Pixels>,
	points: &[CurvePoint],
	colors: &gpui_kit::colors::Colors,
	window: &mut Window,
	_cx: &mut App,
) {
	let width = f32::from(bounds.size.width);
	let height = f32::from(bounds.size.height);
	if width <= 0.0 || height <= 0.0 {
		return;
	}
	let to_px = |v: CurveVec2| {
		point(
			bounds.left() + px((v.x as f32) * width),
			bounds.top() + px((1.0 - v.y as f32) * height),
		)
	};

	// Subtle grid lines.
	let grid = Hsla::from(colors.border).opacity(0.35);
	for i in 1..4 {
		let fx = i as f32 / 4.0;
		window.paint_quad(fill(
			Bounds::new(
				point(bounds.left() + px(fx * width), bounds.top()),
				size(px(1.0), bounds.size.height),
			),
			grid,
		));
		window.paint_quad(fill(
			Bounds::new(
				point(bounds.left(), bounds.top() + px(fx * height)),
				size(bounds.size.width, px(1.0)),
			),
			grid,
		));
	}

	// The curve polyline.
	let line = curve::polyline(points, 16);
	if line.len() >= 2 {
		let mut path = PathBuilder::stroke(px(2.0));
		let mut iter = line.iter();
		if let Some(first) = iter.next() {
			path.move_to(to_px(*first));
		}
		for v in iter {
			path.line_to(to_px(*v));
		}
		if let Ok(path) = path.build() {
			window.paint_path(path, Hsla::from(colors.selected));
		}
	}

	// Points and handles.
	let point_color = Hsla::from(colors.text);
	let handle_color = Hsla::from(colors.disabled);
	for pt in points {
		let p = to_px(CurveVec2::new(pt.x, pt.y));
		for handle in [pt.handle_in, pt.handle_out] {
			if let Some(h) = handle {
				let hp = to_px(CurveVec2::new(pt.x + h.x, pt.y + h.y));
				let mut line = PathBuilder::stroke(px(1.0));
				line.move_to(p);
				line.line_to(hp);
				if let Ok(path) = line.build() {
					window.paint_path(path, handle_color);
				}
				let dot = Bounds::new(
					point(hp.x - px(3.0), hp.y - px(3.0)),
					size(px(6.0), px(6.0)),
				);
				window.paint_quad(quad(
					dot,
					Corners::all(px(3.0)),
					handle_color,
					Edges::all(px(0.0)),
					handle_color,
					BorderStyle::Solid,
				));
			}
		}
		let r = px(5.0);
		let circle = Bounds::new(point(p.x - r, p.y - r), size(r * 2.0, r * 2.0));
		window.paint_quad(quad(
			circle,
			Corners::all(r),
			point_color,
			Edges::all(px(1.5)),
			Hsla::from(colors.background),
			BorderStyle::Solid,
		));
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};

	fn demo_points() -> Vec<CurvePoint> {
		vec![
			CurvePoint::with_handles(0.0, 0.0, CurveVec2::new(0.0, 0.5)),
			CurvePoint::with_handles(1.0, 1.0, CurveVec2::new(0.0, -0.5)),
		]
	}

	#[gpui_kit::test]
	async fn dragging_a_point_emits_point_moved(cx: &mut TestAppContext) {
		struct Host {
			editor: Entity<CurveEditor>,
			events: Vec<CurveEditorEvent>,
		}
		impl Render for Host {
			fn render(
				&mut self,
				_window: &mut Window,
				_cx: &mut Context<Self>,
			) -> impl IntoElement {
				div().size_full().child(self.editor.clone())
			}
		}

		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(400.0), px(200.0)), |window, cx| {
			let editor = cx.new(|cx| CurveEditor::new(1, demo_points(), window, cx));
			let host = Host {
				editor,
				events: Vec::new(),
			};
			cx.subscribe(
				&host.editor,
				|host: &mut Host,
				 _e: Entity<CurveEditor>,
				 event: &CurveEditorEvent,
				 _cx: &mut Context<Host>| {
					host.events.push(event.clone());
				},
			)
			.detach();
			host
		});
		cx.run_until_parked();
		let host = window.root(cx).unwrap();

		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		// The curve editor spans the full window width, 120px tall. The
		// first point is at normalized (0,0) -> bottom-left of the canvas.
		let start = point(px(5.0), px(115.0));
		let drag_to = point(px(5.0), px(60.0));
		cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
		cx.simulate_mouse_move(
			point(px(5.0), px(105.0)),
			MouseButton::Left,
			Modifiers::none(),
		);
		cx.simulate_mouse_move(drag_to, MouseButton::Left, Modifiers::none());
		cx.simulate_mouse_up(drag_to, MouseButton::Left, Modifiers::none());
		cx.run_until_parked();

		let (points, moved) = cx.read(|app| {
			let host = host.read(app);
			let points = host.editor.read(app).points().to_vec();
			let moved = host
				.events
				.iter()
				.any(|e| matches!(e, CurveEditorEvent::PointMoved { index: 0, .. }));
			(points, moved)
		});
		assert!(moved, "expected a PointMoved event for point 0");
		assert!(
			points[0].y > 0.1,
			"point should have moved up: {:?}",
			points[0]
		);
	}

	#[gpui_kit::test]
	async fn double_click_adds_point(cx: &mut TestAppContext) {
		struct Host {
			editor: Entity<CurveEditor>,
			events: Vec<CurveEditorEvent>,
		}
		impl Render for Host {
			fn render(
				&mut self,
				_window: &mut Window,
				_cx: &mut Context<Self>,
			) -> impl IntoElement {
				div().size_full().child(self.editor.clone())
			}
		}

		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(400.0), px(200.0)), |window, cx| {
			let editor = cx.new(|cx| CurveEditor::new(1, demo_points(), window, cx));
			let host = Host {
				editor,
				events: Vec::new(),
			};
			cx.subscribe(
				&host.editor,
				|host: &mut Host,
				 _e: Entity<CurveEditor>,
				 event: &CurveEditorEvent,
				 _cx: &mut Context<Host>| {
					host.events.push(event.clone());
				},
			)
			.detach();
			host
		});
		cx.run_until_parked();
		let host = window.root(cx).unwrap();

		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		// Double-click in the middle of the canvas (x=200, y=100).
		let pos = point(px(200.0), px(100.0));
		let modifiers = Modifiers::none();
		cx.simulate_event(MouseDownEvent {
			position: pos,
			modifiers,
			button: MouseButton::Left,
			click_count: 2,
			first_mouse: false,
		});
		cx.simulate_event(gpui_kit::MouseUpEvent {
			position: pos,
			modifiers,
			button: MouseButton::Left,
			click_count: 2,
		});
		cx.run_until_parked();

		let (count, added) = cx.read(|app| {
			let host = host.read(app);
			(
				host.editor.read(app).points().len(),
				host.events
					.iter()
					.any(|e| matches!(e, CurveEditorEvent::PointAdded { .. })),
			)
		});
		assert_eq!(count, 3);
		assert!(added);
	}
}
