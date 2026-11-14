//! Typed argument parsing for the 22 frozen tools.
//!
//! Parsing runs *after* JSON Schema validation (the schema is the
//! contract), so these parsers are the second gate: they turn `Value`
//! into typed arguments, resolve the two accepted time shapes and turn
//! shape problems into structured `ToolError`s instead of panics.
//! Backends share them, so the mock and the SDK parse identically.

pub mod common;
pub mod edit;
pub mod export;
pub mod media;
pub mod node;
pub mod project;
pub mod render_tools;
pub mod timeline;
pub mod v2;
