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

//! C ABI for the effect system (prefix `oak_effect_*`): the node factory
//! table (the effect library's data source) and the undoable effect-chain
//! operations (walk / insert / remove / move / enable).
//!
//! The chain semantics mirror `oak-app`'s `oakui::effectchain` exactly —
//! chain order is closest-to-source first, insertion indices clamp to the
//! ends, reorder takes an insertion index after removal — and every edit
//! is one undo row on the global stack ("Add Effect" / "Remove Effect" /
//! "Reorder Effect" / "Toggle Effect"). These are the operations the
//! retired facade exported as `oakengine_node_effect_*`; sunk into the
//! engine, they no longer depend on app-side composition.

use oak_node::factory::Factory;
use oak_node::graph::Graph;
use oak_node::id::NodeId;
use oak_node::node::ENABLED_INPUT;
use oak_node::value::NodeValue;
use oak_undo::undocommand::UndoCommand;

use crate::codec::{copy_str_out, str_arg};
use crate::node::{OAK_NODE_ID_INVALID, ProjectHandle};

// ---------------------------------------------------------------------------
// Pure graph helpers (the chain walk)
// ---------------------------------------------------------------------------

/// The effect-input id of `node`, or `None` when the node cannot host
/// effects (C++ `Node::GetEffectInputID`).
pub(crate) fn effect_input_of(g: &Graph, node: NodeId) -> Option<String> {
	let id = &g.get(node)?.core.effect_input;
	if id.is_empty() { None } else { Some(id.clone()) }
}

/// The node feeding `node`'s input `input_id`, or `None` when the input
/// is unconnected.
fn connected_node(g: &Graph, node: NodeId, input_id: &str) -> Option<NodeId> {
	g.connected_output(node, input_id, -1)
}

/// The effect chain of `host`, **closest-to-source first** (signal order:
/// the first element feeds the media side, the last feeds `host`'s effect
/// input). The walk follows each node's effect input upstream until an
/// unconnected input or a node without an effect input; a `seen` guard
/// protects against malformed cycles.
pub(crate) fn chain(g: &Graph, host: NodeId) -> Vec<NodeId> {
	let mut chain = Vec::new();
	let mut cur = host;
	let mut seen: Vec<NodeId> = Vec::new();
	loop {
		if seen.contains(&cur) {
			break;
		}
		seen.push(cur);
		let Some(input) = effect_input_of(g, cur) else {
			break;
		};
		let Some(up) = connected_node(g, cur, &input) else {
			break;
		};
		chain.push(up);
		cur = up;
	}
	chain.reverse();
	chain
}

/// Whether `node` is enabled (the `enabled_in` standard value; the effect
/// stack's enable toggle).
pub(crate) fn is_enabled(g: &Graph, node: NodeId) -> bool {
	g.get(node)
		.map(|e| {
			matches!(
				e.core.standard_value(ENABLED_INPUT, -1),
				NodeValue::Boolean(true)
			)
		})
		.unwrap_or(false)
}

/// Copies a node's effect-input ID using the bounded-string convention.
/// Returns `-1` for a null project, stale node or node without an effect input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_effect_input_id(
	project: *const ProjectHandle,
	node: u64,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if project.is_null() {
		return -1;
	}
	let Some(node) = NodeId::from_identity(node) else {
		return -1;
	};
	crate::node::with_project(project, |p| effect_input_of(&p.graph, node))
		.flatten()
		.map(|id| copy_str_out(&id, buf, buf_len))
		.unwrap_or(-1)
}

/// The neighbors of chain position `pos` in `chain`: `(upstream,
/// downstream)` — the chain source is `None`, the host closes the chain.
fn neighbors(g: &Graph, host: NodeId, chain: &[NodeId], pos: usize) -> (Option<NodeId>, NodeId) {
	let len = chain.len();
	if pos == 0 {
		if len == 0 {
			(None, host)
		} else {
			let d = chain[0];
			let up = effect_input_of(g, d).and_then(|i| connected_node(g, d, &i));
			(up, d)
		}
	} else if pos >= len {
		(Some(chain[len - 1]), host)
	} else {
		(Some(chain[pos - 1]), chain[pos])
	}
}

