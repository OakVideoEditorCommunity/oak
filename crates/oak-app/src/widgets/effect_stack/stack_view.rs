//! The top-level effect-stack view and its event type.
//!
//! [`EffectStackView`] renders the linear card chain described by an
//! [`EffectStackDataSource`] and reports every user edit intent as an
//! [`EffectStackEvent`]. See the [module-level docs](crate::widgets::effect_stack)
//! for the "edits are requests" contract.

use std::rc::Rc;

use gpui_kit::{
	AnyView, App, AppContext, ClickEvent, Context, DragMoveEvent, ElementId, Entity, EventEmitter,
	FocusHandle, Focusable, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
	Point, Render, SharedString, StatefulInteractiveElement, Styled, Window, colors::DefaultColors,
	div,
};

use super::card::{EffectCard, InsertIndicator};
use super::data::{EffectCardKind, EffectId, EffectStackDataSource, LibraryEffectDrag};

/// Callback the app registers with
/// [`EffectStackView::params_renderer`] to render the parameter controls of
/// one effect inside its expanded card.
///
/// Called when a card's parameter area is first shown; the returned
/// [`AnyView`] is **cached per effect** (so stateful controls — sliders,
/// checkboxes, text fields — survive re-renders and in-progress drags)
/// and pruned when the effect leaves the stack. The view is placed in the
/// card's content slot; its size drives the expanded height of the card.
/// Return any empty view (e.g. [`gpui_kit::div()`]'s default) to render a
/// blank parameter area.
pub type ParamsRenderer = Rc<dyn Fn(&EffectId, &mut Window, &mut App) -> AnyView>;

/// Edit requests emitted by [`EffectStackView`].
///
/// **Every variant is a request, not a completed edit.** The view does not
/// mutate the [`EffectStackDataSource`]. The app subscribes, applies the
/// request through its engine and undo stack, then calls
/// [`cx.notify()`](Context::notify) so the stack re-renders from the
/// updated model. If the request is rejected (invalid engine state, failed
/// validation), the app simply does nothing — the view keeps rendering the
/// unchanged model.
#[derive(Clone, Debug)]
pub enum EffectStackEvent {
	/// The user dragged a card to a new position.
	///
	/// `new_index` is an index into the list returned by
	/// [`EffectStackDataSource::effects`] **after** removal of the dragged
	/// card (i.e. an insertion position). The app maps this to rewiring the
	/// node-graph path; see the [module docs](crate::widgets::effect_stack). The
	/// view has already filtered positions rejected by
	/// [`EffectStackDataSource::can_reorder`], but the app must re-validate.
	ReorderRequested {
		/// The dragged effect.
		effect: EffectId,
		/// Insertion index in the post-removal card list.
		new_index: usize,
	},
	/// The user clicked the enable/disable toggle on a card.
	EnableToggled {
		/// The toggled effect.
		effect: EffectId,
		/// The desired new enabled state.
		enabled: bool,
	},
	/// The user clicked a card header to expand or collapse its parameter
	/// area.
	ExpansionToggled {
		/// The toggled effect.
		effect: EffectId,
		/// The desired new expansion state.
		expanded: bool,
	},
	/// The user clicked a card header to select it (the same gesture that
	/// toggles expansion). Emitted alongside
	/// [`ExpansionToggled`](Self::ExpansionToggled) so hosts can sync the
	/// effect selection with the node graph (highlighting the effect's node).
	CardSelected {
		/// The clicked effect.
		effect: EffectId,
	},
	/// The user clicked the remove button on a removable card.
	RemoveRequested(EffectId),
	/// The user invoked an "add effect" affordance at a stack position.
	///
	/// The app typically responds by opening its effect browser; once the
	/// user picks an effect, the app inserts the corresponding node at
	/// `index` and notifies.
	AddRequested {
		/// Insertion index into the current card list.
		index: usize,
	},
	/// The user dropped an effect dragged from the app's effect library
	/// onto the stack at a stack position (the payload is
	/// [`LibraryEffectDrag`](crate::widgets::effect_stack::LibraryEffectDrag)).
	/// Unlike [`AddRequested`](Self::AddRequested) the effect type is
	/// already chosen, so the app inserts it directly at `index`.
	AddTypeRequested {
		/// Insertion index into the current card list.
		index: usize,
		/// The dropped effect's type id.
		type_id: SharedString,
	},
	/// The user secondary-clicked a card. The app owns the menu itself —
	/// the view only reports where and on which card it happened.
	ContextMenuRequested {
		/// The effect that was clicked.
		effect: EffectId,
		/// Mouse position in window coordinates, suitable for positioning a
		/// context menu.
		position: Point<Pixels>,
	},
	/// A parameter of an effect changed inside its card's parameter area.
	///
	/// Emitted when the app's parameter UI calls
	/// [`EffectStackView::notify_parameter_changed`]. The view uses it to
	/// refresh card metadata (e.g. the animated-parameter badge); the app
	/// may additionally subscribe to e.g. schedule a preview re-render.
	ParameterChanged {
		/// The effect whose parameters changed.
		effect: EffectId,
	},
}

