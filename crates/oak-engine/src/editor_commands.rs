//! Composite editor commands grouped by domain.
//!
//! These functions operate on the session's live `ProjectRef` and use oak-undo
//! commands for user-visible edits. The dispatch layer supplies sequence/session
//! context; this layer owns graph validation and undo grouping.

pub mod prelude;
pub use prelude::*;
mod project;
pub use project::*;
mod sequence;
pub use sequence::*;
mod media;
pub use media::*;
mod library;
pub use library::*;
mod tracks;
pub use tracks::*;
mod transitions;
pub use transitions::*;
mod generators;
pub use generators::*;
mod editing;
pub use editing::*;
mod moving;
pub use moving::*;
mod edit_menu;
pub use edit_menu::*;
#[cfg(test)]
mod tests;