// ---------------------------------------------------------------------------
// Undo plumbing (oak-app's graphops helpers, engine-local)
// ---------------------------------------------------------------------------

type ProjectRef = std::sync::Arc<std::sync::Mutex<oak_node::project::Project>>;

fn lock(p: &ProjectRef) -> std::sync::MutexGuard<'_, oak_node::project::Project> {
	p.lock().unwrap_or_else(|e| e.into_inner())
}

fn push(cmd: UndoCommand, name: &str) -> Result<(), String> {
	oak_undo::global::push(cmd, name).map_err(|e| e.to_string())
}

/// Runs `f` as one undo group (one undo row for the whole edit).
fn grouped<T>(name: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
	oak_undo::global::group_begin(name).map_err(|e| e.to_string())?;
	match f() {
		Ok(v) => {
			oak_undo::global::group_end().map_err(|e| e.to_string())?;
			Ok(v)
		}
		Err(e) => {
			let _ = oak_undo::global::group_abort();
			Err(e)
		}
	}
}

/// An undoable edge add: validated at creation (existence, connectability,
/// not already connected); the redo connects, the undo disconnects.
fn connect_command(
	p: &ProjectRef,
	from: NodeId,
	to: NodeId,
	input_id: &str,
) -> Result<UndoCommand, String> {
	{
		let g = lock(p);
		if !g.graph.is_valid(from) || !g.graph.is_valid(to) {
			return Err("connect: node not found".to_string());
		}
		let entry = g.graph.get(to).ok_or("connect: node not found")?;
		let input = entry
			.core
			.get_input(input_id)
			.ok_or_else(|| format!("connect: no input \"{input_id}\""))?;
		if !input.is_connectable() {
			return Err(format!("connect: input \"{input_id}\" is not connectable"));
		}
		if g.graph.connected_output(to, input_id, -1).is_some() {
			return Err(format!(
				"connect: input \"{input_id}\" is already connected"
			));
		}
	}
	let (p1, p2) = (p.clone(), p.clone());
	let (id1, id2) = (input_id.to_string(), input_id.to_string());
	Ok(UndoCommand::from_closures(
		move || {
			let mut g = lock(&p1);
			let _ = g.graph.connect(from, to, &id1, -1);
		},
		move || {
			let mut g = lock(&p2);
			g.graph.disconnect_input(to, &id2, -1);
		},
	))
}

/// An undoable edge remove: succeeds even when nothing is connected; the
/// undo re-connects the edge captured at construction.
fn disconnect_command(p: &ProjectRef, to: NodeId, input_id: &str) -> Result<UndoCommand, String> {
	let source = {
		let g = lock(p);
		let has_input = g
			.graph
			.get(to)
			.map(|e| e.core.has_input(input_id))
			.unwrap_or(false);
		if !has_input {
			return Err(format!("disconnect: no input \"{input_id}\""));
		}
		g.graph.connected_output(to, input_id, -1)
	};
	let (p1, p2) = (p.clone(), p.clone());
	let (id1, id2) = (input_id.to_string(), input_id.to_string());
	Ok(UndoCommand::from_closures(
		move || {
			let mut g = lock(&p1);
			g.graph.disconnect_input(to, &id1, -1);
		},
		move || {
			if let Some(from) = source {
				let mut g = lock(&p2);
				let _ = g.graph.connect(from, to, &id2, -1);
			}
		},
	))
}

/// The shared state of an add-node command: the entry awaiting its first
/// redo, and the live id once assigned.
type AddNodeState = std::sync::Arc<
	std::sync::Mutex<(Option<oak_node::graph::NodeEntry>, Option<NodeId>)>,