/// Transient drag state for an in-progress card reorder.
///
/// Purely visual: tracks which card is being dragged and the current
/// insertion position so [`render`](Render::render) can draw the ghost and
/// the [`InsertIndicator`](crate::widgets::effect_stack::InsertIndicator). Cleared
/// on drop or cancel; never survives into the app's model.
#[derive(Clone, Copy, Debug, Default)]
pub struct DragState {
	/// The card currently being dragged, if any.
	pub dragged: Option<EffectId>,
	/// Current insertion index (into the post-removal list) while dragging,
	/// if the pointer is over a valid drop position.
	pub insertion_index: Option<usize>,
}

/// The linear effect-stack inspector view.
///
/// Generic over the app's data-source entity `D`. Construct with
/// [`EffectStackView::new`], optionally register a parameter renderer with
/// [`EffectStackView::params_renderer`], then subscribe to
/// [`EffectStackEvent`]s on the entity.
///
/// Implements [`Render`], [`Focusable`] (for keyboard interaction) and
/// [`EventEmitter<EffectStackEvent>`].
///
/// # Interactions
///
/// - **Click card header**: emits [`EffectStackEvent::ExpansionToggled`].
/// - **Enable toggle (eye/power)**: emits [`EffectStackEvent::EnableToggled`].
/// - **Drag card by its handle**: live
///   [`InsertIndicator`](crate::widgets::effect_stack::InsertIndicator) tracks the
///   pointer; drop emits [`EffectStackEvent::ReorderRequested`]. Cards with
///   [`EffectData::is_reorderable`](crate::widgets::effect_stack::EffectData::is_reorderable)
///   `== false` (source/output by default) cannot be dragged.
/// - **Remove button**: emits [`EffectStackEvent::RemoveRequested`].
/// - **Secondary click**: emits [`EffectStackEvent::ContextMenuRequested`];
///   the app renders the actual menu.
/// - **Empty selection** ([`EffectStackDataSource::target_label`] is
///   `None`): renders an empty state and disables all interactions.
///
/// # Virtualization
///
/// The card list is rendered as a plain vertical flex column, **not** a
/// [`uniform_list`](gpui_kit::uniform_list). Effect stacks in a video editor
/// rarely exceed a few dozen cards, and expanded cards have variable,
/// content-driven heights with stateful child views (parameter UIs), which
/// a virtualized list would fight against. Revisit if profiles show
/// otherwise.
pub struct EffectStackView<D: EffectStackDataSource> {
	data: Entity<D>,
	params_renderer: Option<ParamsRenderer>,
	/// Per-effect cache of the params view: the renderer creates stateful
	/// child entities (sliders, checkboxes, text fields), so recreating the
	/// view on every render would destroy an in-progress drag or click the
	/// moment any edit notifies. Entries are created on first expansion and
	/// pruned when their effect leaves the stack; value re-syncs are the
	/// view's own job (it observes the engine).
	params_views: std::collections::HashMap<EffectId, AnyView>,
	focus_handle: FocusHandle,
	drag_state: DragState,
}

impl<D: EffectStackDataSource> EffectStackView<D> {
	/// Creates a new view over the given data-source entity.
	///
	/// The view does not subscribe to the entity itself; the app is
	/// expected to call [`cx.notify()`](Context::notify) on the data source
	/// (or on this view) after applying edits, per the "edits are requests"
	/// contract.
	pub fn new(data: Entity<D>, cx: &mut Context<Self>) -> Self {
		Self {
			data,
			params_renderer: None,
			params_views: std::collections::HashMap::new(),
			focus_handle: cx.focus_handle(),
			drag_state: DragState::default(),
		}
	}

