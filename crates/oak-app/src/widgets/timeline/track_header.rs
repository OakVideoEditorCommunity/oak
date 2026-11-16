//! Track headers: the per-track control column left of the clip area.
//!
//! One [`TrackHeader`] per track shows the track name and the toggle buttons
//! appropriate for its [`TrackKind`], and hosts the drag separator that
//! resizes the track's height.
//!
//! # Toggles per track kind
//!
//! | [`TrackKind`]             | Buttons                     |
//! |---------------------------|-----------------------------|
//! | [`TrackKind::Video`]      | lock, show (visibility)     |
//! | [`TrackKind::Audio`]      | lock, mute, solo            |
//! | [`TrackKind::Subtitle`]   | lock, show                  |
//!
//! Toggles emit [`TrackHeaderEvent`]s; like every other edit surface of this
//! module the header never mutates the model — the host applies the change
//! through its engine and the next data read reflects it.
//!
//! The toggle glyphs are clickable when [`TrackHeader::on_toggle`] is
//! installed (the [`TimelineView`](super::TimelineView) wires it to emit
//! `TimelineEvent::TrackToggleRequested`); without a handler they render as
//! inert status glyphs.

use gpui_kit::{
	App, ClickEvent, ElementId, SharedString, Window, colors::DefaultColors, div, hsla, prelude::*,
	px,
};

use super::data::TrackKind;

/// A user action on a track header's controls.
///
/// The host applies these to its track state (Oak: the facade's track
/// lock/mute/solo/show setters, wrapped in undo where the engine treats them
/// as undoable) and notifies the data source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackHeaderEvent {
	/// Lock toggle requested. Locked tracks reject all clip edits.
	ToggleLock,
	/// Mute toggle requested (audio tracks).
	ToggleMute,
	/// Solo toggle requested (audio tracks).
	ToggleSolo,
	/// Visibility (show) toggle requested (video/subtitle tracks).
	ToggleVisibility,
}

/// The handler a host attaches to turn a toggle click into an engine
/// request: `(track index, requested toggle, window, app)`.
pub type TrackToggleHandler =
	std::sync::Arc<dyn Fn(usize, TrackHeaderEvent, &mut Window, &mut App)>;

/// The header control for one track, rendered in the left column.
///
/// The header fills its cell in the view's row layout; the bottom
/// [`Self::SEPARATOR_HEIGHT`] pixels are the height-resize drag zone.
#[derive(IntoElement)]
pub struct TrackHeader {
	index: usize,
	name: SharedString,
	kind: TrackKind,
	locked: bool,
	muted: bool,
	solo: bool,
	visible: bool,
	/// Click handler for the toggle glyphs; `None` renders them inert.
	on_toggle: Option<TrackToggleHandler>,
}

impl TrackHeader {
	/// Height of the drag separator zone at the header's bottom edge, in
	/// pixels.
	pub const SEPARATOR_HEIGHT: f32 = 4.0;

	/// Creates a header for the track at `index` from its
	/// [`TrackData`](super::TrackData) snapshot.
	pub fn new(index: usize, name: SharedString, kind: TrackKind) -> Self {
		TrackHeader {
			index,
			name,
			kind,
			locked: false,
			muted: false,
			solo: false,
			visible: true,
			on_toggle: None,
		}
	}

	/// Builder: current lock state (drives the lock button's active style).
	pub fn locked(mut self, locked: bool) -> Self {
		self.locked = locked;
		self
	}

	/// Builder: current mute state. Only meaningful for
	/// [`TrackKind::Audio`]; ignored otherwise.
	pub fn muted(mut self, muted: bool) -> Self {
		self.muted = muted;
		self
	}

	/// Builder: current solo state. Only meaningful for
	/// [`TrackKind::Audio`]; ignored otherwise.
	pub fn solo(mut self, solo: bool) -> Self {
		self.solo = solo;
		self
	}

