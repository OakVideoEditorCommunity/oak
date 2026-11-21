//! Built-in AI agent backend (T8.1, the "all in-process" route the
//! sovereign settled on 2026-11-11: chat panel + tool loop live in the oak
//! process, no OPP plugin, no pi).
//!
//! * [`config`] — `OAK_AI_API_KEY` / `OAK_AI_BASE_URL` / `OAK_AI_MODEL`;
//!   a missing key is a structured error, and the key never leaves memory.
//! * [`llm`] — openai-completions client behind the [`llm::LlmClient`]
//!   trait (the mock seam), plus the frozen-registry → `tools` adapter.
//! * [`agent`] — the tool loop: schema → LLM → `tool_calls` →
//!   `oak-ai-tools::dispatch::execute_with` (hooks gate included) →
//!   results back into the conversation, capped by `max_rounds`.
//! * [`memory`] — `memory.*` tools over the memlite CLI (BM25, two tiers,
//!   Rust-side secret refusal);
//! * [`backend`] — which `EngineBackend` implements the tools; the
//!   two-scheme investigation lives in its module docs pending the lead's
//!   ruling.
//! * [`confirm`] — the write-confirmation gate: an injectable
//!   [`confirm::Confirmer`] plus headless/strict default policies.
//! * [`readback`] — post-write self-check: probe the state, compare with
//!   what the call promised, fail with a diff instead of trusting the
//!   success envelope.
//! * [`progress`] — step `n/m` events for long tasks (report + sink).
//! * [`plan`] — explicit plans/recipes with trajectory, retry/fallback,
//!   a hard cost guard and undo-based rollback.
//!
//! The 22-tool face, its schemas, the hooks policy and the transaction
//! gate all come from `ai/oak-ai-tools` unchanged — this module only
//! orchestrates them.

pub mod agent;
pub mod backend;
pub mod config;
pub mod confirm;
pub mod llm;
pub mod memory;
pub mod plan;
pub mod progress;
pub mod readback;
pub mod secrets;
pub mod tool_text;

pub use agent::{Agent, AgentConfig, AgentError, AgentReport, ToolLogEntry};
pub use backend::{EngineBackend, MockBackend};
pub use config::{AiConfig, ConfigError};
pub use confirm::{
	AutoConfirm, Confirmer, ConfirmerHooks, ConfirmAnswer, ConfirmMode, ConfirmRequest,
	DenyWrites, GatedHooks, Interactive,
};
pub use llm::{ChatMessage, ChatResponse, LlmClient, LlmError, ToolCall, UreqClient};
pub use memory::{MemoryConfig, MemoryTools};
pub use plan::{
	Baseline, CostGuard, CostStop, CostUsage, OnFail, Plan, PlanStep, Recipe, RecipeBook,
	RetryPolicy, RollbackReport, StepStatus, StepTrace, estimate_tokens, substitute,
};
pub use progress::{FnSink, ProgressEvent, ProgressSink};
pub use readback::{Expect, ReadbackReport};
pub use secrets::{MemoryStore, SecretError, SecretStore};
