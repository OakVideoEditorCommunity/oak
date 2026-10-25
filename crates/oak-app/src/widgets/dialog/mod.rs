//! Modal dialog framework: a mask, a titled card with a content slot and a
//! button row, plus ready-made dialogs (message box, progress, file path).
//!
//! A [`Modal`] is a view the host renders on top of its content (e.g. as the
//! last child of the root view) and focuses when shown. `escape` dismisses,
//! `enter` activates the primary button, and every button click emits
//! [`ModalEvent::ButtonClicked`] as a request.

pub mod file_dialog;
pub mod message_box;
pub mod progress;

use gpui_kit::{
	AnyView, App, ClickEvent, Context, ElementId, EventEmitter, FocusHandle, Focusable, Hsla,
	KeyDownEvent, Render, SharedString, Window, colors::DefaultColors, div, prelude::*,
};

/// How a button behaves in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogButtonRole {
	/// The default action (`enter` triggers it).
	Primary,
	/// A secondary action.
	Secondary,
	/// Cancels the dialog (`escape` also triggers it).
	Cancel,
}

/// A button in a dialog's button row.
#[derive(Debug, Clone, PartialEq)]
pub struct DialogButton {
	/// The button label.
	pub label: SharedString,
	/// The button's role.
	pub role: DialogButtonRole,
}

impl DialogButton {
	/// Create a button.
	pub fn new(label: impl Into<SharedString>, role: DialogButtonRole) -> Self {
		Self {
			label: label.into(),
			role,
		}
	}

	/// A primary button.
	pub fn primary(label: impl Into<SharedString>) -> Self {
		Self::new(label, DialogButtonRole::Primary)
	}

	/// A cancel button.
	pub fn cancel(label: impl Into<SharedString>) -> Self {
		Self::new(label, DialogButtonRole::Cancel)
	}
}

/// Configuration for a [`Modal`].
#[derive(Debug, Clone)]
pub struct ModalOptions {
	/// The title shown in the card's header.
	pub title: SharedString,
	/// The width of the card.
	pub width: gpui_kit::Pixels,
	/// The buttons in the footer row.
	pub buttons: Vec<DialogButton>,
	/// Whether a click on the dimmed backdrop dismisses the dialog
	/// (launcher-style dialogs opt in; form dialogs keep the accidental-
	/// click protection).
	pub dismiss_on_mask: bool,
}

impl ModalOptions {
	/// Create options.
	pub fn new(title: impl Into<SharedString>, width: gpui_kit::Pixels) -> Self {
		Self {
			title: title.into(),
			width,
			buttons: Vec::new(),
			dismiss_on_mask: false,
		}
	}

	/// Add a button.
	pub fn with_button(mut self, button: DialogButton) -> Self {
		self.buttons.push(button);
		self
	}

	/// Allow a backdrop click to dismiss the dialog (emits
	/// [`ModalEvent::Dismissed`] like Escape does).
	pub fn with_dismiss_on_mask(mut self, enabled: bool) -> Self {
		self.dismiss_on_mask = enabled;
		self
	}
}

/// A request emitted by a modal dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum ModalEvent {
	/// A button was clicked.
	ButtonClicked {
		/// The modal's stable id.
		control: usize,
		/// The index of the clicked button in [`ModalOptions::buttons`].
		button: usize,
	},
	/// The dialog was dismissed (escape or backdrop).
	Dismissed {
		/// The modal's stable id.
		control: usize,
	},
}

/// A modal dialog frame: mask, title bar, content slot and button row.
pub struct Modal {
	control: usize,
	options: ModalOptions,
	content: Option<AnyView>,
	focus_handle: FocusHandle,
}

impl Modal {
	/// Create a modal with `options` (no content yet).
	pub fn new(
		control: usize,
		options: ModalOptions,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		Self {
			control,
			options,
			content: None,
			focus_handle: cx.focus_handle(),
		}
	}

	/// Attach the dialog's content view.
	pub fn with_content(mut self, content: impl Into<AnyView>) -> Self {
		self.content = Some(content.into());
		self
	}

	/// Replace the content view.
	pub fn set_content(&mut self, content: impl Into<AnyView>, cx: &mut Context<Self>) {
		self.content = Some(content.into());
		cx.notify();
	}

	fn emit_button(&mut self, index: usize, cx: &mut Context<Self>) {
		cx.emit(ModalEvent::ButtonClicked {
			control: self.control,
			button: index,
		});
		cx.notify();
	}
}

impl EventEmitter<ModalEvent> for Modal {}

impl Focusable for Modal {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl Render for Modal {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let control = self.control;
		let buttons = self.options.buttons.clone();
		let primary = buttons
			.iter()
			.position(|b| b.role == DialogButtonRole::Primary);

