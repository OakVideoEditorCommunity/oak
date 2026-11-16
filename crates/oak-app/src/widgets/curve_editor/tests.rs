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
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
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
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
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