>;

/// An undoable node add: the redo moves the entry into the graph
/// (re-adding under the same id on re-redo), the undo takes it back out.
/// The new id lands in the returned state for follow-up wiring.
fn add_node_command(p: &ProjectRef, entry: oak_node::graph::NodeEntry) -> (UndoCommand, AddNodeState) {
	let state: AddNodeState = std::sync::Arc::new(std::sync::Mutex::new((Some(entry), None)));
	let (s1, s2) = (state.clone(), state.clone());
	let (p1, p2) = (p.clone(), p.clone());
	(
		UndoCommand::from_closures(
			move || {
				let mut st = s1.lock().unwrap_or_else(|e| e.into_inner());
				let Some(entry) = st.0.take() else {
					return;
				};
				let mut g = lock(&p1);
				st.1 = Some(match st.1 {
					Some(id) => g.graph.add_entry(entry, id),
					None => g.graph.add_node(entry.core, entry.behavior),
				});
			},
			move || {
				let mut st = s2.lock().unwrap_or_else(|e| e.into_inner());
				let Some(id) = st.1 else {
					return;
				};
				let mut g = lock(&p2);
				st.0 = g.graph.take_node(id);
			},
		),
		state,
	)
}

// ---------------------------------------------------------------------------
// The undoable chain edits
// ---------------------------------------------------------------------------

/// Undoable insertion of a new effect of `type_id` at chain position
/// `index` (0 = closest to the source; out-of-range indices clamp to the
/// ends). One undo row ("Add Effect"). Returns the new node's id.
pub(crate) fn insert_impl(
	p: &ProjectRef,
	host: NodeId,
	index: usize,
	type_id: &str,
) -> Result<NodeId, String> {
	let (chain_vec, new_entry, new_input) = {
		let g = lock(p);
		if effect_input_of(&g.graph, host).is_none() {
			return Err("node cannot host effects (no effect input)".to_string());
		}
		let Some((core, behavior)) = Factory::global().create_any(type_id) else {
			return Err(format!("unknown node type id \"{type_id}\""));
		};
		if core.effect_input.is_empty() {
			return Err("node type has no effect input; cannot be chained".to_string());
		}
		let new_input = core.effect_input.clone();
		let entry = oak_node::graph::NodeEntry {
			core,
			behavior,
			generation: 0,
			vacant: false,
		};
		(chain(&g.graph, host), entry, new_input)
	};
	let pos = index.min(chain_vec.len());
	let (upstream, downstream) = {
		let g = lock(p);
		neighbors(&g.graph, host, &chain_vec, pos)
	};
	let downstream_input = {
		let g = lock(p);
		effect_input_of(&g.graph, downstream).unwrap_or_default()
	};

	grouped("Add Effect", || {
		let (add, state) = add_node_command(p, new_entry);
		push(add, "Add Effect")?;
		let fresh = state
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.1
			.ok_or_else(|| "the add-node command produced no id".to_string())?;
		if upstream.is_some() {
			push(disconnect_command(p, downstream, &downstream_input)?, "Add Effect")?;
		}
		push(connect_command(p, fresh, downstream, &downstream_input)?, "Add Effect")?;
		if let Some(upstream) = upstream {
			push(connect_command(p, upstream, fresh, &new_input)?, "Add Effect")?;
		}
		Ok(fresh)
	})
}

