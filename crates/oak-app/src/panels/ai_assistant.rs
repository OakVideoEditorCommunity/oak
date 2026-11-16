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

use crate::ai::backend::HostAdapter;
use crate::ai::{
	Agent, AgentConfig, AgentError, AgentReport, AiConfig, EngineBackend, LlmClient, MockBackend,
	UreqClient,
};
use crate::oakui::AppEngine;
use crate::oakui::component::text_input::{set_input_text, text_input};
use crate::opp::OppHost;
use gpui_kit::base::input::InputState;
use gpui_kit::colors::{Colors, DefaultColors};
use gpui_kit::component::dock::{BasePanel, Panel, PanelEvent};
use gpui_kit::component::input::InputEvent;
use gpui_kit::component::text::TextView;
use gpui_kit::{
	AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
	Focusable, Render, WeakEntity, Window, div, prelude::*, px,
};
use oak_ai_tools::error::ToolError;
use oak_ai_tools::timecode::{Rat, TimeContext, TimeValue};
use oak_ai_tools::txn::TxnOps;
use serde_json::Value;

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
		// The engine has a project but keeps no shared handle for the
		// tool layer (the mock's demo project is a plain value) — a
		// different truth from "no project is open", so it gets its own
		// wording instead of borrowing the real engine's error.
		"ENGINE_UNSUPPORTED" => "ai.error.engine_unsupported",
		// Saving to the system keyring (T13B): the codes come from
		// `ai::secrets::SecretError` and get their own wording so a
		// failed save reads as a keyring problem, not "unknown error".
		"AI_SECRET_UNAVAILABLE" => "ai.error.secret_unavailable",
		"AI_SECRET_WRITE" => "ai.error.secret_write",
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
		"unparsed" => "ai.outcome.unparsed",
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
			if cleaned
				.last()
				.is_some_and(|prev: &String| !prev.trim().is_empty())
			{
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
		if let Some(stripped) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
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
	// Unparseable text-form markup is reported, never shown as an answer:
	// the raw text lands in the tool bubble's detail for diagnosis.
	if report.unparsed_text_calls > 0 {
		bubbles.push(Bubble::System(
			crate::i18n::tr("ai.notice.unparsed_tool_call").to_owned(),
		));
		if let Some(raw) = &report.unparsed_text {
			bubbles.push(Bubble::Tool {
				name: "text_tool_call".to_string(),
				outcome: "unparsed",
				detail: raw.clone(),
			});
		}
	}
	// A stall is reported before the answer: the user must know the run
	// stopped on a repeated failure rather than silently hitting the cap.
	if let Some(stall) = &report.stalled {
		bubbles.push(Bubble::System(format!(
			"{} ({} · {} ×{})",
			crate::i18n::tr("ai.notice.stalled"),
			stall.tool,
			stall.code,
			stall.repeats
		)));
	}
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
/// background executor. The model face is `dyn` so tests can drive the loop
/// with a local stub while production keeps [`UreqClient`].
type AgentOutcome = (
	Agent<Box<dyn LlmClient + Send>, PanelBackend>,
	Result<AgentReport, AgentError>,
);

/// The panel's agent holds `Box<dyn LlmClient + Send>`; this forwards the seam so
/// the boxed trait object satisfies `LlmClient` itself.
impl LlmClient for Box<dyn LlmClient + Send> {
	fn complete(
		&self,
		request: &crate::ai::llm::ChatRequest<'_>,
	) -> Result<crate::ai::ChatResponse, crate::ai::LlmError> {
		(**self).complete(request)
	}
}

/// The agent's tool-execution backend: the real engine through
/// [`HostAdapter`] by default, the frozen [`MockBackend`] only when the
/// operator asks for it (`OAK_AI_BACKEND=mock`) or a test builds it.
///
/// The panel needs one concrete `Agent<L, B>` type, so the two paths meet
/// in this enum and forward every `TxnOps`/`EngineBackend` call to the
/// variant in hand.
pub enum PanelBackend {
	/// The real engine's tool face (the default path).
	Real(HostAdapter<OppHost>),
	/// The frozen mock (explicit opt-in / tests).
	Mock(MockBackend),
}

impl PanelBackend {
	/// Which variant this backend is — the panel rebuilds the agent when
	/// an env change would disagree with the live variant (dropping the
	/// conversation is the honest cost of switching mid-flight).
	pub fn is_mock(&self) -> bool {
		matches!(self, Self::Mock(_))
	}

	/// Points the real variant's host at the engine's live project handle;
	/// the mock variant has no host, so it ignores the handle.
	pub fn attach(&mut self, project: crate::oakui::graphops::ProjectRef) {
		if let Self::Real(adapter) = self {
			adapter.host().attach_project(project);
		}
	}
}

impl oak_ai_tools::backend::EngineExt for PanelBackend {
	fn sequence_get_settings(
		&self,
		args: serde_json::Value,
	) -> Result<serde_json::Value, ToolError> {
		match self {
			Self::Mock(inner) => {
				oak_ai_tools::backend::EngineExt::sequence_get_settings(inner, args)
			}
			Self::Real(inner) => {
				oak_ai_tools::backend::EngineExt::sequence_get_settings(inner, args)
			}
		}
	}

	fn sequence_set_format(
		&mut self,
		args: serde_json::Value,
	) -> Result<serde_json::Value, ToolError> {
		match self {
			Self::Mock(inner) => oak_ai_tools::backend::EngineExt::sequence_set_format(inner, args),
			Self::Real(inner) => oak_ai_tools::backend::EngineExt::sequence_set_format(inner, args),
		}
	}
}

impl TxnOps for PanelBackend {
	fn txn_begin(&mut self, label: &str) -> Result<(), ToolError> {
		match self {
			Self::Mock(inner) => inner.txn_begin(label),
			Self::Real(inner) => inner.txn_begin(label),
		}
	}

	fn txn_commit(&mut self) -> Result<(), ToolError> {
		match self {
			Self::Mock(inner) => inner.txn_commit(),
			Self::Real(inner) => inner.txn_commit(),
		}
	}

	fn txn_abort(&mut self) -> Result<(), ToolError> {
		match self {
			Self::Mock(inner) => inner.txn_abort(),
			Self::Real(inner) => inner.txn_abort(),
		}
	}
}

impl EngineBackend for PanelBackend {
	fn time_rate(&self, ctx: TimeContext) -> Result<Rat, ToolError> {
		match self {
			Self::Mock(inner) => inner.time_rate(ctx),
			Self::Real(inner) => inner.time_rate(ctx),
		}
	}

	fn rational_at(&self, ctx: TimeContext, value: TimeValue) -> Result<Rat, ToolError> {
		match self {
			Self::Mock(inner) => inner.rational_at(ctx, value),
			Self::Real(inner) => inner.rational_at(ctx, value),
		}
	}

	fn project_open(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.project_open(args),
			Self::Real(inner) => inner.project_open(args),
		}
	}

	fn project_save(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.project_save(args),
			Self::Real(inner) => inner.project_save(args),
		}
	}

	fn project_get_overview(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.project_get_overview(args),
			Self::Real(inner) => inner.project_get_overview(args),
		}
	}

	fn media_probe(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.media_probe(args),
			Self::Real(inner) => inner.media_probe(args),
		}
	}

	fn media_import_footage(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.media_import_footage(args),
			Self::Real(inner) => inner.media_import_footage(args),
		}
	}

	fn media_list_footage(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.media_list_footage(args),
			Self::Real(inner) => inner.media_list_footage(args),
		}
	}

	fn timeline_add_track(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.timeline_add_track(args),
			Self::Real(inner) => inner.timeline_add_track(args),
		}
	}

	fn timeline_place_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.timeline_place_clip(args),
			Self::Real(inner) => inner.timeline_place_clip(args),
		}
	}

	fn timeline_split_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.timeline_split_clip(args),
			Self::Real(inner) => inner.timeline_split_clip(args),
		}
	}

	fn timeline_trim_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.timeline_trim_clip(args),
			Self::Real(inner) => inner.timeline_trim_clip(args),
		}
	}

	fn timeline_move_clip(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.timeline_move_clip(args),
			Self::Real(inner) => inner.timeline_move_clip(args),
		}
	}

	fn timeline_ripple_delete(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.timeline_ripple_delete(args),
			Self::Real(inner) => inner.timeline_ripple_delete(args),
		}
	}

	fn timeline_add_transition(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.timeline_add_transition(args),
			Self::Real(inner) => inner.timeline_add_transition(args),
		}
	}

	fn timeline_add_marker(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.timeline_add_marker(args),
			Self::Real(inner) => inner.timeline_add_marker(args),
		}
	}

	fn node_add_effect(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.node_add_effect(args),
			Self::Real(inner) => inner.node_add_effect(args),
		}
	}

	fn node_set_param(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.node_set_param(args),
			Self::Real(inner) => inner.node_set_param(args),
		}
	}

	fn node_list_types(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.node_list_types(args),
			Self::Real(inner) => inner.node_list_types(args),
		}
	}

	fn render_get_frame(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.render_get_frame(args),
			Self::Real(inner) => inner.render_get_frame(args),
		}
	}

	fn render_get_thumbnails(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.render_get_thumbnails(args),
			Self::Real(inner) => inner.render_get_thumbnails(args),
		}
	}

	fn render_get_audio_levels(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.render_get_audio_levels(args),
			Self::Real(inner) => inner.render_get_audio_levels(args),
		}
	}

	fn export_start(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.export_start(args),
			Self::Real(inner) => inner.export_start(args),
		}
	}

	fn edit_undo(&mut self, args: &Value) -> Result<Value, ToolError> {
		match self {
			Self::Mock(inner) => inner.edit_undo(args),
			Self::Real(inner) => inner.edit_undo(args),
		}
	}
}

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
	agent: Option<Agent<Box<dyn LlmClient + Send>, PanelBackend>>,
	/// The bubble index whose 「已复制」 confirmation is showing.
	copied: Option<usize>,
	/// A 「新会话」 click on a non-empty conversation is pending
	/// confirmation (the confirm bar renders while this is `true`).
	pending_new_session: bool,
	/// T13B: the settings view is open (three fields + save/cancel).
	show_settings: bool,
	/// T13B: the injected secret store. Production uses the system
	/// keyring ([`KeyringStore`](crate::ai::secrets::KeyringStore));
	/// tests swap in a `MemoryStore` and never touch the real keyring.
	secrets: Box<dyn crate::ai::secrets::SecretStore>,
	/// The write half of the same store (the save button's path). Kept
	/// beside `secrets` so one swap moves both seams — see
	/// `set_secret_store`.
	secrets_writer: Box<dyn crate::ai::secrets::KeyringWriter>,
	/// The three settings fields (API key masked; url/model show their
	/// defaults as placeholders).
	settings_key: Entity<InputState>,
	settings_url: Entity<InputState>,
	settings_model: Entity<InputState>,
	/// Set when a save was refused (empty key) or the keyring errored;
	/// shown in the settings view and mapped through [`error_i18n_key`].
	settings_error: Option<String>,
}

