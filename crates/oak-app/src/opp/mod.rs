//! OPP/1 host wiring, app side (T7.2 P1.2).
//!
//! [`host`] implements `oak_extensions::HostServices` against the real
//! engine (`graphops` / `renderops` / `effectchain` / the SDK undo stack), routing
//! six domains of plugin calls into Oak and answering a structured
//! `Unsupported` for everything the spec marks 🆕. [`dispatch`] turns one
//! wire request into that call, falling through to the P1 protocol handler
//! for `events.*` / `session.*`; [`discover`] scans plugin manifests and
//! starts them behind `OAK_OPP_PLUGINS`.
//!
//! The GUI hookup (P2) only needs [`discover::start_enabled`] plus a shared
//! [`host::OppHost`]; P1.2 keeps the switch environment-only on purpose.

pub mod discover;
pub mod dispatch;
pub mod host;

pub use discover::{discover, discover_all, search_dirs, start_enabled, DiscoveredPlugin};
pub use dispatch::dispatch;
pub use host::OppHost;