	/// Builder: current visibility (show) state. Only meaningful for
	/// [`TrackKind::Video`] and [`TrackKind::Subtitle`]; ignored otherwise.
	pub fn visible(mut self, visible: bool) -> Self {
		self.visible = visible;
		self
	}

	/// Builder: the handler invoked when a toggle glyph is clicked. Without
	/// it the glyphs render inert (pure status display).
	pub fn on_toggle(mut self, handler: TrackToggleHandler) -> Self {
		self.on_toggle = Some(handler);
		self
	}

	/// Index of the track this header controls.
	pub fn track_index(&self) -> usize {
		self.index
	}

	/// A small toggle glyph (one or two letters) reflecting `active`. When an
	/// [`Self::on_toggle`] handler is installed the glyph is a click target
	/// emitting `event`; the click stops propagating so it never toggles the
	/// header row's track selection.
	fn toggle_glyph(
		&self,
		glyph: &'static str,
		active: bool,
		event: TrackHeaderEvent,
	) -> impl IntoElement {
		let index = self.index;
		let on_toggle = self.on_toggle.clone();
		let mut target = div()
			.id(ElementId::Name(
				format!("track-toggle-{index}-{glyph}").into(),
			))
			.debug_selector(move || format!("track-toggle-{index}-{glyph}").into())
			.px_1()
			.rounded(px(3.))
			.text_xs()
			.font_weight(if active {
				gpui_kit::FontWeight::BOLD
			} else {
				gpui_kit::FontWeight::NORMAL
			})
			.text_color(if active {
				hsla(0.63, 0.6, 0.65, 1.0)
			} else {
				hsla(0.0, 0.0, 0.5, 0.55)
			})
			.child(glyph.to_string());
		if let Some(handler) = on_toggle {
			target = target
				.cursor_pointer()
				.hover(|style| style.bg(hsla(0.63, 0.6, 0.65, 0.35)))
				.on_click(move |_click: &ClickEvent, _window, cx: &mut App| {
					handler(index, event, _window, cx);
					cx.stop_propagation();
				});
		}
		target
	}

	/// The kind-appropriate toggle glyphs, left of the separator.
	fn toggle_row(&self) -> impl IntoElement {
		let lock = self.toggle_glyph("L", self.locked, TrackHeaderEvent::ToggleLock);
		match self.kind {
			TrackKind::Audio => div()
				.flex()
				.flex_row()
				.items_center()
				.gap(px(3.))
				.child(lock)
				.child(self.toggle_glyph("M", self.muted, TrackHeaderEvent::ToggleMute))
				.child(self.toggle_glyph("S", self.solo, TrackHeaderEvent::ToggleSolo)),
			TrackKind::Video | TrackKind::Subtitle => div()
				.flex()
				.flex_row()
				.items_center()
				.gap(px(3.))
				.child(lock)
				.child(self.toggle_glyph("V", self.visible, TrackHeaderEvent::ToggleVisibility)),
		}
	}
}

impl RenderOnce for TrackHeader {
	fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
		// Neutral chrome per the design: the header cell sits on the panel
		// container color with primary-text labels; the track kind is conveyed
		// by the toggle glyphs, not by a tinted background.
		let colors = cx.default_colors().clone();
		let separator_height = px(TrackHeader::SEPARATOR_HEIGHT);

		div()
			.size_full()
			.bg(colors.container)
			.flex()
			.flex_col()
			.child(
				div()
					.flex_1()
					.flex()
					.flex_row()
					.items_center()
					.gap(px(6.))
					.px_2()
					.child(
						div()
							.text_sm()
							.text_color(colors.text)
							.child(self.name.clone()),
					)
					.child(div().flex_1())
					.child(self.toggle_row()),
			)
			.child(
				div()
					.h(separator_height)
					.w_full()
					.bg(hsla(0.0, 0.0, 0.5, 0.25)),
			)
	}
}