/// Undoable removal of `effect`: unhook both edges and bridge the gap,
/// one undo row ("Remove Effect"). The node itself is left orphaned in
/// the project graph: the node-transfer commands are one-way, so a
/// reversible detach does not exist — documented limitation carried over
/// from the facade.
pub(crate) fn remove_impl(p: &ProjectRef, host: NodeId, effect: NodeId) -> Result<(), String> {
	let (chain_vec, pos) = {
		let g = lock(p);
		let chain_vec = chain(&g.graph, host);
		let Some(pos) = chain_vec.iter().position(|&c| c == effect) else {
			return Err("remove effect: the node is not in the chain".to_string());
		};
		(chain_vec, pos)
	};
	let (upstream, downstream) = {
		let g = lock(p);
		let upstream = if pos == 0 {
			effect_input_of(&g.graph, chain_vec[0])
				.and_then(|i| connected_node(&g.graph, chain_vec[0], &i))
		} else {
			Some(chain_vec[pos - 1])
		};
		let downstream = if pos + 1 == chain_vec.len() {
			host
		} else {
			chain_vec[pos + 1]
		};
		(upstream, downstream)
	};
	let (eff_input, downstream_input) = {
		let g = lock(p);
		(
			effect_input_of(&g.graph, effect).unwrap_or_default(),
			effect_input_of(&g.graph, downstream).unwrap_or_default(),
		)
	};

	grouped("Remove Effect", || {
		if upstream.is_some() {
			push(disconnect_command(p, effect, &eff_input)?, "Remove Effect")?;
		}
		push(
			disconnect_command(p, downstream, &downstream_input)?,
			"Remove Effect",
		)?;
		if let Some(upstream) = upstream {
			push(
				connect_command(p, upstream, downstream, &downstream_input)?,
				"Remove Effect",
			)?;
		}
		Ok(())
	})
}

/// Undoable reorder of `effect` to chain position `new_index` (an
/// insertion index **after** removal; `0..=len-1` where `len` is the
/// post-removal chain length). One undo row ("Reorder Effect").
pub(crate) fn move_impl(p: &ProjectRef, host: NodeId, effect: NodeId, new_index: usize) -> Result<(), String> {
	let (chain_vec, from) = {
		let g = lock(p);
		let chain_vec = chain(&g.graph, host);
		let Some(from) = chain_vec.iter().position(|&c| c == effect) else {
			return Err("reorder effect: the node is not in the chain".to_string());
		};
		(chain_vec, from)
	};
	let len = chain_vec.len();
	let to = new_index.min(len - 1);

	let (upstream, downstream, eff_input, downstream_input) = {
		let g = lock(p);
		let upstream = if from == 0 {
			effect_input_of(&g.graph, chain_vec[0])
				.and_then(|i| connected_node(&g.graph, chain_vec[0], &i))
		} else {
			Some(chain_vec[from - 1])
		};
		let downstream = if from + 1 == len {
			host
		} else {
			chain_vec[from + 1]
		};
		(
			upstream,
			downstream,
			effect_input_of(&g.graph, effect).unwrap_or_default(),
			effect_input_of(&g.graph, downstream).unwrap_or_default(),
		)
	};
	// The post-removal chain and the reinsertion neighbors: position `to`
	// sits between index `to - 1` and `to` of the remaining list, where
	// index `-1` is the chain source and index `len-1` is the host.
	let remaining: Vec<NodeId> = chain_vec
		.iter()
		.enumerate()
		.filter(|(i, _)| *i != from)
		.map(|(_, &c)| c)
		.collect();
	let (up2, down2) = {
		let g = lock(p);
		let rlen = remaining.len();
		if to == 0 {
			if rlen == 0 {
				(None, host)
			} else {
				let d = remaining[0];
				let up = effect_input_of(&g.graph, d).and_then(|i| connected_node(&g.graph, d, &i));
				(up, d)
			}
		} else if to >= rlen {
			(Some(remaining[rlen - 1]), host)
		} else {
			(Some(remaining[to - 1]), remaining[to])
		}
	};
	let down2_input = {
		let g = lock(p);
		effect_input_of(&g.graph, down2).unwrap_or_default()
	};

	grouped("Reorder Effect", || {
		// Unhook the effect from its current neighbors and bridge the gap…
		if upstream.is_some() {
			push(disconnect_command(p, effect, &eff_input)?, "Reorder Effect")?;
		}
		push(
			disconnect_command(p, downstream, &downstream_input)?,
			"Reorder Effect",
		)?;
		if let Some(upstream) = upstream {
			push(
				connect_command(p, upstream, downstream, &downstream_input)?,
				"Reorder Effect",
			)?;
		}
		// …then wire it between its new neighbors.
		push(disconnect_command(p, down2, &down2_input)?, "Reorder Effect")?;
		push(connect_command(p, effect, down2, &down2_input)?, "Reorder Effect")?;
		if let Some(up2) = up2 {
			push(connect_command(p, up2, effect, &eff_input)?, "Reorder Effect")?;
		}
		Ok(())
	})
}

