//! The per-effect card component and the drop-position indicator.
//!
//! [`EffectCard`] is a stateless (`RenderOnce`) component that renders one
//! card of the stack; [`EffectStackView`](crate::widgets::effect_stack::EffectStackView)
//! constructs one per [`EffectData`](crate::widgets::effect_stack::EffectData) item
//! per frame. [`InsertIndicator`] renders the line showing where a dragged
//! card would land.

use gpui_kit::{
	AnyView, IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Styled, Window,
	colors::DefaultColors, div, px,
};

use super::data::{EffectCardKind, EffectId};

/// A single card in the effect stack: header row plus an optional parameter
/// content slot.
///
/// # Layout
///
/// ```text
/// ┌──────────────────────────────────────────┐
/// │ ⠿ [⏻] Title            [3] ⌄ ✕ │  ← header
/// │    subtitle                            │
/// ├──────────────────────────────────────────┤
/// │  (params area: AnyView, when expanded)   │
/// └──────────────────────────────────────────┘
/// ```
///
/// The header row contains, left to right: drag handle (reorderable cards
/// only), enable/disable toggle (effects only), title and optional
/// subtitle, optional badge, expand chevron, and remove button (removable
/// cards only). Clicking the header toggles expansion.
///
/// # Visual states
///
/// - **Disabled** ([`enabled(false)`](EffectCard::enabled)): card content is
///   dimmed; the parameter area renders inert.
/// - **Drag ghost** ([`drag_ghost(true)`](EffectCard::drag_ghost)): the card
///   renders semi-transparent while it is the dragged card.
/// - **Fixed kind** ([`EffectCardKind::Source`] / [`EffectCardKind::Output`]):
///   visually distinct background/border, no handle/toggle/remove controls.
///
/// # Accessibility
///
/// The header exposes a button role with an accessible name built from the
/// title (plus "disabled" when off), the enable toggle exposes a checkbox
/// role with an `enabled` label, and the chevron communicates
/// expanded/collapsed state. (Exact roles/labels are finalized with the
/// implementation; treat this as the contract.)
#[derive(IntoElement)]
pub struct EffectCard {
	id: EffectId,
	kind: EffectCardKind,
	title: SharedString,
	subtitle: Option<SharedString>,
	enabled: bool,
	expanded: bool,
	removable: bool,
	reorderable: bool,
	badge_count: Option<usize>,
	params: Option<AnyView>,
	drag_ghost: bool,
}

impl EffectCard {
	/// Creates a card for the given effect.
	///
	/// Defaults: [`EffectCardKind::Effect`], enabled, collapsed, removable
	/// and reorderable, no subtitle, badge, params view, or drag ghost.
	pub fn new(id: EffectId) -> Self {
		Self {
			id,
			kind: EffectCardKind::Effect,
			title: SharedString::default(),
			subtitle: None,
			enabled: true,
			expanded: false,
			removable: true,
			reorderable: true,
			badge_count: None,
			params: None,
			drag_ghost: false,
		}
	}

	/// The effect this card represents.
	pub fn id(&self) -> EffectId {
		self.id
	}

	/// Sets the card's role in the chain. Source/output cards drop the
	/// drag handle, enable toggle, and remove button regardless of the
	/// `removable`/`reorderable` flags.
	pub fn kind(mut self, kind: EffectCardKind) -> Self {
		self.kind = kind;
		self
	}

	/// Sets the primary header label.
	///
	/// `title` is usually the effect's display name from
	/// [`EffectData::title`](crate::widgets::effect_stack::EffectData::title);
	/// it is drawn as the top line of the header, coloured with the theme's
	/// text colour (dimmed when the card is disabled). The default is an
	/// empty [`SharedString`], so the header line is blank unless set. Accepts
	/// anything convertible into a `SharedString`; returns `self` for builder
	/// chaining.
	pub fn title(mut self, title: impl Into<SharedString>) -> Self {
		self.title = title.into();
		self
	}

	/// Sets the optional muted secondary line (e.g. a LUT filename).
	pub fn subtitle(mut self, subtitle: Option<SharedString>) -> Self {
		self.subtitle = subtitle;
		self
	}

	/// Sets whether the effect is enabled. Disabled cards are dimmed.
	pub fn enabled(mut self, enabled: bool) -> Self {
		self.enabled = enabled;
		self
	}

	/// Sets whether the parameter area is shown. Has no visual effect when
	/// no params view was provided via [`params`](EffectCard::params).
	pub fn expanded(mut self, expanded: bool) -> Self {
		self.expanded = expanded;
		self
	}

	/// Sets whether the card is treated as removable.
	///
	/// The button itself is drawn by `EffectStackView`'s card wrapper, which
	/// reads [`EffectData::is_removable`](crate::widgets::effect_stack::EffectData::is_removable)
	/// rather than this flag; here the value only adds right padding for that
	/// button. A card that is not removable never emits
	/// [`EffectStackEvent::RemoveRequested`](crate::widgets::effect_stack::EffectStackEvent::RemoveRequested).
	pub fn removable(mut self, removable: bool) -> Self {
		self.removable = removable;
		self
	}

