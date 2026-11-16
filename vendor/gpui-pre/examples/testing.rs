#![cfg_attr(target_family = "wasm", no_main)]

//! Example demonstrating GPUI's testing infrastructure.
//!
//! When run normally, this displays an interactive counter window.
//! The tests below demonstrate various GPUI testing patterns.
//!
//! Run the app: cargo run -p gpui --example testing
//! Run tests:   cargo test -p gpui --example testing --features test-support

#[path = "example_support/fonts.rs"]
mod example_support;

use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, Render, Task, Window, WindowBounds,
    WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use gpui_platform::application;

actions!(counter, [Increment, Decrement]);

struct Counter {
    count: i32,
    focus_handle: FocusHandle,
    _subscription: gpui::Subscription,
}

/// Event emitted by Counter
struct CounterEvent;

impl gpui::EventEmitter<CounterEvent> for Counter {}

impl Counter {
    fn new(cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe_self(|this: &mut Self, _event: &CounterEvent, _cx| {
            this.count = 999;
        });

        Self {
            count: 0,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn increment(&mut self, _: &Increment, _window: &mut Window, cx: &mut Context<Self>) {
        self.count += 1;
        cx.notify();
    }

    fn decrement(&mut self, _: &Decrement, _window: &mut Window, cx: &mut Context<Self>) {
        self.count -= 1;
        cx.notify();
    }

    fn load(&self, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            // Simulate loading data (e.g., from disk or network)
            this.update(cx, |counter, _| {
                counter.count = 100;
            })
            .ok();
        })
    }

    fn reload(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // Simulate reloading data in the background
            this.update(cx, |counter, _| {
                counter.count += 50;
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for Counter {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("counter")
            .key_context("Counter")
            .on_action(cx.listener(Self::increment))
            .on_action(cx.listener(Self::decrement))
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .gap_4()
            .bg(rgb(0x1e1e2e))
            .size_full()
            .justify_center()
            .items_center()
            .child(
                div()
                    .text_3xl()
                    .text_color(rgb(0xcdd6f4))
                    .child(format!("{}", self.count)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("decrement")
                            .px_4()
                            .py_2()
                            .bg(rgb(0x313244))
                            .hover(|s| s.bg(rgb(0x45475a)))
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(0xcdd6f4))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.decrement(&Decrement, window, cx)
                            }))
                            .child("−"),
                    )
                    .child(
                        div()
                            .id("increment")
                            .px_4()
                            .py_2()
                            .bg(rgb(0x313244))
                            .hover(|s| s.bg(rgb(0x45475a)))
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(0xcdd6f4))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.increment(&Increment, window, cx)
                            }))
                            .child("+"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("load")
                            .px_4()
                            .py_2()
                            .bg(rgb(0x313244))
                            .hover(|s| s.bg(rgb(0x45475a)))
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(0xcdd6f4))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load(cx).detach();
                            }))
                            .child("Load"),
                    )
                    .child(
                        div()
                            .id("reload")
                            .px_4()
                            .py_2()
                            .bg(rgb(0x313244))
                            .hover(|s| s.bg(rgb(0x45475a)))
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(0xcdd6f4))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reload(cx);
                            }))
                            .child("Reload"),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x6c7086))
                    .child("Press ↑/↓ or click buttons"),
            )
    }
}

fn run_example() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        cx.bind_keys([
            gpui::KeyBinding::new("up", Increment, Some("Counter")),
            gpui::KeyBinding::new("down", Decrement, Some("Counter")),
        ]);

        let bounds = Bounds::centered(None, size(px(300.), px(200.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let counter = cx.new(|cx| Counter::new(cx));
                counter.focus_handle(cx).focus(window, cx);
                counter
            },
        )
        .unwrap();
    });
}

#[cfg(not(target_family = "wasm"))]
fn main() {
    run_example();
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    gpui_platform::web_init();
    run_example();
}

#[cfg(test)]
mod tests;
