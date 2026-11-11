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

//! The AI assistant panel (AI 助手): a chat surface over the built-in agent
//! (T8.1's [`crate::ai`] module). The user types a line, the agent runs its
//! tool loop on the background executor — the UI thread never blocks on the
//! LLM's HTTP call — and the reply plus every tool call land as bubbles.
//!
//! Key discipline: the API key is read from the process environment
//! (`OAK_AI_API_KEY`) only at send time, never stored in the panel, never
//! logged. Without a key the panel renders the setup guide and the send
//! path refuses instead of panicking.

use crate::ai::{
	Agent, AgentConfig, AgentError, AgentReport, AiConfig, MockBackend, UreqClient,
};
use crate::oakui::component::text_input::{set_input_text, text_input};
use crate::oakui::AppEngine;
use gpui_kit::base::input::InputState;
use gpui_kit::colors::{Colors, DefaultColors};
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::input::InputEvent;
use gpui_kit::component::text::TextView;
use gpui_kit::{
	div, prelude::*, AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, EventEmitter,
	FocusHandle, Focusable, Render, Window,
};

// ---------------------------------------------------------------------------
// Chat model — pure data, unit-tested without a window.
// ---------------------------------------------------------------------------

/// One chat entry, rendered as a bubble.
#[derive(Debug, Clone, PartialEq)]
pub enum Bubble {
	/// The user's line.
	User(String),
	/// The agent's final answer.
	Assistant(String),
	/// One executed tool call: name, outcome, model-facing summary.
	Tool {
		name: String,
		outcome: &'static str,
		detail: String,
	},
	/// A system notice (round cap, empty answer).
	System(String),
}

/// The i18n key for a structured agent error code — the codes are stable
/// (`crate::ai`), the wording is the panel's locale.
pub fn error_i18n_key(code: &str) -> &'static str {
	match code {
		"AI_NOT_CONFIGURED" => "ai.error.not_configured",
		"AI_HTTP_ERROR" => "ai.error.http",
		"AI_API_ERROR" => "ai.error.api",
		"AI_BAD_RESPONSE" => "ai.error.bad_response",
		_ => "ai.error.unknown",
	}
}

/// The i18n key for a tool outcome recorded by the loop.
pub fn outcome_i18n_key(outcome: &str) -> &'static str {
	match outcome {
		"applied" => "ai.outcome.applied",
		"proposed" => "ai.outcome.proposed",
		"rejected" => "ai.outcome.rejected",
		"failed" => "ai.outcome.failed",
		_ => "ai.error.unknown",
	}
}

/// Whether the send button is enabled: v1 allows exactly one run at a time
/// and needs a key plus a non-empty draft.
pub fn can_send(busy: bool, configured: bool, draft_nonempty: bool) -> bool {
	!busy && configured && draft_nonempty
}

/// Strips Markdown markers so a bubble can be copied as readable plain text.
///
/// This is the only place the panel parses Markdown by hand: rendering is
/// `TextView::markdown`'s job, and the copy button only needs the text a
/// reader would see — tool names stay bare (`project.open`, not
/// ``` `project.open` ```), fences/link syntax disappear, list and heading
/// markers go. Unknown or unbalanced syntax degrades to the literal text.
pub fn markdown_to_plain(source: &str) -> String {
	let mut out: Vec<String> = Vec::new();
	let mut fence: Option<String> = None;

	for line in source.lines() {
		let trimmed = line.trim_start();
		if let Some(open) = fence.as_deref() {
			// Inside a fence: keep the body verbatim, close on the same
			// fence marker (``` or ~~~, any length >= 3).
			if trimmed.starts_with(open) {
				fence = None;
			} else {
				out.push(line.to_string());
			}
			continue;
		}
		if let Some(marker) = fence_marker(trimmed) {
			fence = Some(marker);
			continue;
		}
		out.push(strip_inline(&strip_block_prefix(trimmed)));
	}

	// Collapse runs of blank lines; keep inner single blanks as paragraph
	// breaks, and never return leading/trailing blank lines.
	let mut cleaned: Vec<String> = Vec::with_capacity(out.len());
	for line in out {
		if line.trim().is_empty() {
			if cleaned.last().is_some_and(|prev: &String| !prev.trim().is_empty()) {
				cleaned.push(String::new());
			}
		} else {
			cleaned.push(line);
		}
	}
	while cleaned.last().is_some_and(|last| last.trim().is_empty()) {
		cleaned.pop();
	}
	cleaned.join("\n")
}

