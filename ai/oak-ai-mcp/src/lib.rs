//! T5 — thin MCP wrapper over the T1 tool execution layer.
//!
//! The crate is a **transport adapter**: it turns MCP `tools/list` /
//! `tools/call` into [`oak_ai_tools::dispatch`] calls and nothing else.
//! There is no second tool table, no second validator and no second
//! transaction implementation here — that is what keeps the MCP face
//! from drifting from the direct execution layer.
//!
//! * [`rmcp_server`] — the MCP SDK adapter (transport, framing, lifecycle);
//! * [`rpc`] — JSON-RPC 2.0 response envelopes only;
//! * [`server`] — lifecycle, `tools/list`, `tools/call`, error shaping;
//! * [`backend_select`] — `--backend mock` / `--backend sdk` gating.
//!
//! The binary (`src/main.rs`) only parses arguments and pumps frames;
//! everything testable lives in this library so the zero-drift and
//! protocol tests can drive it without a subprocess.

pub mod backend_select;
pub mod rmcp_server;
pub mod rpc;
pub mod server;
