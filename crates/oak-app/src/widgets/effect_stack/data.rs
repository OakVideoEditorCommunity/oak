//! Data-source traits and metadata types for the effect stack.
//!
//! The app implements [`EffectStackDataSource`] on an entity it owns and
//! returns one [`EffectData`] object per card, ordered top-to-bottom in
//! signal order. See the [module-level docs](crate::widgets::effect_stack) for the
//! overall architecture.

use std::fmt;
use std::sync::Arc;

use gpui_kit::SharedString;

/// Stable identifier of a single effect (card) in the stack.
///
/// The app assigns IDs; the widget treats them as opaque keys used for card
/// identity across frames (element IDs, drag payloads) and in
/// [`EffectStackEvent`](crate::widgets::effect_stack::EffectStackEvent)s.
///
/// # Invariants
///
/// - IDs must be unique within one stack and stable for the lifetime of the
///   underlying effect node — the view diffs card lists by `EffectId` to
///   keep element state (expansion animation, focus) attached to the right
///   card across re-renders.
/// - In Oak, an `EffectId` typically wraps (or maps 1:1 to) the engine's
///   node ID on the graph path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EffectId(pub u64);

impl fmt::Display for EffectId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "effect-{}", self.0)
	}
}

impl From<u64> for EffectId {
	fn from(raw: u64) -> Self {
		Self(raw)
	}
}

/// Drag payload of an effect dragged out of the app's effect library onto
/// an effect stack (or node graph): the factory/plugin type id to insert
/// plus the display name (used by the drag ghost). Dropping it on a stack
/// emits [`EffectStackEvent::AddTypeRequested`](crate::widgets::effect_stack::EffectStackEvent::AddTypeRequested).
#[derive(Clone, Debug)]
pub struct LibraryEffectDrag {
	/// The addable-effect type id (factory entry or OFX plugin identifier).
	pub type_id: SharedString,
	/// The display name, shown on the drag ghost.
	pub name: SharedString,
}

/// Which role a card plays in the linear chain.
///
/// A well-formed stack is exactly one [`Source`](EffectCardKind::Source)
/// card pinned at the top, zero or more [`Effect`](EffectCardKind::Effect)
/// cards in the middle, and exactly one [`Output`](EffectCardKind::Output)
/// card pinned at the bottom. The view renders source/output cards with
/// fixed styling and ignores reorder/remove gestures for them regardless of
/// what [`EffectData::is_removable`] / [`EffectData::is_reorderable`] say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectCardKind {
	/// The media/source card (e.g. the clip's footage). Fixed at the top of
	/// the stack; not removable, not reorderable.
	Source,
	/// A regular effect card. Reorderable and removable by default.
	Effect,
	/// The output card. Fixed at the bottom of the stack; not removable,
	/// not reorderable.
	Output,
}