/// The fence marker (` ``` ` or ` ~~~ `, at least three) that starts a code
/// block, if this line is one.
fn fence_marker(trimmed: &str) -> Option<String> {
	for ch in ['`', '~'] {
		let count = trimmed.chars().take_while(|c| *c == ch).count();
		if count >= 3 {
			return Some(ch.to_string().repeat(count));
		}
	}
	None
}

/// Drops ATX heading markers (`#`…`######` plus one space) and list markers
/// (`-`, `*`, `+`, `1.`, `1)`).
fn strip_block_prefix(trimmed: &str) -> &str {
	let hashes = trimmed.chars().take_while(|c| *c == '#').count();
	if (1..=6).contains(&hashes) {
		let rest = &trimmed[hashes..];
		if let Some(stripped) = rest.strip_prefix(' ') {
			return stripped;
		}
	}
	for bullet in ["- ", "* ", "+ "] {
		if let Some(stripped) = trimmed.strip_prefix(bullet) {
			return stripped;
		}
	}
	// Ordered list: digits followed by `.` or `)` and a space.
	let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
	if digits > 0 && digits <= 3 {
		let rest = &trimmed[digits..];
		if let Some(stripped) = rest
			.strip_prefix(". ")
			.or_else(|| rest.strip_prefix(") "))
		{
			return stripped;
		}
	}
	trimmed
}

/// Removes inline emphasis/code/link markers from one line, keeping the text.
fn strip_inline(line: &str) -> String {
	let mut out = String::with_capacity(line.len());
	let mut chars = line.char_indices().peekable();
	while let Some((ix, ch)) = chars.next() {
		match ch {
			'`' => {
				// Drop the opening run, keep the code text, drop the closer.
				let run = 1 + line[ix + 1..].chars().take_while(|c| *c == '`').count();
				let opener_rest = &line[ix + run..];
				let closer = "`".repeat(run);
				match opener_rest.find(&closer) {
					Some(end) => {
						out.push_str(&opener_rest[..end]);
						let consumed = ix + run + end + run;
						while let Some((next_ix, _)) = chars.peek() {
							if *next_ix < consumed {
								chars.next();
							} else {
								break;
							}
						}
					}
					// Unbalanced: keep the literal backticks.
					None => out.push_str(&"`".repeat(run)),
				}
			}
			'*' | '_' | '~' => {
				// Strikethrough needs `~~`; bold/italic markers just vanish.
				let run = 1 + line[ix + 1..].chars().take_while(|c| *c == ch).count();
				let keep = if ch == '~' && run == 1 { "~" } else { "" };
				out.push_str(keep);
				for _ in 1..run {
					chars.next();
				}
			}
			'[' => {
				// `[label](url)` → `label`; a bare `[` stays literal.
				// `consumed` counts the WHOLE pattern, so the iterator is
				// advanced by the same amount that was skipped — deriving
				// indices from the tail (`find` offsets relative to `rest`)
				// mis-shifted and left `url)` in the output.
				let rest = &line[ix + 1..];
				match (rest.find(']'), rest.find("](")) {
					(Some(close), Some(open_paren)) if close == open_paren => {
						let label = &rest[..close];
						let after = &rest[close..];
						match after.find(')') {
							Some(paren_close) => {
								out.push_str(label);
								let consumed = ix + 1 + (close + 1) + paren_close + 1;
								while let Some((next_ix, _)) = chars.peek() {
									if *next_ix < consumed {
										chars.next();
									} else {
										break;
									}
								}
							}
							None => out.push('['),
						}
					}
					_ => out.push('['),
				}
			}
			_ => out.push(ch),
		}
	}
	out
}

