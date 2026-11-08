//! Clip rendering: the clip body element and pluggable content decorators.
//!
//! [`ClipElement`] draws one clip in the clip area: the body rect, label,
//! trim handles, transition wedges, and the visual states (hover, selected,
//! disabled, locked-track). Rich content — filmstrip thumbnails for video,
//! waveform for audio — is **not** painted by the element itself; it is
//! delegated to a [`ClipDecorator`] so hosts with their own decode/analysis
//! caches (Oak's codec and render caches) can plug them in without forking
//! the widget.
//!
//! # Hit zones
//!
//! The outer [`TRIM_HANDLE_WIDTH`] pixels at each clip edge are trim
//! handles (cursor changes to a horizontal resize cursor, drag starts a trim
//! gesture). The interior is the move-grab region. The zone width is a
//! *screen-space* constant, so clips narrower than two handles prioritize
//! trimming — the move region may vanish on very short clips.

use std::sync::Arc;

use gpui_kit::{
	App, BorderStyle, Bounds, Hsla, PathBuilder, Pixels, SharedString, Window, canvas, div, fill,
	hsla, outline, point, prelude::*, px,
};

use super::{
	data::ClipId,
	time::{Frame, FrameRange},
};

/// Screen-space width of each trim-handle hit zone, in pixels.
pub const TRIM_HANDLE_WIDTH: f32 = 6.0;

/// Corner radius of the clip body, per the design's rounded bars. Painted
/// into the body quad itself: gpui's content masks are rectangular only, so
/// a rounded `overflow_hidden` wrapper would not clip the canvas corners.
const CLIP_CORNER_RADIUS: Pixels = px(4.0);

/// Fallback pixels-per-frame used for internal decoration geometry.
///
/// [`ClipElement`] receives no zoom (see the type docs), so transition wedge
/// widths and the decorator's visible frame range are approximated at
/// 1 px/frame. That is exact at 100% zoom and drifts as the user zooms.
/// TODO(timeline): thread the timeline zoom (or the clip's on-screen frame
/// range) through [`ClipElement`] so wedges and decorator content track the
/// zoom level.
const PX_PER_FRAME_FALLBACK: f32 = 1.0;

/// The luma scale applied to a selected clip's body: darkening one notch
/// (×0.7) makes the selection read clearly even when the clip color matches
/// the selection accent (green clips against a green selection outline would
/// otherwise show no visible change at all).
const SELECTED_LUMA_SCALE: f32 = 0.7;

