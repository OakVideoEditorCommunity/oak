//! Scope widgets: histogram, waveform and vectorscope, painted from host
//! frame samples.
//!
//! Data-agnostic: the host implements [`LumaDataSource`] / [`ChromaDataSource`]
//! over its frame buffers (in Oak, sampled from the renderer via C ABI). The
//! pure math lives in [`math`] and is unit-tested.

mod math;

pub use math::*;

use gpui_kit::{
	App, Bounds, Context, Entity, FocusHandle, Focusable, Hsla, Render, Window, canvas,
	colors::DefaultColors, fill, point, prelude::*, px, size,
};

/// Provides luma samples (`0..1`) for the histogram and waveform scopes.
pub trait LumaDataSource: 'static {
	/// Luma samples for the current frame.
	fn luma_samples(&self) -> Vec<f32>;
}

/// Provides chroma samples for the vectorscope.
pub trait ChromaDataSource: 'static {
	/// `(u, v)` samples in `0..1` (centered on `0.5`).
	fn chroma_samples(&self) -> Vec<(f32, f32)>;
}

/// A luminance histogram.
pub struct Histogram<D: LumaDataSource> {
	data: Entity<D>,
	focus_handle: FocusHandle,
}

impl<D: LumaDataSource> Histogram<D> {
	/// Create a histogram over `data`.
	///
	/// `data` is the luma source sampled on every frame (one `0..1` value per
	/// pixel); `cx` supplies the focus handle so the panel can be focused
	/// like the other dock panels. `_control` and `_window` are unused — the
	/// scope is built by the panel factory with the same signature as its
	/// siblings.
	pub fn new(
		_control: usize,
		data: Entity<D>,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			data,
			focus_handle: cx.focus_handle(),
		}
	}

	/// The current histogram bins for the frame, always 64 of them.
	///
	/// `cx` provides the data source; each luma sample (`0..1`, clamped) is
	/// counted into one of 64 equal bins and the counts are returned in
	/// ascending luma order. An empty source yields 64 zero counts rather
	/// than an empty vector, so the render pass and hosts can index it
	/// directly. Also used by tests to assert the mapping.
	pub fn bins(&self, cx: &App) -> Vec<u32> {
		histogram_bins(&self.data.read(cx).luma_samples(), 64)
	}
}

impl<D: LumaDataSource> Focusable for Histogram<D> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<D: LumaDataSource> Render for Histogram<D> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let bins = self.bins(cx);
		let height = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
		canvas(
			move |_bounds, _window, _cx| (),
			move |bounds, (), window, _cx| {
				let width = f32::from(bounds.size.width);
				let bar_w = width / bins.len() as f32;
				let bar_color = Hsla::from(colors.selected);
				for (index, &count) in bins.iter().enumerate() {
					let h = (count as f32 / height) * f32::from(bounds.size.height);
					let bar = Bounds::new(
						point(
							bounds.left() + px(index as f32 * bar_w),
							bounds.bottom() - px(h),
						),
						size(px((bar_w - 1.0).max(1.0)), px(h)),
					);
					window.paint_quad(fill(bar, bar_color));
				}
			},
		)
		.size_full()
	}
}

/// A waveform scope (min/max envelope over the frame's luma).
pub struct Waveform<D: LumaDataSource> {
	data: Entity<D>,
	focus_handle: FocusHandle,
}

impl<D: LumaDataSource> Waveform<D> {
	/// Create a waveform scope over `data`.
	///
	/// `data` is the luma source sampled on every frame (one `0..1` value per
	/// pixel); `cx` supplies the focus handle so the panel can be focused
	/// like the other dock panels. `_control` and `_window` are unused — the
	/// scope is built by the panel factory with the same signature as its
	/// siblings.
	pub fn new(
		_control: usize,
		data: Entity<D>,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			data,
			focus_handle: cx.focus_handle(),
		}
	}

	/// The current envelope columns, one `(min, max)` luma pair per column.
	///
	/// `cx` provides the data source; the frame's samples are split into 128
	/// equal runs and each run is reduced to its min and max in `0..1`. Both
	/// values are 0 for an empty source or an empty run, so the render pass
	/// can draw every column unconditionally.
	pub fn envelope(&self, cx: &App) -> Vec<(f32, f32)> {
		waveform_envelope(&self.data.read(cx).luma_samples(), 128)
	}
}

impl<D: LumaDataSource> Focusable for Waveform<D> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<D: LumaDataSource> Render for Waveform<D> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let envelope = self.envelope(cx);
		canvas(
			move |_bounds, _window, _cx| (),
			move |bounds, (), window, _cx| {
				let width = f32::from(bounds.size.width);
				let height = f32::from(bounds.size.height);
				let col_w = width / envelope.len() as f32;
				let line = Hsla::from(colors.selected);
				for (column, &(min, max)) in envelope.iter().enumerate() {
					let y_min = (1.0 - min.clamp(0.0, 1.0)) * height;
					let y_max = (1.0 - max.clamp(0.0, 1.0)) * height;
					let band = Bounds::new(
						point(
							bounds.left() + px(column as f32 * col_w),
							bounds.top() + px(y_max),
						),
						size(px((col_w - 0.5).max(0.5)), px((y_min - y_max).max(1.0))),
					);
					window.paint_quad(fill(band, line));
				}
			},
		)
		.size_full()
	}
}