/// Appends a finished run to the transcript: tool calls first (in call
/// order), then the final answer — or the round-cap notice when the loop
/// ran out of rounds.
pub fn append_report(bubbles: &mut Vec<Bubble>, report: &AgentReport) {
	for entry in &report.tools {
		bubbles.push(Bubble::Tool {
			name: entry.tool.clone(),
			outcome: entry.outcome,
			detail: entry.detail.clone(),
		});
	}
	match &report.final_text {
		Some(text) if !text.trim().is_empty() => {
			bubbles.push(Bubble::Assistant(text.clone()));
		}
		_ if !report.converged => {
			bubbles.push(Bubble::System(
				crate::i18n::tr("ai.notice.limit").to_owned(),
			));
		}
		_ => {}
	}
}

/// The panel's own agent handle plus the run result that came back from the
/// background executor.
type AgentOutcome = (
	Agent<UreqClient, MockBackend>,
	Result<AgentReport, AgentError>,
);

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

/// The AI assistant panel: transcript, input row, busy/error status strip.
pub struct AiAssistantPanel<E: AppEngine> {
	/// The dock's focus handle (zoom/focus + shell routing).
	focus_handle: FocusHandle,
	/// Held for type parity with the other shell panels; the real-engine
	/// backend plugs in here when the assembly point provides one.
	#[allow(dead_code)]
	engine: Entity<E>,
	/// The transcript, oldest first.
	bubbles: Vec<Bubble>,
	/// The composer. Read live from here (`.value()`), not mirrored.
	input: Entity<InputState>,
	/// One run at a time (v1: the send button greys out).
	busy: bool,
	/// The last structured error code, mapped through [`error_i18n_key`].
	error: Option<String>,
	/// Whether `OAK_AI_API_KEY` was present at panel construction.
	configured: bool,
	/// The live conversation, moved into a run and handed back after.
	agent: Option<Agent<UreqClient, MockBackend>>,
	/// The bubble index whose 「已复制」 confirmation is showing.
	copied: Option<usize>,
}

impl<E: AppEngine> AiAssistantPanel<E> {
	/// Builds the panel; `configured` snapshots the key's presence so the
	/// setup guide renders without ever reading the key itself.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let input = cx.new(|cx| InputState::new(window, cx));
		// Repaint on every keystroke so the send button's enabled state
		// tracks the draft (the value itself is read at render time).
		cx.subscribe(&input, |_this, _state, event: &InputEvent, cx| {
			if matches!(event, InputEvent::Change) {
				cx.notify();
			}
		})
		.detach();
		Self {
			focus_handle: cx.focus_handle(),
			engine,
			bubbles: Vec::new(),
			input,
			busy: false,
			error: None,
			configured: AiConfig::from_env().is_ok(),
			agent: None,
			copied: None,
		}
	}

	/// Sends the draft. Refuses (no panic, no queue) while a run is active,
	/// when the key is missing, or when the draft is blank; otherwise clears
	/// the composer, pushes the user bubble, and runs the agent on the
	/// background executor — same shape as the shell's update check, so the
	/// first frame never waits on the network.
	fn send(&mut self, cx: &mut Context<Self>) {
		let text = self.input.read(cx).value().trim().to_string();
		if !can_send(self.busy, self.configured, !text.is_empty()) {
			return;
		}
		// The key is read here, into memory only; a config error surfaces as
		// a structured code instead of a panic.
		let config = match AiConfig::from_env() {
			Ok(config) => config,
			Err(err) => {
				self.error = Some(err.code().to_string());
				cx.notify();
				return;
			}
		};
		self.bubbles.push(Bubble::User(text.clone()));
		set_input_text(&self.input, "", cx);
		self.busy = true;
		self.error = None;

		// The conversation lives across runs: take it, run, hand it back.
		let agent = self.agent.take().unwrap_or_else(|| {
			Agent::new(
				UreqClient::new(config),
				MockBackend::new(),
				AgentConfig::default(),
			)
		});
		let executor = cx.background_executor().clone();
		cx.spawn(async move |this, cx| {
			let outcome = executor
				.spawn(async move {
					let mut agent = agent;
					let result = agent.run(&text);
					(agent, result)
				})
				.await;
			let _ = this.update(cx, |this, cx| this.on_agent_done(outcome, cx));
		})
		.detach();
	}

	/// Applies a finished run on the UI thread: the conversation comes back,
	/// the busy flag clears, and the report or its structured error becomes
	/// bubbles / the status strip.
	fn on_agent_done(&mut self, (agent, outcome): AgentOutcome, cx: &mut Context<Self>) {
		self.agent = Some(agent);
		self.busy = false;
		match outcome {
			Ok(report) => append_report(&mut self.bubbles, &report),
			Err(err) => self.error = Some(err.code().to_string()),
		}
		cx.notify();
	}

	/// Shows the 「已复制」 confirmation on `index` for a moment, then clears
	/// it on the background executor so the button label returns on its own.
	fn mark_copied(&mut self, index: usize, cx: &mut Context<Self>) {
		self.copied = Some(index);
		cx.notify();
		let executor = cx.background_executor().clone();
		cx.spawn(async move |this, cx| {
			executor
				.timer(std::time::Duration::from_millis(1400))
				.await;
			let _ = this.update(cx, |this, cx| {
				this.copied = None;
				cx.notify();
			});
		})
		.detach();
	}
}

