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

//! OFX plugin project serialization round-trip (CI-gated): a project that
//! carries a plugin node must save to XML and load back with the same
//! plugin type id — the serializer resolves plugin types through the
//! factory's dynamic (runtime-registered) entries.
//!
//! Gated on `OAK_OFX_FIXTURE_DIR` pointing at a directory with a built
//! `OakCiTest.ofx.bundle` (see `tests/fixtures/build_fixture.sh`); the
//! test skips silently when the variable is unset so plain `cargo test`
//! runs stay hermetic. The CI workflow builds the fixture and sets it.

use oak_ofx_plugin::host::Host;

/// The fixture plugin's type id (tests/fixtures/ci_test_plugin.c).
const FIXTURE_TYPE_ID: &str = "rs.oak.CiTestPlugin";

#[test]
fn plugin_node_survives_save_load_roundtrip() {
	let Ok(fixture_dir) = std::env::var("OAK_OFX_FIXTURE_DIR") else {
		eprintln!("OAK_OFX_FIXTURE_DIR unset; skipping the OFX round-trip test");
		return;
	};
	let host = Host::global();
	host.cache
		.scan_path(std::path::Path::new(&fixture_dir))
		.expect("fixture dir scans");
	let registered = oak_ofx_plugin::node_factory::register_plugin_nodes();
	assert!(
		registered.iter().any(|id| id == FIXTURE_TYPE_ID),
		"the fixture plugin registered (got {registered:?})"
	);

	// Build a project carrying one plugin node.
	let project = oak_node::project::Project::new();
	let node_id = {
		let mut p = project.lock().unwrap_or_else(|e| e.into_inner());
		let (core, behavior) = oak_node::factory::Factory::global()
			.create_any(FIXTURE_TYPE_ID)
			.expect("the fixture type resolves through the factory");
		p.graph.add_node(core, behavior)
	};

	// Save, wipe, reload: the type id must resolve again (this is the path
	// that used to fail with "unknown node type" for plugin nodes).
	let xml = {
		let p = project.lock().unwrap_or_else(|e| e.into_inner());
		oak_node::serializer::save(&p).expect("project saves")
	};
	let loaded = oak_node::serializer::load(&xml).expect("project with a plugin node loads");
	let p = loaded.lock().unwrap_or_else(|e| e.into_inner());
	let mut found = false;
	for id in p.graph.node_ids() {
		let entry = p.graph.get(id).expect("listed node exists");
		if entry.behavior.type_id() == FIXTURE_TYPE_ID {
			found = true;
		}
	}
	assert!(found, "the plugin node survived the round trip (node {node_id:?})");
}
