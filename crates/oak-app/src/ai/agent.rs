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
use super::confirm::{ConfirmMode, GatedHooks};
use super::plan::{
	Baseline, CostGuard, CostStop, CostUsage, OnFail, Plan, PlanStep, RetryPolicy, RollbackReport,
	StepStatus, StepTrace, estimate_tokens, rollback_to, substitute,
};
use super::progress::{ProgressEvent, ProgressSink};
use super::readback;
use super::tool_text;
use oak_ai_tools::error;
use oak_ai_tools::schema_v2;
use serde_json::{Value, json};

use oak_ai_tools::backend::{EngineBackend, EngineExt};
use oak_ai_tools::dispatch;
use oak_ai_tools::hooks::{GatedOutcome, Hooks, NoHooks, OperationClass, Policy, class_of,
	class_of_v2};

use super::llm::{ChatMessage, ChatRequest, LlmClient, LlmError, ToolCall, openai_tools};

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
	/// Confirm gate for mutating tools before they run (reads never
	/// ask). `Headless` = auto-approve (today's behaviour), `Strict` =
	/// refuse every write.
	pub confirm: ConfirmMode,
}

impl Default for AgentConfig {
	fn default() -> Self {
		Self {
			max_rounds: 8,
			policy: Policy::Auto,
			model: config::DEFAULT_MODEL.to_string(),
			confirm: ConfirmMode::Headless,
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
	/// The explicit plan this run executed (plan-based runs only):
	/// produced *before* the steps ran.
	pub plan: Option<Plan>,
	/// What actually happened, step by step, with attempts and failures.
	pub trajectory: Vec<StepTrace>,
	/// Progress events in order (rounds and plan steps).
	pub progress: Vec<ProgressEvent>,
	/// Cost consumed by this run (tool calls, LLM rounds, ~tokens).
	pub cost: CostUsage,
	/// The `stopped_reason` when a hard cost limit ended the run.
	pub cost_stop: Option<CostStop>,
	/// Set when a failed plan was rolled back — including a rollback that
	/// could not confirm the restore (`restored: false` + the failing
	/// undo calls; never a silent success).
	pub rollback: Option<RollbackReport>,
	/// Readback checks performed / failed / skipped (probe unavailable).
	pub readbacks: usize,
	pub readback_failures: usize,
	pub readback_skipped: usize,
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
	/// Progress subscribers; the events also land in [`AgentReport::progress`].
	progress_sink: Option<Box<dyn ProgressSink>>,
	/// Hard cost limits (default: unlimited).
	cost_guard: CostGuard,
	/// What the current run has spent.
	cost_usage: CostUsage,
	/// Retry policy for plan steps (default: 2 attempts, 100 ms backoff).
	retry: RetryPolicy,
	/// Post-write readback self-check (default: on).
	readback: bool,
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
			progress_sink: None,
			cost_guard: CostGuard::unlimited(),
			cost_usage: CostUsage::default(),
			retry: RetryPolicy::default(),
			readback: true,
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

	/// Subscribes to progress events (rounds and plan steps); the events
	/// also land in [`AgentReport::progress`].
	pub fn with_progress_sink(mut self, sink: impl ProgressSink + 'static) -> Self {
		self.progress_sink = Some(Box::new(sink));
		self
	}

	/// Replaces the cost guard (default: unlimited).
	pub fn with_cost_guard(mut self, guard: CostGuard) -> Self {
		self.cost_guard = guard;
		self
	}

	/// Replaces the plan-step retry policy (default: 2 attempts, 100 ms).
	pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
		self.retry = retry;
		self
	}

	/// Turns the post-write readback self-check off (default: on).
	pub fn without_readback(mut self) -> Self {
		self.readback = false;
		self
	}

	/// What the current run has spent so far.
	pub fn cost_usage(&self) -> &CostUsage {
		&self.cost_usage
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

	/// One user turn through the configured confirm gate
	/// ([`AgentConfig::confirm`], default `Headless` = auto-approve).
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
		self.cost_usage = CostUsage::default();
		// The confirm gate sits between the loop and the caller's hooks:
		// mutating calls are asked per `config.confirm` before they run,
		// and an approval still falls through to the caller's hooks.
		let mut gate = GatedHooks::new(self.config.confirm, hooks);
		let mut report = self.run_loop(user, &mut gate)?;
		report.cost = self.cost_usage.clone();
		Ok(report)
	}

	/// The policy the engine actually runs under: a strict confirm gate
	/// forces `Policy::Confirm` so the gate is consulted (reads stay
	/// exempt by class); headless keeps the configured policy untouched.
	fn effective_policy(&self) -> Policy {
		match self.config.confirm {
			ConfirmMode::Strict => Policy::Confirm,
			ConfirmMode::Headless => self.config.policy,
		}
	}

	/// The turn body behind [`run_with`]: history, rounds, tool calls.
	fn run_loop(
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

		let total_rounds = self.config.max_rounds.max(1);
		for round in 1..=total_rounds {
			// Hard stop before spending a round the budget does not cover.
			if let Some(stop) = self.cost_guard.check_llm(&self.cost_usage) {
				report.cost_stop = Some(stop);
				break;
			}
			self.emit_progress(
				&mut report,
				ProgressEvent::new(round, total_rounds, "round"),
			);
			let response = self.llm.complete(&ChatRequest {
				model: &self.config.model,
				messages: &self.messages,
				tools: &self.tools,
			})?;
			self.cost_usage.llm_rounds += 1;
			self.cost_usage.tokens +=
				estimate_tokens(response.content.as_deref().unwrap_or_default());
			for asked in &response.tool_calls {
				self.cost_usage.tokens += estimate_tokens(&asked.arguments.to_string());
			}
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
		// Hard cost gate first: a capped run stops before spending the
		// next call, on every face, and says so instead of limping on.
		if let Some(stop) = self.cost_guard.check_tool(&self.cost_usage) {
			let payload = json!({
				"ok": false,
				"code": "COST_LIMIT",
				"message": stop.detail.clone(),
				"budget": stop.limit,
			});
			self.messages
				.push(ChatMessage::tool_result(&call.id, &payload));
			report.tools.push(ToolLogEntry {
				round,
				tool: call.name.clone(),
				outcome: "cost-stopped",
				detail: payload.to_string(),
			});
			if report.cost_stop.is_none() {
				report.cost_stop = Some(stop);
			}
			return Ok(true);
		}

		// Three faces, one loop: memory tools answer host-side (a
		// subprocess, no engine and no edit transaction); the engine faces
		// (frozen 22 and v2) share `engine_call`, which adds cost
		// accounting and the readback self-check. All three end in the
		// same payload/streak handling.
		let (payload, label) = if super::memory::is_memory_tool(&call.name) {
			self.cost_usage.tool_calls += 1;
			memory_payload(super::memory::execute(
				&self.memory,
				&call.name,
				call.arguments.clone(),
			))
		} else {
			self.engine_call(call, hooks, report)
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

	/// One engine tool call (frozen 22 or v2): cost accounting →
	/// optional baseline probe → dispatch → readback self-check.
	///
	/// A readback that cannot confirm the write **replaces** the success
	/// payload with a structured `READBACK_MISMATCH` failure and relabels
	/// the call: a write whose effect is not observable never reports
	/// `applied`.
	fn engine_call(
		&mut self,
		call: &ToolCall,
		hooks: &mut dyn Hooks,
		report: &mut AgentReport,
	) -> (Value, &'static str) {
		if let Some(stop) = self.cost_guard.check_tool(&self.cost_usage) {
			let payload = json!({
				"ok": false,
				"code": "COST_LIMIT",
				"message": stop.detail,
				"budget": stop.limit,
			});
			if report.cost_stop.is_none() {
				report.cost_stop = Some(stop);
			}
			return (payload, "cost-stopped");
		}
		self.cost_usage.tool_calls += 1;

		let v2 = schema_v2::is_v2_tool(&call.name);
		let class = if v2 {
			class_of_v2(&call.name)
		} else {
			class_of(&call.name)
		};
		let mutating = !matches!(class, Some(OperationClass::Read));
		let probe_plan = if mutating && self.readback {
			readback::probe_plan(&call.name)
		} else {
			readback::ProbePlan::Skip
		};
		// Baseline only when a "changed" verdict may need it: `AfterOnly`
		// tools have no usable baseline by construction (no project yet /
		// the timeline does not move).
		let baseline = if probe_plan == readback::ProbePlan::BeforeAndAfter {
			Some(readback::probe(&mut self.backend))
		} else {
			None
		};

		// Resolved before `&mut self.backend` is lent to dispatch.
		let policy = self.effective_policy();
		let outcome = if v2 {
			dispatch::execute_v2_with(
				&mut self.backend,
				&call.name,
				call.arguments.clone(),
				policy,
				hooks,
			)
		} else {
			dispatch::execute_with(
				&mut self.backend,
				&call.name,
				call.arguments.clone(),
				policy,
				hooks,
			)
		};
		let (mut payload, mut label) = outcome_payload(outcome);

		if label == "applied" && probe_plan != readback::ProbePlan::Skip {
			match readback::probe(&mut self.backend) {
				Ok(after) => {
					report.readbacks += 1;
					let result = payload.get("result").cloned().unwrap_or(Value::Null);
					if let Some(expect) = readback::expectation_for(&call.name, &result) {
						let before = baseline.as_ref().and_then(|probe| probe.as_ref().ok());
						let verdict = readback::verify(before, &after, expect);
						if !verdict.ok {
							report.readback_failures += 1;
							payload = verdict.failure_payload();
							label = "readback-failed";
						}
					}
				}
				Err(_) => {
					// The probe itself is unavailable: count it rather than
					// pretend the write was verified.
					report.readback_skipped += 1;
				}
			}
		}
		(payload, label)
	}

	/// One user turn: first produce an explicit plan for `user`, then
	/// execute it step by step. Plan and trajectory land in the report.
	pub fn run_planned(
		&mut self,
		user: &str,
		hooks: &mut dyn Hooks,
	) -> Result<AgentReport, AgentError> {
		self.cost_usage = CostUsage::default();
		let mut report = AgentReport::default();
		let plan = self.plan_from_llm(user)?;
		report.rounds = self.cost_usage.llm_rounds;
		self.run_steps(plan, hooks, &mut report)?;
		report.cost = self.cost_usage.clone();
		Ok(report)
	}

	/// Executes a pre-built plan through the default [`NoHooks`] gate
	/// (no model in the loop: the plan is the instruction).
	pub fn run_plan(&mut self, plan: Plan) -> Result<AgentReport, AgentError> {
		self.run_plan_with(plan, &mut NoHooks)
	}

	/// [`run_plan`] through the hooks transaction gate.
	pub fn run_plan_with(
		&mut self,
		plan: Plan,
		hooks: &mut dyn Hooks,
	) -> Result<AgentReport, AgentError> {
		self.cost_usage = CostUsage::default();
		let mut report = AgentReport::default();
		let mut gate = GatedHooks::new(self.config.confirm, hooks);
		self.run_steps(plan, &mut gate, &mut report)?;
		report.cost = self.cost_usage.clone();
		Ok(report)
	}

	/// Asks the model for an explicit plan of `user`'s request — one LLM
	/// round; the answer must be JSON
	/// `{"goal": "...", "steps": [{"title","tool","args"}]}` (code fences
	/// tolerated). The raw exchange stays in the conversation.
	pub fn plan_from_llm(&mut self, user: &str) -> Result<Plan, AgentError> {
		let prompt = format!(
			"{user}\n\nBefore executing anything, reply with ONLY a JSON plan, no \
			 other prose: {{\"goal\": \"...\", \"steps\": [{{\"title\": \"...\", \
			 \"tool\": \"...\", \"args\": {{}}}}]}}. Use only the provided tools."
		);
		self.messages.push(ChatMessage::user(&prompt));
		let response = self.llm.complete(&ChatRequest {
			model: &self.config.model,
			messages: &self.messages,
			tools: &self.tools,
		})?;
		self.cost_usage.llm_rounds += 1;
		let content = response.content.unwrap_or_default();
		self.cost_usage.tokens += estimate_tokens(&content);
		self.messages
			.push(ChatMessage::assistant(Some(content.clone()), Vec::new()));
		parse_plan(&content, user)
	}

	/// Executes `plan` step by step: progress events, template vars from
	/// earlier results, retry while the feedback says retryable, the
	/// step's `on_fail` policy, the hard cost guard — and, when a step
	/// fails, an undo-based rollback to the baseline taken before the
	/// first step.
	fn run_steps(
		&mut self,
		plan: Plan,
		hooks: &mut dyn Hooks,
		report: &mut AgentReport,
	) -> Result<(), AgentError> {
		report.plan = Some(plan.clone());
		let total = plan.steps.len();
		let baseline = if self.readback && total > 0 {
			Some(Baseline::capture(&mut self.backend))
		} else {
			None
		};
		let mut vars = if plan.vars.is_object() {
			plan.vars.clone()
		} else {
			json!({})
		};

		for (index, step) in plan.steps.iter().enumerate() {
			self.emit_progress(
				report,
				ProgressEvent::new(index + 1, total, step.title.clone())
					.with_detail(step.tool.clone()),
			);

			// Hard stop: the budget is spent before this step begins.
			if let Some(stop) = self.cost_guard.check_tool(&self.cost_usage) {
				if report.cost_stop.is_none() {
					report.cost_stop = Some(stop);
				}
				report.trajectory.push(StepTrace {
					index,
					title: step.title.clone(),
					tool: step.tool.clone(),
					status: StepStatus::Blocked,
					attempts: 0,
					detail: Some("cost budget exhausted before this step".into()),
				});
				break;
			}

			let args = match substitute(&step.args, &vars) {
				Ok(args) => args,
				Err(message) => {
					report.trajectory.push(StepTrace {
						index,
						title: step.title.clone(),
						tool: step.tool.clone(),
						status: StepStatus::Failed,
						attempts: 0,
						detail: Some(message),
					});
					self.rollback_after(report, &baseline, &step.title);
					break;
				}
			};

			let (payload, label, attempts) = self.run_step_attempts(step, &args, hooks, report);
			if label == "applied" {
				if let Some(result) = payload.get("result") {
					merge_vars(&mut vars, result);
				}
				report.trajectory.push(StepTrace {
					index,
					title: step.title.clone(),
					tool: step.tool.clone(),
					status: StepStatus::Done,
					attempts,
					detail: None,
				});
				continue;
			}
			if label == "cost-stopped" {
				report.trajectory.push(StepTrace {
					index,
					title: step.title.clone(),
					tool: step.tool.clone(),
					status: StepStatus::Blocked,
					attempts,
					detail: Some("cost budget exhausted mid-step".into()),
				});
				break;
			}

			let detail = payload
				.get("message")
				.and_then(Value::as_str)
				.unwrap_or(label)
				.to_string();
			match &step.on_fail {
				OnFail::Skip => {
					report.trajectory.push(StepTrace {
						index,
						title: step.title.clone(),
						tool: step.tool.clone(),
						status: StepStatus::Skipped,
						attempts,
						detail: Some(detail),
					});
					continue;
				}
				OnFail::Fallback {
					tool: fallback_tool,
					args: fallback_args,
				} => {
					let fallback_args =
						substitute(fallback_args, &vars).unwrap_or_else(|_| fallback_args.clone());
					let call = ToolCall {
						id: format!("plan_fb_{index}"),
						name: fallback_tool.clone(),
						arguments: fallback_args,
					};
					let (fb_payload, fb_label) = self.engine_call(&call, hooks, report);
					if fb_label == "applied" {
						if let Some(result) = fb_payload.get("result") {
							merge_vars(&mut vars, result);
						}
						report.trajectory.push(StepTrace {
							index,
							title: step.title.clone(),
							tool: fallback_tool.clone(),
							status: StepStatus::Degraded,
							attempts,
							detail: Some(format!(
								"degraded to `{fallback_tool}` after `{}` failed: {detail}",
								step.tool
							)),
						});
						continue;
					}
					let fb_detail = fb_payload
						.get("message")
						.and_then(Value::as_str)
						.unwrap_or(fb_label)
						.to_string();
					report.trajectory.push(StepTrace {
						index,
						title: step.title.clone(),
						tool: step.tool.clone(),
						status: StepStatus::Failed,
						attempts,
						detail: Some(format!(
							"step failed ({detail}); fallback `{fallback_tool}` failed too 							 ({fb_detail})"
						)),
					});
					self.rollback_after(report, &baseline, &step.title);
					break;
				}
				OnFail::Fail => {
					report.trajectory.push(StepTrace {
						index,
						title: step.title.clone(),
						tool: step.tool.clone(),
						status: StepStatus::Failed,
						attempts,
						detail: Some(detail),
					});
					self.rollback_after(report, &baseline, &step.title);
					break;
				}
			}
		}
		Ok(())
	}

	/// Runs one step's primary call, retrying while the failure's
	/// feedback says the error is retryable and the policy allows it.
	fn run_step_attempts(
		&mut self,
		step: &PlanStep,
		args: &Value,
		hooks: &mut dyn Hooks,
		report: &mut AgentReport,
	) -> (Value, &'static str, usize) {
		let mut attempts = 0;
		loop {
			attempts += 1;
			let call = ToolCall {
				id: format!("plan_{attempts}"),
				name: step.tool.clone(),
				arguments: args.clone(),
			};
			let (payload, label) = self.engine_call(&call, hooks, report);
			let retryable = payload
				.get("retryable")
				.and_then(Value::as_bool)
				.unwrap_or(false);
			if label == "applied"
				|| label == "cost-stopped"
				|| !self.retry.should_retry(attempts, retryable)
			{
				return (payload, label, attempts);
			}
			self.retry.backoff();
		}
	}

	/// Rolls back to `baseline` after a failed step. A rollback that
	/// cannot confirm the restore lists the steps it could not undo — it
	/// never reports a silent success.
	fn rollback_after(
		&mut self,
		report: &mut AgentReport,
		baseline: &Option<Baseline>,
		step_title: &str,
	) {
		let Some(baseline) = baseline else {
			return;
		};
		let mut rollback = rollback_to(
			&mut self.backend,
			baseline,
			format!("step `{step_title}` failed"),
			64,
		);
		if !rollback.restored {
			rollback.unrestored_steps = report
				.trajectory
				.iter()
				.filter(|trace| matches!(trace.status, StepStatus::Done | StepStatus::Degraded))
				.map(|trace| trace.title.clone())
				.collect();
			rollback.detail = format!(
				"{}; steps not confirmed undone: {}",
				rollback.detail,
				if rollback.unrestored_steps.is_empty() {
					"<none recorded>".into()
				} else {
					rollback.unrestored_steps.join(", ")
				}
			);
		}
		report.rollback = Some(rollback);
	}

	/// Fans one progress event out to the subscriber and the report.
	fn emit_progress(&mut self, report: &mut AgentReport, event: ProgressEvent) {
		if let Some(sink) = self.progress_sink.as_mut() {
			sink.on_progress(&event);
		}
		report.progress.push(event);
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

/// Parses a plan out of a model reply (tolerates ```json fences).
fn parse_plan(content: &str, goal: &str) -> Result<Plan, AgentError> {
	let trimmed = content.trim();
	let json_text = if trimmed.starts_with("```") {
		trimmed
			.trim_start_matches("```json")
			.trim_start_matches("```")
			.trim_end_matches("```")
			.trim()
	} else {
		trimmed
	};
	let root: Value = serde_json::from_str(json_text).map_err(|err| {
		AgentError::Llm(LlmError::Parse(format!("the plan is not JSON: {err}")))
	})?;
	let steps = root
		.get("steps")
		.and_then(Value::as_array)
		.ok_or_else(|| {
			AgentError::Llm(LlmError::Parse("the plan has no `steps` array".into()))
		})?;
	let mut plan = Plan::new(
		root.get("goal")
			.and_then(Value::as_str)
			.unwrap_or(goal),
	);
	for (index, step) in steps.iter().enumerate() {
		let tool = step
			.get("tool")
			.and_then(Value::as_str)
			.ok_or_else(|| {
				AgentError::Llm(LlmError::Parse(format!("plan step {index} has no `tool`")))
			})?
			.to_string();
		let title = step
			.get("title")
			.and_then(Value::as_str)
			.map(str::to_string)
			.unwrap_or_else(|| format!("step {}", index + 1));
		let args = step.get("args").cloned().unwrap_or_else(|| json!({}));
		plan.steps.push(PlanStep::new(title, tool, args));
	}
	Ok(plan)
}

/// Folds a step's result into the template variables (top-level fields:
/// `${track}` resolves against what the earlier step returned).
fn merge_vars(vars: &mut Value, result: &Value) {
	if let (Some(into), Value::Object(from)) = (vars.as_object_mut(), result) {
		for (key, value) in from {
			into.insert(key.clone(), value.clone());
		}
	}
}

#[cfg(test)]
mod tests;