	/// Registers the app callback that renders an effect's parameter
	/// controls inside its expanded card. See [`ParamsRenderer`].
	///
	/// Builder style; call once at setup:
	///
	/// ```ignore
	/// let stack = cx.new(|cx| {
	///     EffectStackView::new(data, cx).params_renderer(|id, window, cx| {
	///         my_effect_params_view(*id).into()
	///     })
	/// });
	/// ```
	///
	/// Cards of [`EffectCardKind::Source`](crate::widgets::effect_stack::EffectCardKind::Source)
	/// and [`Output`](crate::widgets::effect_stack::EffectCardKind::Output) never
	/// invoke the renderer — they have no parameter area.
	pub fn params_renderer(
		mut self,
		renderer: impl Fn(&EffectId, &mut Window, &mut App) -> AnyView + 'static,
	) -> Self {
		self.params_renderer = Some(Rc::new(renderer));
		self
	}

	/// The data-source entity this view reads from.
	pub fn data(&self) -> &Entity<D> {
		&self.data
	}

	/// Current transient drag state (visual only).
	pub fn drag_state(&self) -> DragState {
		self.drag_state
	}

	/// Helper for the app's parameter UIs: reports that a parameter of
	/// `effect` changed, causing the view to refresh card metadata and to
	/// emit [`EffectStackEvent::ParameterChanged`].
	///
	/// Call this from within the app's parameter view after applying a
	/// parameter edit to the engine. This is a notification of an edit the
	/// app already performed — unlike the other events, it does not require
	/// a follow-up model change.
	pub fn notify_parameter_changed(&mut self, effect: EffectId, cx: &mut Context<Self>) {
		cx.emit(EffectStackEvent::ParameterChanged { effect });
		cx.notify();
	}

	/// Updates the transient drag state while a reorder drag moves over the
	/// card `card_id`.
	///
	/// The root-level drag-move listener runs first (capture phase,
	/// registration order) and clears the insertion index, so a card's
	/// listener only needs to set it while the pointer is inside that card's
	/// own bounds — a pointer is inside at most one card, so the indicator
	/// tracks exactly the card under the pointer. When the pointer is
	/// elsewhere, this returns without touching the (already cleared) state.
	fn update_drag(
		&mut self,
		card_id: EffectId,
		event: &DragMoveEvent<EffectId>,
		cx: &mut Context<Self>,
	) {		let effects = self.data.read(cx).effects();
		let dragged = *event.drag(cx);
		self.drag_state.dragged = Some(dragged);
		if !event.bounds.contains(&event.event.position) {
			return;
		}
		let Some(i0) = effects.iter().position(|e| e.id() == dragged) else {
			self.drag_state.insertion_index = None;
			cx.notify();
			return;
		};
		let Some(j) = effects.iter().position(|e| e.id() == card_id) else {
			self.drag_state.insertion_index = None;
			cx.notify();
			return;
		};
		// The pointer is inside this card, so the indicator sits on the
		// nearer of the two edges of the card; the index is expressed in
		// the post-removal card list.
		let insert_before = event.event.position.y < event.bounds.center().y;
		let pos_in_removed = j - usize::from(j > i0);
		let new_index = if insert_before {
			pos_in_removed
		} else {
			pos_in_removed + 1
		};
		self.drag_state.insertion_index = if self.data.read(cx).can_reorder(dragged, new_index) {
			Some(new_index)
		} else {
			None
		};
		cx.notify();
	}

	/// Clears the transient drag state (mouse released outside the stack, or
	/// any gesture that should abort an in-progress drag).
	fn cancel_drag(&mut self, cx: &mut Context<Self>) {
		self.drag_state = DragState::default();
		cx.notify();
	}

	/// Tracks an effect dragged in from the app's effect library
	/// ([`LibraryEffectDrag`]): the insertion indicator follows the pointer
	/// exactly like a card reorder, except nothing is being removed (the
	/// index addresses the current card list directly).
	fn update_library_drag(
		&mut self,
		card_id: EffectId,
		event: &DragMoveEvent<LibraryEffectDrag>,
		cx: &mut Context<Self>,
	) {
		let effects = self.data.read(cx).effects();
		if !event.bounds.contains(&event.event.position) {
			return;
		}
		let Some(j) = effects.iter().position(|e| e.id() == card_id) else {
			self.drag_state.insertion_index = None;
			cx.notify();
			return;
		};
		let insert_before = event.event.position.y < event.bounds.center().y;
		self.drag_state.insertion_index = Some(if insert_before { j } else { j + 1 });
		cx.notify();
	}

