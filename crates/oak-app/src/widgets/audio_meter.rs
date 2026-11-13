//! An audio level meter: lit segments per channel with a peak-hold marker.
//!
//! Data-agnostic: the host implements [`AudioMeterDataSource`] over its
//! engine's channel levels (in Oak, queried from the audio engine). The peak
//! hold decays locally; the pure arithmetic is in [`scopes::math`].

use gpui_kit::{
	App, Bounds, Context, Entity, FocusHandle, Focusable, Hsla, Render, Window, canvas,
	colors::DefaultColors, fill, hsla, point, prelude::*, px, size,
};

use crate::widgets::scopes::{decay_peak, meter_lit_segments};

/// The number of segments per channel.
const SEGMENTS: usize = 16;
/// Peak decay per frame (fraction of full scale).
const PEAK_DECAY: f32 = 0.01;

/// Segment color by position, per the NLE convention: green for the body,
/// yellow approaching full scale, red at the top (segments count from the
/// meter's zero end).
fn segment_color(segment: usize) -> Hsla {
	let yellow_from = SEGMENTS * 5 / 8; // top ~37% caution
	let red_from = SEGMENTS * 13 / 16; // top ~19% clip zone
	if segment >= red_from {
		hsla(0.0, 0.65, 0.5, 1.0)
	} else if segment >= yellow_from {
		hsla(0.13, 0.75, 0.5, 1.0)
	} else {
		hsla(0.35, 0.6, 0.45, 1.0)
	}
}

/// The orientation of an [`AudioLevelMeter`].
///
/// A horizontal meter stacks channels vertically and lights segments left to
/// right; a vertical meter (e.g. the 26px strip in the Oak transport bar)
/// places channels side by side and lights segments bottom to top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeterOrientation {
	/// Channels stacked vertically, segments lit left to right.
	#[default]
	Horizontal,
	/// Channels side by side, segments lit bottom to top.
	Vertical,
}

/// Provides per-channel levels in `0..1` (linear or dB-normalized).
pub trait AudioMeterDataSource: 'static {
	/// The current level of each channel, `0..1`.
	fn levels(&self) -> Vec<f32>;
}

/// An audio level meter.
pub struct AudioLevelMeter<D: AudioMeterDataSource> {
	data: Entity<D>,
	focus_handle: FocusHandle,
	peak: Vec<f32>,
	orientation: MeterOrientation,
}

impl<D: AudioMeterDataSource> AudioLevelMeter<D> {
	/// Creates a meter that visualizes `data`.
	///
	/// `data` is the shared entity the meter reads channel levels from on
	/// every render; the meter only caches the peak-hold state, it never
	/// mutates the source. `_control` is currently ignored; callers pass a
	/// numeric control id for it. `_window` is unused, and `cx`
	/// supplies the focus handle. The meter starts as horizontal with an
	/// empty peak vector; `data` is not read until the first render or
	/// [`Self::update`].
	pub fn new(
		_control: usize,
		data: Entity<D>,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			data,
			focus_handle: cx.focus_handle(),
			peak: Vec::new(),
			orientation: MeterOrientation::Horizontal,
		}
	}

	/// Builder-style setter for the meter's layout orientation.
	///
	/// `orientation` chooses how channels and lit segments are arranged (see
	/// [`MeterOrientation`]); the default set by [`Self::new`] is
	/// horizontal. Consumes `self` and returns it, so it can be chained onto
	/// the constructor: `AudioLevelMeter::new(..).with_orientation(
	/// MeterOrientation::Vertical)`. The field is read on the next render;
	/// nothing is repainted or notified here.
	pub fn with_orientation(mut self, orientation: MeterOrientation) -> Self {
		self.orientation = orientation;
		self
	}

	/// Returns the orientation the meter is currently laid out in.
	///
	/// Defaults to [`MeterOrientation::Horizontal`]; the value is set with
	/// [`Self::with_orientation`] and read when rendering to decide whether
	/// channels stack vertically with left-to-right segments, or sit side by
	/// side with bottom-to-top segments.
	pub fn orientation(&self) -> MeterOrientation {
		self.orientation
	}

	/// Reads the current per-channel levels from the data source.
	///
	/// `cx` is only used to borrow the [`AudioMeterDataSource`] entity. The
	/// result has one linear value per channel in `0..1` (the same values
	/// the render path lights); it may be empty when the source reports no
	/// channels, so callers should not index it blindly. This is a read-only
	/// query: it does not advance the peak-hold state, which only
	/// [`Self::update`] does.
	pub fn levels(&self, cx: &App) -> Vec<f32> {
		self.data.read(cx).levels()
	}

	/// Update the peak-hold state from the current levels (call each frame).
	pub fn update(&mut self, cx: &mut Context<Self>) {
		let levels = self.data.read(cx).levels();
		self.peak.resize(levels.len(), 0.0);
		for (peak, level) in self.peak.iter_mut().zip(&levels) {
			*peak = decay_peak(*peak, *level, PEAK_DECAY);
		}
		cx.notify();
	}
}

