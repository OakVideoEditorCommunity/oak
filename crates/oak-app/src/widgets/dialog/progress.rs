//! A progress dialog: a cancelable dialog with a progress bar.
//!
//! The host updates the bar via [`ProgressContent::set_progress`]; the Cancel
//! button (index `1`) emits the modal's button event.

use gpui_kit::{App, Context, Entity, Render, Window, colors::DefaultColors, div, prelude::*, px};

use super::{DialogButton, Modal, ModalOptions};

/// The content view of a progress dialog: a labeled progress bar.
pub struct ProgressContent {
	label: gpui_kit::SharedString,
	/// Progress in `0..=1`.
	fraction: f32,
}

impl ProgressContent {
	/// Creates the content view with a label and an initial progress value.
	///
	/// `label` is shown above the bar (already-localized text) and `fraction`
	/// is the starting progress in `0.0..=1.0`, clamped so an out-of-range
	/// caller cannot overflow the bar. The dialog builder always starts at
	/// `0.0` and updates through [`ProgressContent::set_progress`].
	pub fn new(label: impl Into<gpui_kit::SharedString>, fraction: f32) -> Self {
		Self {
			label: label.into(),
			fraction: fraction.clamp(0.0, 1.0),
		}
	}

	/// Updates the progress and repaints.
	///
	/// `fraction` is clamped to `0.0..=1.0` before being stored, so an
	/// out-of-range driver value cannot overflow the bar; `cx` is notified so
	/// the bar and the percentage label repaint on the next frame.
	pub fn set_progress(&mut self, fraction: f32, cx: &mut Context<Self>) {
		self.fraction = fraction.clamp(0.0, 1.0);
		cx.notify();
	}

	/// The current progress.
	pub fn fraction(&self) -> f32 {
		self.fraction
	}
}

impl Render for ProgressContent {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let fraction = self.fraction;
		div()
			.flex()
			.flex_col()
			.gap_2()
			.child(div().text_color(colors.text).child(self.label.clone()))
			.child(
				div()
					.h(px(10.0))
					.rounded_full()
					.bg(colors.background)
					.border_1()
					.border_color(colors.border)
					.overflow_hidden()
					.child(
						div()
							.h_full()
							.w(px((fraction * 100.0).clamp(0.0, 100.0)))
							.bg(colors.selected),
					),
			)
			.child(
				div()
					.text_color(colors.disabled)
					.child(format!("{:.0}%", fraction * 100.0)),
			)
	}
}

/// Builds a progress dialog: a modal with a label, a bar and Run/Cancel.
///
/// `control` is the host-assigned stable id carried by every
/// [`super::ModalEvent`] from this dialog; `title` is the modal title; `label`
/// is the text above the bar (the operation description); `window`/`cx` create
/// the views. The buttons are `DialogButton::primary("Run")` at index `0` and
/// `DialogButton::cancel("Cancel")` at index `1`. The content starts at 0% —
/// the host drives the bar with [`ProgressContent::set_progress`] and reacts to
/// the button events itself.
///
/// Returns the modal entity and its content.
pub fn progress_dialog(
	control: usize,
	title: impl Into<gpui_kit::SharedString>,
	label: impl Into<gpui_kit::SharedString>,
	window: &mut Window,
	cx: &mut App,
) -> (Entity<Modal>, Entity<ProgressContent>) {
	let content = cx.new(|_| ProgressContent::new(label, 0.0));
	let modal = cx.new(|cx| {
		Modal::new(
			control,
			ModalOptions::new(title, px(360.0))
				.with_button(DialogButton::primary("Run"))
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
	use gpui_kit::TestAppContext;

	#[test]
	fn progress_clamps_at_construction() {
		assert_eq!(ProgressContent::new("Encoding", 2.0).fraction(), 1.0);
		assert_eq!(ProgressContent::new("Encoding", -0.5).fraction(), 0.0);
	}

	#[gpui_kit::test]
	async fn set_progress_clamps(cx: &mut TestAppContext) {
		cx.update(|app| {
			let content = app.new(|_| ProgressContent::new("Encoding", 0.0));
			content.update(app, |content, cx| {
				content.set_progress(1.5, cx);
				assert_eq!(content.fraction(), 1.0);
				content.set_progress(-1.0, cx);
				assert_eq!(content.fraction(), 0.0);
			});
		});
	}

	#[gpui_kit::test]
	async fn progress_content_renders(cx: &mut TestAppContext) {
		use gpui_kit::{Entity, Render, VisualTestContext, Window, div, prelude::*, px, size};

		struct Host {
			content: Entity<ProgressContent>,
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

		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(320.0), px(80.0)), |_window, cx| {
			let content = cx.new(|_| ProgressContent::new("Encoding", 0.5));
			Host { content }
		});
		cx.run_until_parked();
		let host = window.root(cx).unwrap();
		window
			.update(cx, |host, _, cx| {
				host.content
					.update(cx, |content, cx| content.set_progress(0.75, cx));
			})
			.unwrap();
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		cx.update(|window, cx| {
			window.draw(cx).clear(cx);
		});
		let fraction = cx.read(|app| host.read(app).content.read(app).fraction());
		assert_eq!(fraction, 0.75);
	}
}