/// The clip body fill: the clip color, darkened one luma notch when selected
/// and dimmed (halved alpha) when the clip is disabled.
fn clip_body_color(color: Hsla, selected: bool, enabled: bool) -> Hsla {
	let color = if selected {
		Hsla {
			l: color.l * SELECTED_LUMA_SCALE,
			..color
		}
	} else {
		color
	};
	if enabled {
		color
	} else {
		Hsla {
			a: color.a * 0.5,
			..color
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The selected-clip body darkens one luma notch (×0.7) while keeping hue
	/// and saturation; a disabled clip halves the alpha; the two compose.
	#[test]
	fn selected_clip_body_darkens_one_luma_notch() {
		let color = hsla(0.402, 0.385, 0.459, 1.0);

		let plain = clip_body_color(color, false, true);
		assert_eq!(plain.l, color.l, "an unselected enabled clip keeps its color");
		assert_eq!(plain.a, color.a);

		let selected = clip_body_color(color, true, true);
		assert!(
			(selected.l - color.l * SELECTED_LUMA_SCALE).abs() < 1e-6,
			"the selected body luma scales by {} (got {})",
			SELECTED_LUMA_SCALE,
			selected.l
		);
		assert_eq!(selected.h, color.h, "hue is untouched");
		assert_eq!(selected.s, color.s, "saturation is untouched");
		assert_eq!(selected.a, color.a, "alpha is untouched");

		let disabled = clip_body_color(color, false, false);
		assert!(
			(disabled.a - color.a * 0.5).abs() < 1e-6,
			"a disabled clip halves its alpha"
		);
		assert_eq!(disabled.l, color.l, "disabling alone does not darken");

		let selected_disabled = clip_body_color(color, true, false);
		assert_eq!(selected_disabled.l, color.l * SELECTED_LUMA_SCALE);
		assert_eq!(selected_disabled.a, color.a * 0.5);
	}
}

/// What the clip content area should show, per track kind and zoom.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipContent {
	/// Just the body color and label (also the fallback when zoomed out).
	#[default]
	None,
	/// Video thumbnails — painted via [`ClipDecorator::paint_thumbnail`].
	Thumbnails,
	/// Audio waveform — painted via [`ClipDecorator::paint_waveform`].
	Waveform,
}

/// Paints rich clip content (thumbnails, waveforms) into the clip body.
///
/// The timeline calls these hooks during paint with the clip's identity, the
/// currently visible frame range of that clip, and the bounds to paint into.
/// Implementations should cache aggressively — paint is called every frame —
/// which is exactly where Oak plugs in its codec frame cache and audio
/// peak caches.
///
/// Both hooks have default no-op implementations, so a decorator can provide
/// only what it needs; [`NoopClipDecorator`] provides neither.
///
/// The trait is object-safe and used behind `Arc`.
pub trait ClipDecorator: 'static {
	/// Paints a video thumbnail filmstrip for `clip` covering
	/// `visible_range` (the part of the clip currently on screen) into
	/// `bounds`.
	///
	/// `media_in` semantics matter here: the strip starts at
	/// [`ClipData::media_in`](super::ClipData::media_in), so frame
	/// `visible_range.start` of the *timeline* maps to source frame
	/// `media_in + (visible_range.start - clip.range().start)`.
	///
	/// `window` is the paint context, `clip` the element's id (the thumbnail
	/// cache key), `visible_range` the on-screen part of the clip in timeline
	/// frames and `bounds` the pixel rect to fill. Default implementation is a
	/// no-op; implementors should cache, since paint runs every frame.
	fn paint_thumbnail(
		&mut self,
		_window: &mut Window,
		_clip: ClipId,
		_visible_range: FrameRange,
		_bounds: Bounds<Pixels>,
	) {
		// no-op by default
	}

	/// Paints an audio waveform for `clip` covering `visible_range` into
	/// `bounds`. Same media-time mapping as
	/// [`ClipDecorator::paint_thumbnail`]. Typically drawn with
	/// [`PathBuilder`](gpui_kit::PathBuilder) from cached peak data.
	///
	/// `window` is the paint context, `clip` the element's id (the natural cache
	/// key for peak data), `visible_range` the on-screen part of the clip in
	/// timeline frames and `bounds` the pixel rect to fill. Default
	/// implementation is a no-op, so decorators that draw no waveform inherit it
	/// unchanged.
	fn paint_waveform(
		&mut self,
		_window: &mut Window,
		_clip: ClipId,
		_visible_range: FrameRange,
		_bounds: Bounds<Pixels>,
	) {
		// no-op by default
	}
}

/// The default decorator: paints no thumbnails and no waveforms.
///
/// Used when the host doesn't need rich clip content (or hasn't wired its
/// caches yet — Oak will replace this with a decorator backed by its codec
/// and render caches).
#[derive(Debug, Default)]
pub struct NoopClipDecorator;

impl ClipDecorator for NoopClipDecorator {}

/// One clip body in the clip area.
///
/// Constructed per visible clip per frame by
/// [`TimelineView`](super::TimelineView) from a
/// [`ClipData`](super::ClipData) snapshot. The element owns no frame→pixel
/// mapping: the view positions and sizes it via its container, and internal
/// decorations (transition wedges, decorator content) fall back to
/// [`PX_PER_FRAME_FALLBACK`] until a zoom is threaded through — see that
/// constant for the limitation.
#[derive(IntoElement)]
pub struct ClipElement {
	id: ClipId,
	label: SharedString,
	color: Hsla,
	selected: bool,
	enabled: bool,
	locked: bool,
	multicam: bool,
	in_transition: Option<FrameRange>,
	out_transition: Option<FrameRange>,
	content: ClipContent,
	decorator: Arc<std::sync::RwLock<dyn ClipDecorator>>,
}

