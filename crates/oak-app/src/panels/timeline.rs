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

//! The timeline panel (时间线): the design's 31px toolbar (tools, snap
//! toggle) above the full-width [`TimelineView`](gpui::timeline::TimelineView)
//! over the engine's sequence model.
//!
//! # Layout (fixed 2026-08)
//!
//! ```text
//! ┌─────────────────────────────────────────┬─────────────┐
//! │ toolbar row (fixed 31px): tools + − ⏵  │             │
//! ├─────────────────────────────────────────┤ right-side  │
//! │ timeline (ruler takes remaining width,  │ controls    │
//! │  clip area below)                       │ (fixed 140px│
//! │                                         │  zoom /     │
//! │                                         │  track hgt) │
//! └─────────────────────────────────────────┴─────────────┘
//! ```
//!
//! The zoom and track-height sliders used to sit at the right end of the
//! toolbar, where they overflowed into the ruler's timecode labels (the
//! toolbar is exactly 31px but the sliders' value rows are taller, and at
//! narrow widths the sliders squeezed into the ruler's right side). They
//! now live in a fixed-width trailing slot beside the timeline body, and the
//! timeline wrapper is `min_w_0` so the ruler always keeps the remaining
//! space — no overlap at 1600×900 or down to ~1100px wide.

use crate::oakui::component::controls::ValueKind;
use crate::oakui::component::controls::{CheckBox, CheckBoxEvent, CheckState};
use crate::oakui::component::controls::{Slider, SliderEvent, SliderModel};
use crate::oakui::component::menu::{Menu, MenuItem};
use gpui::colors::DefaultColors;
use gpui::dock::{DockPanel, PanelEvent};
use gpui::timeline::{
	ClipData, ClipId, Frame, TimelineEvent, TimelineHit, TimelineTool, TimelineView, TrackData,
	TrackKind, HEADER_WIDTH, MIN_TRACK_HEIGHT, RULER_HEIGHT,
};
use gpui::{div, img, prelude::*, px, Context, Entity, MouseButton, Pixels, Point, Window};
use gpui::{AnyElement, App, ClickEvent, DragMoveEvent, EventEmitter, Render, SharedString};
use gpui_widgets::project_explorer::FootageDrag;
use gpui_widgets::tooltip::tooltip_view;
use gpui_widgets::viewer::PlaybackClock;

use crate::actions::ActionId;
use crate::i18n;
use crate::oakui::component::menu;
use crate::oakui::component::menu::{ContextMenuHandle, ContextMenuTriggered};
use crate::oakui::icons;
use crate::oakui::{AppEngine, Monitor};
use crate::panels::commands::{self as panel_commands, PanelCommandHandler};
use crate::panels::ids::TIMELINE;

/// Toolbar height, per the design (31px).
const TOOLBAR_HEIGHT: f32 = 31.0;
/// Fixed width of the trailing controls slot (zoom / track-height sliders).
/// Kept constant so the sliders can never intrude into the ruler's labels.
const RIGHT_CONTROLS_WIDTH: f32 = 140.0;
/// The demo tool set, by i18n key, with the matching toolbar icon (the
/// legacy C++ icon set). Only the visual selection is implemented; each
/// tool's behavior arrives with the real tool system later.
const TOOLS: [(&str, &str); 8] = [
	("timeline.tool.select", crate::oakui::icons::ICON_ARROW),
	("timeline.tool.razor", crate::oakui::icons::ICON_RAZOR),
	("timeline.tool.ripple", crate::oakui::icons::ICON_RIPPLE),
	("timeline.tool.slip", crate::oakui::icons::ICON_SLIP),
	("timeline.tool.roll", crate::oakui::icons::ICON_ROLLING),
	("timeline.tool.zoom", crate::oakui::icons::ICON_ZOOM),
	("timeline.tool.slide", crate::oakui::icons::ICON_SLIDE),
	(
		"timeline.tool.track_select",
		crate::oakui::icons::ICON_TRACK_SELECT,
	),
];

/// The timeline panel.
pub struct TimelinePanel<E: AppEngine> {
	timeline: Entity<TimelineView<E>>,
	engine: Entity<E>,
	zoom: Entity<Slider>,
	height: Entity<Slider>,
	snap: Entity<CheckBox>,
	/// The drop point of an in-flight footage drag: the display track under
	/// the cursor plus the start frame. `None` outside the clip area or while
	/// no footage drag is active.
	footage_drop: Option<FootageDropTarget>,
	/// The drop point of an in-flight effect-library drag (generator
	/// effects becoming standalone clips). Same shape as `footage_drop`.
	effect_drop: Option<FootageDropTarget>,
	/// The right-click context menu (opened from
	/// [`TimelineEvent::ContextMenuRequested`]).
	context_menu: ContextMenuHandle,
	/// The track behind the currently open track-head menu (the "Delete"
	/// item's target); `None` when a different menu is open.
	context_track: Option<usize>,
	/// The empty-area click behind the currently open empty-area menu: the
	/// display track under the cursor plus the clicked frame ("Add
	/// Adjustment Layer" anchors there); `None` when a different menu is
	/// open.
	context_empty: Option<(usize, Frame)>,
}

/// A footage drop target resolved from the cursor: the display track under
/// the pointer and the clip's start frame.
struct FootageDropTarget {
	/// The pointed track's kind.
	track_kind: TrackKind,
	/// The pointed display track index.
	track_index: usize,
	/// The start frame at the pointer.
	time: Frame,
	/// The footage's length in frames (the ghost's extent).
	length: i64,
}

impl<E: AppEngine> TimelinePanel<E> {
	/// Builds the panel around `timeline` (created by the app shell so it can
	/// sync the playhead).
	pub fn new(
		engine: Entity<E>,
		timeline: Entity<TimelineView<E>>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let zoom = cx.new(|cx| {
			Slider::new(
				10,
				SliderModel::new(ValueKind::Float, 0.5, 8.0, 0.1, 2.0),
				window,
				cx,
			)
		});
		let height = cx.new(|cx| {
			Slider::new(
				11,
				SliderModel::new(ValueKind::Float, 24.0, 160.0, 8.0, 64.0),
				window,
				cx,
			)
		});
		let snap = cx.new(|cx| CheckBox::new(12, CheckState::Checked, window, cx));

		// Zoom slider → timeline zoom (pixels per frame).
		cx.subscribe(&zoom, |this, _zoom, event: &SliderEvent, cx| {
			if let SliderEvent::ValueChanged { value, .. } = event {
				let zoom = value.to_f64() as f32;
				this.timeline.update(cx, |timeline, cx| {
					timeline.state.set_zoom(zoom, px(0.0));
					cx.notify();
				});
			}
		})
		.detach();

		// Track-height slider → engine model (persisted per sequence).
		cx.subscribe(&height, |this, _height, event: &SliderEvent, cx| {
			if let SliderEvent::ValueChanged { value, .. } = event {
				let height = value.to_f64() as f32;
				this.engine
					.update(cx, |engine, cx| engine.set_track_height(px(height), cx));
			}
		})
		.detach();

		// Snap toggle → timeline view state.
		cx.subscribe(&snap, |this, _snap, event: &CheckBoxEvent, cx| {
			let CheckBoxEvent::Toggled { state, .. } = event;
			let enabled = *state == CheckState::Checked;
			this.timeline.update(cx, |timeline, cx| {
				timeline.state.snap_enabled = enabled;
				cx.notify();
			});
		})
		.detach();

		// The right-click menu: the view reports what was hit
		// (`ContextMenuRequested`), the panel assembles the matching menu
		// and opens the popup at the click position.
		let context_menu = ContextMenuHandle::new(Self::on_local_menu_item, window, cx);
		cx.subscribe(&timeline, |this, _view, event: &TimelineEvent, cx| {
			if let TimelineEvent::ContextMenuRequested { position, hit } = event {
				this.open_context_menu(*position, hit.clone(), cx);
			}
		})
		.detach();

		Self {
			timeline,
			engine,
			zoom,
			height,
			snap,
			footage_drop: None,
		effect_drop: None,
			context_menu,
			context_track: None,
			context_empty: None,
		}
	}

	/// Opens the context menu matching `hit` at `position` (window
	/// coordinates). Registry-backed items leave through
	/// [`ContextMenuTriggered`]; local items are handled by
	/// [`Self::on_local_menu_item`].
	fn open_context_menu(
		&mut self,
		position: Point<Pixels>,
		hit: TimelineHit,
		cx: &mut Context<Self>,
	) {
		let menu = match &hit {
			TimelineHit::Clip(clip) => {
				// C++ parity: right-clicking an unselected clip selects it
				// first — the context menu acts on the clicked clip, never
				// on a stale or empty selection (this is what made
				// Cut/Delete appear to do nothing).
				if !self.timeline.read(cx).selection().contains(clip) {
					let clip = *clip;
					self.timeline.update(cx, |view, cx| {
						view.state.selection.clear();
						view.state.selection.insert(clip);
						cx.emit(TimelineEvent::SelectionChanged);
						cx.notify();
					});
				}
				let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
				let (sync, proxy, multicam) = {
					let engine = self.engine.read(cx);
					(
						engine.sync_eligibility(&ids),
						engine.clip_footage_entries(&ids),
						MulticamMenuState {
							eligible: engine.multicam_eligible(&ids),
							enabled: engine.multicam_enabled_on_selection(&ids),
						},
					)
				};
				clip_menu(sync, &proxy, Some(multicam))
			}
			TimelineHit::Empty { track, frame } => {
				self.context_empty = Some((*track, *frame));
				empty_area_menu()
			}
			TimelineHit::TrackHead(track) => {
				self.context_track = Some(*track);
				track_head_menu()
			}
			TimelineHit::RulerMarker(_) => marker_menu(),
			TimelineHit::Ruler(_) => ruler_menu(),
		};
		if !matches!(hit, TimelineHit::TrackHead(_)) {
			self.context_track = None;
		}
		if !matches!(hit, TimelineHit::Empty { .. }) {
			self.context_empty = None;
		}
		self.context_menu.show(position, menu, cx);
	}

	/// Handles the timeline's local (non-registry) context-menu items.
	fn on_local_menu_item(&mut self, item: usize, cx: &mut Context<Self>) {
		// Color labels apply to the selected clips; the engine has no
		// clip-color surface yet, so they log for now (kept visible so the
		// wiring is testable in the demo).
		if let Some(color) = menu::color_label_index(item) {
			println!("[timeline] set clip color label to {color}");
			return;
		}
		match item {
			LOCAL_ADD_VIDEO_TRACK => {
				self.engine
					.update(cx, |engine, cx| engine.add_track(TrackKind::Video, cx));
			}
			LOCAL_ADD_AUDIO_TRACK => {
				self.engine
					.update(cx, |engine, cx| engine.add_track(TrackKind::Audio, cx));
			}
			LOCAL_DELETE_TRACK => {
				if let Some(track) = self.context_track {
					self.engine
						.update(cx, |engine, cx| engine.remove_track(track, cx));
				}
			}
			LOCAL_DELETE_ALL_EMPTY => {
				self.engine
					.update(cx, |engine, cx| engine.delete_empty_tracks(cx));
			}
			LOCAL_ADD_ADJUSTMENT_LAYER => {
				if let Some((track, frame)) = self.context_empty {
					if let Err(err) = self
						.engine
						.update(cx, |engine, cx| engine.add_adjustment_layer(track, frame, cx))
					{
						println!("[timeline] add adjustment layer failed: {err}");
					}
				}
			}
			LOCAL_CACHE_ALL | LOCAL_CACHE_IN_OUT | LOCAL_CACHE_DISCARD => {
				println!("[timeline] cache action {item} (not implemented yet)");
			}
			LOCAL_PROXY_GENERATE | LOCAL_PROXY_USE | LOCAL_PROXY_REVEAL | LOCAL_PROXY_DELETE => {
				let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
				let rows = self.engine.read(cx).clip_footage_entries(&ids);
				match item {
					LOCAL_PROXY_GENERATE => {
						for row in rows.into_iter().filter(|row| row.can_generate) {
							if let Err(err) = self
								.engine
								.update(cx, |engine, cx| engine.proxy_generate(row.id, cx))
							{
								println!("[timeline] proxy generate failed: {err}");
							}
						}
					}
					LOCAL_PROXY_USE => {
						// The C++ flips the group: every footage enabled
						// turns the whole selection off, anything less
						// turns it on.
						let enable = !rows.iter().all(|row| row.enabled);
						for row in rows {
							self.engine.update(cx, |engine, cx| {
								engine.proxy_set_enabled(row.id, enable, cx)
							});
						}
					}
					LOCAL_PROXY_REVEAL => {
						for row in rows.into_iter().filter(|row| row.has_proxy) {
							self.engine.read(cx).proxy_reveal(row.id);
						}
					}
					_ => {
						for row in rows.into_iter().filter(|row| row.has_proxy) {
							self.engine
								.update(cx, |engine, cx| engine.proxy_delete(row.id, cx));
						}
					}
				}
			}
			LOCAL_TIMECODE_DROP_FRAME
			| LOCAL_TIMECODE_NON_DROP_FRAME
			| LOCAL_TIMECODE_SECONDS
			| LOCAL_TIMECODE_FRAMES
			| LOCAL_TIMECODE_MILLISECONDS => {
				println!("[timeline] timecode display {item} (not implemented yet)");
			}
			LOCAL_MULTICAM => {
				// The C++ `multicam_enabled_triggered` flip: checked clips
				// disable, unchecked ones enable.
				let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
				let enable = !self.engine.read(cx).multicam_enabled_on_selection(&ids);
				self.engine.update(cx, |engine, cx| {
					engine.multicam_enable_selected(ids, enable, cx)
				});
			}
			_ => {
				println!("[timeline] unhandled local menu item {item}");
			}
		}
	}