	/// Toggles a card's expansion by emitting
	/// [`EffectStackEvent::ExpansionToggled`], and reports the card click as
	/// [`EffectStackEvent::CardSelected`] so hosts can sync the node-graph
	/// selection with the stack.
	fn toggle_expanded(&mut self, id: EffectId, cx: &mut Context<Self>) {
		self.drag_state = DragState::default();
		let expanded = self
			.data
			.read(cx)
			.effects()
			.iter()
			.find(|e| e.id() == id)
			.map(|e| !e.is_expanded())
			.unwrap_or(false);
		cx.emit(EffectStackEvent::ExpansionToggled {
			effect: id,
			expanded,
		});
		cx.emit(EffectStackEvent::CardSelected { effect: id });
		cx.notify();
	}

	/// Toggles a card's enabled state by emitting
	/// [`EffectStackEvent::EnableToggled`].
	fn toggle_enabled(&mut self, id: EffectId, cx: &mut Context<Self>) {
		self.drag_state = DragState::default();
		let enabled = self
			.data
			.read(cx)
			.effects()
			.iter()
			.find(|e| e.id() == id)
			.map(|e| !e.is_enabled())
			.unwrap_or(false);
		cx.emit(EffectStackEvent::EnableToggled {
			effect: id,
			enabled,
		});
		cx.notify();
	}

	/// Requests removal of a card by emitting
	/// [`EffectStackEvent::RemoveRequested`].
	fn remove(&mut self, id: EffectId, cx: &mut Context<Self>) {
		self.drag_state = DragState::default();
		cx.emit(EffectStackEvent::RemoveRequested(id));
		cx.notify();
	}

	/// Reports a secondary click on a card by emitting
	/// [`EffectStackEvent::ContextMenuRequested`].
	fn context_menu(&mut self, id: EffectId, position: Point<Pixels>, cx: &mut Context<Self>) {
		self.drag_state = DragState::default();
		cx.emit(EffectStackEvent::ContextMenuRequested {
			effect: id,
			position,
		});
		cx.notify();
	}

	/// Requests insertion of a new effect at the end of the stack by
	/// emitting [`EffectStackEvent::AddRequested`].
	fn add(&mut self, cx: &mut Context<Self>) {
		let index = self.data.read(cx).effects().len();
		cx.emit(EffectStackEvent::AddRequested { index });
		cx.notify();
	}
}

impl<D: EffectStackDataSource> EventEmitter<EffectStackEvent> for EffectStackView<D> {}

impl<D: EffectStackDataSource> Focusable for EffectStackView<D> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<D: EffectStackDataSource> Render for EffectStackView<D> {
	fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let focus_handle = self.focus_handle.clone();
		let params_renderer = self.params_renderer.clone();

		// A drag that ended without a drop (released outside a drop target, or
		// cancelled) leaves transient drag state behind; clear it on the next
		// render so no card stays half-transparent and no indicator lingers.
		if self.drag_state.dragged.is_some() && !cx.has_active_drag() {
			self.drag_state = DragState::default();
		}

		let (label, effects, selected_effect) = {
			let data = self.data.read(cx);
			(data.target_label(), data.effects(), data.selected_effect())
		};
		// Prune cached params views whose effect left the stack (remove /
		// undo); live effects keep their view across re-renders so an
		// in-progress slider drag or click survives edit notifications.
		self.params_views
			.retain(|id, _| effects.iter().any(|e| e.id() == *id));
		let insertion_index = self.drag_state.insertion_index;
		let dragged_id = self.drag_state.dragged;
		let i0 = dragged_id.and_then(|d| effects.iter().position(|e| e.id() == d));
		let i0_guard = i0.unwrap_or(usize::MAX);

		// Constructs the ghost view shown under the pointer while a card is
		// being dragged.
		let ghost_ctor = {
			let data = self.data.clone();
			move |_id: &EffectId, _origin: Point<Pixels>, _window: &mut Window, cx: &mut App| {
				let title = data
					.read(cx)
					.effects()
					.iter()
					.find(|e| e.id() == *_id)
					.map(|e| e.title())
					.unwrap_or_else(|| SharedString::from("Effect"));
				cx.new(|_cx| DragGhost { title })
			}
		};