/// A vectorscope (chroma projection with a graticule).
pub struct Vectorscope<D: ChromaDataSource> {
	data: Entity<D>,
	focus_handle: FocusHandle,
}

impl<D: ChromaDataSource> Vectorscope<D> {
	/// Create a vectorscope over `data`.
	///
	/// `data` is the chroma source the scope samples on every frame; `cx`
	/// supplies the focus handle so the panel can be focused like the other
	/// dock panels. `_control` and `_window` are unused — the scope is built
	/// by the panel factory with the same signature as its siblings.
	pub fn new(
		_control: usize,
		data: Entity<D>,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			data,
			focus_handle: cx.focus_handle(),
		}
	}

	/// The projected chroma points for the current frame, centered on 0.
	///
	/// `cx` provides the data source. Each source `(u, v)` sample (already in
	/// `0..1`, centered on `0.5`) becomes a point in `-0.5..0.5` after
	/// subtracting the 0.5 center and clamping; the render pass maps these
	/// onto the canvas. Empty when the source has no samples.
	pub fn points(&self, cx: &App) -> Vec<(f32, f32)> {
		vectorscope_points(&self.data.read(cx).chroma_samples())
	}
}

impl<D: ChromaDataSource> Focusable for Vectorscope<D> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<D: ChromaDataSource> Render for Vectorscope<D> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let points = self.points(cx);
		canvas(
			move |_bounds, _window, _cx| (),
			move |bounds, (), window, _cx| {
				let width = f32::from(bounds.size.width);
				let height = f32::from(bounds.size.height);
				let center = point(
					bounds.left() + px(width / 2.0),
					bounds.top() + px(height / 2.0),
				);
				let grid = Hsla::from(colors.border);
				// Graticule: crosshair + box.
				window.paint_quad(fill(
					Bounds::new(
						point(bounds.left(), center.y - px(0.5)),
						size(bounds.size.width, px(1.0)),
					),
					grid,
				));
				window.paint_quad(fill(
					Bounds::new(
						point(center.x - px(0.5), bounds.top()),
						size(px(1.0), bounds.size.height),
					),
					grid,
				));
				// Points: u -> x, v -> y (inverted).
				let point_color = Hsla::from(colors.selected);
				for &(u, v) in points.iter().take(4096) {
					let x = center.x + px(u / 0.5 * width / 2.0);
					let y = center.y - px(v / 0.5 * height / 2.0);
					let dot = Bounds::new(point(x - px(1.0), y - px(1.0)), size(px(2.0), px(2.0)));
					window.paint_quad(fill(dot, point_color));
				}
			},
		)
		.size_full()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use gpui_kit::{Entity, Render, TestAppContext, Window, div, px, size};

	struct MockLuma(Vec<f32>);
	impl LumaDataSource for MockLuma {
		fn luma_samples(&self) -> Vec<f32> {
			self.0.clone()
		}
	}
	struct MockChroma(Vec<(f32, f32)>);
	impl ChromaDataSource for MockChroma {
		fn chroma_samples(&self) -> Vec<(f32, f32)> {
			self.0.clone()
		}
	}

	struct Host {
		histogram: Entity<Histogram<MockLuma>>,
		waveform: Entity<Waveform<MockLuma>>,
		vectorscope: Entity<Vectorscope<MockChroma>>,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div()
				.size_full()
				.child(self.histogram.clone())
				.child(self.waveform.clone())
				.child(self.vectorscope.clone())
		}
	}

	#[gpui_kit::test]
	async fn scopes_render_from_mock_data(cx: &mut TestAppContext) {
		use gpui_kit::VisualTestContext;
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(300.0), px(200.0)), |window, cx| {
			let luma = cx.new(|_| MockLuma((0..100).map(|i| i as f32 / 100.0).collect()));
			let chroma = cx.new(|_| MockChroma(vec![(0.5, 0.5), (0.75, 0.25), (0.25, 0.75)]));
			let histogram = cx.new(|cx| Histogram::new(1, luma.clone(), window, cx));
			let waveform = cx.new(|cx| Waveform::new(2, luma.clone(), window, cx));
			let vectorscope = cx.new(|cx| Vectorscope::new(3, chroma.clone(), window, cx));
			Host {
				histogram,
				waveform,
				vectorscope,
			}
		});
		cx.run_until_parked();
		let host = window.root(cx).unwrap();
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		// Force a draw so the canvas paint closures run (no double-lease:
		// VisualTestContext::update goes through App::update_window).
		cx.update(|window, cx| {
			window.draw(cx).clear(cx);
		});

		let (bins, envelope, points) = cx.read(|app| {
			let host = host.read(app);
			(
				host.histogram.read(app).bins(app),
				host.waveform.read(app).envelope(app),
				host.vectorscope.read(app).points(app),
			)
		});
		assert_eq!(bins.len(), 64);
		let total: u32 = bins.iter().sum();
		assert_eq!(total, 100);
		assert_eq!(envelope.len(), 128);
		assert_eq!(points.len(), 3);
	}
}