	/// Resolves a drag pointer (relative to the timeline body) into a
	/// display track + start frame using the timeline view's zoom/scroll
	/// state and the engine's track heights — the same affine mapping the
	/// timeline itself uses (see [`TimelineState::frame_at_point`] and the
	/// view's track-row walk). `None` above the ruler (outside the clip
	/// area).
	fn resolve_drop_point(
		&self,
		now: Point<Pixels>,
		cx: &App,
	) -> Option<(TrackKind, usize, Frame)> {
		// The clip area starts below the ruler and right of the track
		// headers column.
		if f32::from(now.y) < RULER_HEIGHT {
			return None;
		}
		let clip_x = f32::from(now.x - px(HEADER_WIDTH)).max(0.0);
		let clip_y = now.y - px(RULER_HEIGHT);
		let state = self.timeline.read(cx).state.clone();
		// No upper clamp: dropping past the sequence end is how the
		// timeline extends (the old clamp to the sequence length squashed
		// every drop onto an empty/near-empty timeline to frame zero).
		let time = state.frame_at_point(px(clip_x)).max(Frame::ZERO);
		// Walk the display rows top-down, clamping each to the minimum row
		// height exactly like the timeline's own `track_at_y`.
		let (track_kind, track_index) = {
			let engine = self.engine.read(cx);
			let mut acc = 0.0f32;
			let mut found = None;
			for index in 0..engine.track_count() {
				if let Some(track) = engine.track(index) {
					acc += f32::from(track.height()).max(MIN_TRACK_HEIGHT);
					if f32::from(clip_y) < acc {
						found = Some((track.kind(), index));
						break;
					}
				}
			}
			found.unwrap_or_else(|| {
				let last = engine.track_count().saturating_sub(1);
				engine
					.track(last)
					.map(|t| (t.kind(), last))
					.unwrap_or((TrackKind::Video, 0))
			})
		};
		Some((track_kind, track_index, time))
	}

	/// Resolves the footage-drop target under the cursor (see
	/// [`Self::resolve_drop_point`]). Hovering outside the clip area
	/// clears the target.
	fn update_footage_drop(&mut self, event: &DragMoveEvent<FootageDrag>, cx: &mut Context<Self>) {
		let now = event.event.position - event.bounds.origin;
		let Some((track_kind, track_index, time)) = self.resolve_drop_point(now, cx) else {
			if self.footage_drop.take().is_some() {
				cx.notify();
			}
			return;
		};
		self.footage_drop = Some(FootageDropTarget {
			track_kind,
			track_index,
			time,
			length: event
				.dragged_item()
				.downcast_ref::<FootageDrag>()
				.and_then(|drag| self.engine.read(cx).footage_length_frames(drag.0))
				.unwrap_or(1),
		});
		cx.notify();
	}

	/// Resolves the effect-library drop target under the cursor (same
	/// mapping as the footage drop; a generator clip's 5-second default
	/// length in sequence frames).
	fn update_effect_drop(
		&mut self,
		event: &DragMoveEvent<gpui::effect_stack::LibraryEffectDrag>,
		cx: &mut Context<Self>,
	) {
		let now = event.event.position - event.bounds.origin;
		let Some((track_kind, track_index, time)) = self.resolve_drop_point(now, cx) else {
			if self.effect_drop.take().is_some() {
				cx.notify();
			}
			return;
		};
		let length = {
			let engine = self.engine.read(cx);
			let fps = engine.frame_rate();
			((5.0 * fps.num as f64 / fps.den.max(1) as f64).round() as i64).max(1)
		};
		self.effect_drop = Some(FootageDropTarget {
			track_kind,
			track_index,
			time,
			length,
		});
		cx.notify();
	}

	/// Applies a finished effect-library drop: generator-category effects
	/// become a standalone generator clip at the hovered track + frame
	/// (undoable, one row); transition effects snap to the nearest clip
	/// edge as a junction or single-sided transition. Everything else is
	/// ignored.
	fn finish_effect_drop(
		&mut self,
		drag: &gpui::effect_stack::LibraryEffectDrag,
		cx: &mut Context<Self>,
	) {
		let Some(target) = self.effect_drop.take() else {
			return;
		};
		let type_id = drag.type_id.as_str();
		let engine = self.engine.clone();
		if matches!(
			type_id,
			"org.olivevideoeditor.Olive.transition" | "org.olivevideoeditor.Olive.transitionfx"
		) {
			engine.update(cx, |engine, cx| {
				if let Err(err) =
					engine.drop_transition_at(type_id, target.track_index, target.time, cx)
				{
					println!("[timeline] transition drop: {err}");
				}
			});
			cx.notify();
			return;
		}
		let is_generator = oak_node::factory::Factory::global()
			.create_any(type_id)
			.map(|(_, behavior)| {
				behavior
					.categories()
					.contains(&oak_node::node::Category::Generator)
			})
			.unwrap_or(false);
		if !is_generator {
			println!("[timeline] effect drop: \"{type_id}\" is not a generator");
			return;
		}
		engine.update(cx, |engine, cx| {
			if let Err(err) =
				engine.drop_generator_clip(type_id, target.track_index, target.time, cx)
			{
				println!("[timeline] generator clip drop failed: {err}");
			}
		});
		cx.notify();
	}

	/// Applies a finished footage drop: routes the payload's footage id with
	/// the last hovered track + frame to the engine, which resolves the
	/// footage, validates the track and places the clip (undoable).
	///
	/// A drop onto a timeline with no open sequence cannot place a clip:
	/// the panel hands the pending drop back to the shell through
	/// [`FootageDropNeedsSequence`], which shows the probe-vs-manual
	/// choice (the shell re-routes the drop through [`AppEngine::drop_footage`]
	/// once a sequence exists).
	fn finish_footage_drop(&mut self, drag: &FootageDrag, cx: &mut Context<Self>) {
		let Some(target) = self.footage_drop.take() else {
			return;
		};
		let FootageDropTarget {
			track_kind,
			track_index,
			time,
			..
		} = target;
		if self.engine.read(cx).current_sequence().is_none() {
			cx.emit(FootageDropNeedsSequence {
				footage_id: drag.0,
				track_kind,
				track_index,
				time,
			});
			cx.notify();
			return;
		}
		self.engine.update(cx, |engine, cx| {
			engine.drop_footage(drag.0, track_kind, track_index, time, cx);
		});
		cx.notify();
	}

	/// Routes a transport command to the engine's program monitor (the
	/// timeline shuttles the program, like the viewers do when focused).
	fn transport(&mut self, action: ActionId, cx: &mut Context<Self>) -> bool {
		let engine = self.engine.clone();
		let clock = self.engine.read(cx).program_clock().clone();
		panel_commands::viewer_transport(&engine, &clock, Monitor::Program, action, cx)
	}

	/// Deletes the selected clips (ripple or gap) through the engine's edit
	/// commands (the focused-panel counterpart of the shell's Edit menu).
	fn delete_selection(&mut self, ripple: bool, cx: &mut Context<Self>) {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		if ids.is_empty() {
			println!("[timeline] delete: nothing selected");
			return;
		}
		for id in ids {
			self.engine
				.update(cx, |engine, cx| engine.delete_clip(id, ripple, cx));
		}
	}

	/// Moves the work area's start (`in_point`) or end to the program
	/// playhead as ONE undoable entry — the same commit the shell's
	/// playback-menu in/out points use.
	fn set_point_at_playhead(&mut self, in_point: bool, cx: &mut Context<Self>) {
		let clock = self.engine.read(cx).program_clock().clone();
		let playhead = clock.read(cx).current_frame();
		let seq_len = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|s| s.length)
			.unwrap_or(Frame(playhead.0 + 1));
		let (old_start, old_end) = self
			.engine
			.read(cx)
			.workarea()
			.unwrap_or((Frame::ZERO, seq_len));
		let (start, end) = if in_point {
			(playhead, old_end.max(Frame(playhead.0 + 1)))
		} else {
			(old_start.min(Frame((playhead.0 - 1).max(0))), playhead)
		};
		if end.0 <= start.0 {
			println!("[timeline] set in/out point: empty range, ignored");
			return;
		}
		self.engine.update(cx, |engine, cx| {
			engine.commit_workarea(old_start, old_end, start, end, cx);
		});
	}

	/// Scales the timeline zoom around its left edge (the focused-panel
	/// counterpart of 视图 → 放大/缩小).
	fn zoom_timeline(&mut self, factor: f32, cx: &mut Context<Self>) {
		self.timeline.update(cx, |view, cx| {
			let zoom = view.state.zoom * factor;
			view.state.set_zoom(zoom, px(0.));
			cx.notify();
		});
	}

	/// Steps every track's height by `delta` pixels, clamped to the
	/// track-height slider's range (24–160px).
	fn nudge_track_height(&mut self, delta: f32, cx: &mut Context<Self>) {
		let current = self
			.engine
			.read(cx)
			.track(0)
			.map(|track| f32::from(track.height()))
			.unwrap_or(64.0);
		let next = (current + delta).clamp(24.0, 160.0);
		self.engine
			.update(cx, |engine, cx| engine.set_track_height(px(next), cx));
	}
}