		let mut root = div()
			.id("effect-stack")
			.flex()
			.flex_col()
			.w_full()
			.h_full()
			// The card list can outgrow the panel, so scroll vertically
			// (header and add button scroll along with the cards).
			.overflow_y_scroll()
			.track_focus(&focus_handle)
			.on_drag_move::<EffectId>(cx.listener(|this, _event, _window, cx| {
				// Runs first (capture phase, registration order): clear the
				// indicator by default; the per-card listener re-sets it
				// while the pointer is inside that card.
				this.drag_state.insertion_index = None;
				cx.notify();
			}))
			.on_drag_move::<LibraryEffectDrag>(cx.listener(|this, _event, _window, cx| {
				// Same capture-phase clear for an effect-library drag.
				this.drag_state.insertion_index = None;
				cx.notify();
			}))
			.on_mouse_up_out(
				MouseButton::Left,
				cx.listener(|this, _event, _window, cx| this.cancel_drag(cx)),
			);

		let Some(label) = label else {
			return root.child(
				div()
					.id("empty-state")
					.flex()
					.flex_1()
					.items_center()
					.justify_center()
					.text_sm()
					.text_color(colors.disabled)
					.child(crate::widgets::i18n::tr("effect_stack.empty", "No selection")),
			);
		};

		root = root.child(
			div()
				.id("effect-stack-header")
				.px_3()
				.py_2()
				.text_sm()
				.text_color(colors.text)
				.child(label),
		);

		// Cards breathe as separate rounded panels (the design's stack), not
		// as a contiguous list.
		let mut column = div()
			.id("effect-stack-cards")
			.flex()
			.flex_col()
			.w_full()
			.gap_2()
			.p_2();