		// Mask + card, centered.
		div()
			.id(ElementId::named_usize("gpui-widgets-modal", control))
			.absolute()
			.size_full()
			.bg(Hsla {
				h: 0.0,
				s: 0.0,
				l: 0.0,
				a: 0.4,
			})
			.occlude()
			.block_mouse_except_scroll()
			// Backdrop click dismisses when the dialog opts in (the card
			// below stops propagation, so only the true backdrop hits this).
			.on_mouse_down(
				gpui_kit::MouseButton::Left,
				cx.listener(move |this, _event: &gpui_kit::MouseDownEvent, _window, cx| {
					if this.options.dismiss_on_mask {
						let control = this.control;
						cx.emit(ModalEvent::Dismissed { control });
						cx.notify();
					}
				}),
			)
			.flex()
			.items_center()
			.justify_center()
			.track_focus(&self.focus_handle)
			.on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
				match event.keystroke.key.as_str() {
					"escape" => {
						cx.emit(ModalEvent::Dismissed {
							control: this.control,
						});
						cx.notify();
					}
					"enter" => {
						if let Some(index) = primary {
							this.emit_button(index, cx);
						}
					}
					_ => {}
				}
			}))
			.child(
				div()
					.w(self.options.width)
					.rounded_lg()
					.border_1()
					.border_color(colors.border)
					.bg(colors.container)
					.debug_selector(|| "dialog-card".into())
					.shadow_lg()
					// Clicks inside the card never reach the backdrop's
					// dismiss handler.
					.on_mouse_down(gpui_kit::MouseButton::Left, |_event, _window, cx| {
						cx.stop_propagation();
					})
					.flex()
					.flex_col()
					.child(
						div()
							.px_4()
							.py_2()
							.border_b_1()
							.border_color(colors.border)
							.text_color(colors.text)
							.flex()
							.items_center()
							.child(div().flex_1().child(self.options.title.clone()))
							.child(
								// The universal close affordance (emits
								// Dismissed, same as Escape).
								div()
									.id(ElementId::named_usize("gpui-widgets-modal-close", control))
									.debug_selector(|| "dialog-close".into())
									.px_1()
									.rounded_sm()
									.cursor_pointer()
									.text_color(colors.disabled)
									.hover(|style| {
										style
											.bg(crate::widgets::theme::neutral_hover(&colors))
											.text_color(colors.text)
									})
									.on_mouse_down(
										gpui_kit::MouseButton::Left,
										cx.listener(move |this, _event, _window, cx| {
											let control = this.control;
											cx.emit(ModalEvent::Dismissed { control });
											cx.notify();
											cx.stop_propagation();
										}),
									)
									.child(crate::widgets::glyphs::icon(
										crate::widgets::glyphs::Glyph::Close,
										colors.text,
									)),
							),
					)
					.child(
						div()
							.p_4()
							.flex_1()
							.child(if let Some(content) = &self.content {
								content.clone().into_any_element()
							} else {
								div().into_any_element()
							}),
					)
					.child(div().px_4().py_3().flex().justify_end().gap_2().children(
						buttons.into_iter().enumerate().map(|(index, button)| {
							let role = button.role;
							let label = button.label;
							let bg = match role {
								DialogButtonRole::Primary => colors.selected,
								_ => colors.background,
							};
							let hover_bg = match role {
								DialogButtonRole::Primary => crate::widgets::theme::accent_hover(&colors),
								_ => crate::widgets::theme::neutral_hover(&colors),
							};
							let text = match role {
								DialogButtonRole::Primary => colors.selected_text,
								_ => colors.text,
							};
							div()
								.id(ElementId::named_usize(
									format!("gpui-widgets-modal-button-{control}"),
									index,
								))
								.debug_selector(|| format!("dialog-button-{index}").into())
								.px_3()
								.py_1()
								.rounded_md()
								.bg(bg)
								.hover(move |style| style.bg(hover_bg))
								.text_color(text)
								.cursor_pointer()
								.on_click(cx.listener(
									move |this, _event: &ClickEvent, _window, cx| {
										this.emit_button(index, cx);
										cx.stop_propagation();
									},
								))
								.child(label)
						}),
					)),
			)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext, px, size};

	#[test]
	fn button_roles_and_primary_index() {
		let options = ModalOptions::new("Save", px(400.0))
			.with_button(DialogButton::cancel("Cancel"))
			.with_button(DialogButton::primary("Save"));
		assert_eq!(options.buttons.len(), 2);
		assert_eq!(options.buttons[0].role, DialogButtonRole::Cancel);
		assert_eq!(options.buttons[1].role, DialogButtonRole::Primary);
	}

	#[gpui_kit::test]
	async fn button_click_emits_routed_event(cx: &mut TestAppContext) {
		struct Host {
			modal: Entity<Modal>,
			events: Vec<ModalEvent>,
		}
		impl Render for Host {
			fn render(
				&mut self,
				_window: &mut Window,
				_cx: &mut Context<Self>,
			) -> impl IntoElement {
				div().size_full().child(self.modal.clone())
			}
		}

		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(500.0), px(300.0)), |window, cx| {
			let modal = cx.new(|cx| {
				Modal::new(
					1,
					ModalOptions::new("Prompt", px(360.0))
						.with_button(DialogButton::cancel("Cancel"))
						.with_button(DialogButton::primary("Apply")),
					window,
					cx,
				)
			});
			let host = Host {
				modal,
				events: Vec::new(),
			};
			cx.subscribe(
				&host.modal,
				|host: &mut Host,
				 _m: Entity<Modal>,
				 event: &ModalEvent,
				 _cx: &mut Context<Host>| {
					host.events.push(event.clone());
				},
			)
			.detach();
			host
		});
		cx.run_until_parked();
		let host = window.root(cx).unwrap();

		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		// Click the last button in the card's bottom-right corner.
		let card = cx
			.debug_bounds("dialog-card")
			.expect("dialog card rendered");
		eprintln!(
			"card={card:?} btn0={:?} btn1={:?}",
			cx.debug_bounds("dialog-button-0"),
			cx.debug_bounds("dialog-button-1")
		);
		let btn = cx.debug_bounds("dialog-button-1").expect("button rendered");
		cx.simulate_click(btn.center(), Modifiers::none());
		cx.run_until_parked();

		let routed = cx.read(|app| {
			host.read(app)
				.events
				.iter()
				.any(|e| matches!(e, ModalEvent::ButtonClicked { button: 1, .. }))
		});
		assert!(
			routed,
			"expected ButtonClicked for the Apply (index 1) button"
		);
	}
}
