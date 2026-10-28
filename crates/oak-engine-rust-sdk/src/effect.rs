// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! Safe wrapper over the engine's effect-system C ABI (`oak_effect_*`):
//! the node factory table (the effect library's data source) and the
//! undoable effect-chain operations (walk / insert / remove / move /
//! enable) on a project's clips.
//!
//! Presentation (group keys, i18n names, sort order) is the app's
//! concern: this module returns the raw factory rows and lets the caller
//! filter by [`NODE_FLAG_VIDEO_EFFECT`] / [`NODE_FLAG_DONT_SHOW_IN_CREATE_MENU`]
//! and group by [`FactoryEntry::category`].

use crate::codec::string_out;
use crate::node::{NodeId, Project};

#[cfg_attr(
	windows,
	link(name = "oak_engine.dll.lib", kind = "dylib", modifiers = "+verbatim")
)]
#[cfg_attr(not(windows), link(name = "oak_engine", kind = "dylib"))]
unsafe extern "C" {
	fn oak_effect_factory_count() -> i32;
	fn oak_effect_factory_type_id_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_effect_factory_name_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_effect_factory_flags_at(index: i32) -> u64;
	fn oak_effect_factory_category_at(index: i32) -> i32;
	fn oak_effect_factory_dynamic_count() -> i32;
	fn oak_effect_factory_dynamic_type_id_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_effect_factory_dynamic_name_at(index: i32, buf: *mut u8, buf_len: usize) -> i32;
	fn oak_effect_factory_dynamic_sub_category_at(
		index: i32,
		buf: *mut u8,
		buf_len: usize,
	) -> i32;

	fn oak_effect_chain_count(project: *const std::ffi::c_void, host: u64) -> i32;
	fn oak_effect_chain_at(project: *const std::ffi::c_void, host: u64, index: i32) -> u64;
	fn oak_effect_chain_insert(
		project: *const std::ffi::c_void,
		host: u64,
		index: i32,
		type_id: *const u8,
		type_id_len: usize,
	) -> u64;
	fn oak_effect_chain_remove(project: *const std::ffi::c_void, host: u64, effect: u64) -> bool;
	fn oak_effect_chain_move(
		project: *const std::ffi::c_void,
		host: u64,
		effect: u64,
		new_index: i32,
	) -> bool;
	fn oak_effect_set_enabled(project: *const std::ffi::c_void, effect: u64, enabled: bool)
		-> bool;
	fn oak_effect_is_enabled(project: *const std::ffi::c_void, effect: u64) -> bool;
	fn oak_effect_input_id(project: *const std::ffi::c_void, node: u64, buf: *mut u8, len: usize) -> i32;
}

// Node flags (oak-node's `node::flags` — compile-time values mirrored
// across the FFI boundary, like `crate::core`'s oak-core constants).
/// `k_video_effect`: the node is a video effect (effect-library eligible).
pub const NODE_FLAG_VIDEO_EFFECT: u64 = 0x2;
/// `k_audio_effect`: the node is an audio effect.
pub const NODE_FLAG_AUDIO_EFFECT: u64 = 0x4;
/// `k_dont_show_in_create_menu`: hidden from the create menu / library.
pub const NODE_FLAG_DONT_SHOW_IN_CREATE_MENU: u64 = 0x8;

/// A node category, as the C++ `Node::CategoryID` ordinal the engine
/// reports for a factory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum NodeCategory {
	Output = 0,
	Effect = 1,
	Generator = 2,
	Input = 3,
	Math = 4,
	Color = 5,
	Distort = 6,
	Filter = 7,
	Keying = 8,
	OpenFx = 9,
	Timeline = 10,
	Group = 11,
}

impl NodeCategory {
	fn from_ordinal(ordinal: i32) -> Option<Self> {
		Some(match ordinal {
			0 => Self::Output,
			1 => Self::Effect,
			2 => Self::Generator,
			3 => Self::Input,
			4 => Self::Math,
			5 => Self::Color,
			6 => Self::Distort,
			7 => Self::Filter,
			8 => Self::Keying,
			9 => Self::OpenFx,
			10 => Self::Timeline,
			11 => Self::Group,
			_ => return None,
		})
	}
}

