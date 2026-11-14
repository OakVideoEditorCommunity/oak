//! The v2 tool face: tools added after the frozen 22 (T9.2, plan §6 方案 B).
//!
//! Nothing here touches the frozen 22: their schemas, their registry and
//! [`crate::backend::EngineBackend`] stay byte-identical. A v2 tool is
//! parsed here and dispatched to [`crate::backend::EngineExt`].

pub mod sequence;
