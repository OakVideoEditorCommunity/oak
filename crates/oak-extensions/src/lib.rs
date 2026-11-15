//! Oak external plugin host core (OPP/1) — task T7.1.
//!
//! This crate is the **host-side protocol and lifecycle core**: NDJSON
//! framing, the JSON-RPC envelope, the `plugin.toml` manifest, the 11-state
//! process lifecycle with bounded restarts, and the bidirectional session
//! (in-flight pairing + dispatch serialization).
//!
//! Deliberately **not** in this crate (later phases):
//! - engine routing — the app implements [`host_services::HostServices`] (P1.2);
//! - the declarative UI / panel shell — `ui.*` ships as typed placeholders only;
//! - shm frame pools (P3).
//!
//! Frozen wire reference: `docs/zh/plans/external-plugin-protocol.md`.

pub mod host_services;
pub mod lifecycle;
pub mod manifest;
pub mod protocol;
pub mod session;

pub use host_services::HostServices;
pub use lifecycle::{FailureReason, KillMethod, LifecycleConfig, PluginHost, PluginState};
pub use manifest::Manifest;
pub use protocol::{Message, Notification, Request, Response};
