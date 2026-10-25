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

//! Oak's custom widget library, lifted from the oak-gpui fork onto
//! gpui-kit (see docs/zh/gpui-kit-migration.md): the fork's `gpui` crate
//! additions (effect stack, node graph, timeline, the i18n string-table
//! hook) and its `gpui_widgets` library, all built on gpui-kit's public
//! API. The fork's dock is the one module not here — gpui-kit's built-in
//! `component::dock` replaces it.

pub mod audio_meter;
pub mod color_wheel;
pub mod curve_editor;
pub mod dialog;
pub mod effect_stack;
pub mod glyphs;
pub mod i18n;
pub mod icons;
pub mod keyable;
pub mod menu;
pub mod node_graph;
pub mod project_explorer;
pub mod scopes;
pub mod slider;
pub mod theme;
pub mod timeline;
pub mod tooltip;
pub mod value;
pub mod viewer;
