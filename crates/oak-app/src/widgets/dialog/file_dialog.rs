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

//! A file dialog: a modal with a path field and OK/Cancel buttons.
//!
//! The path field is gpui-kit's [`Input`] (IME, selection, undo included);
//! the modal chrome stays this module's own. It emits
//! [`ModalEvent::ButtonClicked`] with the path available from
//! [`FileDialogContent::path`]; a host can back it with a real platform
//! picker later.

use gpui_kit::base::input::InputState;
use gpui_kit::component::input::Input;
use gpui_kit::{
	App, Context, Entity, Render, SharedString, Window, colors::DefaultColors, div, prelude::*, px,
};

use super::{DialogButton, Modal, ModalOptions};

/// The content view of a file dialog: a path text field.
pub struct FileDialogContent {
	editor: Entity<InputState>,
}

impl FileDialogContent {
	/// Builds the content view over a fresh, empty input state.
	///
	/// `window`/`cx` create the `InputState`, which supplies IME, selection and
	/// undo handling from gpui-kit. Prefill it afterwards with
	/// [`FileDialogContent::set_path`].
	pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		Self {
			editor: cx.new(|cx| InputState::new(window, cx)),
		}
	}

	/// The path currently entered in the field.
	///
	/// `app` reads the input state's value as a [`SharedString`] (empty when the
	/// field is empty). The host reads it on the OK click and owns the decision
	/// whether the path is acceptable.
	pub fn path(&self, app: &App) -> SharedString {
		self.editor.read(app).value()
	}

	/// Sets the path shown in the field.
	///
	/// `path` replaces the whole input value (selection and cursor move to the
	/// end, via `InputState::set_value`), `window` is forwarded to the input
	/// state and `cx` is notified so the dialog repaints. Used to prefill the
	/// dialog, e.g. with the last used directory.
	pub fn set_path(
		&mut self,
		path: impl Into<SharedString>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let path = path.into();
		self.editor.update(cx, |editor, cx| {
			editor.set_value(path, window, cx);
		});
		cx.notify();
	}
}

impl Render for FileDialogContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		div()
			.flex()
			.flex_col()
			.gap_2()
			.child(div().text_color(colors.text).child("Path"))
			.child(
				div()
					.rounded_md()
					.border_1()
					.border_color(colors.border)
					.bg(colors.background)
					.child(Input::new(&self.editor).appearance(false)),
			)
	}
}

/// Builds a file dialog: a modal with a path field and OK/Cancel buttons.
///
/// `control` is the host-assigned stable id carried by every
/// [`super::ModalEvent`] from this dialog, so one handler can tell dialogs
/// apart; `title` is the modal title; `window`/`cx` create the views. The
/// buttons are `DialogButton::primary("Open")` at index `0` and
/// `DialogButton::cancel("Cancel")` at index `1`. The host reads the chosen
/// path from [`FileDialogContent::path`] when the OK button is clicked.
///
/// Returns the modal entity and its content, so the host can prefill or
/// inspect the field.
pub fn file_dialog(
	control: usize,
	title: impl Into<SharedString>,
	window: &mut Window,
	cx: &mut App,
) -> (Entity<Modal>, Entity<FileDialogContent>) {
	let content = cx.new(|cx| FileDialogContent::new(window, cx));
	let modal = cx.new(|cx| {
		Modal::new(
			control,
			ModalOptions::new(title, px(420.0))
				.with_button(DialogButton::primary("Open"))
				.with_button(DialogButton::cancel("Cancel")),
			window,
			cx,
		)
		.with_content(content.clone())
	});
	(modal, content)
}

#[cfg(test)]
mod tests {
	use super::*;
	use gpui_kit::{Entity, Render, TestAppContext, VisualTestContext, Window, div, px, size};

	#[gpui_kit::test]
	async fn path_round_trips(cx: &mut TestAppContext) {
		struct Host {
			content: Entity<FileDialogContent>,
		}
		impl Render for Host {
			fn render(
				&mut self,
				_window: &mut Window,
				_cx: &mut Context<Self>,
			) -> impl IntoElement {
				div().size_full().child(self.content.clone())
			}
		}

		cx.update(|cx| {
			gpui_kit::init(cx);
			cx.init_colors();
		});
		let mut captured = None;
		let window = cx.open_window(size(px(400.0), px(120.0)), |window, cx| {
			let content = cx.new(|cx| FileDialogContent::new(window, cx));
			captured = Some(content.clone());
			Host { content }
		});
		cx.run_until_parked();
		let content = captured.unwrap();
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		cx.update(|window, cx| {
			content.update(cx, |content, cx| {
				content.set_path("/tmp/movie.mov", window, cx);
				assert_eq!(content.path(cx), "/tmp/movie.mov");
			});
		});
	}

	#[gpui_kit::test]
	async fn file_dialog_content_renders(cx: &mut TestAppContext) {
		struct Host {
			content: Entity<FileDialogContent>,
		}
		impl Render for Host {
			fn render(
				&mut self,
				_window: &mut Window,
				_cx: &mut Context<Self>,
			) -> impl IntoElement {
				div().size_full().child(self.content.clone())
			}
		}

		cx.update(|cx| {
			gpui_kit::init(cx);
			cx.init_colors();
		});
		let window = cx.open_window(size(px(400.0), px(120.0)), |window, cx| {
			let content = cx.new(|cx| FileDialogContent::new(window, cx));
			content.update(cx, |content, cx| content.set_path("/tmp/movie.mov", window, cx));
			Host { content }
		});
		cx.run_until_parked();
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		cx.update(|window, cx| {
			window.draw(cx).clear(cx);
		});
	}
}