// ---------------------------------------------------------------------------
// Rendering — small pure helpers over the shared theme.
// ---------------------------------------------------------------------------

/// The status strip: setup guide (no key), busy notice, or last error.
fn status_strip(
	configured: bool,
	busy: bool,
	error: Option<&str>,
	colors: &Colors,
) -> gpui_kit::Div {
	let body = if !configured {
		Some(crate::i18n::tr("ai.not_configured").to_owned())
	} else if let Some(code) = error {
		Some(crate::i18n::tr(error_i18n_key(code)).to_owned())
	} else if busy {
		Some(crate::i18n::tr("ai.busy").to_owned())
	} else {
		None
	};
	let Some(body) = body else {
		return div().flex_shrink_0();
	};
	div()
		.flex_shrink_0()
		.px_3()
		.py_2()
		.border_b_1()
		.border_color(colors.border)
		.bg(colors.container)
		.text_xs()
		.text_color(if configured { colors.text } else { colors.disabled })
		.child(body)
}

/// The 「复制」 button: writes the bubble's plain text to the system
/// clipboard and flashes 「已复制」. Markdown bubbles copy through
/// [`markdown_to_plain`], so a reader gets `project.open`, not
/// ``` `project.open` ```.
fn copy_button<E: AppEngine>(
	index: usize,
	plain: String,
	colors: &Colors,
	copied: bool,
	panel: Entity<AiAssistantPanel<E>>,
) -> AnyElement {
	div()
		.id(("ai-copy", index))
		.px_2()
		.py_1()
		.rounded_md()
		.text_xs()
		.text_color(if copied { colors.text } else { colors.disabled })
		.child(if copied {
			crate::i18n::tr("ai.copied")
		} else {
			crate::i18n::tr("ai.copy")
		})
		.on_click(move |_: &ClickEvent, _window, cx| {
			cx.write_to_clipboard(ClipboardItem::new_string(plain.clone()));
			panel.update(cx, |this, cx| this.mark_copied(index, cx));
		})
		.into_any_element()
}