/// Read-only view of one card in the effect stack.
///
/// The app returns implementations of this trait from
/// [`EffectStackDataSource::effects`]. All methods are pure reads — the view
/// calls them on every render, so they must be cheap and must not mutate.
///
/// Mutations always flow the other way: the user gestures produce
/// [`EffectStackEvent`](crate::widgets::effect_stack::EffectStackEvent)s, the app
/// applies them to its model, and the next render observes the new values
/// here.
pub trait EffectData: 'static {
	/// Stable identity of this card; see [`EffectId`] for invariants.
	fn id(&self) -> EffectId;

	/// The role of this card in the chain. See [`EffectCardKind`].
	fn kind(&self) -> EffectCardKind;

	/// Primary label shown in the card header (e.g. `"Transform"`,
	/// `"OCIO LUT"`).
	fn title(&self) -> SharedString;

	/// Optional secondary line shown under the title in a muted style, e.g.
	/// the LUT filename or a one-line parameter summary.
	///
	/// Defaults to `None` (no subtitle row is laid out).
	fn subtitle(&self) -> Option<SharedString> {
		None
	}

	/// Whether the effect is currently enabled (not bypassed).
	///
	/// Disabled cards are rendered dimmed and their parameter area is
	/// inert. Source and output cards should return `true`; the view does
	/// not render an enable toggle for them.
	fn is_enabled(&self) -> bool;

	/// Whether the card's parameter area is currently expanded.
	///
	/// Expansion state lives in the app's model (so it can persist and sync
	/// with [`crate::widgets::node_graph`]); toggling it is requested via
	/// [`EffectStackEvent::ExpansionToggled`](crate::widgets::effect_stack::EffectStackEvent::ExpansionToggled).
	fn is_expanded(&self) -> bool;

	/// Whether the card may be removed from the stack.
	///
	/// Defaults to `true` for [`EffectCardKind::Effect`] and `false` for
	/// source/output cards. When `false`, the remove affordance is hidden
	/// and [`EffectStackEvent::RemoveRequested`](crate::widgets::effect_stack::EffectStackEvent::RemoveRequested)
	/// is never emitted for this card.
	fn is_removable(&self) -> bool {
		self.kind() == EffectCardKind::Effect
	}

	/// Whether the card may be reordered by dragging.
	///
	/// Defaults to `true` for [`EffectCardKind::Effect`] and `false` for
	/// source/output cards. When `false`, the drag handle is hidden and the
	/// card never participates in a reorder (neither as the dragged card
	/// nor as a displaced neighbor position).
	fn is_reorderable(&self) -> bool {
		self.kind() == EffectCardKind::Effect
	}

	/// Optional numeric badge shown in the card header, e.g. the number of
	/// animated parameters on the effect.
	///
	/// `Some(0)` and `None` both render no badge; prefer `None`. Large
	/// values are clamped visually (e.g. `99+`).
	fn badge_count(&self) -> Option<usize> {
		None
	}
}

/// Ordered collection of effect cards backing an [`EffectStackView`](crate::widgets::effect_stack::EffectStackView).
///
/// Implemented by the app on an [`Entity`](gpui_kit::Entity)-backed model. The
/// view holds the entity and reads through this trait every frame.
///
/// # Cardinality and ordering
///
/// [`effects`](EffectStackDataSource::effects) returns cards top-to-bottom
/// in signal order: index `0` is the source card, the last index is the
/// output card. Reorder indices in
/// [`EffectStackEvent::ReorderRequested`](crate::widgets::effect_stack::EffectStackEvent::ReorderRequested)
/// and [`can_reorder`](EffectStackDataSource::can_reorder) refer to
/// positions in this list.
pub trait EffectStackDataSource: 'static {
	/// All cards in the stack, top-to-bottom (signal order).
	///
	/// May be empty (or contain only source/output) — see
	/// [`target_label`](EffectStackDataSource::target_label) for the
	/// empty-selection state, which is distinct from a stack with no
	/// effects.
	fn effects(&self) -> Vec<Arc<dyn EffectData>>;

	/// Label describing what this stack edits, e.g. the clip name shown in
	/// the panel header.
	///
	/// Return `None` when there is no valid selection (no clip under the
	/// playhead, multi-selection, etc.). The view then renders an empty
	/// state instead of cards and suppresses all card interactions.
	fn target_label(&self) -> Option<SharedString>;

	/// The card the stack should highlight as selected — the effect whose
	/// node is selected in the node graph, when the two views are linked.
	/// `None` means no card is highlighted. Defaults to `None`.
	///
	/// This is a read-only selection mirror; the view never mutates it.
	fn selected_effect(&self) -> Option<EffectId> {
		None
	}

	/// Whether dropping effect `id` at `new_index` — an index into the list
	/// returned by [`effects`](EffectStackDataSource::effects) — would be a
	/// valid reorder.
	///
	/// `id` is the effect being dragged and `new_index` the target slot. Called
	/// continuously during a drag to drive the insertion indicator; invalid
	/// positions render as "not allowed". The default implementation allows
	/// every position; apps override it to reject reorders that would produce
	/// an invalid signal chain (e.g. a node that requires two inputs).
	///
	/// Advisory UI feedback only — the app re-validates when the actual
	/// [`ReorderRequested`](crate::widgets::effect_stack::EffectStackEvent::ReorderRequested)
	/// event arrives.
	fn can_reorder(&self, _id: EffectId, _new_index: usize) -> bool {
		true
	}
}