impl<E: AppEngine> PanelCommandHandler for TimelinePanel<E> {
	// --- transport (the program monitor) ---
	fn play_pause(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::PlayPause, cx)
	}
	fn prev_frame(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::PrevFrame, cx)
	}
	fn next_frame(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::NextFrame, cx)
	}
	fn go_to_start(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::GoToStart, cx)
	}
	fn go_to_end(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::GoToEnd, cx)
	}
	fn play_in_to_out(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::PlayInToOut, cx)
	}
	fn go_to_in(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::GoToIn, cx)
	}
	fn go_to_out(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::GoToOut, cx)
	}
	fn shuttle_left(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::ShuttleLeft, cx)
	}
	fn shuttle_stop(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::ShuttleStop, cx)
	}
	fn shuttle_right(&mut self, cx: &mut Context<Self>) -> bool {
		self.transport(ActionId::ShuttleRight, cx)
	}

	// --- in / out points (the work area) ---
	fn set_in(&mut self, cx: &mut Context<Self>) -> bool {
		self.set_point_at_playhead(true, cx);
		true
	}
	fn set_out(&mut self, cx: &mut Context<Self>) -> bool {
		self.set_point_at_playhead(false, cx);
		true
	}
	fn reset_in(&mut self, cx: &mut Context<Self>) -> bool {
		// Reset the in point to the sequence start, keeping the out point.
		let seq_len = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|s| s.length)
			.unwrap_or(Frame(1));
		let (_old_start, old_end) = self
			.engine
			.read(cx)
			.workarea()
			.unwrap_or((Frame::ZERO, seq_len));
		let end = old_end.max(Frame(1));
		self.engine.update(cx, |engine, cx| {
			engine.commit_workarea(_old_start, old_end, Frame::ZERO, end, cx);
		});
		true
	}
	fn reset_out(&mut self, cx: &mut Context<Self>) -> bool {
		// Reset the out point to the sequence end, keeping the in point.
		let seq_len = self
			.engine
			.read(cx)
			.current_sequence()
			.map(|s| s.length)
			.unwrap_or(Frame(1));
		let (old_start, _old_end) = self
			.engine
			.read(cx)
			.workarea()
			.unwrap_or((Frame::ZERO, seq_len));
		let end = seq_len.max(Frame(old_start.0 + 1));
		self.engine.update(cx, |engine, cx| {
			engine.commit_workarea(old_start, _old_end, old_start, end, cx);
		});
		true
	}
	fn clear_in_out(&mut self, cx: &mut Context<Self>) -> bool {
		self.engine
			.update(cx, |engine, cx| engine.clear_workarea(cx));
		true
	}

	// --- selection ---
	fn select_all(&mut self, cx: &mut Context<Self>) -> bool {
		let ids: Vec<ClipId> = {
			let engine = self.engine.read(cx);
			let mut ids = Vec::new();
			for index in 0..engine.track_count() {
				if let Some(track) = engine.track(index) {
					ids.extend(track.clips().iter().map(|clip| clip.id()));
				}
			}
			ids
		};
		self.timeline.update(cx, |view, cx| {
			view.state.select_range(ids.iter().copied());
			cx.notify();
		});
		self.engine
			.update(cx, |engine, cx| engine.set_selected_clips(ids, cx));
		true
	}
	fn deselect_all(&mut self, cx: &mut Context<Self>) -> bool {
		self.timeline.update(cx, |view, cx| {
			view.state.select_range(std::iter::empty::<ClipId>());
			cx.notify();
		});
		self.engine
			.update(cx, |engine, cx| engine.set_selected_clips(Vec::new(), cx));
		true
	}

	// --- editing ---
	fn cut_selected(&mut self, cx: &mut Context<Self>) -> bool {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		self.engine
			.update(cx, |engine, cx| engine.clipboard_cut(ids, cx));
		true
	}
	fn copy_selected(&mut self, cx: &mut Context<Self>) -> bool {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		self.engine
			.update(cx, |engine, cx| engine.clipboard_copy(ids, cx));
		true
	}
	fn paste(&mut self, cx: &mut Context<Self>) -> bool {
		self.engine
			.update(cx, |engine, cx| engine.clipboard_paste(cx));
		true
	}
	fn delete_selected(&mut self, cx: &mut Context<Self>) -> bool {
		self.delete_selection(false, cx);
		true
	}
	fn ripple_delete(&mut self, cx: &mut Context<Self>) -> bool {
		self.delete_selection(true, cx);
		true
	}
	fn split_at_playhead(&mut self, cx: &mut Context<Self>) -> bool {
		self.engine
			.update(cx, |engine, cx| engine.split_at_playhead(cx));
		true
	}
	/// 编辑 → 设为默认转场 / Ctrl+Shift+D: adds a transition of the
	/// configured default length at every seam around the selected clips.
	fn default_transition(&mut self, cx: &mut Context<Self>) -> bool {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		let result = self
			.engine
			.update(cx, |engine, cx| engine.add_default_transition(ids, cx));
		if let Err(error) = result {
			println!("[timeline] add default transition failed: {error}");
		}
		true
	}
	fn set_marker(&mut self, cx: &mut Context<Self>) -> bool {
		self.engine
			.update(cx, |engine, cx| engine.add_marker_at_playhead(cx));
		true
	}

	// --- synchronization ---
	fn sync_by_source_time(&mut self, cx: &mut Context<Self>) -> bool {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		self.engine
			.update(cx, |engine, cx| engine.sync_clips_by_source_time(ids, cx));
		true
	}
	fn sync_by_waveform(&mut self, cx: &mut Context<Self>) -> bool {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		self.engine.update(cx, |engine, cx| {
			engine.sync_clips_by_waveform(ids, false, cx)
		});
		true
	}
	fn sync_by_waveform_speed(&mut self, cx: &mut Context<Self>) -> bool {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		self.engine.update(cx, |engine, cx| {
			engine.sync_clips_by_waveform(ids, true, cx)
		});
		true
	}
	/// 编辑 → 链接/重新链接: toggles the graph links among the selected
	/// clips through the engine (one undoable entry).
	fn toggle_links(&mut self, cx: &mut Context<Self>) -> bool {
		let ids: Vec<ClipId> = self.timeline.read(cx).selection().iter().copied().collect();
		self.engine
			.update(cx, |engine, cx| engine.toggle_clip_links(ids, cx));
		true
	}

	// --- view ---
	fn zoom_in(&mut self, cx: &mut Context<Self>) -> bool {
		self.zoom_timeline(1.25, cx);
		true
	}
	fn zoom_out(&mut self, cx: &mut Context<Self>) -> bool {
		self.zoom_timeline(0.8, cx);
		true
	}
	fn increase_track_height(&mut self, cx: &mut Context<Self>) -> bool {
		self.nudge_track_height(8.0, cx);
		true
	}
	fn decrease_track_height(&mut self, cx: &mut Context<Self>) -> bool {
		self.nudge_track_height(-8.0, cx);
		true
	}
}

impl<E: AppEngine> Render for TimelinePanel<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();

		// --- toolbar row (fixed 31px, above the ruler) --------------------
		let mut toolbar = div()
			.debug_selector(|| "timeline-toolbar".into())
			.h(px(TOOLBAR_HEIGHT))
			.flex_shrink_0()
			.flex()
			.items_center()
			.gap_2()
			.px_2()
			.overflow_hidden()
			.border_b_1()
			.border_color(colors.border)
			.bg(colors.container);

		// A tool button: a 16px icon on a 24px hit target with a localized
		// tooltip; the selected tool is highlighted (driven by the widget's
		// current tool, so the toolbar and the Tools menu stay in sync).
		let tool_button =
			|index: usize, icon_name: &'static str, key: &'static str, cx: &mut Context<Self>| {
				let tool = i18n::tr(key);
				let selected = self.timeline.read(cx).tool().index() == index;
				let background = if selected {
					colors.selected
				} else {
					colors.background
				};
				let hover_bg = colors.selected;
				let path = icons::icon_path(icon_name, cx);
				div()
					.id(SharedString::from(format!("tool-{index}")))
					.size(px(24.0))
					.flex()
					.items_center()
					.justify_center()
					.rounded_sm()
					.cursor_pointer()
					.bg(background)
					.hover(move |style| style.bg(hover_bg))
					.tooltip(move |window, cx| tooltip_view(tool.into(), window, cx))
					.on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
						if let Some(tt) = TimelineTool::from_index(index) {
							this.timeline.update(cx, |view, cx| view.set_tool(tt, cx));
						}
					}))
					.child(img(path).size(px(16.0)))
			};

		for (index, (tool_key, icon_name)) in TOOLS.iter().enumerate() {
			toolbar = toolbar.child(tool_button(index, icon_name, tool_key, cx));
		}

		// Add-track buttons (the convenient way to create tracks; the track
		// header context menu offers the same two entries).
		let add_track_btn =
			|id: &'static str, key: &'static str, kind: TrackKind, cx: &mut Context<Self>| {
				let label = i18n::tr(key);
				let hover_bg = colors.selected;
				div()
					.id(id)
					.h(px(24.0))
					.px_2()
					.flex()
					.items_center()
					.rounded_sm()
					.cursor_pointer()
					.text_color(colors.text)
					.text_xs()
					.hover(move |style| style.bg(hover_bg))
					.tooltip(move |window, cx| tooltip_view(label.into(), window, cx))
					.on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
						this.engine
							.update(cx, |engine, cx| engine.add_track(kind, cx));
					}))
					.child(label)
			};
		toolbar = toolbar
			.child(add_track_btn(
				"toolbar-add-video-track",
				"timeline.add_video_track",
				TrackKind::Video,
				cx,
			))
			.child(add_track_btn(
				"toolbar-add-audio-track",
				"timeline.add_audio_track",
				TrackKind::Audio,
				cx,
			));

		// A plain icon button (no selection state), e.g. zoom in/out. The
		// zoom buttons scale around the playhead (the design's fixed anchor).
		let icon_btn = |id: &'static str,
		                icon_name: &'static str,
		                key: &'static str,
		                factor: f32,
		                cx: &mut Context<Self>| {
			let label = i18n::tr(key);
			let hover_bg = colors.container;
			let path = icons::icon_path(icon_name, cx);
			div()
				.id(id)
				.size(px(24.0))
				.flex()
				.items_center()
				.justify_center()
				.rounded_sm()
				.cursor_pointer()
				.text_color(colors.text)
				.hover(move |style| style.bg(hover_bg))
				.tooltip(move |window, cx| tooltip_view(label.into(), window, cx))
				.on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
					this.timeline.update(cx, |view, cx| {
						let anchor = view.state.point_at_frame(view.state.playhead);
						let zoom = view.state.zoom * factor;
						view.state.set_zoom(zoom, anchor);
						cx.emit(TimelineEvent::ZoomChanged(zoom));
						cx.notify();
					});
				}))
				.child(img(path).size(px(16.0)))
		};

		// The snap toggle: the magnet icon next to the checkbox box. The icon
		// is a real toggle too — clicking it flips snap state and mirrors it
		// into the checkbox, keeping the two in sync.
		let snap_row = div()
			.flex()
			.items_center()
			.gap_1()
			.text_color(colors.text)
			.child(
				div()
					.id("snap-toggle")
					.size(px(24.0))
					.flex()
					.items_center()
					.justify_center()
					.cursor_pointer()
					.tooltip(move |window, cx| {
						tooltip_view(i18n::tr("timeline.snap").into(), window, cx)
					})
					.on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
						let enabled = !this.timeline.read(cx).state.snap_enabled;
						this.timeline.update(cx, |timeline, cx| {
							timeline.state.snap_enabled = enabled;
							cx.notify();
						});
						this.snap.update(cx, |snap, cx| {
							snap.set_state(
								if enabled {
									CheckState::Checked
								} else {
									CheckState::Unchecked
								},
								cx,
							);
						});
					}))
					.child(img(icons::icon_path(icons::ICON_SNAP, cx)).size(px(16.0))),
			)
			.child(self.snap.clone());

		let toolbar = toolbar
			.child(icon_btn(
				"toolbar-zoom-in",
				icons::ICON_ZOOM_IN,
				"timeline.zoom_in",
				1.25,
				cx,
			))
			.child(icon_btn(
				"toolbar-zoom-out",
				icons::ICON_ZOOM_OUT,
				"timeline.zoom_out",
				0.8,
				cx,
			))
			.child(
				div()
					.w_1()
					.h_full()
					.border_l_1()
					.border_color(colors.border),
			)
			.child(snap_row);

		// --- trailing controls slot (fixed width, right of the body) -------
		let right_controls = div()
			.debug_selector(|| "timeline-right-controls".into())
			.w(px(RIGHT_CONTROLS_WIDTH))
			.flex_shrink_0()
			.flex()
			.flex_col()
			.justify_center()
			.gap_1()
			.px_2()
			.border_l_1()
			.border_color(colors.border)
			.bg(colors.container)
			.child(
				div()
					.flex()
					.flex_col()
					.gap_1()
					.text_xs()
					.text_color(colors.disabled)
					.child(i18n::tr("timeline.zoom"))
					.child(self.zoom.clone()),
			)
			.child(
				div()
					.flex()
					.flex_col()
					.gap_1()
					.text_xs()
					.text_color(colors.disabled)
					.child(i18n::tr("timeline.track_height"))
					.child(self.height.clone()),
			);

		div()
			.size_full()
			.flex()
			.flex_col()
			.overflow_hidden()
			// Any click inside the panel makes it the focused panel (the
			// dock re-emits this as `DockEvent::PanelFocused`, which the
			// shell uses to route focused-panel commands).
			.on_mouse_down(MouseButton::Left, {
				cx.listener(|_this, _event: &gpui::MouseDownEvent, _window, cx| {
					cx.emit(PanelEvent::Focused);
				})
			})
			.child(toolbar)
			.child(
				div()
					.debug_selector(|| "timeline-body".into())
					.flex_1()
					.min_h_0()
					.flex()
					.flex_row()
					.child(
						div()
							.debug_selector(|| "timeline-canvas".into())
							.flex_1()
							.min_w_0()
							// Footage drop target: hover resolves the track +
							// frame (see [`TimelinePanel::update_footage_drop`]),
							// the release routes the payload to the engine.
							.on_drag_move(cx.listener(
								|this, event: &DragMoveEvent<FootageDrag>, _window, cx| {
									this.update_footage_drop(event, cx);
								},
							))
							.on_drop(cx.listener(|this, drag: &FootageDrag, _window, cx| {
								if std::env::var("OAK_DEBUG_DRAG").is_ok() {
									eprintln!("[drag] timeline drop: {drag:?}");
								}
								this.finish_footage_drop(drag, cx);
							}))
							// Effect-library drop target (generator effects →
							// standalone generator clips).
							.on_drag_move(cx.listener(
								|this,
								 event: &DragMoveEvent<gpui::effect_stack::LibraryEffectDrag>,
								 _window,
								 cx| {
									this.update_effect_drop(event, cx);
								},
							))
							.on_drop(cx.listener(
								|this,
								 drag: &gpui::effect_stack::LibraryEffectDrag,
								 _window,
								 cx| {
									this.finish_effect_drop(drag, cx);
								},
							))
							.child({
								let mut inner =
									div().relative().size_full().child(self.timeline.clone());
								// The drop ghost: a translucent block at the
								// resolved track + frame, spanning the footage's
								// length, so the user sees where the clip lands.
								if let Some(target) = self.footage_drop.as_ref().or(self.effect_drop.as_ref()) {
									let state = self.timeline.read(cx).state.clone();
									let x = px(HEADER_WIDTH) + state.point_at_frame(target.time);
									let width = px(target.length as f32 * state.zoom).max(px(4.0));
									let engine = self.engine.read(cx);
									let mut y = px(RULER_HEIGHT);
									let mut row_h = px(64.0);
									for index in 0..=target.track_index {
										let Some(track) = engine.track(index) else {
											break;
										};
										let h = track.height().max(px(MIN_TRACK_HEIGHT));
										if index == target.track_index {
											row_h = h;
											break;
										}
										y += h;
									}
									inner = inner.child(
										div()
											.absolute()
											.left(x)
											.top(y)
											.w(width)
											.h(row_h)
											.rounded_sm()
											.border_1()
											.border_color(colors.selected)
											.bg(gpui::Rgba {
												a: 0.35,
												..colors.selected
											})
											.into_any_element(),
									);
								}
								inner
							}),
					)
					.child(right_controls),
			)
			// The right-click popup renders anchored above the panel.
			.child(self.context_menu.widget())
	}
}

