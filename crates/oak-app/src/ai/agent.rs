//! The agent tool loop (T8.1).
//!
//! One turn = system + history + frozen tools → LLM → `tool_calls` →
//! [`oak_ai_tools::dispatch::execute_with`] (the hooks transaction gate,
//! unchanged) → results appended as `tool` messages → repeat until the
//! model stops asking for tools or the round cap is hit.
//!
//! Nothing here talks to the network ([`crate::ai::llm::LlmClient`] is the
//! seam) and nothing here knows which engine backs the tools
//! ([`oak_ai_tools::backend::EngineBackend`] is the seam), so the whole
//! loop runs under test with a scripted model and a mock engine.

use super::config;
use super::tool_text;
use oak_ai_tools::error;
use oak_ai_tools::schema_v2;
use serde_json::{Value, json};

use oak_ai_tools::backend::{EngineBackend, EngineExt};
use oak_ai_tools::dispatch;
use oak_ai_tools::hooks::{GatedOutcome, Hooks, NoHooks, Policy};

use super::llm::{ChatMessage, ChatRequest, LlmClient, ToolCall, openai_tools};

/// Loop bounds and gate policy.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentConfig {
	/// Maximum LLM turns per `run`. The cap exists so a model that keeps
	/// asking for tools cannot spin forever.
	pub max_rounds: usize,
	/// Hooks policy for every tool call (see `oak_ai_tools::hooks`).
	pub policy: Policy,
	/// Model name sent on each request (from `OAK_AI_MODEL`).
	pub model: String,
}

impl Default for AgentConfig {
	fn default() -> Self {
		Self {
			max_rounds: 8,
			policy: Policy::Auto,
			model: config::DEFAULT_MODEL.to_string(),
		}
	}
}

/// One executed tool call, for the chat panel / logs.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolLogEntry {
	pub round: usize,
	pub tool: String,
	/// `applied` / `proposed` / `rejected` / `failed`.
	pub outcome: &'static str,
	/// Short model-facing summary (the same JSON handed back to the model).
	pub detail: String,
}

/// What a `run` did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentReport {
	/// LLM turns consumed.
	pub rounds: usize,
	/// True when the model answered without asking for tools.
	pub converged: bool,
	/// Its final text (only set when converged).
	pub final_text: Option<String>,
	/// Every tool call executed, in order.
	pub tools: Vec<ToolLogEntry>,
	/// Tool calls that came from the **text fallback** (the model wrote a
	/// `<tool_call>` block instead of using the function-calling channel).
	pub text_fallback_calls: usize,
	/// Responses that looked like text-form calls but could not be parsed.
	pub unparsed_text_calls: usize,
	/// The offending raw markup of the last unparsed response, for `detail`.
	pub unparsed_text: Option<String>,
	/// Set when the loop stopped early because the same tool kept failing
	/// with the same error code: what it was stuck on, for the user.
	pub stalled: Option<StallStop>,
}

/// Why the loop stopped before the round cap: the same tool failed with the
/// same code `repeats` times in a row, so further rounds would burn the
/// budget on one unfixable call (the measured 6-round failure).
#[derive(Debug, Clone, PartialEq)]
pub struct StallStop {
	pub tool: String,
	pub code: String,
	pub repeats: usize,
	/// The last structured message, verbatim, for the user-facing notice.
	pub message: String,
}

impl StallStop {
	/// The user-facing sentence: what is stuck and what to do about it.
	pub fn describe(&self) -> String {
		format!(
			"stopped early: `{}` failed {} times in a row with `{}` ({}); \
			 nothing further was attempted. Fix that call (or tell the assistant \
			 what to do differently) and try again",
			self.tool, self.repeats, self.code, self.message
		)
	}
}

/// How many identical failures in a row end the loop (首辅 ruling, T9.3).
pub const MAX_IDENTICAL_FAILURES: usize = 3;

/// Loop failure. Both variants are structured (`code()`), never a panic.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AgentError {
	/// Configuration missing (e.g. no `OAK_AI_API_KEY`).
	#[error(transparent)]
	Config(#[from] config::ConfigError),
	/// The model call failed.
	#[error(transparent)]
	Llm(#[from] super::llm::LlmError),
}

impl AgentError {
	pub fn code(&self) -> &'static str {
		match self {
			Self::Config(e) => e.code(),
			Self::Llm(e) => e.code(),
		}
	}
}

