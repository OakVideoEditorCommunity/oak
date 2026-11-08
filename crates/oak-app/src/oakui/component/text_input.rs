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

//! The app's text input component: gpui-kit's [`Input`] with the app
//! theme's colors applied.
//!
//! The gpui-kit input engine (IME composition, caret, selection, undo
//! history, editing key bindings) comes fully wired — `gpui_kit::init`
//! installs the keymap, so there is nothing app-side to install (the
//! fork's `EditableText` engine needed a separate binding install step;
//! that is gone).
//!
//! The wrapper exists to keep the call-site shape the app already has
//! (`text_input(id, cx).state(…)`) and to paint the field in the app
//! theme (`App::default_colors`) rather than the component theme.

use gpui_kit::base::input::InputState;
use gpui_kit::component::input::Input;
use gpui_kit::component::Sizable as _;
use gpui_kit::{
	colors::DefaultColors, div, App, AppContext, ElementId, Entity, IntoElement,
	InteractiveElement, ParentElement, RenderOnce, Styled, WeakEntity, Window,
};

/// A no-op kept for source compatibility: gpui-kit's input keymap is
/// installed by `gpui_kit::init`, so there is nothing to do here.
pub fn install_text_input_bindings(_cx: &mut App) {}

/// The app's text input component.
///
/// Wraps gpui-kit's [`Input`], defaulting to the borderless, backgroundless
/// appearance (the app's fields draw their own chrome) in the app theme's
/// text color. Build with [`text_input`], then chain `.state(..)`, …
#[derive(IntoElement)]
pub struct TextInput {
	id: ElementId,
	state: Option<WeakEntity<InputState>>,
	accepts_input: bool,
	appearance: bool,
}

impl TextInput {
	/// Bind the field to an existing input state entity (the caller reads
	/// the value / subscribes to changes through it).
	pub fn state(mut self, state: WeakEntity<InputState>) -> Self {
		self.state = Some(state);
		self
	}

	/// Whether the field accepts input at all (a disabled field renders
	/// read-only).
	pub fn accepts_input(mut self, enabled: bool) -> Self {
		self.accepts_input = enabled;
		self
	}

	/// Draw the component chrome (border + background); off by default.
	pub fn appearance(mut self, appearance: bool) -> Self {
		self.appearance = appearance;
		self
	}
}

impl RenderOnce for TextInput {
	fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let base = div().id(self.id).text_color(colors.text);
		let Some(state) = self.state.and_then(|state| state.upgrade()) else {
			return base;
		};
		base.child(
			Input::new(&state)
				.small()
				.appearance(self.appearance)
				.disabled(!self.accepts_input),
		)
	}
}

/// Builds the app's text input for `id`: a borderless, backgroundless,
/// input-accepting field with no bound state. The caller binds an
/// [`InputState`] with [`TextInput::state`]; `_cx` is unused because the
/// theme colors are read at render time, not at construction.
///
/// # Arguments
///
/// * `id` - element id for the field (anything convertible into an
///   [`ElementId`]).
/// * `_cx` - application context, kept for call-site compatibility and unused.
pub fn text_input(id: impl Into<ElementId>, _cx: &App) -> TextInput {
	TextInput {
		id: id.into(),
		state: None,
		accepts_input: true,
		appearance: false,
	}
}

/// Set an input's text from a windowless context (event subscriptions
/// and render passes have no `&mut Window`).
///
/// The update is deferred to the end of the current effect cycle:
/// calling `update_window` directly from inside window event dispatch
/// or a render pass fails silently (the window is already borrowed),
/// which used to leave the field stale when e.g. the colour picker's
/// palette refreshed its hex field mid-click. The deferred closure
/// runs once the window is back in the app map and routes the set
/// through the app's first window (the dialogs and panels using this
/// are single-window UI). `Context<T>` callers pass `cx` straight
/// through (DerefMut coercion).
pub fn set_input_text(state: &Entity<InputState>, text: &str, cx: &mut App) {
	let state = state.clone();
	let text = text.to_string();
	cx.defer(move |cx| {
		let Some(window) = cx.windows().first().copied() else {
			return;
		};
		let _ = cx.update_window(window, |_, window, cx| {
			state.update(cx, |state, cx| state.set_value(text, window, cx));
		});
	});
}
