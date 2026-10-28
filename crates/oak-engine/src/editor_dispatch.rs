//! Semantic editor commands with native C-ABI data carriers.
//!
//! `abi` decodes caller buffers, `command` owns the decoded request, `execute`
//! performs one domain operation, and `types` carries native results back.

mod command;
mod execute;
mod types;
pub mod abi;

pub use command::*;
pub(crate) use execute::*;
pub use types::*;