/// The loop: a model, an engine backend, a configuration, and the running
/// conversation.
pub struct Agent<L, B> {
	llm: L,
	backend: B,
	config: AgentConfig,
	messages: Vec<ChatMessage>,
	/// Openai tools array built once from the frozen registry (plus the v2
	/// face and the `memory.*` face).
	tools: Vec<Value>,
	/// Long-term memory tools (memlite subprocess; see [`super::memory`]).
	memory: super::memory::MemoryTools,
}

impl<L: LlmClient, B: EngineBackend + EngineExt> Agent<L, B> {
	pub fn new(llm: L, backend: B, config: AgentConfig) -> Self {
		// The frozen 22 first (their order is the schema-directory order),
		// then the v2 face — one tool list for the model, two dispatch paths
		// behind it.
		let mut tools = openai_tools(dispatch::default_schemas());
		tools.extend(schema_v2::v2_openai_tools());
		tools.extend(super::memory::openai_tools());
		let messages = vec![ChatMessage::system(
			"You are Oak's video-editing agent. Use the provided tools to inspect \
			 and change the project; answer briefly when done. Call tools through the \
			 function-calling channel, never as XML or text markup in your reply.",
		)];
		Self {
			llm,
			backend,
			config,
			messages,
			tools,
			memory: super::memory::MemoryTools::from_env(),
		}
	}

	/// Replaces the memory tools (tests pass a private library root; the
	/// panel may pass the project identity it resolved).
	pub fn with_memory(mut self, memory: super::memory::MemoryTools) -> Self {
		self.memory = memory;
		self
	}

	/// The memory tools in use.
	pub fn memory(&self) -> &super::memory::MemoryTools {
		&self.memory
	}

	/// The backend, for tests and for the panel that renders engine state.
	pub fn backend(&self) -> &B {
		&self.backend
	}

	pub fn backend_mut(&mut self) -> &mut B {
		&mut self.backend
	}

	/// The conversation so far (system + every message exchanged).
	pub fn messages(&self) -> &[ChatMessage] {
		&self.messages
	}

	/// One user turn with the default gate policy ([`NoHooks`]).
	pub fn run(&mut self, user: &str) -> Result<AgentReport, AgentError> {
		self.run_with(user, &mut NoHooks)
	}