		for (index, effect) in effects.iter().enumerate() {
			let id = effect.id();
			let fixed = effect.kind() != EffectCardKind::Effect;
			let enabled = effect.is_enabled();
			let expanded = effect.is_expanded();
			let removable = effect.is_removable();
			let reorderable = effect.is_reorderable();

			// Insertion indicator for a drop position just before this card.
			if let Some(p) = insertion_index {
				let k = p + usize::from(p >= i0_guard);
				if k == index {
					column = column.child(InsertIndicator::valid());
				}
			}

			// Header row: drag handle, enable toggle, the card itself
			// (flexing to fill), and the remove button. Effect cards get the
			// design's two-tone treatment: a raised header strip over the
			// darker card body.
			let mut header_row = div()
				.id(ElementId::named_usize("effect-header", id.0 as usize))
				.flex()
				.flex_row()
				.items_center()
				.gap_1()
				.px_2()
				.py_1()
				.on_aux_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
					cx.stop_propagation();
					this.context_menu(id, event.position(), cx);
				}));
			if !fixed {
				header_row = header_row.bg(colors.container);
			}

			if !fixed {
				header_row = header_row.cursor_pointer().on_click(cx.listener(
					move |this, _event, _window, cx| {
						cx.stop_propagation();
						this.toggle_expanded(id, cx);
					},
				))
				.hover(|style| {
					style.bg(crate::widgets::theme::neutral_hover(&colors))
				});
			}

			if reorderable {
				header_row = header_row.child(
					div()
						.id(ElementId::named_usize("effect-handle", id.0 as usize))
						.cursor_grab()
						.text_color(colors.disabled)
						.child("⠿")
						.on_drag(id, ghost_ctor.clone()),
				);
			}

			if !fixed {
				header_row = header_row.child(
					div()
						.id(ElementId::named_usize("effect-toggle", id.0 as usize))
						.cursor_pointer()
						.rounded_sm()
						.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
						.text_color(if enabled {
							colors.text
						} else {
							colors.disabled
						})
						.child("⏻")
						.on_click(cx.listener(move |this, _event, _window, cx| {
							cx.stop_propagation();
							this.toggle_enabled(id, cx);
						})),
				);
			}

			let card = EffectCard::new(id)
				.kind(effect.kind())
				.title(effect.title())
				.subtitle(effect.subtitle())
				.enabled(enabled)
				.expanded(expanded)
				.removable(removable)
				.reorderable(reorderable)
				.badge_count(effect.badge_count())
				.drag_ghost(dragged_id == Some(id));
			header_row = header_row.child(card);

			if removable {
				header_row = header_row.child(
					div()
						.id(ElementId::named_usize("effect-remove", id.0 as usize))
						.cursor_pointer()
						.rounded_sm()
						.hover(|style| {
							style
								.bg(crate::widgets::theme::neutral_hover(&colors))
								.text_color(colors.text)
						})
						.text_color(colors.disabled)
						.child(crate::widgets::glyphs::icon(
							crate::widgets::glyphs::Glyph::Close,
							colors.text,
						))
						.on_click(cx.listener(move |this, _event, _window, cx| {
							cx.stop_propagation();
							this.remove(id, cx);
						})),
				);
			}

			let mut wrapper = div()
				.id(ElementId::named_usize("effect", id.0 as usize))
				.flex()
				.flex_col()
				.w_full()
				.rounded_md()
				.border_1()
				.border_color(if selected_effect == Some(id) {
					colors.selected
				} else {
					colors.border
				})
				// The source card is the design's accent-tinted media bar; the
				// output card and effect bodies stay neutral.
				.bg(match effect.kind() {
					EffectCardKind::Source => {
						gpui_kit::Background::from(gpui_kit::Hsla::from(colors.selected).opacity(0.3))
					}
					EffectCardKind::Output => gpui_kit::Background::from(colors.container),
					EffectCardKind::Effect => gpui_kit::Background::from(colors.background),
				})
				.overflow_hidden();
			wrapper = wrapper.child(header_row);

			if expanded && !fixed {
				if let Some(renderer) = &params_renderer {
					let params = self
						.params_views
						.entry(id)
						.or_insert_with(|| renderer(&id, window, cx))
						.clone();
					wrapper = wrapper.child(
						div()
							.id(ElementId::named_usize("effect-params", id.0 as usize))
							.border_t_1()
							.border_color(colors.separator)
							.child(params),
					);
				}
			}

			if reorderable {
				wrapper = wrapper
					.on_drag_move::<EffectId>(cx.listener(move |this, event, _window, cx| {
						this.update_drag(id, event, cx);
					}))
					.on_drop::<EffectId>(cx.listener(
						move |this, &dragged: &EffectId, _window, cx| {
							let index = this.drag_state.insertion_index;
							this.drag_state = DragState::default();
							if let Some(index) = index {
								cx.emit(EffectStackEvent::ReorderRequested {
									effect: dragged,
									new_index: index,
								});
							}
							cx.notify();
						},
					))
					// An effect dragged in from the app's effect library
					// lands at the indicator position.
					.on_drag_move::<LibraryEffectDrag>(cx.listener(
						move |this, event, _window, cx| {
							this.update_library_drag(id, event, cx);
						},
					))
					.on_drop::<LibraryEffectDrag>(cx.listener(
						move |this, dragged: &LibraryEffectDrag, _window, cx| {
							let index = this.drag_state.insertion_index;
							this.drag_state = DragState::default();
							if let Some(index) = index {
								cx.emit(EffectStackEvent::AddTypeRequested {
									index,
									type_id: dragged.type_id.clone(),
								});
							}
							cx.notify();
						},
					))
					.can_drop(|payload, _window, _cx| {
						payload.is::<EffectId>() || payload.is::<LibraryEffectDrag>()
					});
			}

			column = column.child(wrapper);
		}

		// Insertion indicator for a drop position after the last card.
		if let Some(p) = insertion_index {
			let k = p + usize::from(p >= i0_guard);
			if k == effects.len() {
				column = column.child(InsertIndicator::valid());
			}
		}

		let add_button = div()
			.id("effect-add")
			.cursor_pointer()
			.px_3()
			.py_2()
			.rounded_md()
			.hover(|style| style.bg(crate::widgets::theme::neutral_hover(&colors)))
			.text_sm()
			.text_color(colors.text)
			.child(crate::widgets::i18n::tr("effect_stack.add", "+ Add Effect"))
			.on_click(cx.listener(move |this, _event, _window, cx| this.add(cx)));

		root.child(column).child(add_button)
	}
}

/// The floating view shown under the pointer while a card is being dragged.
struct DragGhost {
	title: SharedString,
}

impl Render for DragGhost {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<DragGhost>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div()
			.px_2()
			.py_1()
			.rounded_md()
			.bg(colors.background)
			.border_1()
			.border_color(colors.border)
			.shadow_md()
			.child(self.title.clone())
	}
}