impl<E: AppEngine> AiAssistantPanel<E> {
	/// Builds the panel; `configured` snapshots the key's presence so the
	/// setup guide renders without ever reading the key itself.
	pub fn new(engine: Entity<E>, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let input = cx.new(|cx| InputState::new(window, cx));
		// The settings fields: the API key is masked (a saved key must
		// never be re-displayed in the clear) and starts empty — we
		// deliberately do **not** prefill it from the store, so "what the
		// field shows" can never leak the stored value.
		let settings_key = cx.new(|cx| InputState::new(window, cx).masked(true));
		let settings_url = cx
			.new(|cx| InputState::new(window, cx).placeholder(crate::ai::config::DEFAULT_BASE_URL));
		let settings_model =
			cx.new(|cx| InputState::new(window, cx).placeholder(crate::ai::config::DEFAULT_MODEL));
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
			// T13B: the same layered read the send path uses — env first,
			// then the system keyring (the sovereign's settings UI).
			configured: AiConfig::from_env_and_keyring().is_ok(),
			agent: None,
			copied: None,
			pending_new_session: false,
			show_settings: false,
			secrets: Box::new(crate::ai::secrets::KeyringStore),
			secrets_writer: Box::new(crate::ai::secrets::KeyringStore),
			settings_key,
			settings_url,
			settings_model,
			settings_error: None,
		}
	}

	/// Swaps both store seams at once (tests inject a `MemoryStore`;
	/// production keeps the real keyring installed by [`new`](Self::new)).
	pub fn set_secret_store(
		&mut self,
		store: Box<dyn crate::ai::secrets::SecretStore>,
		writer: Box<dyn crate::ai::secrets::KeyringWriter>,
	) {
		self.secrets = store;
		self.secrets_writer = writer;
		// Re-read `configured` against the new store: an injected store
		// may carry a key the env does not (that is exactly what the
		// save tests assert).
		self.configured =
			AiConfig::load_with(|n| std::env::var(n).ok(), self.secrets.as_ref()).is_ok();
	}

	/// Opens the settings view (empty key field; url/model show their
	/// defaults as placeholders). Clears a stale settings error.
	pub fn open_settings(&mut self, cx: &mut Context<Self>) {
		self.show_settings = true;
		self.settings_error = None;
		set_input_text(&self.settings_key, "", cx);
		set_input_text(&self.settings_url, "", cx);
		set_input_text(&self.settings_model, "", cx);
		cx.notify();
	}

	/// Closes the settings view without touching the stores (cancel's
	/// job — a no-op on storage by construction).
	pub fn close_settings(&mut self, cx: &mut Context<Self>) {
		self.show_settings = false;
		self.settings_error = None;
		cx.notify();
	}

	/// Saves the three fields to the injected store (keyring in
	/// production), then re-reads `configured` so the strip flips
	/// immediately. Rules: an empty key is refused **before** any write
	/// (no empty credentials), url/model fall back to their defaults,
	/// and a `SecretError` surfaces as its code — never a panic.
	pub fn save_settings(&mut self, cx: &mut Context<Self>) {
		let key = self.settings_key.read(cx).value().trim().to_string();
		if key.is_empty() {
			self.settings_error = Some("AI_SETTINGS_KEY_REQUIRED".to_string());
			cx.notify();
			return;
		}
		let url = self.settings_url.read(cx).value().trim().to_string();
		let model = self.settings_model.read(cx).value().trim().to_string();
		let url = if url.is_empty() {
			crate::ai::config::DEFAULT_BASE_URL.to_string()
		} else {
			url
		};
		let model = if model.is_empty() {
			crate::ai::config::DEFAULT_MODEL.to_string()
		} else {
			model
		};

		if let Err(err) = self
			.secrets_writer
			.store(crate::ai::secrets::ACCOUNT_API_KEY, &key)
		{
			self.settings_error = Some(err.code().to_string());
			cx.notify();
			return;
		}
		if let Err(err) = self
			.secrets_writer
			.store(crate::ai::secrets::ACCOUNT_BASE_URL, &url)
		{
			self.settings_error = Some(err.code().to_string());
			cx.notify();
			return;
		}
		if let Err(err) = self
			.secrets_writer
			.store(crate::ai::secrets::ACCOUNT_MODEL, &model)
		{
			self.settings_error = Some(err.code().to_string());
			cx.notify();
			return;
		}

		// All three landed: flip the strip and close the view.
		self.configured =
			AiConfig::load_with(|n| std::env::var(n).ok(), self.secrets.as_ref()).is_ok();
		self.settings_error = None;
		self.show_settings = false;
		// Clear the fields so a re-open never shows the key, even masked.
		set_input_text(&self.settings_key, "", cx);
		cx.notify();
	}

	/// Whether a conversation exists worth confirming before dropping: any
	/// transcript, a live agent (its message history), or a visible error.
	/// The blank first-run state clears without asking.
	pub fn has_conversation(&self) -> bool {
		!self.bubbles.is_empty() || self.agent.is_some() || self.error.is_some()
	}

	/// 「新会话」: a blank session resets directly; a conversation asks first
	/// (`pending_new_session` renders the confirm bar). Never fires while a
	/// run is in flight — the run would refill the transcript right after.
	pub fn request_new_session(&mut self, cx: &mut Context<Self>) {
		if self.busy {
			return;
		}
		if self.has_conversation() {
			self.pending_new_session = true;
		} else {
			self.clear_conversation(cx);
		}
		cx.notify();
	}

	/// The confirm bar's 「确认」: drop the conversation for real.
	pub fn confirm_new_session(&mut self, cx: &mut Context<Self>) {
		self.clear_conversation(cx);
	}

	/// The confirm bar's 「取消」: keep everything, close the prompt.
	pub fn cancel_new_session(&mut self, cx: &mut Context<Self>) {
		self.pending_new_session = false;
		cx.notify();
	}

	/// The actual reset: transcript, live agent, error, copy flag, prompt
	/// state and the composer — back to the blank first-run state, so the
	/// next send starts a genuinely fresh conversation.
	fn clear_conversation(&mut self, cx: &mut Context<Self>) {
		self.bubbles.clear();
		self.agent = None;
		self.error = None;
		self.copied = None;
		self.pending_new_session = false;
		set_input_text(&self.input, "", cx);
		cx.notify();
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
		let config = match AiConfig::from_env_and_keyring() {
			Ok(config) => config,
			Err(err) => {
				self.error = Some(err.code().to_string());
				cx.notify();
				return;
			}
		};

		// --- tool execution (T8.5): resolve the engine's project handle ---
		let (project, engine_has_project) = {
			let engine = self.engine.read(cx);
			(engine.project_ref(), engine.project().is_some())
		};
		// The engine holds a project but cannot hand out a shared handle
		// (the mock's demo project is a plain value): refuse with the
		// true reason. Borrowing the real engine's "no project is open"
		// here would tell the operator to open a project that is already
		// on screen — a lie about what went wrong.
		if project.is_none() && engine_has_project {
			self.error = Some("ENGINE_UNSUPPORTED".to_string());
			cx.notify();
			return;
		}
		// `OAK_AI_BACKEND=mock` opts into the frozen mock's fake success;
		// the default path executes against the real engine.
		let want_mock = std::env::var("OAK_AI_BACKEND")
			.map(|v| v.trim().eq_ignore_ascii_case("mock"))
			.unwrap_or(false);
		// A mid-flight env change would disagree with the live variant:
		// rebuild the agent (losing the conversation is the honest cost
		// of switching backends mid-flight).
		if self
			.agent
			.as_ref()
			.is_some_and(|agent| agent.backend().is_mock() != want_mock)
		{
			self.agent = None;
		}

		self.bubbles.push(Bubble::User(text.clone()));
		set_input_text(&self.input, "", cx);
		self.busy = true;
		self.error = None;

		// The conversation lives across runs: take it, run, hand it back.
		let mut agent = self.agent.take().unwrap_or_else(|| {
			let backend = if want_mock {
				PanelBackend::Mock(MockBackend::new())
			} else {
				PanelBackend::Real(HostAdapter::new(OppHost::new()))
			};
			Agent::new(
				Box::new(UreqClient::new(config)),
				backend,
				AgentConfig::default(),
			)
		});
		// Re-point the real host at the engine's current project on every
		// run, so a project opened or switched between runs is picked up.
		if let Some(project) = project {
			agent.backend_mut().attach(project);
		}
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
			executor.timer(std::time::Duration::from_millis(1400)).await;
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
		return div().min_w_0();
	};
	// T13B header fix (see the `header` builder in `render`): the strip
	// yields `min_w_0` and its text truncates, so a long status cannot
	// push the buttons out — the buttons are `flex_shrink_0` instead.
	div()
		.min_w_0()
		.px_3()
		.py_2()
		.border_b_1()
		.border_color(colors.border)
		.bg(colors.container)
		.text_xs()
		.truncate()
		.text_color(if configured {
			colors.text
		} else {
			colors.disabled
		})
		.child(body)
}