/// One transcript bubble, laid out by role.
///
/// User and assistant bodies render through `TextView::markdown` — GFM
/// Markdown plus native drag-selection and `ctrl-c`/`ctrl-a` (gpui-base's
/// `TextView`, with the copy landing on the clipboard at
/// `text_view.rs:731`) — so a reply's `**bold**`, headings, lists and
/// fenced code show as formatting and every word stays selectable.
fn render_bubble<E: AppEngine>(
	index: usize,
	bubble: &Bubble,
	colors: &Colors,
	copied: bool,
	panel: Entity<AiAssistantPanel<E>>,
) -> AnyElement {
	match bubble {
		Bubble::User(text) => div()
			.flex()
			.justify_end()
			.child(
				div()
					.flex()
					.flex_col()
					.gap_1()
					.max_w_80()
					.px_3()
					.py_2()
					.rounded_xl()
					.bg(colors.container)
					.text_color(colors.text)
					.child(
						TextView::markdown(("ai-md-user", index), text.clone()).selectable(true),
					)
					.child(
						div().flex().justify_end().child(copy_button::<E>(
							index,
							markdown_to_plain(text),
							colors,
							copied,
							panel,
						)),
					),
			)
			.into_any_element(),
		Bubble::Assistant(text) => div()
			.flex()
			.child(
				div()
					.flex()
					.flex_col()
					.gap_1()
					.max_w_80()
					.px_3()
					.py_2()
					.rounded_xl()
					.border_1()
					.border_color(colors.border)
					.text_color(colors.text)
					.child(
						TextView::markdown(("ai-md-assistant", index), text.clone())
							.selectable(true),
					)
					.child(
						div().flex().justify_end().child(copy_button::<E>(
							index,
							markdown_to_plain(text),
							colors,
							copied,
							panel,
						)),
					),
			)
			.into_any_element(),
		Bubble::Tool {
			name,
			outcome,
			detail,
		} => div()
			.flex()
			.gap_2()
			.items_center()
			.px_2()
			.py_1()
			.rounded_md()
			.border_1()
			.border_color(colors.border)
			.text_xs()
			.text_color(colors.disabled)
			.child(
				div()
					.font_weight(gpui_kit::FontWeight(600.0))
					.text_color(colors.text)
					.child(name.clone()),
			)
			.child(div().child(crate::i18n::tr(outcome_i18n_key(outcome))))
			.child(div().truncate().child(detail.clone()))
			.child(copy_button::<E>(
				index,
				format!("{name}\n{detail}"),
				colors,
				copied,
				panel,
			))
			.into_any_element(),
		Bubble::System(text) => div()
			.flex()
			.justify_center()
			.child(
				div()
					.px_2()
					.py_1()
					.text_xs()
					.text_color(colors.disabled)
					.child(text.clone()),
			)
			.into_any_element(),
	}
}

// ---------------------------------------------------------------------------
// Dock plumbing
// ---------------------------------------------------------------------------

impl<E: AppEngine> Render for AiAssistantPanel<E> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors();
		let busy = self.busy;
		let configured = self.configured;
		let error = self.error.clone();
		let draft_nonempty = !self.input.read(cx).value().trim().is_empty();
		let enabled = can_send(busy, configured, draft_nonempty);
		let this = cx.entity();

		let bubbles: Vec<AnyElement> = if self.bubbles.is_empty() {
			vec![div()
				.flex()
				.items_center()
				.justify_center()
				.py_8()
				.text_color(colors.disabled)
				.child(crate::i18n::tr("ai.empty").to_owned())
				.into_any_element()]
		} else {
			let snapshot = self.bubbles.clone();
			let copied = self.copied;
			let panel = cx.entity();
			snapshot
				.iter()
				.enumerate()
				.map(|(index, bubble)| {
					render_bubble::<E>(
						index,
						bubble,
						&colors,
						copied == Some(index),
						panel.clone(),
					)
				})
				.collect()
		};

		div()
			.flex()
			.flex_col()
			.size_full()
			.track_focus(&self.focus_handle)
			.child(status_strip(configured, busy, error.as_deref(), &colors))
			.child(
				div()
					.id("ai-assistant-transcript")
					.flex_1()
					.min_h_0()
					.flex()
					.flex_col()
					.gap_2()
					.px_3()
					.py_2()
					.overflow_y_scroll()
					// No `on_mouse_down` here on purpose: claiming the panel's
					// focus handle on every click bubbles up and steals the
					// `TextView`'s focus mid-drag, which kills text selection
					// (and would eat the composer's keystrokes the same way).
					// Bubbles and the composer own their own focus handles.
					.children(bubbles),
			)
			.child(
				div()
					.flex_shrink_0()
					.p_2()
					.border_t_1()
					.border_color(colors.border)
					.child(
						text_input("ai-assistant-input", cx)
							.state(self.input.downgrade())
							.accepts_input(!busy),
					),
			)
			.child(
				div()
					.flex_shrink_0()
					.flex()
					.justify_end()
					.px_2()
					.pb_2()
					.child(
						// `.id()` puts the button in gpui-pre's method domain,
						// where `on_click` lives; the click handler re-checks
						// `enabled`, so a stale frame cannot fire a run.
						div()
							.id("ai-assistant-send")
							.px_3()
							.py_2()
							.rounded_md()
							.bg(if enabled { colors.selected } else { colors.container })
							.text_color(if enabled { colors.text } else { colors.disabled })
							.child(crate::i18n::tr("ai.send"))
							.on_click(move |_: &ClickEvent, _window, cx| {
								if enabled {
									this.update(cx, |this, cx| this.send(cx));
								}
							}),
					),
			)
	}
}

