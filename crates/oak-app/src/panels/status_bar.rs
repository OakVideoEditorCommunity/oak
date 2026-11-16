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

//! The global status bar (状态栏): ready state, cache, proxy and the
//! library write state (M13 D4: the write-through replaces the manual save,
//! so the old autosave hint becomes "written to the library / library off /
//! write failed") on the left; current timecode / duration, frame rate and
//! resolution on the right.

use crate::widgets::timeline::Frame;
use gpui_kit::colors::DefaultColors;
use gpui_kit::{Context, Entity, Render, Window, div, prelude::*};

use crate::widgets::viewer::PlaybackClock;

use crate::oakui::AppEngine;
use crate::oakui::timecode::{format_duration, format_fps, format_resolution, format_timecode};

/// The global status bar.
pub struct StatusBar<E: AppEngine> {
	engine: Entity<E>,
	program_clock: Entity<E::Clock>,
}

impl<E: AppEngine> StatusBar<E> {
	/// Builds the status bar over the engine and the program clock.
	pub fn new(
		engine: Entity<E>,
		program_clock: Entity<E::Clock>,
		_cx: &mut Context<Self>,
	) -> Self {
		Self {
			engine,
			program_clock,
		}
	}
}

impl<E: AppEngine> Render for StatusBar<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let engine = self.engine.read(cx);
		let frame = self.program_clock.read(cx).current_frame();
		let sequence = engine.current_sequence();
		let format = sequence
			.map(|s| s.format)
			.unwrap_or(crate::oakui::VideoFormat::hd_1080p25());
		let length = sequence.map(|s| s.length).unwrap_or(Frame(0));
		let project = engine
			.project()
			.map(|p| p.name.clone())
			.unwrap_or_else(|| crate::i18n::tr("status.untitled").to_string());

		let segment = |colors: &gpui_kit::colors::Colors, text: String| {
			div().px_2().py_1().text_color(colors.text).child(text)
		};

		// The write-through state (M13 D4): a bound project with no recorded
		// error is written through; an error turns the segment red.
		let (storage_text, storage_color) = if engine.storage_last_error().is_some() {
			(
				crate::i18n::tr("status.storage.error"),
				gpui_kit::rgba(0xcc6666ff),
			)
		} else if engine.storage_bound() {
			(crate::i18n::tr("status.storage.written"), colors.disabled)
		} else {
			(crate::i18n::tr("status.storage.unbound"), colors.disabled)
		};

		// The proxy segment doubles as the in-flight proxy transcode's
		// progress readout (the C++ status bar shows the running task);
		// idle it reflects the global Use Proxy Media switch.
		let proxy_text = match engine.proxy_task_progress() {
			Some((label, progress)) => format!("{label} {}%", (progress * 100.0) as i32),
			None => crate::i18n::tr(if engine.use_proxy_media() {
				"status.proxy.on"
			} else {
				"status.proxy.off"
			})
			.into(),
		};

		div()
			.h_6()
			.flex()
			.items_center()
			.border_t_1()
			.border_color(colors.border)
			.bg(colors.container)
			.text_xs()
			.child(segment(&colors, crate::i18n::tr("status.ready").into()))
			.child(segment(&colors, crate::i18n::tr("status.cache").into()))
			.child(segment(&colors, proxy_text))
			.child(
				div()
					.px_2()
					.py_1()
					.text_color(storage_color)
					.child(storage_text),
			)
			.child(div().flex_1())
			.child(segment(
				&colors,
				format!(
					"{timecode}/{duration}",
					timecode = format_timecode(frame, format.rate),
					duration = format_duration(length, format.rate),
				),
			))
			.child(segment(&colors, format_fps(format.rate)))
			.child(segment(
				&colors,
				format_resolution(format.width, format.height),
			))
			.child(div().px_2().text_color(colors.disabled).child(project))
			.child(div().px_2().text_color(colors.disabled).child(format!(
				"{} {}",
				crate::i18n::tr("status.backend"),
				self.engine.read(cx).backend_name(),
			)))
	}
}