impl ClipElement {
	/// Creates a clip element.
	///
	/// * `id` / `label` / `color` — from the clip's
	///   [`ClipData`](super::ClipData); `color` is the clip color or the
	///   track-kind default resolved by the caller.
	/// * `in_transition` / `out_transition` — the frame ranges (in
	///   *clip-local* time) of the head/tail transition wedges, if any.
	/// * `decorator` — shared decorator instance; see [`ClipDecorator`].
	#[allow(clippy::too_many_arguments)]
	pub fn new(
		id: ClipId,
		label: SharedString,
		color: Hsla,
		in_transition: Option<FrameRange>,
		out_transition: Option<FrameRange>,
		decorator: Arc<std::sync::RwLock<dyn ClipDecorator>>,
	) -> Self {
		ClipElement {
			id,
			label,
			color,
			selected: false,
			enabled: true,
			locked: false,
			multicam: false,
			in_transition,
			out_transition,
			content: ClipContent::None,
			decorator,
		}
	}

	/// Builder: render in the selected state (selection outline).
	///
	/// `selected` is stored as-is; the element starts unselected, so this is only
	/// needed when the clip is part of the timeline selection. The outline is
	/// drawn in addition to transition wedges and decorator content. Returns
	/// `self` for chaining with [`Self::enabled`], [`Self::locked`] and the other
	/// builder flags.
	pub fn selected(mut self, selected: bool) -> Self {
		self.selected = selected;
		self
	}

	/// Builder: render in the disabled state (dimmed, no snapping target).
	pub fn enabled(mut self, enabled: bool) -> Self {
		self.enabled = enabled;
		self
	}

	/// Builder: render as belonging to a locked track (no trim handles, no
	/// drag cursor).
	pub fn locked(mut self, locked: bool) -> Self {
		self.locked = locked;
		self
	}

	/// Builder: render as a multi-cam clip (accent border + corner badge).
	pub fn multicam(mut self, multicam: bool) -> Self {
		self.multicam = multicam;
		self
	}

	/// Builder: what content the decorator should paint inside the body.
	pub fn content(mut self, content: ClipContent) -> Self {
		self.content = content;
		self
	}

	/// The clip this element renders.
	pub fn clip_id(&self) -> ClipId {
		self.id
	}

	/// The trim-handle hit zone width, in pixels.
	pub fn trim_handle_width(&self) -> Pixels {
		gpui_kit::px(TRIM_HANDLE_WIDTH)
	}

	// TODO(implementor): register the trim-handle hit zones when the element
	// grows interactive handles; they are currently part of the view's
	// interaction layer.
}

/// Paint-time snapshot of a [`ClipElement`], captured by the canvas prepaint
/// callback and consumed by the paint callback.
struct ClipPaint {
	id: ClipId,
	color: Hsla,
	selected: bool,
	enabled: bool,
	locked: bool,
	multicam: bool,
	in_transition: Option<FrameRange>,
	out_transition: Option<FrameRange>,
	content: ClipContent,
	decorator: Arc<std::sync::RwLock<dyn ClipDecorator>>,
}

impl RenderOnce for ClipElement {
	fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
		let ClipElement {
			id,
			label,
			color,
			selected,
			enabled,
			locked,
			multicam,
			in_transition,
			out_transition,
			content,
			decorator,
			..
		} = self;