/// One static (built-in) factory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactoryEntry {
	/// The type id (what [`Project::effect_chain_insert`] takes back).
	pub type_id: String,
	/// The display name (English; the app localizes via its packs).
	pub name: String,
	/// The node flags (`NODE_FLAG_*`).
	pub flags: u64,
	/// The first category, when the entry carries one.
	pub category: Option<NodeCategory>,
}

/// One runtime-registered (OpenFX plugin) factory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicFactoryEntry {
	/// The type id (the plugin identifier).
	pub type_id: String,
	/// The display name (the plugin descriptor's label).
	pub name: String,
	/// The sub-category (the OFX context display name, e.g. "Filter").
	pub sub_category: String,
}

/// The built-in factory table (registration order, C++ parity).
pub fn factory_entries() -> Vec<FactoryEntry> {
	let count = unsafe { oak_effect_factory_count() }.max(0);
	(0..count)
		.map(|i| FactoryEntry {
			type_id: string_out(|buf, len| unsafe {
				oak_effect_factory_type_id_at(i, buf, len)
			})
			.unwrap_or_default(),
			name: string_out(|buf, len| unsafe { oak_effect_factory_name_at(i, buf, len) })
				.unwrap_or_default(),
			flags: unsafe { oak_effect_factory_flags_at(i) },
			category: NodeCategory::from_ordinal(unsafe { oak_effect_factory_category_at(i) }),
		})
		.collect()
}

/// The runtime-registered (OpenFX plugin) factory table, in registration
/// order. Empty until `crate::ofx::scan` + `register_plugin_nodes` ran.
pub fn dynamic_factory_entries() -> Vec<DynamicFactoryEntry> {
	let count = unsafe { oak_effect_factory_dynamic_count() }.max(0);
	(0..count)
		.map(|i| DynamicFactoryEntry {
			type_id: string_out(|buf, len| unsafe {
				oak_effect_factory_dynamic_type_id_at(i, buf, len)
			})
			.unwrap_or_default(),
			name: string_out(|buf, len| unsafe {
				oak_effect_factory_dynamic_name_at(i, buf, len)
			})
			.unwrap_or_default(),
			sub_category: string_out(|buf, len| unsafe {
				oak_effect_factory_dynamic_sub_category_at(i, buf, len)
			})
			.unwrap_or_default(),
		})
		.collect()
}

impl Project {
	/// Returns the input that carries a node's effect chain, if it has one.
	pub fn effect_input_id(&self, node: NodeId) -> Option<String> {
		string_out(|buf, len| unsafe { oak_effect_input_id(self.as_raw(), node.raw(), buf, len) })
	}

	/// The effect chain of `host`, closest-to-source first (signal order).
	/// Empty when the host is unknown or cannot host effects.
	pub fn effect_chain(&self, host: NodeId) -> Vec<NodeId> {
		let count = unsafe { oak_effect_chain_count(self.as_raw(), host.raw()) }.max(0);
		(0..count)
			.map(|i| NodeId::from_raw(unsafe { oak_effect_chain_at(self.as_raw(), host.raw(), i) }))
			.collect()
	}

	/// Undoable insert ("Add Effect") of a new effect of `type_id` at
	/// chain position `index` (clamped to the ends; `usize::MAX`
	/// appends). Returns the new node, or `None` on failure (unknown
	/// host/type, or the host/type cannot host effects).
	pub fn effect_chain_insert(
		&self,
		host: NodeId,
		index: usize,
		type_id: &str,
	) -> Option<NodeId> {
		let index = index.min(i32::MAX as usize) as i32;
		let id = unsafe {
			oak_effect_chain_insert(
				self.as_raw(),
				host.raw(),
				index,
				type_id.as_ptr(),
				type_id.len(),
			)
		};
		let id = NodeId::from_raw(id);
		id.is_valid().then_some(id)
	}

	/// Undoable removal ("Remove Effect") of `effect` from `host`'s
	/// chain. The node stays in the project graph, orphaned (documented
	/// engine limitation: node transfer is one-way).
	pub fn effect_chain_remove(&self, host: NodeId, effect: NodeId) -> bool {
		unsafe { oak_effect_chain_remove(self.as_raw(), host.raw(), effect.raw()) }
	}