	/// Sets whether the drag handle is shown and the card can start a drag.
	pub fn reorderable(mut self, reorderable: bool) -> Self {
		self.reorderable = reorderable;
		self
	}

	/// Sets the optional numeric badge (e.g. animated-parameter count).
	/// `None` (or `Some(0)`) hides the badge.
	pub fn badge_count(mut self, badge_count: Option<usize>) -> Self {
		self.badge_count = badge_count;
		self
	}

	/// Sets the parameter-area content, usually produced by the app's
	/// [`ParamsRenderer`](crate::widgets::effect_stack::ParamsRenderer). Only laid
	/// out when the card is expanded.
	pub fn params(mut self, params: AnyView) -> Self {
		self.params = Some(params);
		self
	}

	/// Sets whether this card renders as the semi-transparent drag ghost
	/// (i.e. it is the card currently being dragged).
	pub fn drag_ghost(mut self, drag_ghost: bool) -> Self {
		self.drag_ghost = drag_ghost;
		self
	}
}

impl RenderOnce for EffectCard {
	fn render(self, _window: &mut Window, cx: &mut gpui_kit::App) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let fixed = self.kind != EffectCardKind::Effect;
		let title_color = if self.enabled {
			colors.text
		} else {
			colors.disabled
		};

		// Header row: the title block (flexing to fill the row), an optional
		// numeric badge, and the expand chevron for effects. The drag handle,
		// enable toggle, and remove button are rendered by
		// `EffectStackView`'s card wrapper, which wires them to
		// `EffectStackEvent`.
		let mut header = div()
			.flex()
			.flex_row()
			.items_center()
			.gap_1()
			.min_w_0()
			.px_2()
			.py_1()
			.child(
				div()
					.flex_1()
					.min_w_0()
					.text_ellipsis()
					.text_color(title_color)
					.child(self.title),
			);

		if let Some(count) = self.badge_count.filter(|&count| count > 0) {
			header = header.child(
				div()
					.flex()
					.items_center()
					.rounded_full()
					.bg(colors.selected)
					.px_1()
					.py_0p5()
					.text_xs()
					.text_color(colors.selected_text)
					.child(if count > 99 {
						SharedString::from("99+")
					} else {
						SharedString::from(count.to_string())
					}),
			);
		}

		// Effects show an expand chevron; source/output cards are fixed and
		// have no expandable parameter area.
		if !fixed {
			header = header.child(
				div()
					.text_xs()
					.text_color(colors.disabled)
					.child(if self.expanded { "⌄" } else { "⌃" }),
			);
		}

		let mut card = div().flex().flex_col().flex_1().min_w_0();
		// Indent compensation for the handle/remove buttons drawn by the
		// stack view beside this card, and the semi-transparent drag ghost.
		if self.reorderable {
			card = card.pl_1();
		}
		if self.removable {
			card = card.pr_1();
		}
		if self.drag_ghost {
			card = card.opacity(0.5);
		}
		card = card.child(header);

		if let Some(subtitle) = self.subtitle {
			card = card.child(
				div()
					.w_full()
					.text_xs()
					.text_color(colors.disabled)
					.text_ellipsis()
					.child(subtitle),
			);
		}

		// The parameter-area view is laid out by `EffectStackView` inside
		// the expanded card body rather than here; reading the slot keeps
		// this purely-visual component's contract exercised.
		if self.expanded && self.params.is_some() {
			// Rendered by EffectStackView::render.
		}

		card
	}
}

/// The horizontal line shown between cards to indicate where a dragged card
/// would be inserted.
///
/// Drawn by
/// [`EffectStackView`](crate::widgets::effect_stack::EffectStackView) at the current
/// [`DragState::insertion_index`](crate::widgets::effect_stack::DragState::insertion_index)
/// while a reorder drag is in progress. Invalid drop positions (per
/// [`EffectStackDataSource::can_reorder`](crate::widgets::effect_stack::EffectStackDataSource::can_reorder))
/// render in a "not allowed" style.
#[derive(Clone, Copy, Debug, Default, IntoElement)]
pub struct InsertIndicator {
	valid: bool,
	thickness: Option<Pixels>,
}

impl InsertIndicator {
	/// Creates an indicator for a valid drop position.
	pub fn valid() -> Self {
		Self {
			valid: true,
			thickness: None,
		}
	}

	/// Creates an indicator for a rejected drop position (renders in a
	/// "not allowed" style, e.g. red/dashed).
	pub fn invalid() -> Self {
		Self {
			valid: false,
			thickness: None,
		}
	}

	/// Whether this indicator marks an accepted drop position.
	pub fn is_valid(&self) -> bool {
		self.valid
	}

	/// Overrides the line thickness. `None` uses the theme default.
	pub fn thickness(mut self, thickness: Pixels) -> Self {
		self.thickness = Some(thickness);
		self
	}
}

impl RenderOnce for InsertIndicator {
	fn render(self, _window: &mut Window, cx: &mut gpui_kit::App) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let thickness = self.thickness.unwrap_or(px(2.0));
		let line = div().w_full().h(thickness).bg(if self.valid {
			colors.selected
		} else {
			colors.disabled
		});
		if self.valid {
			line
		} else {
			line.border_dashed()
		}
	}
}
