//! Safe Rust SDK over the oak-engine C ABI.
//!
//! The engine ships as a cdylib; the SDK links it dynamically
//! (`#[link]`) and talks to it exclusively through its exported
//! `extern "C"` symbols — the same surface a C consumer sees — wrapped
//! in idiomatic Rust types and error handling.

pub mod audio;
pub mod codec;
pub mod color;
pub mod core;
pub mod effect;
pub mod editor;
pub mod node;
pub mod ofx;
pub mod otio;
pub mod plugin;
pub mod render;
pub mod state;
pub mod storage;
pub mod task;
pub mod timeline;
pub mod types;
pub mod utils;
pub mod vecs;

#[cfg(test)]
mod test_support;
