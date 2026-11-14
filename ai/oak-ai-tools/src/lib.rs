//! T1 tool execution layer for the Oak AI editing pipeline.
//!
//! The crate turns the frozen T0 tool face (`ai/schema/tools/*.json`,
//! 22 tools) into callable operations:
//!
//! * [`schema`] compiles and enforces the frozen JSON Schemas;
//! * [`dispatch`] binds each tool name to a parser + backend method and
//!   runs it through validation and one undo group;
//! * [`txn`] owns the "one tool call = one undo group" transaction
//!   orchestration (begin / commit / abort), which the model never sees;
//! * [`hooks`] carries the adapter-layer interception points (plan §4):
//!   the confirmation gate (`Policy` + the 22-tool grading table), the
//!   readback assertion trigger and the structured failure feedback;
//! * [`backend`] is the backend contract, implemented twice:
//!   [`mock::MockBackend`] (in-memory, feature-free, used by the unit
//!   tests and the mock E2E chain) and `sdk::SdkBackend` (feature
//!   `sdk`, the real engine through `oak-engine-rust-sdk`);
//! * [`render`] carries frames as F32 RGBA / PNG with golden-frame
//!   assertion helpers;
//! * [`timecode`] enforces the two accepted time shapes (rational
//!   seconds and frame indices) and rejects bare floats at the parsing
//!   layer.
//!
//! Feature gates:
//!
//! * `default = []` — no engine dependency; builds and tests anywhere;
//! * `sdk` — links `oak-engine-rust-sdk` (and through it the
//!   `oak_engine` cdylib) and enables [`sdk::SdkBackend`] plus the
//!   real-machine E2E test.

pub mod backend;
pub mod dispatch;
pub mod error;
pub mod hooks;
pub mod mock;
pub mod render;
#[cfg(feature = "sdk")]
pub mod sdk;
pub mod schema;
pub mod schema_v2;
pub mod timecode;
pub mod tools;
pub mod txn;