	/// One user turn through the hooks transaction gate.
	///
	/// The gate itself is `oak-ai-tools`' business: this loop only routes
	/// the outcome back to the model (`ok` / `rejected` / `failed`) and
	/// records it in the report.
	pub fn run_with(
		&mut self,
		user: &str,
		hooks: &mut dyn Hooks,
	) -> Result<AgentReport, AgentError> {
		self.messages.push(ChatMessage::user(user));
		let mut report = AgentReport::default();

		// Repeated-identical-failure guard: the streak spans rounds on
		// purpose (the measured failure re-sent the same call every round);
		// a different tool or a different code resets it.
		let mut streak: Option<(String, String, usize)> = None;

		for round in 1..=self.config.max_rounds.max(1) {
			let response = self.llm.complete(&ChatRequest {
				model: &self.config.model,
				messages: &self.messages,
				tools: &self.tools,
			})?;
			report.rounds = round;

			if response.tool_calls.is_empty() {
				// Some models write the call as text instead of using the
				// function-calling channel; execute that through the very same
				// path rather than showing markup to the user.
				let scan = tool_text::scan(response.content.as_deref().unwrap_or_default());
				if !scan.calls.is_empty() {
					let calls: Vec<ToolCall> = scan
						.calls
						.into_iter()
						.enumerate()
						.map(|(index, mut call)| {
							call.id = format!("text_{round}_{index}");
							call
						})
						.collect();
					report.text_fallback_calls += calls.len();
					// History gets the **prose only**: feeding the raw markup
					// back would teach the model that this format works.
					self.messages.push(ChatMessage::assistant(
						Some(scan.prose.clone()),
						calls.clone(),
					));
					for call in &calls {
						self.execute_call(call, round, hooks, &mut report, &mut streak)?;
					}
					continue;
				}
				if let Some(raw) = scan.unparsed {
					// Unparseable markup is a failure to report, not an answer
					// to display: stop here, keep the raw text for `detail`.
					report.unparsed_text_calls += 1;
					report.unparsed_text = Some(raw);
					report.final_text = None;
					report.converged = false;
					report.rounds = round;
					return Ok(report);
				}
				// No tools asked → the turn is over (protocol's normal end).
				report.converged = true;
				report.final_text = response.content;
				return Ok(report);
			}

			self.messages.push(ChatMessage::assistant(
				response.content,
				response.tool_calls.clone(),
			));

			for call in &response.tool_calls {
				if self.execute_call(call, round, hooks, &mut report, &mut streak)? {
					return Ok(report);
				}
			}
		}

		// Round cap reached with the model still asking for tools: report
		// honestly instead of looping on.
		Ok(report)
	}
	/// Executes one tool call through the same gate the structured path uses.
	///
	/// Returns `true` when the loop must stop (the identical-failure streak
	/// tripped). Shared by the structured path and the text fallback, so the
	/// two can never diverge in semantics.
	fn execute_call(
		&mut self,
		call: &ToolCall,
		round: usize,
		hooks: &mut dyn Hooks,
		report: &mut AgentReport,
		streak: &mut Option<(String, String, usize)>,
	) -> Result<bool, AgentError> {
		// Three faces, one loop: memory tools answer host-side (a
		// subprocess, no engine and no edit transaction), a v2 name goes
		// to the extension dispatcher, everything else keeps the frozen
		// 22 path. All three end in the same payload/streak handling.
		let (payload, label) = if super::memory::is_memory_tool(&call.name) {
			memory_payload(super::memory::execute(
				&self.memory,
				&call.name,
				call.arguments.clone(),
			))
		} else if schema_v2::is_v2_tool(&call.name) {
			outcome_payload(dispatch::execute_v2_with(
				&mut self.backend,
				&call.name,
				call.arguments.clone(),
				self.config.policy,
				hooks,
			))
		} else {
			outcome_payload(dispatch::execute_with(
				&mut self.backend,
				&call.name,
				call.arguments.clone(),
				self.config.policy,
				hooks,
			))
		};
		self.messages
			.push(ChatMessage::tool_result(&call.id, &payload));
		report.tools.push(ToolLogEntry {
			round,
			tool: call.name.clone(),
			outcome: label,
			detail: payload.to_string(),
		});

		// Track the failing streak. Only structured refusals count:
		// an applied edit is progress even if an earlier one failed.
		if let Some(code) = payload.get("code").and_then(Value::as_str) {
			let message = payload
				.get("message")
				.and_then(Value::as_str)
				.unwrap_or_default()
				.to_string();
			let repeat = match &streak {
				Some((tool, prev, count)) if tool == &call.name && prev == code => count + 1,
				_ => 1,
			};
			if repeat >= MAX_IDENTICAL_FAILURES {
				report.stalled = Some(StallStop {
					tool: call.name.clone(),
					code: code.to_string(),
					repeats: repeat,
					message,
				});
				// `round` is this turn's 1-based index, so the turns
				// consumed are exactly `round` (not round + 1).
				report.rounds = round;
				return Ok(true);
			}
			let _ = message;
			*streak = Some((call.name.clone(), code.to_string(), repeat));
		} else {
			// Anything that is not a structured failure resets it.
			*streak = None;
		}
		Ok(false)
	}
}

/// Model-facing payload for a `memory.*` call.
///
/// Same envelope as the engine tools (`ok` / `code` / `message`), so the
/// model meets one error vocabulary whichever face answered.
fn memory_payload(result: Result<Value, error::ToolError>) -> (Value, &'static str) {
	match result {
		Ok(value) => (json!({ "ok": true, "result": value }), "applied"),
		Err(error) => {
			let mut payload = json!({
				"ok": false,
				"code": error.code,
				"message": error.message,
			});
			if let Some(gap) = error.gap {
				payload["gap"] = json!(gap);
			}
			(payload, "failed")
		}
	}
}

/// Turns a gated outcome into the JSON the model reads, plus the log label.
fn outcome_payload(outcome: GatedOutcome) -> (Value, &'static str) {
	match outcome {
		GatedOutcome::Applied { result } => (json!({ "ok": true, "result": result }), "applied"),
		GatedOutcome::Proposed { proposal } => (
			json!({
				"ok": true,
				"proposed": true,
				"tool": proposal.tool,
				"note": "not applied; awaiting confirmation",
			}),
			"proposed",
		),
		GatedOutcome::Rejected { reason, error } => (
			json!({
				"ok": false,
				"code": error.code,
				"message": reason,
				"note": "the confirmation gate refused this call",
			}),
			"rejected",
		),
		GatedOutcome::Failed { error, feedback } => (
			json!({
				"ok": false,
				"code": error.code,
				"message": error.message,
				"retryable": feedback.retryable,
				"hint": feedback.hint,
			}),
			"failed",
		),
	}
}

#[cfg(test)]
mod tests;
