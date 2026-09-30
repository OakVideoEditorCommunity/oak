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

//! push button → `kOfxActionInstanceChanged` 路由（端到端）。
//!
//! 链路：`scan_path`（最小测试插件，cbits/oak_test_plugin.c，describe
//! 里定义了 push button 参数 "button"，instanceChanged 时按
//! `OAK_TEST_PLUGIN_INSTANCECHANGED_MARKER` 指向的文件追加一行）→
//! `create_instance` → `push_button_clicked`（置值 + 路由
//! UserEdited instanceChanged）→ 插件记录回调次数。
//!
//! 测试插件未构建时 skip（common 约定）。宿主单例经 `common::with_host`
//! 串行化。

mod common;

use oak_ofx_plugin::host::Host;

const PLUGIN_ID: &str = "org.oak.test-plugin";

#[test]
fn push_button_press_routes_instance_changed() {
	common::with_host(|| {
		let Some(dir) = common::test_plugin_scan_dir() else {
			common::skip("最小测试插件未构建");
			return;
		};
		if Host::global().cache.scan_path(&dir).is_err() {
			common::skip("测试插件扫描失败");
			return;
		}
		let inst = Host::global()
			.create_instance(PLUGIN_ID, None)
			.expect("实例");
		let id = oak_ofx_plugin::node_factory::register_instance(inst.clone());

		// The C plugin appends one line per instanceChanged("button") to the
		// marker file; the assertion reads it back.
		let marker = std::env::temp_dir().join(format!("oak-push-{}.log", std::process::id()));
		let _ = std::fs::remove_file(&marker);
		std::env::set_var("OAK_TEST_PLUGIN_INSTANCECHANGED_MARKER", &marker);

		// The button param exists and is a push button.
		let p = inst.value.params.find("button").expect("button 参数");
		assert_eq!(p.def.ofx_type, oak_ofx_plugin::param::TYPE_PUSHBUTTON);

		// First press: set + routed to the plugin.
		assert!(oak_ofx_plugin::node_factory::push_button_clicked(id, "button"));
		// Unknown / non-button params are rejected without touching the entry.
		assert!(!oak_ofx_plugin::node_factory::push_button_clicked(id, "gain"));
		assert!(!oak_ofx_plugin::node_factory::push_button_clicked(id, "nope"));
		assert!(!oak_ofx_plugin::node_factory::push_button_clicked(u64::MAX, "button"));
		// Second press on the real button: routed again.
		assert!(oak_ofx_plugin::node_factory::push_button_clicked(id, "button"));

		// Exactly the two real presses reached the plugin's instanceChanged.
		let log = std::fs::read_to_string(&marker).expect("marker 文件应存在");
		assert_eq!(log.lines().count(), 2, "marker log:\n{log}");
		assert!(log.contains("instanceChanged"), "marker log:\n{log}");

		std::env::remove_var("OAK_TEST_PLUGIN_INSTANCECHANGED_MARKER");
		let _ = std::fs::remove_file(&marker);
		oak_ofx_plugin::node_factory::unregister_instance(id);
		Host::global().shutdown();
	});
}