/// Undoable enable toggle of an effect node ("Toggle Effect"; the
/// `enabled_in` flag).
pub(crate) fn set_enabled_impl(p: &ProjectRef, effect: NodeId, enabled: bool) -> Result<(), String> {
	let old = {
		let g = lock(p);
		if !g.graph.is_valid(effect) {
			return Err("toggle effect: node not found".to_string());
		}
		is_enabled(&g.graph, effect)
	};
	let (p1, p2) = (p.clone(), p.clone());
	push(
		UndoCommand::from_closures(
			move || {
				let mut g = lock(&p1);
				if let Some(e) = g.graph.get_mut(effect) {
					e.core
						.set_standard_value(ENABLED_INPUT, -1, NodeValue::Boolean(enabled));
				}
			},
			move || {
				let mut g = lock(&p2);
				if let Some(e) = g.graph.get_mut(effect) {
					e.core
						.set_standard_value(ENABLED_INPUT, -1, NodeValue::Boolean(old));
				}
			},
		),
		"Toggle Effect",
	)
}

// ---------------------------------------------------------------------------
// C ABI: the factory table
// ---------------------------------------------------------------------------

/// Number of static (built-in) factory entries.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_count() -> i32 {
	Factory::global().entries().len() as i32
}

/// The type id of the static entry at `index`, snprintf-style; -1 when
/// out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_type_id_at(index: i32, buf: *mut u8, buf_len: usize) -> i32 {
	let entries = Factory::global().entries();
	if index < 0 || index as usize >= entries.len() {
		return -1;
	}
	copy_str_out(entries[index as usize].type_id, buf, buf_len)
}

/// The display name of the static entry at `index`, snprintf-style; -1
/// when out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_name_at(index: i32, buf: *mut u8, buf_len: usize) -> i32 {
	let entries = Factory::global().entries();
	if index < 0 || index as usize >= entries.len() {
		return -1;
	}
	copy_str_out(entries[index as usize].name, buf, buf_len)
}

/// The flags of the static entry at `index` (oak-node's `node::flags`:
/// VIDEO_EFFECT = 0x2, DONT_SHOW_IN_CREATE_MENU = 0x8, …). Reading the
/// flags needs a scratch instance — the factory metadata carries no flag
/// copy (C++ parity). 0 when out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_flags_at(index: i32) -> u64 {
	let entries = Factory::global().entries();
	if index < 0 || index as usize >= entries.len() {
		return 0;
	}
	let (core, _behavior) = (entries[index as usize].create)();
	core.flags
}

/// The first category of the static entry at `index`, as the C++
/// `Node::CategoryID` ordinal (Output=0, Effect=1, Generator=2, Input=3,
/// Math=4, Color=5, Distort=6, Filter=7, Keying=8, OpenFx=9, Timeline=10,
/// Group=11); -1 when the entry has no category or is out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_category_at(index: i32) -> i32 {
	let entries = Factory::global().entries();
	if index < 0 || index as usize >= entries.len() {
		return -1;
	}
	entries[index as usize]
		.categories
		.first()
		.map(|c| *c as i32)
		.unwrap_or(-1)
}

/// Number of runtime-registered (OpenFX plugin) factory entries.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_dynamic_count() -> i32 {
	Factory::global().dynamic_entries().len() as i32
}