impl<E: AppEngine> EventEmitter<PanelEvent> for AiAssistantPanel<E> {}

impl<E: AppEngine> Focusable for AiAssistantPanel<E> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<E: AppEngine> BasePanel for AiAssistantPanel<E> {
	fn panel_name(&self) -> &'static str {
		crate::panels::ids::AI_ASSISTANT
	}
}

impl<E: AppEngine> Panel for AiAssistantPanel<E> {
	fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		crate::i18n::tr("panel.ai_assistant")
	}
}

// ---------------------------------------------------------------------------
// Tests — state transitions only; rendering is covered by the e2e run.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ai::ToolLogEntry;

	/// Every structured code the loop can surface maps to a real key, and
	/// unknown codes degrade to the fallback instead of leaking the raw
	/// string as a translation key.
	#[test]
	fn error_i18n_key_maps_structured_codes() {
		assert_eq!(error_i18n_key("AI_NOT_CONFIGURED"), "ai.error.not_configured");
		assert_eq!(error_i18n_key("AI_HTTP_ERROR"), "ai.error.http");
		assert_eq!(error_i18n_key("AI_API_ERROR"), "ai.error.api");
		assert_eq!(error_i18n_key("AI_BAD_RESPONSE"), "ai.error.bad_response");
		assert_eq!(error_i18n_key("SOMETHING_NEW"), "ai.error.unknown");
	}

	#[test]
	fn outcome_i18n_key_maps_loop_outcomes() {
		assert_eq!(outcome_i18n_key("applied"), "ai.outcome.applied");
		assert_eq!(outcome_i18n_key("proposed"), "ai.outcome.proposed");
		assert_eq!(outcome_i18n_key("rejected"), "ai.outcome.rejected");
		assert_eq!(outcome_i18n_key("failed"), "ai.outcome.failed");
		assert_eq!(outcome_i18n_key("??"), "ai.error.unknown");
	}

	/// One run at a time; a run needs a key and a non-empty draft.
	#[test]
	fn send_guard_blocks_busy_unconfigured_and_blank() {
		assert!(can_send(false, true, true));
		assert!(!can_send(true, true, true), "busy blocks a second send");
		assert!(!can_send(false, false, true), "missing key blocks");
		assert!(!can_send(false, true, false), "blank draft blocks");
	}

	/// The transcript gets tool bubbles in call order, then the answer; a
	/// capped run gets the round-limit notice instead of a fake answer.
	#[test]
	fn append_report_orders_tools_then_answer() {
		let tool = |name: &str, outcome: &'static str| ToolLogEntry {
			round: 1,
			tool: name.to_owned(),
			outcome,
			detail: format!("{name}-detail"),
		};
		let mut bubbles = Vec::new();
		let report = AgentReport {
			rounds: 2,
			converged: true,
			final_text: Some("done".to_owned()),
			tools: vec![tool("project.open", "applied"), tool("node.set_param", "failed")],
		};
		append_report(&mut bubbles, &report);
		assert_eq!(
			bubbles,
			vec![
				Bubble::Tool {
					name: "project.open".to_owned(),
					outcome: "applied",
					detail: "project.open-detail".to_owned()
				},
				Bubble::Tool {
					name: "node.set_param".to_owned(),
					outcome: "failed",
					detail: "node.set_param-detail".to_owned()
				},
				Bubble::Assistant("done".to_owned()),
			]
		);
	}

	/// The capped run (no final text) becomes the round-limit notice, never
	/// a silent drop.
	#[test]
	fn capped_run_becomes_the_round_limit_notice() {
		let mut bubbles = Vec::new();
		let report = AgentReport {
			rounds: 3,
			converged: false,
			final_text: None,
			tools: Vec::new(),
		};
		append_report(&mut bubbles, &report);
		assert_eq!(
			bubbles,
			vec![Bubble::System(crate::i18n::tr("ai.notice.limit").to_owned())]
		);
	}

	/// Inline emphasis and code markers disappear; the words stay.
	#[test]
	fn inline_markers_are_stripped_for_the_clipboard() {
		assert_eq!(
			markdown_to_plain("**Project & Media**"),
			"Project & Media"
		);
		assert_eq!(
			markdown_to_plain("call `project.open` then `project.save`"),
			"call project.open then project.save"
		);
		assert_eq!(markdown_to_plain("*italic* and __bold__"), "italic and bold");
		assert_eq!(markdown_to_plain("~~gone~~ now"), "gone now");
	}

	/// Block markers (headings, bullets, ordered items) are dropped.
	#[test]
	fn block_markers_are_stripped_for_the_clipboard() {
		assert_eq!(markdown_to_plain("## Effects & Nodes"), "Effects & Nodes");
		assert_eq!(markdown_to_plain("- `media.probe` — inspect"), "media.probe — inspect");
		assert_eq!(markdown_to_plain("1. first\n2) second"), "first\nsecond");
	}

	/// A fenced block keeps its body verbatim and loses both fences.
	#[test]
	fn fenced_code_keeps_body_and_drops_the_fence() {
		assert_eq!(
			markdown_to_plain("before\n```rust\nlet x = 1;\n```\nafter"),
			"before\nlet x = 1;\nafter"
		);
		// Tildes work the same way, and markers inside stay untouched.
		assert_eq!(
			markdown_to_plain("~~~\n**not bold**\n~~~"),
			"**not bold**"
		);
	}

	/// Links degrade to their label; the URL is not copied.
	#[test]
	fn links_degrade_to_their_label() {
		assert_eq!(
			markdown_to_plain("see [the docs](https://example.com/x)"),
			"see the docs"
		);
	}

	/// An empty reply copies as an empty string, not a panic.
	#[test]
	fn empty_input_copies_as_empty() {
		assert_eq!(markdown_to_plain(""), "");
		assert_eq!(markdown_to_plain("\n\n   \n"), "");
	}

	/// Unbalanced markers stay literal and blank runs collapse.
	#[test]
	fn malformed_markdown_degrades_to_literal_text() {
		assert_eq!(markdown_to_plain("**unclosed"), "unclosed");
		assert_eq!(markdown_to_plain("a `tick"), "a `tick");
		assert_eq!(markdown_to_plain("a [bracket"), "a [bracket");
		assert_eq!(markdown_to_plain("a\n\n\n\nb"), "a\n\nb");
	}

	/// Static check: every `ai.*` key the panel renders exists in all eight
	/// shipped locales (a missing key would render as the raw key).
	#[test]
	fn ai_keys_exist_in_every_language() {
		const LANGS: [(&str, &str); 8] = [
			("zh-CN", include_str!("../../../../assets/i18n/zh-CN.yaml")),
			("en-US", include_str!("../../../../assets/i18n/en-US.yaml")),
			("de", include_str!("../../../../assets/i18n/de.yaml")),
			("pt", include_str!("../../../../assets/i18n/pt.yaml")),
			("fr", include_str!("../../../../assets/i18n/fr.yaml")),
			("es", include_str!("../../../../assets/i18n/es.yaml")),
			("ru", include_str!("../../../../assets/i18n/ru.yaml")),
			("ja", include_str!("../../../../assets/i18n/ja.yaml")),
		];
		const KEYS: [&str; 19] = [
			"menu.window.ai_assistant",
			"panel.ai_assistant",
			"ai.placeholder",
			"ai.send",
			"ai.copy",
			"ai.copied",
			"ai.busy",
			"ai.empty",
			"ai.not_configured",
			"ai.notice.limit",
			"ai.error.not_configured",
			"ai.error.http",
			"ai.error.api",
			"ai.error.bad_response",
			"ai.error.unknown",
			"ai.outcome.applied",
			"ai.outcome.proposed",
			"ai.outcome.rejected",
			"ai.outcome.failed",
		];
		for (lang, yaml) in LANGS {
			for key in KEYS {
				assert!(
					yaml.contains(key),
					"locale {lang} is missing the key {key}"
				);
			}
		}
	}
}
