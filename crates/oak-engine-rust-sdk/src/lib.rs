//! Safe Rust SDK over the oak-engine C ABI.
//!
//! The engine ships as a cdylib; the SDK links it dynamically
//! (`#[link]`) and talks to it exclusively through its exported
//! `extern "C"` symbols — the same surface a C consumer sees — wrapped
//! in idiomatic Rust types and error handling.

pub mod audio;
pub mod vecs;