	/// Undoable reorder ("Reorder Effect") of `effect` to chain position
	/// `new_index` (an insertion index **after** removal).
	pub fn effect_chain_move(&self, host: NodeId, effect: NodeId, new_index: usize) -> bool {
		let new_index = new_index.min(i32::MAX as usize) as i32;
		unsafe { oak_effect_chain_move(self.as_raw(), host.raw(), effect.raw(), new_index) }
	}

	/// Undoable enable toggle ("Toggle Effect") of an effect node.
	pub fn effect_set_enabled(&self, effect: NodeId, enabled: bool) -> bool {
		unsafe { oak_effect_set_enabled(self.as_raw(), effect.raw(), enabled) }
	}

	/// Whether the effect node is enabled.
	pub fn effect_is_enabled(&self, effect: NodeId) -> bool {
		unsafe { oak_effect_is_enabled(self.as_raw(), effect.raw()) }
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The global undo stack is process-wide; serialize the undoing tests.
	fn undo_lock() -> std::sync::MutexGuard<'static, ()> {
		crate::node::UNDO_SERIAL
			.lock()
			.unwrap_or_else(|e| e.into_inner())
	}

	/// A video-effect type id the factory really registers.
	fn some_effect_type() -> String {
		factory_entries()
			.into_iter()
			.find(|e| {
				e.flags & NODE_FLAG_VIDEO_EFFECT != 0
					&& e.flags & NODE_FLAG_DONT_SHOW_IN_CREATE_MENU == 0
			})
			.expect("the factory registers at least one video effect")
			.type_id
	}

	/// The factory table through the wrapper: non-empty, video effects
	/// carry flags and a category.
	#[test]
	fn factory_table_enumerates() {
		let entries = factory_entries();
		assert!(entries.len() > 60, "the built-in node types ({})", entries.len());
		let video_effects = entries
			.iter()
			.filter(|e| e.flags & NODE_FLAG_VIDEO_EFFECT != 0)
			.count();
		assert!(video_effects > 10, "the video effects ({video_effects})");
		// No dynamic entries without a plugin scan, but the call answers.
		let _ = dynamic_factory_entries();
	}

	/// The chain edits through the wrapper: insert (clamped), walk,
	/// remove (bridges), move, enable — each one undo row.
	#[test]
	fn chain_edits_walk_and_undo() {
		let _guard = undo_lock();
		assert!(crate::node::undo_clear());
		let project = Project::new();
		let host = project.create_clip().expect("clip");
		let ty = some_effect_type();

		let first = project
			.effect_chain_insert(host, 0, &ty)
			.expect("insert into an empty chain");
		assert_eq!(project.effect_chain(host), vec![first]);
		let second = project
			.effect_chain_insert(host, 0, &ty)
			.expect("insert at the source side");
		assert_eq!(project.effect_chain(host), vec![second, first]);
		let third = project
			.effect_chain_insert(host, usize::MAX, &ty)
			.expect("append (clamped)");
		assert_eq!(project.effect_chain(host), vec![second, first, third]);

		// One undo row per insert.
		assert!(crate::node::undo());
		assert_eq!(project.effect_chain(host), vec![second, first]);
		assert!(crate::node::undo());
		assert_eq!(project.effect_chain(host), vec![first]);
		assert!(crate::node::redo());
		assert!(crate::node::redo());
		assert_eq!(project.effect_chain(host), vec![second, first, third]);

		// Remove the middle effect: the gap bridges.
		assert!(project.effect_chain_remove(host, first));
		assert_eq!(project.effect_chain(host), vec![second, third]);
		assert!(crate::node::undo());
		assert_eq!(project.effect_chain(host), vec![second, first, third]);

		// Reorder: move the source-side effect to the end.
		assert!(project.effect_chain_move(host, second, 2));
		assert_eq!(project.effect_chain(host), vec![first, third, second]);
		assert!(crate::node::undo());
		assert_eq!(project.effect_chain(host), vec![second, first, third]);

		// Enable toggle.
		assert!(project.effect_is_enabled(first));
		assert!(project.effect_set_enabled(first, false));
		assert!(!project.effect_is_enabled(first));
		assert!(crate::node::undo());
		assert!(project.effect_is_enabled(first));

		// Failure paths.
		assert!(project.effect_chain_insert(host, 0, "no.such.type").is_none());
		assert!(!project.effect_chain_remove(host, NodeId::INVALID));

		assert!(crate::node::undo_clear());
	}
}