impl<E: AppEngine> EventEmitter<PanelEvent> for TimelinePanel<E> {}

impl<E: AppEngine> EventEmitter<ContextMenuTriggered> for TimelinePanel<E> {}

/// A footage drop landed onto the timeline while no sequence is open: the
/// shell must set up a sequence first (probe the footage's params as the
/// sequence's, or open the manual setup dialog) before the clip can be
/// placed. Re-issued by the shell through
/// [`AppEngine::drop_footage`](crate::oakui::AppEngine::drop_footage) once
/// the sequence exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FootageDropNeedsSequence {
	/// The footage's project-explorer entry id (the drag payload).
	pub footage_id: u64,
	/// The pointed display track kind.
	pub track_kind: TrackKind,
	/// The pointed display track index.
	pub track_index: usize,
	/// The start frame at the pointer.
	pub time: Frame,
}

impl<E: AppEngine> EventEmitter<FootageDropNeedsSequence> for TimelinePanel<E> {}

impl<E: AppEngine> DockPanel for TimelinePanel<E> {
	fn panel_id(&self) -> gpui::dock::PanelId {
		TIMELINE
	}

	fn title(&self, _cx: &App) -> SharedString {
		i18n::tr("panel.timeline").into()
	}

	fn tab_content(&self, _cx: &App) -> AnyElement {
		div().child(i18n::tr("panel.timeline")).into_any_element()
	}
}

// ---------------------------------------------------------------------------
// Context menus — the Rust counterpart of the C++
// `TimelineWidget::show_context_menu` (clip + empty area), the
// `TrackViewItem` track-head menu and the `TimeRuler` /
// `SeekableWidget` ruler menus.
// ---------------------------------------------------------------------------

/// Local (non-registry) item ids of the timeline's context menus.
const LOCAL_USE_AUDIO_TIME_UNITS: usize = 2101;
const LOCAL_SHOW_WAVEFORMS: usize = 2102;
const LOCAL_THUMBNAIL_OFF: usize = 2103;
const LOCAL_THUMBNAIL_IN_OUT: usize = 2104;
const LOCAL_THUMBNAIL_ON: usize = 2105;
const LOCAL_CACHE_AUTO: usize = 2109;
const LOCAL_CACHE_ALL: usize = 2110;
const LOCAL_CACHE_IN_OUT: usize = 2111;
const LOCAL_CACHE_DISCARD: usize = 2112;
const LOCAL_PROXY_GENERATE: usize = 2113;
const LOCAL_PROXY_USE: usize = 2114;
const LOCAL_PROXY_REVEAL: usize = 2115;
const LOCAL_PROXY_DELETE: usize = 2116;
const LOCAL_REVEAL_FOOTAGE_VIEWER: usize = 2117;
const LOCAL_REVEAL_PROJECT: usize = 2118;
const LOCAL_MULTICAM: usize = 2119;
const LOCAL_DELETE_TRACK: usize = 2120;
const LOCAL_DELETE_ALL_EMPTY: usize = 2121;
const LOCAL_MARKER_PROPERTIES: usize = 2122;
const LOCAL_TIMECODE_DROP_FRAME: usize = 2123;
const LOCAL_TIMECODE_NON_DROP_FRAME: usize = 2124;
const LOCAL_TIMECODE_SECONDS: usize = 2125;
const LOCAL_TIMECODE_FRAMES: usize = 2126;
const LOCAL_TIMECODE_MILLISECONDS: usize = 2127;
const LOCAL_ADD_VIDEO_TRACK: usize = 2130;
const LOCAL_ADD_AUDIO_TRACK: usize = 2131;
const LOCAL_ADD_ADJUSTMENT_LAYER: usize = 2132;

/// A registry-backed item shown under a "Properties" label (the C++ clip
/// and sequence "Properties" entries open the Speed/Duration and Sequence
/// dialogs respectively, so the item keeps the registry id — and with it
/// the shared dispatch path — while wearing the dialog's menu label).
fn properties_item(action: ActionId) -> MenuItem {
	let entry = action.entry();
	let mut item = MenuItem::new(entry.menu_id(), i18n::tr("menu.context.properties"));
	if let Some(shortcut) = crate::actions::display_shortcut(action) {
		item = item.with_shortcut(shortcut);
	}
	item
}

/// The multicam menu state of the selected clips (the C++ conditions in
/// `timelinewidget.cpp::show_context_menu`: the Multi-Cam item enables when
/// any selected clip's connected viewer is a sequence, and is checked when
/// that clip's texture chain contains a multicam node).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MulticamMenuState {
	/// Whether any selected clip can host multicam (its connected viewer is
	/// a sequence).
	pub eligible: bool,
	/// Whether the selected clips are currently multicam-enabled.
	pub enabled: bool,
}

/// The clip context menu (`TimelineWidget::show_context_menu` with a
/// selection): the shared clip-edit section, color labels, the synchronize
/// / cache / proxy groups, reveal entries and "Properties". `sync` and
/// `proxy` carry the selection-derived enable state (the C++ enables the
/// synchronize entries at ≥ 2 eligible clips and the proxy entries per
/// the selected footage's proxy fields); `multicam` carries the Multi-Cam
/// item's enable/checked state.
pub(crate) fn clip_menu(
	sync: crate::oakui::engine::SyncEligibility,
	proxy: &[crate::oakui::engine::ProxyFootageRow],
	multicam: Option<MulticamMenuState>,
) -> Menu {
	let mut items = menu::edit_section(true);
	// The C++ puts a separator between the edit section and the color
	// labels, and another after them.
	if let Some(last) = items.last_mut() {
		last.separator_after = true;
	}
	items.push(menu::color_label_item(None).separated());
	// Synchronize group (registry actions; enabled at ≥ 2 eligible clips,
	// the C++ `get_selected_source_sync_clips` / `_waveform_sync_clips`
	// counts).
	let sync_enabled = sync.source_time >= 2;
	let wave_enabled = sync.waveform >= 2;
	let mut source_time = menu::action_item(ActionId::SyncBySourceTime);
	if !sync_enabled {
		source_time = source_time.disabled();
	}
	items.push(source_time);
	let mut waveform = menu::action_item(ActionId::SyncByWaveform);
	if !wave_enabled {
		waveform = waveform.disabled();
	}
	items.push(waveform);
	let mut waveform_speed = menu::action_item(ActionId::SyncByWaveformSpeed).separated();
	if !wave_enabled {
		waveform_speed = waveform_speed.disabled();
	}
	items.push(waveform_speed);
	// Cache group (placeholders: the engine has no cache surface yet).
	let cache_menu = Menu::new(vec![
		MenuItem::new(LOCAL_CACHE_AUTO, i18n::tr("timeline.context.auto_cache"))
			.with_checked(false)
			.separated(),
		MenuItem::new(LOCAL_CACHE_ALL, i18n::tr("timeline.context.cache_all")),
		MenuItem::new(
			LOCAL_CACHE_IN_OUT,
			i18n::tr("timeline.context.cache_in_out"),
		),
		MenuItem::new(
			LOCAL_CACHE_DISCARD,
			i18n::tr("timeline.context.cache_discard"),
		),
	]);
	items.push(MenuItem::new(0, i18n::tr("timeline.context.cache")).with_submenu(cache_menu));
	// Proxy group: the enable state mirrors the C++
	// `get_selected_proxy_footage` conditions over the selected clips'
	// footage; the settings entry is the real registry action.
	let has_footage = !proxy.is_empty();
	let can_generate = proxy.iter().any(|row| row.can_generate);
	let any_proxy = proxy.iter().any(|row| row.has_proxy);
	let all_enabled = has_footage && proxy.iter().all(|row| row.enabled);
	let mut generate = MenuItem::new(
		LOCAL_PROXY_GENERATE,
		i18n::tr("timeline.context.generate_proxy"),
	);
	if !can_generate {
		generate = generate.disabled();
	}
	let mut use_proxy = MenuItem::new(LOCAL_PROXY_USE, i18n::tr("timeline.context.use_proxy"))
		.with_checked(all_enabled);
	if !has_footage {
		use_proxy = use_proxy.disabled();
	}
	let mut reveal = MenuItem::new(
		LOCAL_PROXY_REVEAL,
		i18n::tr("timeline.context.reveal_proxy"),
	);
	if !any_proxy {
		reveal = reveal.disabled();
	}
	let mut delete = MenuItem::new(
		LOCAL_PROXY_DELETE,
		i18n::tr("timeline.context.delete_proxy"),
	);
	if !any_proxy {
		delete = delete.disabled();
	}
	let proxy_menu = Menu::new(vec![
		generate,
		use_proxy,
		reveal,
		delete,
		menu::action_item(ActionId::ProxySettings).separated(),
	]);
	items.push(MenuItem::new(0, i18n::tr("timeline.context.proxy")).with_submenu(proxy_menu));
	// Reveal / multi-cam entries (the C++ shows them only when the clip is
	// connected to a viewer; the reveal entries stay disabled — the Rust
	// app has no footage-reveal surface yet).
	items.push(
		MenuItem::new(
			LOCAL_REVEAL_FOOTAGE_VIEWER,
			i18n::tr("timeline.context.reveal_in_footage_viewer"),
		)
		.disabled(),
	);
	items.push(
		MenuItem::new(
			LOCAL_REVEAL_PROJECT,
			i18n::tr("timeline.context.reveal_in_project"),
		)
		.disabled(),
	);
	// Multi-Cam (checkable): enabled when any selected clip's connected
	// viewer is a sequence, checked when that clip already has a multicam —
	// the C++ `connected_viewer()` + `find_ways_node_arrives_here` checks.
	let multicam = multicam.unwrap_or_default();
	let mut multicam_item = MenuItem::new(LOCAL_MULTICAM, i18n::tr("timeline.context.multicam"))
		.with_checked(multicam.enabled);
	if !multicam.eligible {
		multicam_item = multicam_item.disabled();
	}
	items.push(multicam_item.separated());
	items.push(properties_item(ActionId::SpeedDuration));
	Menu::new(items)
}

