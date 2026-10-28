//! Shared domain imports for editor command modules.

pub use std::path::Path;
pub use std::sync::{Arc, Mutex, MutexGuard};
pub use oak_core::{Rational, TimeRange};
pub use oak_node::block::{AdjustmentBlockBehavior, BlockCore, ClipBlockBehavior};
pub use oak_node::folder::FolderBehavior;
pub use oak_node::footage::FootageBehavior;
pub use oak_node::graph::{Graph, NodeEntry};
pub use oak_node::id::NodeId;
pub use oak_node::project::Project;
pub use oak_node::sequence::SequenceBehavior;
pub use oak_node::track::{TrackBehavior, TrackListBehavior, TrackType};
pub use oak_node::value::VideoParams;
pub use oak_timeline::handle::CHandle;
pub use oak_timeline::util::{
	block_in, block_length, block_out, block_set_in, block_set_length_and_media_out,
	block_set_length_keeping_out, clip_set_media_in, NodeRef,
};
pub use oak_storage::backend::StorageBackend;
/// Shared ownership of the live project graph and its settings.
///
/// The mutex protects mutable project access; it does not replace the outer
/// editor-session lock that serializes session-level command state.
pub type ProjectRef = Arc<Mutex<Project>>;