/// One settings row: a label plus the app's text input bound to `state`.
/// The state itself carries the interesting bits — `masked(true)` for the
/// API key, the default value as `placeholder` for url/model — so this
/// wrapper stays a plain label + field.
fn settings_field(
	id: &'static str,
	state: WeakEntity<InputState>,
	label: String,
	cx: &App,
) -> gpui_kit::Div {
	div()
		.flex()
		.flex_shrink_0()
		.items_center()
		.gap_2()
		.child(
			div()
				.w(px(96.))
				.flex_shrink_0()
				.text_xs()
				.text_color(cx.default_colors().text)
				.child(label),
		)
		.child(
			div()
				.min_w_0()
				.flex_1()
				.border_1()
				.rounded_md()
				.border_color(cx.default_colors().border)
				.child(text_input(id, cx).state(state)),
		)
}

/// The i18n key the unconfigured status strip shows — and, because the
/// strip is now a button, the promise it makes ("open Settings and
/// paste your key"). Split out so a test can pin the key itself.
pub fn not_configured_key() -> &'static str {
	"ai.not_configured"
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
					.child(TextView::markdown(("ai-md-user", index), text.clone()).selectable(true))
					.child(div().flex().justify_end().child(copy_button::<E>(
						index,
						markdown_to_plain(text),
						colors,
						copied,
						panel,
					))),
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
					.child(div().flex().justify_end().child(copy_button::<E>(
						index,
						markdown_to_plain(text),
						colors,
						copied,
						panel,
					))),
			)
			.into_any_element(),
		Bubble::Tool {
			name,
			outcome,
			detail,
		} => div()
			// Test-visible handle: lets a render test assert the tool row
			// really reached the frame (inert outside debug builds).
			.debug_selector(move || format!("ai-tool-{index}"))
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
		// 「新会话」 is inert while a run is in flight (the run would refill
		// the transcript right after the reset).
		let new_session_enabled = !busy;
		let pending_confirm = self.pending_new_session;

		let bubbles: Vec<AnyElement> = if self.bubbles.is_empty() {
			vec![
				div()
					.flex()
					.items_center()
					.justify_center()
					.py_8()
					.text_color(colors.disabled)
					.child(crate::i18n::tr("ai.empty").to_owned())
					.into_any_element(),
			]
		} else {
			let snapshot = self.bubbles.clone();
			let copied = self.copied;
			let panel = cx.entity();
			snapshot
				.iter()
				.enumerate()
				.map(|(index, bubble)| {
					render_bubble::<E>(index, bubble, &colors, copied == Some(index), panel.clone())
				})
				.collect()
		};

		// The strip is a button while unconfigured: one click opens
		// Settings (the old copy told users to set an env var before
		// launch, which stopped being the only way in T13B).
		let strip = status_strip(configured, busy, error.as_deref(), &colors);
		let strip: AnyElement = if configured {
			strip.into_any_element()
		} else {
			strip
				.id("ai-assistant-not-configured")
				.on_click({
					let this = this.clone();
					move |_: &ClickEvent, _window, cx| {
						this.update(cx, |this, cx| {
							if !this.busy {
								this.open_settings(cx);
							}
						});
					}
				})
				.into_any_element()
		};

		let header = div()
			.flex()
			.flex_shrink_0()
			.items_center()
			.justify_between()
			.gap_2()
			.px_2()
			.py_1()
			// Shrinking text first, `min_w_0` so it may actually shrink
			// (T13B truncation fix): the strip yields, the buttons do not.
			.child(div().flex_1().min_w_0().child(strip))
			.child(
				// 设置 sits right of the status strip, left of 新会话 —
				// both buttons `flex_shrink_0`, so neither can be clipped
				// no matter how long the status text runs.
				div()
					.id("ai-assistant-settings")
					.flex_shrink_0()
					.px_3()
					.py_1()
					.rounded_md()
					.border_1()
					.border_color(colors.border)
					.text_color(if new_session_enabled {
						colors.text
					} else {
						colors.disabled
					})
					.child(crate::i18n::tr("ai.settings").to_owned())
					.on_click({
						let this = this.clone();
						move |_: &ClickEvent, _window, cx| {
							if new_session_enabled {
								this.update(cx, |this, cx| this.open_settings(cx));
							}
						}
					}),
			)
			.child(
				div()
					.id("ai-assistant-new-session")
					.flex_shrink_0()
					.px_3()
					.py_1()
					.rounded_md()
					.border_1()
					.border_color(colors.border)
					.text_color(if new_session_enabled {
						colors.text
					} else {
						colors.disabled
					})
					.child(crate::i18n::tr("ai.new_session").to_owned())
					.on_click({
						let this = this.clone();
						move |_: &ClickEvent, _window, cx| {
							if new_session_enabled {
								this.update(cx, |this, cx| this.request_new_session(cx));
							}
						}
					}),
			);

		// The confirm bar: only while a reset is pending (a conversation
		// exists — dropping it silently would lose context on one misclick).
		let confirm_bar = if pending_confirm {
			Some(
				div()
					.id("ai-assistant-new-session-confirm")
					.flex()
					.flex_shrink_0()
					.items_center()
					.justify_between()
					.gap_3()
					.px_3()
					.py_2()
					.border_b_1()
					.border_color(colors.border)
					.bg(colors.container)
					.child(
						div()
							.flex_1()
							.text_color(colors.text)
							.child(crate::i18n::tr("ai.new_session.prompt").to_owned()),
					)
					.child(
						div()
							.id("ai-assistant-new-session-confirm-ok")
							.px_3()
							.py_1()
							.rounded_md()
							.bg(colors.selected)
							.text_color(colors.text)
							.child(crate::i18n::tr("ai.new_session.confirm").to_owned())
							.on_click({
								let this = this.clone();
								move |_: &ClickEvent, _window, cx| {
									this.update(cx, |this, cx| this.confirm_new_session(cx));
								}
							}),
					)
					.child(
						div()
							.id("ai-assistant-new-session-confirm-cancel")
							.px_3()
							.py_1()
							.rounded_md()
							.border_1()
							.border_color(colors.border)
							.text_color(colors.text)
							.child(crate::i18n::tr("ai.new_session.cancel").to_owned())
							.on_click({
								let this = this.clone();
								move |_: &ClickEvent, _window, cx| {
									this.update(cx, |this, cx| this.cancel_new_session(cx));
								}
							}),
					),
			)
		} else {
			None
		};

		// The settings view (T13B): inline under the header, above the
		// transcript. State lives on the panel — never global.
		let settings_view = if self.show_settings {
			let settings_error = self.settings_error.clone();
			let key_state = self.settings_key.downgrade();
			let url_state = self.settings_url.downgrade();
			let model_state = self.settings_model.downgrade();
			Some(
				div()
					.id("ai-assistant-settings-view")
					.flex()
					.flex_shrink_0()
					.flex_col()
					.gap_2()
					.px_3()
					.py_2()
					.border_b_1()
					.border_color(colors.border)
					.bg(colors.container)
					.child(
						div()
							.text_xs()
							.font_weight(gpui_kit::FontWeight(600.0))
							.text_color(colors.text)
							.child(crate::i18n::tr("ai.settings.title").to_owned()),
					)
					.child(settings_field(
						"ai-assistant-settings-key",
						key_state,
						crate::i18n::tr("ai.settings.api_key").to_owned(),
						cx,
					))
					.child(settings_field(
						"ai-assistant-settings-url",
						url_state,
						crate::i18n::tr("ai.settings.base_url").to_owned(),
						cx,
					))
					.child(settings_field(
						"ai-assistant-settings-model",
						model_state,
						crate::i18n::tr("ai.settings.model").to_owned(),
						cx,
					))
					.child(
						div()
							.flex()
							.flex_shrink_0()
							.justify_end()
							.gap_2()
							.child(
								div()
									.id("ai-assistant-settings-save")
									.px_3()
									.py_1()
									.rounded_md()
									.bg(colors.selected)
									.text_color(colors.text)
									.child(crate::i18n::tr("ai.settings.save").to_owned())
									.on_click({
										let this = this.clone();
										move |_: &ClickEvent, _window, cx| {
											this.update(cx, |this, cx| this.save_settings(cx));
										}
									}),
							)
							.child(
								div()
									.id("ai-assistant-settings-cancel")
									.px_3()
									.py_1()
									.rounded_md()
									.border_1()
									.border_color(colors.border)
									.text_color(colors.text)
									.child(crate::i18n::tr("ai.settings.cancel").to_owned())
									.on_click({
										let this = this.clone();
										move |_: &ClickEvent, _window, cx| {
											this.update(cx, |this, cx| this.close_settings(cx));
										}
									}),
							),
					)
					// A refused save (empty key) or a keyring error — the
					// codes map through `error_i18n_key`, shown in place.
					.children(settings_error.map(|code| {
						div()
							.id("ai-assistant-settings-error")
							.text_xs()
							.text_color(colors.disabled)
							.child(crate::i18n::tr(error_i18n_key(&code)).to_owned())
					})),
			)
		} else {
			None
		};

		div()
			.debug_selector(|| "ai-assistant-root".to_string())
			.flex()
			.flex_col()
			.size_full()
			.track_focus(&self.focus_handle)
			.child(header)
			.children(confirm_bar)
			.children(settings_view)
			.child(
				div()
					.id("ai-assistant-transcript")
					.debug_selector(|| "ai-assistant-transcript".to_string())
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
							.bg(if enabled {
								colors.selected
							} else {
								colors.container
							})
							.text_color(if enabled {
								colors.text
							} else {
								colors.disabled
							})
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
mod tests;
