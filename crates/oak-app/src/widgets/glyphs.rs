use gpui_kit::{
	Bounds, IntoElement, PathBuilder, Pixels, Rgba, canvas, fill, point, prelude::*, px, size,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
	List,
	Grid,
	Close,
	Check,
	ChevronRight,
	ChevronDown,
}

pub fn icon(glyph: Glyph, color: Rgba) -> impl IntoElement {
	canvas(
		move |_bounds, _window, _cx| (),
		move |bounds: Bounds<Pixels>, (), window, _cx| match glyph {
			Glyph::List => {
				for y in [3.0, 7.0, 11.0] {
					window.paint_quad(fill(
						Bounds::new(
							point(bounds.left() + px(2.0), bounds.top() + px(y)),
							size(px(12.0), px(2.0)),
						),
						color,
					));
				}
			}
			Glyph::Grid => {
				for (x, y) in [(2.0, 2.0), (9.0, 2.0), (2.0, 9.0), (9.0, 9.0)] {
					window.paint_quad(fill(
						Bounds::new(
							point(bounds.left() + px(x), bounds.top() + px(y)),
							size(px(5.0), px(5.0)),
						),
						color,
					));
				}
			}
			Glyph::Close => {
				stroke(window, color, &[(4.0, 4.0), (12.0, 12.0)], bounds);
				stroke(window, color, &[(12.0, 4.0), (4.0, 12.0)], bounds);
			}
			Glyph::Check => {
				stroke(
					window,
					color,
					&[(3.0, 8.0), (6.5, 11.5), (13.0, 4.0)],
					bounds,
				);
			}
			Glyph::ChevronRight => {
				stroke(
					window,
					color,
					&[(6.0, 3.0), (11.0, 8.0), (6.0, 13.0)],
					bounds,
				);
			}
			Glyph::ChevronDown => {
				stroke(
					window,
					color,
					&[(3.0, 6.0), (8.0, 11.0), (13.0, 6.0)],
					bounds,
				);
			}
		},
	)
	.size(px(16.0))
	.flex_shrink_0()
}

fn stroke(
	window: &mut gpui_kit::Window,
	color: Rgba,
	points: &[(f32, f32)],
	bounds: Bounds<Pixels>,
) {
	let mut path = PathBuilder::stroke(px(1.5));
	let Some((first, rest)) = points.split_first() else {
		return;
	};
	path.move_to(point(
		bounds.left() + px(first.0),
		bounds.top() + px(first.1),
	));
	for point_xy in rest {
		path.line_to(point(
			bounds.left() + px(point_xy.0),
			bounds.top() + px(point_xy.1),
		));
	}
	if let Ok(path) = path.build() {
		window.paint_path(path, color);
	}
}
