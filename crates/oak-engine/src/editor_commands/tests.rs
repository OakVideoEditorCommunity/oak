//! Migrated domain-command tests, grouped by editing behavior rather than ABI.
//! Shared fixtures live in `helpers`; cross-module regression scenarios remain here.

#![allow(unused_imports)]
use crate::editor_commands::*;

mod tracks;
pub(crate) use tracks::*;
mod history;
pub(crate) use history::*;
mod adjustment;
pub(crate) use adjustment::*;
mod transitions;
pub(crate) use transitions::*;
mod helpers;
pub(crate) use helpers::*;
mod graph;
pub(crate) use graph::*;
mod placement;
pub(crate) use placement::*;
mod editing;
pub(crate) use editing::*;
mod documents;
pub(crate) use documents::*;