/// The type id (the plugin identifier) of the dynamic entry at `index`,
/// snprintf-style; -1 when out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_dynamic_type_id_at(
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let entries = Factory::global().dynamic_entries();
	if index < 0 || index as usize >= entries.len() {
		return -1;
	}
	copy_str_out(&entries[index as usize].type_id, buf, buf_len)
}

/// The display name (the plugin descriptor's label) of the dynamic entry
/// at `index`, snprintf-style; -1 when out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_dynamic_name_at(
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let entries = Factory::global().dynamic_entries();
	if index < 0 || index as usize >= entries.len() {
		return -1;
	}
	copy_str_out(&entries[index as usize].name, buf, buf_len)
}

/// The sub-category (the OFX context display name, e.g. "Filter") of the
/// dynamic entry at `index`, snprintf-style; -1 when out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_factory_dynamic_sub_category_at(
	index: i32,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	let entries = Factory::global().dynamic_entries();
	if index < 0 || index as usize >= entries.len() {
		return -1;
	}
	copy_str_out(&entries[index as usize].sub_category, buf, buf_len)
}

// ---------------------------------------------------------------------------
// C ABI: the chain
// ---------------------------------------------------------------------------

/// The length of `host`'s effect chain (closest-to-source first); 0 when
/// the project/host is unknown or the host cannot host effects.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_chain_count(project: *const ProjectHandle, host: u64) -> i32 {
	let Some(host) = NodeId::from_identity(host) else {
		return 0;
	};
	crate::node::with_project(project, |p| chain(&p.graph, host).len() as i32).unwrap_or(0)
}

/// The chain node at `index` (closest-to-source first);
/// `OAK_NODE_ID_INVALID` when out of range.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_chain_at(project: *const ProjectHandle, host: u64, index: i32) -> u64 {
	let Some(host) = NodeId::from_identity(host) else {
		return OAK_NODE_ID_INVALID;
	};
	if index < 0 {
		return OAK_NODE_ID_INVALID;
	}
	crate::node::with_project(project, |p| {
		chain(&p.graph, host)
			.get(index as usize)
			.map(|id| id.identity())
			.unwrap_or(OAK_NODE_ID_INVALID)
	})
	.unwrap_or(OAK_NODE_ID_INVALID)
}

/// Undoable insert ("Add Effect") of a new effect of `type_id` at chain
/// position `index` (clamped to the ends); the new node's id, or
/// `OAK_NODE_ID_INVALID` on failure (unknown project/host/type, or the
/// host/type cannot host effects).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_effect_chain_insert(
	project: *const ProjectHandle,
	host: u64,
	index: i32,
	type_id: *const u8,
	type_id_len: usize,
) -> u64 {
	let (Some(host), Some(type_id)) = (
		NodeId::from_identity(host),
		(unsafe { str_arg(type_id, type_id_len) }),
	) else {
		return OAK_NODE_ID_INVALID;
	};
	if project.is_null() {
		return OAK_NODE_ID_INVALID;
	}
	let inner = unsafe { &*project }.project().clone();
	let index = if index < 0 { 0 } else { index as usize };
	insert_impl(&inner, host, index, type_id)
		.map(|id| id.identity())
		.unwrap_or(OAK_NODE_ID_INVALID)
}

/// Undoable removal ("Remove Effect") of `effect` from `host`'s chain;
/// false on failure.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_chain_remove(
	project: *const ProjectHandle,
	host: u64,
	effect: u64,
) -> bool {
	let (Some(host), Some(effect)) =
		(NodeId::from_identity(host), NodeId::from_identity(effect))
	else {
		return false;
	};
	if project.is_null() {
		return false;
	}
	let inner = unsafe { &*project }.project().clone();
	remove_impl(&inner, host, effect).is_ok()
}

