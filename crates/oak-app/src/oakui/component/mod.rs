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

//! The app's reusable UI components — the presentation layer above raw
//! gpui. Components own their theme wiring (colors come from the app
//! theme, not hard-coded defaults), their key bindings and their
//! interaction rules, so call sites stay declarative:
//!
//! ```
//! use crate::oakui::component::{text_input, TextInput};
//!
//! // in a render():
//! child(
//!     text_input("my-field", window, cx)
//!         .state(self.value.downgrade())
//!         .accepts_input(true),
//! )
//! ```
//!
//! Submodules:
//! - [`text_input`]: the text field (IME, caret/selection, editing keys).
//! - [`menu`]: menus (menu bar entries and context menus).

pub mod controls;
pub mod menu;
pub mod text_input;

pub use controls::{
	CheckBox, CheckBoxEvent, CheckState, ComboBox, ComboBoxEvent, ComboBoxOption, OfxColorPicker,
	Slider, SliderEvent, SliderModel, SliderValue, SpinBox, SpinBoxEvent, ValueKind,
};
pub use menu::{ContextMenu, Menu, MenuBar, MenuItem};
pub use text_input::{TextInput, install_text_input_bindings, text_input};