/// The empty-area context menu (no clips selected): view toggles plus the
/// sequence "Properties" entry.
pub(crate) fn empty_area_menu() -> Menu {
	let thumbnails = Menu::new(vec![
		MenuItem::new(
			LOCAL_THUMBNAIL_OFF,
			i18n::tr("timeline.context.thumbnails_off"),
		)
		.with_checked(false),
		MenuItem::new(
			LOCAL_THUMBNAIL_IN_OUT,
			i18n::tr("timeline.context.thumbnails_at_in_points"),
		)
		.with_checked(false),
		MenuItem::new(
			LOCAL_THUMBNAIL_ON,
			i18n::tr("timeline.context.thumbnails_on"),
		)
		.with_checked(false),
	]);
	Menu::new(vec![
		MenuItem::new(
			LOCAL_USE_AUDIO_TIME_UNITS,
			i18n::tr("timeline.context.use_audio_time_units"),
		)
		.with_checked(false),
		MenuItem::new(0, i18n::tr("timeline.context.show_thumbnails")).with_submenu(thumbnails),
		MenuItem::new(
			LOCAL_SHOW_WAVEFORMS,
			i18n::tr("timeline.context.show_waveforms"),
		)
		.with_checked(false),
		MenuItem::new(
			LOCAL_ADD_ADJUSTMENT_LAYER,
			i18n::tr("timeline.context.add_adjustment_layer"),
		)
		.separated(),
		properties_item(ActionId::SequenceSettings),
	])
}

/// The track-header context menu (`TrackViewItem`): delete this track, or
/// every empty track.
pub(crate) fn track_head_menu() -> Menu {
	Menu::new(vec![
		MenuItem::new(
			LOCAL_ADD_VIDEO_TRACK,
			i18n::tr("timeline.context.add_video_track"),
		),
		MenuItem::new(
			LOCAL_ADD_AUDIO_TRACK,
			i18n::tr("timeline.context.add_audio_track"),
		),
		MenuItem::new(
			LOCAL_DELETE_TRACK,
			i18n::tr("timeline.context.delete_track"),
		)
		.separated(),
		MenuItem::new(
			LOCAL_DELETE_ALL_EMPTY,
			i18n::tr("timeline.context.delete_all_empty"),
		),
	])
}

/// The marker context menu (`SeekableWidget`): color labels, the plain
/// edit section and marker properties.
pub(crate) fn marker_menu() -> Menu {
	let mut items = vec![menu::color_label_item(None).separated()];
	let mut edit_items = menu::edit_section(false);
	// Separator before the trailing "Properties" entry (the C++ layout).
	if let Some(last) = edit_items.last_mut() {
		last.separator_after = true;
	}
	items.extend(edit_items);
	items.push(MenuItem::new(
		LOCAL_MARKER_PROPERTIES,
		i18n::tr("menu.context.properties"),
	));
	Menu::new(items)
}