/// Undoable reorder ("Reorder Effect") of `effect` to chain position
/// `new_index` (an insertion index after removal); false on failure.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_chain_move(
	project: *const ProjectHandle,
	host: u64,
	effect: u64,
	new_index: i32,
) -> bool {
	let (Some(host), Some(effect)) =
		(NodeId::from_identity(host), NodeId::from_identity(effect))
	else {
		return false;
	};
	if project.is_null() {
		return false;
	}
	let inner = unsafe { &*project }.project().clone();
	let new_index = if new_index < 0 { 0 } else { new_index as usize };
	move_impl(&inner, host, effect, new_index).is_ok()
}

/// Undoable enable toggle ("Toggle Effect") of an effect node; false on
/// failure.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_set_enabled(
	project: *const ProjectHandle,
	effect: u64,
	enabled: bool,
) -> bool {
	let Some(effect) = NodeId::from_identity(effect) else {
		return false;
	};
	if project.is_null() {
		return false;
	}
	let inner = unsafe { &*project }.project().clone();
	set_enabled_impl(&inner, effect, enabled).is_ok()
}

/// Whether the effect node is enabled; false when unknown.
#[unsafe(no_mangle)]
pub extern "C" fn oak_effect_is_enabled(project: *const ProjectHandle, effect: u64) -> bool {
	let Some(effect) = NodeId::from_identity(effect) else {
		return false;
	};
	crate::node::with_project(project, |p| is_enabled(&p.graph, effect)).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;

	fn cstr_arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	/// The global undo stack is process-wide; serialize these tests.
	fn undo_lock() -> std::sync::MutexGuard<'static, ()> {
		crate::node::UNDO_SERIAL.lock().unwrap_or_else(|e| e.into_inner())
	}

	/// A project with one clip node (an effect-chain host: clips carry
	/// the `tex_in` effect input).
	fn project_with_clip() -> (*mut ProjectHandle, u64) {
		let project = crate::node::oak_project_new();
		assert!(!project.is_null());
		let clip = {
			let inner = unsafe { &*project }.project().clone();
			let mut g = inner.lock().unwrap_or_else(|e| e.into_inner());
			let (core, behavior) = oak_node::block::clip_create();
			g.graph.add_node(core, behavior).identity()
		};
		(project, clip)
	}

	/// A video-effect type id the factory really registers.
	fn some_effect_type() -> String {
		let count = oak_effect_factory_count();
		assert!(count > 0, "the factory registers node types");
		let mut buf = [0u8; 256];
		for i in 0..count {
			let flags = oak_effect_factory_flags_at(i);
			if flags & oak_node::node::flags::VIDEO_EFFECT != 0
				&& flags & oak_node::node::flags::DONT_SHOW_IN_CREATE_MENU == 0
			{
				let n = oak_effect_factory_type_id_at(i, buf.as_mut_ptr(), buf.len());
				assert!(n > 0);
				return std::str::from_utf8(&buf[..n as usize]).unwrap().to_string();
			}
		}
		panic!("the factory registers at least one video effect");
	}

	/// The factory table over the C ABI: counts, strings, flags and
	/// categories all answer and round-trip.
	#[test]
	fn factory_table_enumerates() {
		let count = oak_effect_factory_count();
		assert!(count > 60, "the built-in node types are registered ({count})");
		let mut buf = [0u8; 256];
		for i in 0..count {
			let n = oak_effect_factory_type_id_at(i, buf.as_mut_ptr(), buf.len());
			assert!(n > 0, "entry {i} has a type id");
			let n = oak_effect_factory_name_at(i, buf.as_mut_ptr(), buf.len());
			assert!(n >= 0, "entry {i} has a name");
			let category = oak_effect_factory_category_at(i);
			assert!((-2..=11).contains(&category), "entry {i} category in range");
		}
		assert_eq!(oak_effect_factory_type_id_at(-1, buf.as_mut_ptr(), buf.len()), -1);
		assert_eq!(
			oak_effect_factory_type_id_at(count, buf.as_mut_ptr(), buf.len()),
			-1
		);
		// Dynamic (OpenFX) entries: none scanned in tests, but the
		// accessors must answer cleanly.
		let dyn_count = oak_effect_factory_dynamic_count();
		assert!(dyn_count >= 0);
		assert_eq!(
			oak_effect_factory_dynamic_type_id_at(0, buf.as_mut_ptr(), buf.len()),
			-1 * (dyn_count == 0) as i32
		);
	}

	/// The chain walk, insert (with clamping), remove, move and enable
	/// over the C ABI — the oak-app effectchain semantics end to end.
	#[test]
	fn chain_edits_walk_and_undo() {
		let _guard = undo_lock();
		oak_undo::global::clear().unwrap();
		let (project, host) = project_with_clip();
		let ty = some_effect_type();
		let (t, tl) = cstr_arg(&ty);

		unsafe {
			let first = oak_effect_chain_insert(project, host, 0, t, tl);
			assert!(crate::node::oak_node_id_is_valid(first), "insert into an empty chain");
			assert_eq!(oak_effect_chain_count(project, host), 1);
			assert_eq!(oak_effect_chain_at(project, host, 0), first);

			let second = oak_effect_chain_insert(project, host, 0, t, tl);
			assert_eq!(oak_effect_chain_count(project, host), 2);
			assert_eq!(oak_effect_chain_at(project, host, 0), second, "source side");
			assert_eq!(oak_effect_chain_at(project, host, 1), first);

			let third = oak_effect_chain_insert(project, host, 99, t, tl);
			assert_eq!(oak_effect_chain_at(project, host, 2), third, "clamped to the end");

			// One undo row per insert.
			oak_undo::global::undo().unwrap();
			assert_eq!(oak_effect_chain_count(project, host), 2);
			oak_undo::global::undo().unwrap();
			assert_eq!(oak_effect_chain_count(project, host), 1);
			assert_eq!(oak_effect_chain_at(project, host, 0), first);
			oak_undo::global::redo().unwrap();
			assert_eq!(oak_effect_chain_count(project, host), 2);
			oak_undo::global::redo().unwrap();
			assert_eq!(oak_effect_chain_count(project, host), 3);

			// Remove the middle effect: the gap bridges.
			assert!(oak_effect_chain_remove(project, host, first));
			assert_eq!(oak_effect_chain_count(project, host), 2);
			assert_eq!(oak_effect_chain_at(project, host, 0), second);
			assert_eq!(oak_effect_chain_at(project, host, 1), third);
			oak_undo::global::undo().unwrap();
			assert_eq!(oak_effect_chain_count(project, host), 3);
			assert_eq!(oak_effect_chain_at(project, host, 1), first);

			// Reorder: move the first effect to the end.
			assert!(oak_effect_chain_move(project, host, second, 2));
			assert_eq!(oak_effect_chain_at(project, host, 0), first);
			assert_eq!(oak_effect_chain_at(project, host, 2), second);
			oak_undo::global::undo().unwrap();
			assert_eq!(oak_effect_chain_at(project, host, 0), second);

			// Enable toggle.
			assert!(oak_effect_is_enabled(project, first));
			assert!(oak_effect_set_enabled(project, first, false));
			assert!(!oak_effect_is_enabled(project, first));
			oak_undo::global::undo().unwrap();
			assert!(oak_effect_is_enabled(project, first));

			// Failure paths.
			assert_eq!(
				oak_effect_chain_insert(project, host, 0, cstr_arg("no.such.type").0, 12),
				OAK_NODE_ID_INVALID
			);
			assert!(!oak_effect_chain_remove(project, host, u64::MAX));
			assert!(!oak_effect_chain_remove(std::ptr::null(), host, first));

			crate::node::oak_project_release(project);
		}
		oak_undo::global::clear().unwrap();
	}
}
