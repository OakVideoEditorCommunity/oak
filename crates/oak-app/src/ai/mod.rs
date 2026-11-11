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
//! * [`backend`] — which `EngineBackend` implements the tools; the
//!   two-scheme investigation lives in its module docs pending the lead's
//!   ruling.
//!
//! The 22-tool face, its schemas, the hooks policy and the transaction
//! gate all come from `ai/oak-ai-tools` unchanged — this module only
//! orchestrates them.

pub mod agent;
pub mod backend;
pub mod config;
pub mod llm;
pub mod tool_text;

pub use agent::{Agent, AgentConfig, AgentError, AgentReport, ToolLogEntry};
pub use backend::{EngineBackend, MockBackend};
pub use config::{AiConfig, ConfigError};
pub use llm::{ChatMessage, ChatResponse, LlmClient, LlmError, ToolCall, UreqClient};