		// The clip body is custom-painted (body quad, transition wedges,
		// state overlays, decorator content); the label rides in a styled
		// overlay div so it inherits the usual text elision.
		div()
			.relative()
			.size_full()
			.child(
				canvas(
					move |_bounds, _window, _cx| ClipPaint {
						id,
						color,
						selected,
						enabled,
						locked,
						multicam,
						in_transition,
						out_transition,
						content,
						decorator,
					},
					|bounds, paint, window, _cx| {
						// Body quad. A selected clip's body is darkened one
						// luma notch so the selection reads even when the clip
						// color matches the selection accent; a disabled clip
						// is dimmed.
						let body_color =
							clip_body_color(paint.color, paint.selected, paint.enabled);
						window.paint_quad(fill(bounds, body_color).corner_radii(CLIP_CORNER_RADIUS));

						// Transition wedges: triangles tapering into the clip
						// from each edge, capped at 40% of the body width so tiny
						// clips don't vanish.
						let max_wedge = bounds.size.width.as_f32() * 0.4;
						if let Some(range) = paint.in_transition {
							let w = (range.len().0 as f32 * PX_PER_FRAME_FALLBACK).min(max_wedge);
							if w > 0.0 {
								let mut path = PathBuilder::fill();
								path.move_to(point(bounds.left(), bounds.top()));
								path.line_to(point(bounds.left() + px(w), bounds.top()));
								path.line_to(point(bounds.left(), bounds.bottom()));
								path.close();
								window.paint_path(
									path.build().expect("wedge path is valid"),
									paint.color,
								);
							}
						}
						if let Some(range) = paint.out_transition {
							let w = (range.len().0 as f32 * PX_PER_FRAME_FALLBACK).min(max_wedge);
							if w > 0.0 {
								let mut path = PathBuilder::fill();
								path.move_to(point(bounds.right(), bounds.top()));
								path.line_to(point(bounds.right() - px(w), bounds.top()));
								path.line_to(point(bounds.right(), bounds.bottom()));
								path.close();
								window.paint_path(
									path.build().expect("wedge path is valid"),
									paint.color,
								);
							}
						}

						// State overlays, in back-to-front order.
						if !paint.enabled {
							window.paint_quad(
								fill(bounds, hsla(0., 0., 0.05, 0.55))
									.corner_radii(CLIP_CORNER_RADIUS),
							);
						}
						if paint.locked {
							window.paint_quad(
								fill(bounds, hsla(0., 0., 0.1, 0.25))
									.corner_radii(CLIP_CORNER_RADIUS),
							);
						}

						// Rich content via the decorator, scoped to the frames
						// visible inside this body.
						let visible_range = FrameRange::new(
							Frame::ZERO,
							Frame((bounds.size.width.as_f32() / PX_PER_FRAME_FALLBACK) as i64),
						);
						match paint.content {
							ClipContent::Thumbnails => paint
								.decorator
								.write()
								.expect("clip decorator lock is not poisoned")
								.paint_thumbnail(window, paint.id, visible_range, bounds),
							ClipContent::Waveform => paint
								.decorator
								.write()
								.expect("clip decorator lock is not poisoned")
								.paint_waveform(window, paint.id, visible_range, bounds),
							ClipContent::None => {}
						}

						// Multi-cam clip overlay: a warm accent border plus
						// a corner "camera wedge" (muted-saturation gold,
						// clearly distinct from the green clip bodies and
						// the green selection outline).
						if paint.multicam {
							window.paint_quad(
								outline(
									bounds,
									hsla(0.13, 0.9, 0.55, 1.0),
									BorderStyle::Solid,
								)
								.corner_radii(CLIP_CORNER_RADIUS),
							);
							// Corner badge: a small triangle in the top-left
							// corner, echoing a lens/record indicator.
							let badge = px(10.0);
							let mut path = PathBuilder::fill();
							path.move_to(point(bounds.left(), bounds.top()));
							path.line_to(point(bounds.left() + badge, bounds.top()));
							path.line_to(point(bounds.left(), bounds.top() + badge));
							path.close();
							window.paint_path(
								path.build().expect("multicam badge path is valid"),
								hsla(0.13, 0.9, 0.42, 1.0),
							);
						}

						// Selection outline on top of everything.
						if paint.selected {
							window.paint_quad(
								outline(bounds, hsla(0.6, 0.8, 0.6, 1.), BorderStyle::Solid)
									.corner_radii(CLIP_CORNER_RADIUS),
							);
						}
					},
				)
				.size_full(),
			)
			.child(
				div()
					.absolute()
					.left(px(4.))
					.top(px(4.))
					.right(px(4.))
					.overflow_hidden()
					.whitespace_nowrap()
					.text_ellipsis()
					.text_size(px(11.))
					.text_color(hsla(0., 0., 1., 0.92))
					.child(label),
			)
	}
}
