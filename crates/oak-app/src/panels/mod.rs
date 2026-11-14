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

//! The dockable panels of the main window.
//!
//! Each panel is a gpui-kit dock panel: `BasePanel` for identity and
//! `Panel` for the tab presentation, so it can live inside the
//! [`DockArea`](gpui_kit::component::dock::DockArea) shell. Panels
//! own their widgets (created in their constructors), hold the
//! [`MockEngine`](crate::oakui::MockEngine) entity so requests can be routed
//! to "the engine", and never mutate engine state directly — every edit is a
//! widget request event that the panel forwards through the gateway.

pub mod ai_assistant;
pub mod commands;
pub mod effect_library;
pub mod history;
pub mod inspector;
pub mod multicam;
pub mod node_editor;
pub mod ofx_params;
pub mod program_viewer;
pub mod project_explorer;
pub mod source_viewer;
pub mod status_bar;
pub mod timeline;

/// A dock panel's identity: its `BasePanel::panel_name` (the layout's
/// persistence key, the 窗口 menu's toggle target).
pub type PanelId = &'static str;

/// Emitted by a panel when a click inside it makes it the focused panel;
/// the shell subscribes per panel (the subscribing entity identifies the
/// panel) and routes focused-panel commands to it.
pub struct PanelFocused;

/// Stable panel ids, unique within the dock area.
pub mod ids {
	use super::PanelId;
	/// The material bin (项目).
	pub const PROJECT: PanelId = "project";
	/// The source viewer (素材查看器).
	pub const SOURCE_VIEWER: PanelId = "source-viewer";
	/// The program viewer (序列查看器).
	pub const PROGRAM_VIEWER: PanelId = "program-viewer";
	/// The node editor (节点编辑器).
	pub const NODE_EDITOR: PanelId = "node-editor";
	/// The inspector / effect stack (检查器·效果栈).
	pub const INSPECTOR: PanelId = "inspector";
	/// The undo history (历史记录).
	pub const HISTORY: PanelId = "history";
	/// The timeline (时间线).
	pub const TIMELINE: PanelId = "timeline";
	/// The effect library (效果库).
	pub const EFFECT_LIBRARY: PanelId = "effect-library";
	/// The multicam panel (多机位).
	pub const MULTICAM: PanelId = "multicam";
	/// The AI assistant panel (AI 助手).
	pub const AI_ASSISTANT: PanelId = "ai-assistant";
}

/// A small info chip used in viewer headers and the status bar: muted
/// background, thin border, small text. `colors` comes from the caller's
/// `cx.default_colors()` so the chip follows the active theme.
pub(crate) fn chip(colors: &gpui_kit::colors::Colors, label: impl gpui_kit::IntoElement) -> gpui_kit::Div {
	use gpui_kit::prelude::*;
	gpui_kit::div()
		.px_2()
		.py_1()
		.rounded_sm()
		.border_1()
		.border_color(colors.border)
		.bg(colors.container)
		.text_color(colors.text)
		.child(label)
}

/// The viewer panel title, per the design: `<面板>·<素材/序列名>` in zh-CN,
/// `<Panel> · <name>` everywhere else — no redundant "Source"/"Program"
/// placeholder suffix. The panel key is the localized panel name; `name` is
/// the media or sequence name (data, not translated).
pub(crate) fn viewer_title(panel_key: &'static str, name: &str) -> String {
	if name.is_empty() {
		return crate::i18n::tr(panel_key).to_owned();
	}
	// CJK locales read better without the spaced separator.
	if crate::i18n::language_code() == "zh-CN" {
		format!("{}·{}", crate::i18n::tr(panel_key), name)
	} else {
		format!("{} · {}", crate::i18n::tr(panel_key), name)
	}
}
