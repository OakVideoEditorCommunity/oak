//! Owned node-graph snapshots and live connection validation for editor sessions.
//!
//! The snapshot is built once and reference counted across the C ABI, so all
//! nodes, ports and edges observed by a consumer come from the same revision.

mod abi;
pub mod snapshot;

pub use abi::*;