/// The ruler context menu (`TimeRuler`): the timecode-display radio group.
pub(crate) fn ruler_menu() -> Menu {
	Menu::new(vec![
		MenuItem::new(
			LOCAL_TIMECODE_DROP_FRAME,
			i18n::tr("timeline.context.timecode_drop_frame"),
		)
		.with_checked(false),
		MenuItem::new(
			LOCAL_TIMECODE_NON_DROP_FRAME,
			i18n::tr("timeline.context.timecode_non_drop_frame"),
		)
		.with_checked(false),
		MenuItem::new(
			LOCAL_TIMECODE_SECONDS,
			i18n::tr("timeline.context.timecode_seconds"),
		)
		.with_checked(false),
		MenuItem::new(
			LOCAL_TIMECODE_FRAMES,
			i18n::tr("timeline.context.timecode_frames"),
		)
		.with_checked(false),
		MenuItem::new(
			LOCAL_TIMECODE_MILLISECONDS,
			i18n::tr("timeline.context.timecode_milliseconds"),
		)
		.with_checked(false),
	])
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::oakui::engine::EngineGateway;
	use crate::oakui::MockEngine;
	use gpui::effect_stack::LibraryEffectDrag;
	use gpui::timeline::{FrameRange, TimelineDataSource};
	use gpui::StatefulInteractiveElement;

	/// The empty ghost for the test drag sources.
	struct DropGhost;

	impl Render for DropGhost {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size(px(10.0))
		}
	}

	/// A test host with draggable footage / effect sources above a real
	/// timeline panel, so the panel's `on_drag_move` / `on_drop` handlers can
	/// be exercised end to end.
	struct DropHost<E: AppEngine> {
		panel: Entity<TimelinePanel<E>>,
	}

	impl<E: AppEngine> Render for DropHost<E> {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			let source = |id: &'static str| {
				div()
					.id(id)
					.debug_selector(move || id.into())
					.h(px(24.0))
					.w_full()
					.flex_shrink_0()
			};
			div()
				.size_full()
				.flex()
				.flex_col()
				.child(
					source("footage-drag-source")
						.on_drag(FootageDrag(3), |_drag, _offset, _window, cx| {
							cx.new(|_| DropGhost)
						}),
				)
				.child(
					source("footage-drag-source-missing").on_drag(
						FootageDrag(9999),
						|_drag, _offset, _window, cx| cx.new(|_| DropGhost),
					),
				)
				.child(
					source("effect-drag-source").on_drag(
						LibraryEffectDrag {
							type_id: "org.olivevideoeditor.Olive.solidgenerator".into(),
							name: "Solid".into(),
						},
						|_drag, _offset, _window, cx| cx.new(|_| DropGhost),
					),
				)
				.child(self.panel.clone())
		}
	}
	use gpui::{px, size, Hsla, TestAppContext, VisualTestContext};

	/// Builds a `TimelinePanel` in a window of the given logical size and
	/// returns a `VisualTestContext` for bounds assertions.
	fn panel_window(
		cx: &mut TestAppContext,
		width: f32,
		height: f32,
	) -> (
		&'static mut VisualTestContext,
		Entity<TimelinePanel<MockEngine>>,
	) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(width), px(height)), |window, cx| {
			let engine = cx.new(crate::oakui::MockEngine::demo);
			let timeline = cx.new(|cx| TimelineView::new(engine.clone(), window, cx).zoom(2.0));
			TimelinePanel::new(engine, timeline, window, cx)
		});
		cx.run_until_parked();
		let panel = window.root(cx).expect("timeline panel root");
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		(cx, panel)
	}

	/// The toolbar row must sit entirely above the timeline body, the right
	/// controls must sit to the right of the timeline canvas (never
	/// overlapping it), and the controls slot must keep its fixed width — at
	/// the default 1600×900 and down to ~1100px wide.
	#[gpui::test]
	async fn toolbar_ruler_and_right_controls_never_overlap(cx: &mut TestAppContext) {
		for width in [1600.0, 1280.0, 1100.0] {
			let (cx, _panel) = panel_window(cx, width, 900.0);

			let toolbar = cx
				.debug_bounds("timeline-toolbar")
				.expect("toolbar row rendered");
			let body = cx
				.debug_bounds("timeline-body")
				.expect("timeline body row rendered");
			let canvas = cx
				.debug_bounds("timeline-canvas")
				.expect("timeline canvas rendered");
			let right = cx
				.debug_bounds("timeline-right-controls")
				.expect("right controls slot rendered");

			// The toolbar is exactly 31px tall and ends where the body starts.
			assert!(
				(f32::from(toolbar.size.height) - TOOLBAR_HEIGHT).abs() < 0.5,
				"toolbar height {width}: {} != {TOOLBAR_HEIGHT}",
				toolbar.size.height
			);
			assert!(
				toolbar.bottom() <= body.top(),
				"toolbar overlaps the body at width {width}"
			);

			// The controls slot is fixed-width and never overlaps the canvas.
			assert!(
				(f32::from(right.size.width) - RIGHT_CONTROLS_WIDTH).abs() < 0.5,
				"right slot width {width}: {} != {RIGHT_CONTROLS_WIDTH}",
				right.size.width
			);
			assert!(
				canvas.right() <= right.left(),
				"right controls overlap the timeline canvas at width {width}"
			);

			// The timeline (ruler) keeps the remaining width: canvas right
			// edge equals the slot's left edge exactly.
			assert!(
				(f32::from(canvas.right()) - f32::from(right.left())).abs() < 0.5,
				"canvas and controls slot are not flush at width {width}"
			);

			// The right controls are inside the body's vertical bounds.
			assert!(
				right.top() >= body.top() && right.bottom() <= body.bottom(),
				"right controls escape the body at width {width}"
			);
		}
	}

	/// Resizing a window keeps the same invariants (the timeline body shrinks
	/// while the toolbar and the right slot stay fixed).
	#[gpui::test]
	async fn resizing_keeps_toolbar_and_right_slot_fixed(cx: &mut TestAppContext) {
		let (cx, _panel) = panel_window(cx, 1600.0, 900.0);

		let before = cx
			.debug_bounds("timeline-right-controls")
			.expect("right controls rendered");
		assert!((f32::from(before.size.width) - RIGHT_CONTROLS_WIDTH).abs() < 0.5);

		cx.simulate_resize(size(px(1100.0), px(900.0)));
		cx.run_until_parked();

		let after = cx
			.debug_bounds("timeline-right-controls")
			.expect("right controls rendered after resize");
		let canvas = cx
			.debug_bounds("timeline-canvas")
			.expect("timeline canvas after resize");
		assert!((f32::from(after.size.width) - RIGHT_CONTROLS_WIDTH).abs() < 0.5);
		assert!(canvas.right() <= after.left());
	}

	/// Right-clicking inside the timeline body opens the popup: the view
	/// emits `ContextMenuRequested`, the panel assembles the matching menu
	/// and the `ContextMenu` popup renders.
	#[gpui::test]
	async fn right_click_opens_the_context_menu(cx: &mut TestAppContext) {
		let (cx, _panel) = panel_window(cx, 1600.0, 900.0);
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert!(
			cx.debug_bounds("menu-popup").is_none(),
			"menu starts hidden"
		);

		let canvas = cx
			.debug_bounds("timeline-canvas")
			.expect("timeline canvas rendered");
		let click = gpui::point(
			canvas.origin.x + canvas.size.width * 0.5,
			canvas.origin.y + canvas.size.height * 0.5,
		);
		cx.simulate_mouse_down(click, gpui::MouseButton::Right, gpui::Modifiers::none());
		cx.run_until_parked();
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});

		let popup = cx
			.debug_bounds("menu-popup")
			.expect("context menu opened on right-click");
		assert!(popup.size.height > px(20.0), "popup lists the items");
	}

	/// The clip menu keeps the C++ shape: edit section, color labels, the
	/// three synchronize entries (registry actions; disabled without ≥ 2
	/// eligible clips), cache and proxy submenus, the reveal/multi-cam
	/// entries and a registry-backed "Properties".
	#[test]
	fn clip_menu_keeps_the_cpp_shape() {
		let menu = clip_menu(crate::oakui::engine::SyncEligibility::default(), &[], None);
		// Color label item sits right after the edit section and carries a
		// submenu of all 16 labels.
		let color = menu
			.items
			.iter()
			.find(|item| item.submenu.is_some() && item.label == i18n::tr("menu.color.label"))
			.expect("color label item");
		assert_eq!(
			color.submenu.as_ref().unwrap().items.len(),
			menu::COLOR_LABEL_COUNT
		);

		// The synchronize entries are the registry actions and stay
		// disabled while fewer than 2 clips are eligible.
		for action in [
			ActionId::SyncBySourceTime,
			ActionId::SyncByWaveform,
			ActionId::SyncByWaveformSpeed,
		] {
			let id = action.entry().menu_id();
			let item = menu
				.items
				.iter()
				.find(|item| item.id == id)
				.unwrap_or_else(|| panic!("clip menu missing synchronize entry {id}"));
			assert!(!item.enabled, "synchronize {id} should be disabled");
		}

		for id in [
			LOCAL_REVEAL_FOOTAGE_VIEWER,
			LOCAL_REVEAL_PROJECT,
			LOCAL_MULTICAM,
		] {
			let item = menu
				.items
				.iter()
				.find(|item| item.id == id)
				.unwrap_or_else(|| panic!("clip menu missing disabled placeholder id {id}"));
			assert!(!item.enabled, "placeholder {id} should be disabled");
		}

		// Cache and proxy are submenus; with no footage selected every
		// proxy entry but the settings action is disabled.
		let cache = menu
			.items
			.iter()
			.find(|item| item.label == i18n::tr("timeline.context.cache"))
			.expect("cache submenu");
		assert_eq!(cache.submenu.as_ref().unwrap().items.len(), 4);
		let proxy = menu
			.items
			.iter()
			.find(|item| item.label == i18n::tr("timeline.context.proxy"))
			.expect("proxy submenu");
		let proxy_items = &proxy.submenu.as_ref().unwrap().items;
		assert_eq!(proxy_items.len(), 5);
		assert!(proxy_items[..4].iter().all(|item| !item.enabled));
		assert!(proxy_items[4].enabled);

		// "Properties" dispatches through the speed/duration registry entry.
		let properties = menu.items.last().expect("properties is the clip menu tail");
		assert_eq!(properties.id, ActionId::SpeedDuration.entry().menu_id());
	}

	/// The synchronize / proxy enable state follows the selection (the C++
	/// `get_selected_*_sync_clips` counts and the proxy-footage flags).
	#[test]
	fn clip_menu_enables_sync_and_proxy_from_selection() {
		use crate::oakui::engine::{ProxyFootageRow, ProxyMediaState, SyncEligibility};
		let rows = vec![
			ProxyFootageRow {
				id: 1,
				name: "a.mp4".into(),
				state: ProxyMediaState::Ready,
				enabled: true,
				has_custom: false,
				can_generate: true,
				has_proxy: true,
			},
			ProxyFootageRow {
				id: 2,
				name: "b.mp4".into(),
				state: ProxyMediaState::Missing,
				enabled: false,
				has_custom: false,
				can_generate: true,
				has_proxy: false,
			},
		];
		let menu = clip_menu(
			SyncEligibility {
				source_time: 2,
				waveform: 1,
			},
			&rows,
			None,
		);
		let find = |id: usize| {
			menu.items
				.iter()
				.find(|item| item.id == id)
				.unwrap_or_else(|| panic!("missing item {id}"))
		};
		assert!(
			find(ActionId::SyncBySourceTime.entry().menu_id()).enabled,
			"2 eligible clips enable source-time sync"
		);
		assert!(
			!find(ActionId::SyncByWaveform.entry().menu_id()).enabled,
			"1 eligible clip keeps waveform sync disabled"
		);
		let proxy = menu
			.items
			.iter()
			.find(|item| item.label == i18n::tr("timeline.context.proxy"))
			.expect("proxy submenu");
		let proxy_items = &proxy.submenu.as_ref().unwrap().items;
		assert!(proxy_items[0].enabled, "generate: footage can generate");
		assert!(proxy_items[1].enabled, "use: footage present");
		assert!(
			!proxy_items[1].checked.unwrap_or(false),
			"use: not every footage has its proxy enabled"
		);
		assert!(proxy_items[2].enabled, "reveal: one footage has a proxy");
		assert!(proxy_items[3].enabled, "delete: one footage has a proxy");
	}

	/// The Multi-Cam item follows the selection's multicam state: it enables
	/// when a selected clip's connected viewer is a sequence and is checked
	/// when that clip already has a multicam (the C++ conditions).
	#[test]
	fn clip_menu_multicam_item_follows_the_state() {
		// No eligible clip: disabled and unchecked.
		let menu = clip_menu(
			crate::oakui::engine::SyncEligibility::default(),
			&[],
			Some(MulticamMenuState {
				eligible: false,
				enabled: false,
			}),
		);
		let item = menu
			.items
			.iter()
			.find(|item| item.id == LOCAL_MULTICAM)
			.expect("multi-cam item");
		assert!(!item.enabled, "ineligible clips keep Multi-Cam disabled");
		assert!(!item.checked.unwrap_or(false));

		// Eligible + enabled: enabled and checked.
		let menu = clip_menu(
			crate::oakui::engine::SyncEligibility::default(),
			&[],
			Some(MulticamMenuState {
				eligible: true,
				enabled: true,
			}),
		);
		let item = menu
			.items
			.iter()
			.find(|item| item.id == LOCAL_MULTICAM)
			.expect("multi-cam item");
		assert!(item.enabled, "a sequence-fed clip enables Multi-Cam");
		assert!(
			item.checked.unwrap_or(false),
			"checked when multicam present"
		);

		// Eligible but not enabled: enabled, unchecked.
		let menu = clip_menu(
			crate::oakui::engine::SyncEligibility::default(),
			&[],
			Some(MulticamMenuState {
				eligible: true,
				enabled: false,
			}),
		);
		let item = menu
			.items
			.iter()
			.find(|item| item.id == LOCAL_MULTICAM)
			.expect("multi-cam item");
		assert!(item.enabled);
		assert!(!item.checked.unwrap_or(false));
	}

	/// The empty-area menu exposes the view toggles, the "Add Adjustment
	/// Layer" creation entry (§3.5) and the sequence settings "Properties"
	/// entry.
	#[test]
	fn empty_area_menu_toggles_and_properties() {
		let menu = empty_area_menu();
		let thumbnails = menu
			.items
			.iter()
			.find(|item| item.label == i18n::tr("timeline.context.show_thumbnails"))
			.expect("thumbnails submenu");
		let sub = &thumbnails.submenu.as_ref().unwrap().items;
		let ids: Vec<usize> = sub.iter().map(|item| item.id).collect();
		assert_eq!(
			ids,
			vec![
				LOCAL_THUMBNAIL_OFF,
				LOCAL_THUMBNAIL_IN_OUT,
				LOCAL_THUMBNAIL_ON
			]
		);
		assert!(sub.iter().all(|item| item.checked == Some(false)));

		// "Add Adjustment Layer" sits between the view toggles and
		// Properties, carries the i18n label and closes its section so the
		// tail reads as the properties block.
		let adjustment = menu
			.items
			.iter()
			.position(|item| item.id == LOCAL_ADD_ADJUSTMENT_LAYER)
			.expect("add adjustment layer item");
		let item = &menu.items[adjustment];
		assert!(item.label == i18n::tr("timeline.context.add_adjustment_layer"));
		assert!(item.enabled);
		assert!(item.separator_after);
		assert_eq!(
			menu.items[adjustment + 1].id,
			ActionId::SequenceSettings.entry().menu_id(),
			"Properties follows the adjustment item"
		);

		let properties = menu.items.last().expect("properties tail");
		assert_eq!(properties.id, ActionId::SequenceSettings.entry().menu_id());
	}

	/// Choosing "Add Adjustment Layer" from the empty-area menu lands the
	/// demo's five-second layer on the video track under the cursor, at the
	/// clicked frame and in a color of its own (§3.5).
	#[gpui::test]
	async fn empty_area_menu_adds_an_adjustment_layer_at_the_clicked_frame(
		cx: &mut TestAppContext,
	) {
		let (cx, panel) = panel_window(cx, 1600.0, 900.0);
		// The demo's track 0 is a video track with one clip (标题.mov, frames
		// 120..300); the created layer must not borrow that clip's color.
		let demo_colors: Vec<Hsla> = cx.read(|app| {
			let engine = panel.read(app).engine.clone();
			let engine = engine.read(app);
			engine
				.track(0)
				.expect("the demo's first track")
				.clips()
				.iter()
				.filter_map(|clip| clip.color())
				.collect()
		});
		assert_eq!(demo_colors.len(), 1, "the demo track has one colored clip");

		// The right-click anchors the menu on (track 0, frame 50); choosing
		// the item creates the layer there.
		cx.update(|_window, cx| {
			panel.update(cx, |panel, cx| {
				panel.open_context_menu(
					gpui::point(px(10.0), px(10.0)),
					TimelineHit::Empty {
						track: 0,
						frame: Frame(50),
					},
					cx,
				);
				panel.on_local_menu_item(LOCAL_ADD_ADJUSTMENT_LAYER, cx);
			});
		});

		cx.read(|app| {
			let engine = panel.read(app).engine.clone();
			let engine = engine.read(app);
			let track = engine.track(0).expect("the demo's first track");
			let clips = track.clips();
			assert_eq!(clips.len(), 2, "the layer joins the track's demo clip");
			let layer = &clips[0];
			assert_eq!(
				layer.range(),
				FrameRange::new(Frame(50), Frame(175)),
				"five seconds (125 frames at 25fps) from the clicked frame"
			);
			assert_eq!(layer.label(), SharedString::from("调整图层"));
			assert_eq!(layer.media_in(), Frame(0));
			let color = layer.color().expect("the layer carries its own color");
			assert!(
				demo_colors.iter().all(|demo| *demo != color),
				"the layer color is distinct from the demo clips"
			);
		});
	}

	/// The track-header menu is exactly the two delete entries.
	#[test]
	fn track_head_menu_offers_add_then_delete_entries() {
		let ids: Vec<usize> = track_head_menu().items.iter().map(|item| item.id).collect();
		assert_eq!(
			ids,
			vec![
				LOCAL_ADD_VIDEO_TRACK,
				LOCAL_ADD_AUDIO_TRACK,
				LOCAL_DELETE_TRACK,
				LOCAL_DELETE_ALL_EMPTY
			]
		);
	}

	/// The marker menu pairs the color labels with the plain edit section
	/// and a local marker-properties entry.
	#[test]
	fn marker_menu_pairs_color_labels_with_edit_section() {
		let menu = marker_menu();
		assert!(menu.items[0].label == i18n::tr("menu.color.label"));
		assert!(menu.items[0].separator_after);
		let last = menu.items.last().expect("marker properties tail");
		assert_eq!(last.id, LOCAL_MARKER_PROPERTIES);
		assert_eq!(last.label, i18n::tr("menu.context.properties"));
	}

	/// The ruler menu is the timecode-display radio group, all unchecked by
	/// default.
	#[test]
	fn ruler_menu_is_the_timecode_radio_group() {
		let menu = ruler_menu();
		let ids: Vec<usize> = menu.items.iter().map(|item| item.id).collect();
		assert_eq!(
			ids,
			vec![
				LOCAL_TIMECODE_DROP_FRAME,
				LOCAL_TIMECODE_NON_DROP_FRAME,
				LOCAL_TIMECODE_SECONDS,
				LOCAL_TIMECODE_FRAMES,
				LOCAL_TIMECODE_MILLISECONDS,
			]
		);
		assert!(menu.items.iter().all(|item| item.checked == Some(false)));
	}

	// -----------------------------------------------------------------------
	// Panel behavior: subscriptions, context menus, commands, drops, render.
	// -----------------------------------------------------------------------

	/// The zoom/height sliders and the snap checkbox drive the view/engine
	/// through the panel's subscriptions.
	#[gpui::test]
	async fn slider_and_snap_events_update_the_view_and_engine(cx: &mut TestAppContext) {
		use crate::oakui::component::controls::SliderValue;
		let (cx, panel) = panel_window(cx, 1280.0, 720.0);

		// Zoom slider -> timeline zoom.
		cx.update(|_window, app| {
			panel.read(app).zoom.clone().update(app, |_slider, cx| {
				cx.emit(SliderEvent::ValueChanged {
					control: 10,
					value: SliderValue::Float(3.5),
				});
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).timeline.read(app).state.zoom),
			3.5
		);

		// Track-height slider -> every engine track.
		cx.update(|_window, app| {
			panel.read(app).height.clone().update(app, |_slider, cx| {
				cx.emit(SliderEvent::ValueChanged {
					control: 11,
					value: SliderValue::Float(80.0),
				});
			});
		});
		cx.read(|app| {
			let engine = panel.read(app).engine.clone();
			let engine = engine.read(app);
			assert!(engine.track_count() > 0, "the demo has tracks");
			for index in 0..engine.track_count() {
				assert_eq!(engine.track(index).unwrap().height(), px(80.0));
			}
		});

		// Snap checkbox -> view state.
		for (state, expected) in [(CheckState::Unchecked, false), (CheckState::Checked, true)] {
			cx.update(|_window, app| {
				panel.read(app).snap.clone().update(app, |_snap, cx| {
					cx.emit(CheckBoxEvent::Toggled { control: 12, state });
				});
			});
			assert_eq!(
				cx.read(|app| panel.read(app).timeline.read(app).state.snap_enabled),
				expected
			);
		}
	}

	/// Every `TimelineHit` variant opens its matching context menu; an
	/// unselected right-clicked clip is selected first.
	#[gpui::test]
	async fn context_menus_open_for_every_hit_kind(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx, 1600.0, 900.0);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				let position = gpui::point(px(20.0), px(20.0));

				panel.open_context_menu(position, TimelineHit::Clip(ClipId(10)), cx);
				assert!(
					panel.timeline.read(cx).selection().contains(&ClipId(10)),
					"an unselected right-clicked clip is selected first"
				);
				assert_eq!(panel.context_track, None);
				assert_eq!(panel.context_empty, None);

				// A second hit on the already-selected clip takes the
				// no-reselect path.
				panel.open_context_menu(position, TimelineHit::Clip(ClipId(10)), cx);

				panel.open_context_menu(
					position,
					TimelineHit::Empty {
						track: 2,
						frame: Frame(42),
					},
					cx,
				);
				assert_eq!(panel.context_empty, Some((2, Frame(42))));
				assert_eq!(panel.context_track, None);

				panel.open_context_menu(position, TimelineHit::TrackHead(3), cx);
				assert_eq!(panel.context_track, Some(3));
				assert_eq!(panel.context_empty, None);

				panel.open_context_menu(position, TimelineHit::RulerMarker(Frame(7)), cx);
				assert_eq!(panel.context_track, None);
				assert_eq!(panel.context_empty, None);

				panel.open_context_menu(position, TimelineHit::Ruler(Frame(9)), cx);
				assert_eq!(panel.context_track, None);
				assert_eq!(panel.context_empty, None);
			});
		});
	}

	/// The local (non-registry) menu items route to the engine; missing
	/// targets and unknown ids are benign no-ops.
	#[gpui::test]
	async fn local_menu_items_route_to_the_engine(cx: &mut TestAppContext) {
		// The Multi-Cam item runs real undoable commands; serialize with the
		// graph tests sharing the global undo stack.
		let _guard = crate::oakui::graphops::test_lock();
		let (cx, panel) = panel_window(cx, 1600.0, 900.0);

		let tracks_before = cx.read(|app| panel.read(app).engine.read(app).track_count());
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.on_local_menu_item(LOCAL_ADD_VIDEO_TRACK, cx);
				panel.on_local_menu_item(LOCAL_ADD_AUDIO_TRACK, cx);
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).track_count()),
			tracks_before + 2
		);

		// Delete one track (with a recorded track-head target). Without a
		// target the click is ignored.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.on_local_menu_item(LOCAL_DELETE_TRACK, cx);
				panel.context_track = Some(2);
				panel.on_local_menu_item(LOCAL_DELETE_TRACK, cx);
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).track_count()),
			tracks_before + 1
		);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.on_local_menu_item(LOCAL_DELETE_ALL_EMPTY, cx);
			});
		});

		// A color label only logs (the engine has no clip-color surface).
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.on_local_menu_item(menu::COLOR_LABEL_BASE + 3, cx);
			});
		});

		// Add adjustment layer: without a recorded empty-area click it is a
		// no-op; with one the demo layer lands on the pointed track.
		let clips_before = cx.read(|app| {
			panel.read(app).engine.read(app).track(0).unwrap().clips().len()
		});
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.on_local_menu_item(LOCAL_ADD_ADJUSTMENT_LAYER, cx);
				panel.context_empty = Some((0, Frame(10)));
				panel.on_local_menu_item(LOCAL_ADD_ADJUSTMENT_LAYER, cx);
			});
		});
		cx.read(|app| {
			assert_eq!(
				panel.read(app).engine.read(app).track(0).unwrap().clips().len(),
				clips_before + 1,
				"the adjustment layer joined the video track"
			);
		});
		// An audio target is rejected (the error branch logs).
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.context_empty = Some((2, Frame(10)));
				panel.on_local_menu_item(LOCAL_ADD_ADJUSTMENT_LAYER, cx);
			});
		});

		// The cache / timecode / thumbnail placeholders and an unknown id.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				for item in [
					LOCAL_CACHE_ALL,
					LOCAL_CACHE_IN_OUT,
					LOCAL_CACHE_DISCARD,
					LOCAL_CACHE_AUTO,
					LOCAL_TIMECODE_DROP_FRAME,
					LOCAL_TIMECODE_NON_DROP_FRAME,
					LOCAL_TIMECODE_SECONDS,
					LOCAL_TIMECODE_FRAMES,
					LOCAL_TIMECODE_MILLISECONDS,
					LOCAL_USE_AUDIO_TIME_UNITS,
					LOCAL_SHOW_WAVEFORMS,
					LOCAL_THUMBNAIL_OFF,
					LOCAL_THUMBNAIL_IN_OUT,
					LOCAL_THUMBNAIL_ON,
					LOCAL_MARKER_PROPERTIES,
					LOCAL_REVEAL_FOOTAGE_VIEWER,
					LOCAL_REVEAL_PROJECT,
				] {
					panel.on_local_menu_item(item, cx);
				}
				panel.on_local_menu_item(999_999, cx);
			});
		});

		// The proxy actions over the selected demo clip (id 10 maps to the
		// explorer's intro.mov): generate first, then use / reveal / delete.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.timeline.update(cx, |view, cx| {
					view.state.selection.clear();
					view.state.selection.insert(ClipId(10));
					cx.notify();
				});
				panel.on_local_menu_item(LOCAL_PROXY_GENERATE, cx);
				panel.on_local_menu_item(LOCAL_PROXY_USE, cx);
				panel.on_local_menu_item(LOCAL_PROXY_REVEAL, cx);
				panel.on_local_menu_item(LOCAL_PROXY_DELETE, cx);
			});
		});
		cx.read(|app| {
			let engine = panel.read(app).engine.clone();
			let engine = engine.read(app);
			assert!(engine.proxy_row(10).is_some(), "the demo row exists");
		});

		// Multi-Cam flips through the real graph commands.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.timeline.update(cx, |view, cx| {
					view.state.selection.insert(ClipId(10));
					cx.notify();
				});
				panel.on_local_menu_item(LOCAL_MULTICAM, cx);
			});
		});
	}

	/// The panel command trait routes transport, in/out points, selection and
	/// editing commands to the engine without panicking.
	#[gpui::test]
	async fn panel_commands_cover_transport_edit_and_view(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx, 1280.0, 720.0);

		// Transport: play/pause toggles the program clock.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				assert!(panel.play_pause(cx));
			});
		});
		assert!(
			cx.read(|app| panel
				.read(app)
				.engine
				.read(app)
				.program_clock()
				.read(app)
				.is_playing()),
			"play_pause starts the program monitor"
		);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				assert!(panel.play_pause(cx));
				assert!(panel.prev_frame(cx));
				assert!(panel.next_frame(cx));
				assert!(panel.go_to_start(cx));
				assert!(panel.go_to_end(cx));
				assert!(panel.shuttle_left(cx));
				assert!(panel.shuttle_right(cx));
				assert!(panel.shuttle_stop(cx));
			});
		});
		assert!(
			!cx.read(|app| panel
				.read(app)
				.engine
				.read(app)
				.program_clock()
				.read(app)
				.is_playing()),
			"shuttle_stop pauses the program monitor"
		);

		// In/out points.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.engine.update(cx, |engine, cx| {
					engine.request_frame(Monitor::Program, Frame(90), cx)
				});
				assert!(panel.set_in(cx));
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).workarea().map(|(s, _)| s)),
			Some(Frame(90))
		);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				assert!(panel.set_out(cx));
				assert!(panel.reset_in(cx));
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).workarea().map(|(s, _)| s)),
			Some(Frame::ZERO),
			"reset_in returns the in point to the sequence start"
		);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				assert!(panel.reset_out(cx));
			});
		});
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				assert!(panel.clear_in_out(cx));
			});
		});
		assert_eq!(cx.read(|app| panel.read(app).engine.read(app).workarea()), None);

		// Reset with no work area set (the unwrap_or fallbacks).
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				assert!(panel.reset_in(cx));
				assert!(panel.reset_out(cx));
			});
		});
		assert!(cx.read(|app| panel.read(app).engine.read(app).workarea()).is_some());

		// Selection + editing commands (the mock's clipboard methods are
		// no-ops; delete/split touch its demo tracks).
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				assert!(panel.select_all(cx));
				assert!(panel.deselect_all(cx));
				assert!(panel.delete_selected(cx), "an empty selection is handled");
				assert!(panel.ripple_delete(cx));
				assert!(panel.select_all(cx));
				assert!(panel.copy_selected(cx));
				assert!(panel.cut_selected(cx));
				assert!(panel.paste(cx));
				assert!(panel.delete_selected(cx));
				assert!(panel.ripple_delete(cx));
				assert!(panel.split_at_playhead(cx));
				assert!(panel.default_transition(cx));
				assert!(panel.set_marker(cx));
				assert!(panel.sync_by_source_time(cx));
				assert!(panel.sync_by_waveform(cx));
				assert!(panel.sync_by_waveform_speed(cx));
				assert!(panel.toggle_links(cx));
				assert!(panel.zoom_in(cx));
				assert!(panel.zoom_out(cx));
				assert!(panel.increase_track_height(cx));
				assert!(panel.decrease_track_height(cx));
			});
		});
	}

	/// `delete_selection` ignores an empty selection and removes the selected
	/// clips through the engine; the work-area commits follow the playhead.
	#[gpui::test]
	async fn delete_selection_and_workarea_paths(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx, 1280.0, 720.0);

		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.timeline.update(cx, |view, cx| {
					view.state.selection.clear();
					cx.notify();
				});
				panel.delete_selection(false, cx);
				panel.delete_selection(true, cx);
			});
		});

		let clips_before = cx.read(|app| {
			panel.read(app).engine.read(app).track(0).unwrap().clips().len()
		});
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.timeline.update(cx, |view, cx| {
					view.state.selection.insert(ClipId(10));
					cx.notify();
				});
				panel.delete_selection(false, cx);
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).track(0).unwrap().clips().len()),
			clips_before - 1,
			"the selected clip left its track"
		);

		// A playhead at frame zero cannot form an out point: the commit is
		// ignored (and the in point falls back to the sequence length).
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.engine.update(cx, |engine, cx| {
					engine.request_frame(Monitor::Program, Frame::ZERO, cx)
				});
				panel.set_point_at_playhead(false, cx);
				panel.set_point_at_playhead(true, cx);
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).workarea().map(|(s, _)| s)),
			Some(Frame::ZERO)
		);
	}

	/// `finish_footage_drop` and `finish_effect_drop` route their pending
	/// targets; a missing target and an unusable effect are benign no-ops.
	#[gpui::test]
	async fn finished_drops_route_to_the_engine(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx, 1280.0, 720.0);

		let target = || FootageDropTarget {
			track_kind: TrackKind::Video,
			track_index: 1,
			time: Frame(20),
			length: 250,
		};

		// No pending footage drop.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.finish_footage_drop(&FootageDrag(3), cx);
			});
		});
		assert!(cx.read(|app| panel.read(app).engine.read(app).footage_drops().is_empty()));

		// A pending drop with an open sequence reaches the engine.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.footage_drop = Some(target());
				panel.finish_footage_drop(&FootageDrag(3), cx);
			});
		});
		cx.read(|app| {
			let panel = panel.read(app);
			assert!(panel.footage_drop.is_none(), "the target is consumed");
			assert_eq!(panel.engine.read(app).footage_drops().len(), 1);
		});

		let generator =
			gpui::effect_stack::LibraryEffectDrag {
				type_id: "org.olivevideoeditor.Olive.solidgenerator".into(),
				name: "Solid".into(),
			};
		let transition =
			gpui::effect_stack::LibraryEffectDrag {
				type_id: "org.olivevideoeditor.Olive.transition".into(),
				name: "Dissolve".into(),
			};
		let non_generator = gpui::effect_stack::LibraryEffectDrag {
			type_id: "org.olivevideoeditor.Olive.blur".into(),
			name: "Blur".into(),
		};

		// No pending effect drop.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.finish_effect_drop(&generator, cx);
			});
		});

		// A transition type routes to the engine's transition drop.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.effect_drop = Some(target());
				panel.finish_effect_drop(&transition, cx);
			});
		});
		cx.read(|app| assert!(panel.read(app).effect_drop.is_none()));

		// A non-generator effect is ignored.
		let clips_before = cx.read(|app| {
			panel.read(app).engine.read(app).track(1).unwrap().clips().len()
		});
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.effect_drop = Some(target());
				panel.finish_effect_drop(&non_generator, cx);
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).track(1).unwrap().clips().len()),
			clips_before,
			"a non-generator makes no clip"
		);

		// A generator type creates a standalone clip on the pointed track.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.effect_drop = Some(target());
				panel.finish_effect_drop(&generator, cx);
			});
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).track(1).unwrap().clips().len()),
			clips_before + 1,
			"the generator clip landed on the pointed track"
		);
	}

	/// `resolve_drop_point` maps the cursor to track + frame, clamps above
	/// the ruler / below the tracks and falls back on an empty timeline.
	#[gpui::test]
	async fn resolve_drop_point_maps_and_clamps(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx, 1280.0, 720.0);

		assert_eq!(
			cx.read(|app| panel
				.read(app)
				.resolve_drop_point(gpui::point(px(200.0), px(10.0)), app)),
			None,
			"above the ruler there is no clip area"
		);

		// Inside the header column the frame clamps to zero; the y walk finds
		// the second track (64px tall rows).
		let (kind, index, frame) = cx
			.read(|app| {
				panel
					.read(app)
					.resolve_drop_point(gpui::point(px(10.0), px(100.0)), app)
			})
			.expect("below the ruler");
		assert_eq!((kind, index), (TrackKind::Video, 1));
		assert_eq!(frame, Frame::ZERO);

		// Below every track clamps to the last one.
		let (kind, index, _) = cx
			.read(|app| {
				panel
					.read(app)
					.resolve_drop_point(gpui::point(px(600.0), px(4000.0)), app)
			})
			.expect("below the ruler");
		assert_eq!((kind, index), (TrackKind::Audio, 3));

		// An empty timeline falls back to (Video, 0).
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				let engine = panel.engine.clone();
				engine.update(cx, |engine, cx| {
					while engine.track_count() > 0 {
						engine.remove_track(0, cx);
					}
				});
				let (kind, index, _) = panel
					.resolve_drop_point(gpui::point(px(600.0), px(100.0)), cx)
					.expect("still below the ruler");
				assert_eq!((kind, index), (TrackKind::Video, 0));
			});
		});
	}

	/// `zoom_timeline` and `nudge_track_height` scale the view and clamp the
	/// engine's track heights to the slider range.
	#[gpui::test]
	async fn zoom_and_track_height_nudges(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx, 1280.0, 720.0);
		let before = cx.read(|app| panel.read(app).timeline.read(app).state.zoom);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| panel.zoom_timeline(1.25, cx));
		});
		assert!(cx.read(|app| panel.read(app).timeline.read(app).state.zoom) > before);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| panel.zoom_timeline(0.8, cx));
		});

		cx.update(|_window, app| {
			panel.update(app, |panel, cx| panel.nudge_track_height(1000.0, cx));
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).track(0).unwrap().height()),
			px(160.0),
			"the height clamps at the slider maximum"
		);
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| panel.nudge_track_height(-1000.0, cx));
		});
		assert_eq!(
			cx.read(|app| panel.read(app).engine.read(app).track(0).unwrap().height()),
			px(24.0),
			"the height clamps at the slider minimum"
		);

		// No tracks: the 64px default is used as the nudge base.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				let engine = panel.engine.clone();
				engine.update(cx, |engine, cx| {
					while engine.track_count() > 0 {
						engine.remove_track(0, cx);
					}
				});
				panel.nudge_track_height(0.0, cx);
			});
		});
	}

	/// The toolbar's tool buttons and trailing controls run their `on_click`
	/// closures: tool selection, add-track buttons, zoom buttons and the
	/// snap icon.
	#[gpui::test]
	async fn toolbar_buttons_switch_tools_and_route_actions(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx, 1600.0, 900.0);
		let toolbar = cx.debug_bounds("timeline-toolbar").expect("toolbar rendered");
		let y = toolbar.origin.y + px(15.0);

		// The eight tool buttons sit at 24px + 8px-gap offsets after the 8px
		// toolbar padding.
		for index in 0..8 {
			let x = toolbar.origin.x + px(8.0 + 32.0 * index as f32 + 12.0);
			cx.simulate_click(gpui::point(x, y), gpui::Modifiers::none());
			assert_eq!(
				cx.read(|app| panel.read(app).timeline.read(app).tool()),
				TimelineTool::from_index(index).unwrap(),
				"toolbar button {index} selects its tool"
			);
		}
		// Clicking the already-selected tool again hits the no-op guard.
		cx.simulate_click(
			gpui::point(toolbar.origin.x + px(8.0 + 7.0 * 32.0 + 12.0), y),
			gpui::Modifiers::none(),
		);

		// Walk the trailing controls: each hit routes through an `on_click`
		// closure (add-track buttons, zoom buttons, the snap toggle).
		let mut saw_add_track = false;
		let mut saw_zoom = false;
		let mut saw_snap = false;
		let mut x = toolbar.origin.x + px(264.0);
		while x < toolbar.right() {
			let (tracks_before, zoom_before, snap_before) = cx.read(|app| {
				let panel = panel.read(app);
				let engine = panel.engine.read(app);
				(
					engine.track_count(),
					panel.timeline.read(app).state.zoom,
					panel.timeline.read(app).state.snap_enabled,
				)
			});
			cx.simulate_click(gpui::point(x, y), gpui::Modifiers::none());
			let (tracks_after, zoom_after, snap_after) = cx.read(|app| {
				let panel = panel.read(app);
				let engine = panel.engine.read(app);
				(
					engine.track_count(),
					panel.timeline.read(app).state.zoom,
					panel.timeline.read(app).state.snap_enabled,
				)
			});
			saw_add_track |= tracks_after != tracks_before;
			saw_zoom |= (zoom_after - zoom_before).abs() > f32::EPSILON;
			saw_snap |= snap_after != snap_before;
			x += px(4.0);
		}
		assert!(saw_add_track, "an add-track button was hit");
		assert!(saw_zoom, "a zoom button was hit");
		assert!(saw_snap, "the snap toggle was hit");
	}

	/// The drop ghost renders (including the out-of-range row break), the
	/// dock metadata is populated and a left click focuses the panel.
	#[gpui::test]
	async fn drop_ghost_and_dock_metadata(cx: &mut TestAppContext) {
		let (cx, panel) = panel_window(cx, 1280.0, 720.0);

		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.footage_drop = Some(FootageDropTarget {
					track_kind: TrackKind::Video,
					track_index: 1,
					time: Frame(50),
					length: 250,
				});
				cx.notify();
			});
		});
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});
		assert!(cx.debug_bounds("timeline-canvas").is_some());

		// A target beyond the last track breaks the row walk instead of
		// panicking; a zero-length ghost keeps its 4px minimum width.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.footage_drop = None;
				panel.effect_drop = Some(FootageDropTarget {
					track_kind: TrackKind::Audio,
					track_index: 999,
					time: Frame(10),
					length: 0,
				});
				cx.notify();
			});
		});
		cx.update(|window, cx| {
			window.draw(cx).clear();
		});

		// Dock metadata.
		cx.update(|_window, app| {
			let panel = panel.read(app);
			assert_eq!(panel.panel_id(), crate::panels::ids::TIMELINE);
			assert!(!panel.title(app).is_empty());
			let _ = panel.tab_content(app);
		});

		// A left click focuses the panel (the root's mouse-down handler).
		cx.simulate_mouse_down(
			gpui::point(px(640.0), px(400.0)),
			gpui::MouseButton::Left,
			gpui::Modifiers::none(),
		);
	}

	/// Dispatches one complete drag gesture against the current rendered
	/// frame (a repaint between the events would consume the listeners).
	fn dispatch_drag(
		cx: &mut VisualTestContext,
		start: gpui::Point<Pixels>,
		hover: gpui::Point<Pixels>,
		ruler: gpui::Point<Pixels>,
	) {
		cx.update(|window, cx| {
			window.dispatch_event(
				gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
					position: start,
					modifiers: gpui::Modifiers::none(),
					button: gpui::MouseButton::Left,
					click_count: 1,
					first_mouse: false,
				}),
				cx,
			);
			let mut move_to = |position| {
				window.dispatch_event(
					gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
						position,
						modifiers: gpui::Modifiers::none(),
						pressed_button: Some(gpui::MouseButton::Left),
					}),
					cx,
				);
			};
			// Past the drag threshold to start the drag.
			move_to(start + gpui::point(px(6.0), px(0.0)));
			// Over the ruler first (no target yet, so the clear is a no-op),
			// then hover the drop point, slide up to the ruler (clearing the
			// target) and hover it again.
			move_to(ruler);
			move_to(hover);
			move_to(ruler);
			move_to(hover);
			window.dispatch_event(
				gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
					position: hover,
					modifiers: gpui::Modifiers::none(),
					button: gpui::MouseButton::Left,
					click_count: 1,
				}),
				cx,
			);
		});
	}

	/// A full drag onto the timeline canvas resolves the drop target (and
	/// clears it over the ruler), then routes the payload to the engine.
	#[gpui::test]
	async fn drag_gestures_resolve_targets_and_route_drops(cx: &mut TestAppContext) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(1280.0), px(720.0)), |window, cx| {
			let engine = cx.new(MockEngine::demo);
			let timeline = cx.new(|cx| TimelineView::new(engine.clone(), window, cx).zoom(2.0));
			let panel = cx.new(|cx| TimelinePanel::new(engine, timeline, window, cx));
			DropHost { panel }
		});
		cx.run_until_parked();
		let host = window.root(cx).expect("drop host root");
		let mut cx = VisualTestContext::from_window(window.into(), cx);
		let panel = cx.read(|app| host.read(app).panel.clone());

		let canvas = cx.debug_bounds("timeline-canvas").expect("canvas rendered");
		let hover = gpui::point(
			canvas.left() + px(HEADER_WIDTH + 100.0),
			canvas.top() + px(RULER_HEIGHT + 20.0),
		);
		let ruler = gpui::point(
			canvas.left() + px(HEADER_WIDTH + 100.0),
			canvas.top() + px(10.0),
		);

		// An unknown footage id falls back to a one-frame ghost and the
		// engine rejects the drop.
		let source = cx
			.debug_bounds("footage-drag-source-missing")
			.expect("missing-footage source");
		dispatch_drag(&mut cx, source.center(), hover, ruler);
		cx.run_until_parked();
		cx.read(|app| {
			let panel = panel.read(app);
			assert!(panel.footage_drop.is_none(), "the drop consumed the target");
			assert!(
				panel.engine.read(app).footage_drops().is_empty(),
				"the unknown entry never placed a clip"
			);
		});

		// The real footage entry places its clip.
		let source = cx
			.debug_bounds("footage-drag-source")
			.expect("footage source");
		dispatch_drag(&mut cx, source.center(), hover, ruler);
		cx.run_until_parked();
		cx.read(|app| {
			let panel = panel.read(app);
			let drops = panel.engine.read(app).footage_drops().to_vec();
			assert_eq!(drops.len(), 1, "the footage drop reached the engine");
			assert_eq!(drops[0].id, 3);
			assert_eq!(drops[0].track_kind, TrackKind::Video);
			assert_eq!(drops[0].track_index, 0);
			assert!(drops[0].time.0 >= 0);
		});

		// The generator effect becomes a standalone clip on the pointed
		// track (the drop point sits on the first row).
		let clips_before = cx.read(|app| {
			panel.read(app).engine.read(app).track(0).unwrap().clips().len()
		});
		let source = cx
			.debug_bounds("effect-drag-source")
			.expect("effect source");
		dispatch_drag(&mut cx, source.center(), hover, ruler);
		cx.run_until_parked();
		cx.read(|app| {
			let panel = panel.read(app);
			assert!(panel.effect_drop.is_none(), "the effect drop is consumed");
			assert_eq!(
				panel.engine.read(app).track(0).unwrap().clips().len(),
				clips_before + 1,
				"the generator clip landed on the pointed track"
			);
		});
	}

	/// With no open sequence the footage drop is handed back to the shell and
	/// the work-area helpers fall back to a one-frame sequence.
	#[gpui::test]
	async fn no_sequence_paths_use_the_fallbacks(cx: &mut TestAppContext) {
		use std::cell::Cell;
		use std::rc::Rc;
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(1280.0), px(720.0)), |window, cx| {
			let engine = cx.new(crate::oakui::RealEngine::create);
			let timeline = cx.new(|cx| TimelineView::new(engine.clone(), window, cx).zoom(2.0));
			let panel = cx.new(|cx| TimelinePanel::new(engine, timeline, window, cx));
			DropHost { panel }
		});
		cx.run_until_parked();
		let host = window.root(cx).expect("host root");
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		let panel = cx.read(|app| host.read(app).panel.clone());

		let emitted = Rc::new(Cell::new(false));
		cx.update(|_window, app| {
			let emitted = emitted.clone();
			app.subscribe(
				&panel,
				move |_panel: Entity<TimelinePanel<crate::oakui::RealEngine>>,
				      _event: &FootageDropNeedsSequence,
				      _cx| {
					emitted.set(true);
				},
			)
			.detach();
			panel.update(app, |panel, cx| {
				panel.footage_drop = Some(FootageDropTarget {
					track_kind: TrackKind::Video,
					track_index: 0,
					time: Frame(5),
					length: 1,
				});
				panel.finish_footage_drop(&FootageDrag(3), cx);
			});
		});
		assert!(
			emitted.get(),
			"the shell is asked to set up a sequence first"
		);
		cx.read(|app| assert!(panel.read(app).footage_drop.is_none()));

		// No sequence length is available: the fallbacks use the playhead.
		cx.update(|_window, app| {
			panel.update(app, |panel, cx| {
				panel.set_point_at_playhead(false, cx);
				panel.set_point_at_playhead(true, cx);
				assert!(panel.reset_in(cx));
				assert!(panel.reset_out(cx));
			});
		});
	}
}