impl<D: AudioMeterDataSource> Focusable for AudioLevelMeter<D> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<D: AudioMeterDataSource> Render for AudioLevelMeter<D> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let levels = self.data.read(cx).levels();
		self.peak.resize(levels.len(), 0.0);
		let peaks = self.peak.clone();
		let lit_counts: Vec<usize> = levels
			.iter()
			.map(|level| meter_lit_segments(*level, SEGMENTS))
			.collect();
		let orientation = self.orientation;

		canvas(
			move |_bounds, _window, _cx| (),
			move |bounds, (), window, _cx| {
				let width = f32::from(bounds.size.width);
				let height = f32::from(bounds.size.height);
				let dim_color = Hsla::from(colors.border);
				let peak_color = Hsla::from(colors.text);

				match orientation {
					MeterOrientation::Horizontal => {
						let channel_h = if lit_counts.is_empty() {
							height
						} else {
							height / lit_counts.len() as f32
						};
						let seg_w = width / SEGMENTS as f32;
						for (channel, &lit) in lit_counts.iter().enumerate() {
							let y = bounds.top() + px(channel as f32 * channel_h);
							for segment in 0..SEGMENTS {
								let seg = Bounds::new(
									point(bounds.left() + px(segment as f32 * seg_w), y),
									size(
										px((seg_w - 1.0).max(1.0)),
										px((channel_h - 2.0).max(2.0)),
									),
								);
								window.paint_quad(fill(
									seg,
									if segment < lit {
										segment_color(segment)
									} else {
										dim_color
									},
								));
							}
							// Peak marker.
							if let Some(peak) = peaks.get(channel) {
								let x = bounds.left() + px((peak.clamp(0.0, 1.0) * width) - 1.0);
								let marker = Bounds::new(
									point(x, y),
									size(px(2.0), px((channel_h - 2.0).max(2.0))),
								);
								window.paint_quad(fill(marker, peak_color));
							}
						}
					}
					MeterOrientation::Vertical => {
						// Channels side by side; segments stack bottom to top,
						// lit from the bottom like an equalizer column.
						let channel_w = if lit_counts.is_empty() {
							width
						} else {
							width / lit_counts.len() as f32
						};
						let seg_h = height / SEGMENTS as f32;
						for (channel, &lit) in lit_counts.iter().enumerate() {
							let x = bounds.left() + px(channel as f32 * channel_w);
							for segment in 0..SEGMENTS {
								let y = bounds.bottom() - px((segment + 1) as f32 * seg_h);
								let seg = Bounds::new(
									point(x, y),
									size(
										px((channel_w - 2.0).max(2.0)),
										px((seg_h - 1.0).max(1.0)),
									),
								);
								window.paint_quad(fill(
									seg,
									if segment < lit {
										segment_color(segment)
									} else {
										dim_color
									},
								));
							}
							// Peak marker.
							if let Some(peak) = peaks.get(channel) {
								let y = bounds.bottom() - px(peak.clamp(0.0, 1.0) * height);
								let marker = Bounds::new(
									point(x, y),
									size(px((channel_w - 2.0).max(2.0)), px(2.0)),
								);
								window.paint_quad(fill(marker, peak_color));
							}
						}
					}
				}
			},
		)
		.size_full()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::widgets::scopes::meter_lit_segments;
	use gpui_kit::{Entity, Render, TestAppContext, Window, div, px, size};

	#[test]
	fn meter_math_matches_scope_core() {
		assert_eq!(meter_lit_segments(0.0, SEGMENTS), 0);
		assert_eq!(meter_lit_segments(0.5, SEGMENTS), SEGMENTS / 2);
	}

	struct MockAudio(Vec<f32>);
	impl AudioMeterDataSource for MockAudio {
		fn levels(&self) -> Vec<f32> {
			self.0.clone()
		}
	}

	struct Host {
		meter: Entity<AudioLevelMeter<MockAudio>>,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.meter.clone())
		}
	}

	#[gpui_kit::test]
	async fn meter_renders_and_decays_peak(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(200.0), px(60.0)), |window, cx| {
			let audio = cx.new(|_| MockAudio(vec![0.8, 0.2]));
			let meter = cx.new(|cx| AudioLevelMeter::new(4, audio, window, cx));
			Host { meter }
		});
		cx.run_until_parked();

		// update() refreshes peaks from levels.
		let (peaks, levels) = window
			.update(cx, |host, _, cx| {
				host.meter.update(cx, |meter, cx| meter.update(cx));
				let levels = host.meter.read(cx).levels(cx);
				let peaks = host.meter.read(cx).peak.clone();
				(peaks, levels)
			})
			.unwrap();
		assert_eq!(levels, vec![0.8, 0.2]);
		// Peaks track the levels on the first update.
		assert!((peaks[0] - 0.8).abs() < 0.001);
	}

	#[gpui_kit::test]
	async fn vertical_meter_renders_in_a_narrow_strip(cx: &mut TestAppContext) {
		// The Oak transport design needs a 26px-wide vertical strip: channels
		// side by side, segments lit bottom to top. Render one at that exact
		// size and exercise the paint path (the orientation default is
		// horizontal, so this also covers the builder).
		struct StripHost {
			meter: Entity<AudioLevelMeter<MockAudio>>,
		}
		impl Render for StripHost {
			fn render(
				&mut self,
				_window: &mut Window,
				_cx: &mut Context<Self>,
			) -> impl IntoElement {
				div().size_full().child(self.meter.clone())
			}
		}

		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(26.0), px(200.0)), |window, cx| {
			let audio = cx.new(|_| MockAudio(vec![0.7]));
			let meter = cx.new(|cx| {
				AudioLevelMeter::new(5, audio, window, cx)
					.with_orientation(MeterOrientation::Vertical)
			});
			assert_eq!(meter.read(cx).orientation(), MeterOrientation::Vertical);
			StripHost { meter }
		});
		cx.run_until_parked();
		window
			.update(cx, |host, _, cx| {
				host.meter.update(cx, |meter, cx| meter.update(cx));
			})
			.unwrap();
		// Still valid after a vertical render + peak update.
		assert!(
			window
				.update(cx, |host, _, cx| host.meter.read(cx).orientation())
				.unwrap() == MeterOrientation::Vertical
		);
	}

	#[test]
	fn meter_orientation_defaults_to_horizontal() {
		assert_eq!(MeterOrientation::default(), MeterOrientation::Horizontal);
		assert_ne!(MeterOrientation::Horizontal, MeterOrientation::Vertical);
	}
}
