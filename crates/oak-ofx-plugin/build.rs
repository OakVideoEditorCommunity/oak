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

//! 构建脚本：编译 C shim（cbits/）。动机见 README 的依赖登记
//! （"C 变长参数"条目）：stable Rust 不能定义 C-variadic 函数，
//! 入口留在 C。`cc` crate 只参与构建，不进产物。

fn main() {
	println!("cargo:rerun-if-changed=cbits/ofx_message_shim.c");
	cc::Build::new()
		.file("cbits/ofx_message_shim.c")
		.warnings(true)
		.compile("ofx_message_shim");
	println!("cargo:rerun-if-changed=cbits/ofx_param_shim.c");
	cc::Build::new()
		.file("cbits/ofx_param_shim.c")
		.warnings(true)
		.compile("ofx_param_shim");
	// 最小测试插件（M11 §2.4）：共享库（dlopen 目标），运行时由
	// common::test_plugin_dir 装配成 bundle 目录。
	// cc 的 compile() 只产静态库，直接调系统编译器出 dylib/so。
	println!("cargo:rerun-if-changed=cbits/oak_test_plugin.c");
	build_test_plugin();
}

/// 编译最小测试插件为共享库（$OUT_DIR/oak_test_plugin.{dylib,so}）。
fn build_test_plugin() {
	use std::process::Command;
	let out = std::env::var("OUT_DIR").expect("OUT_DIR");
	let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
	let (link_flag, ext) = if cfg!(target_os = "macos") {
		("-dynamiclib", "dylib")
	} else {
		("-shared", "so")
	};
	let mut args = vec![
		"-fPIC".to_string(),
		"-Iofx".to_string(),
		"cbits/oak_test_plugin.c".to_string(),
		link_flag.to_string(),
		"-o".to_string(),
		format!("{out}/oak_test_plugin.{ext}"),
	];
	// GL 端到端测试：插件 GL 变体在 macOS 上真实绘制（glClear），需要
	// OpenGL.framework。
	if cfg!(target_os = "macos") {
		args.push("-framework".into());
		args.push("OpenGL".into());
	}
	let status = Command::new(&cc)
		.args(&args)
		.status()
		.expect("编译测试插件失败");
	assert!(status.success(), "测试插件编译失败");
}
