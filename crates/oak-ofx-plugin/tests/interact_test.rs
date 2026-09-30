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

//! Interact 宿主端到端：生命周期 + action 调用面 + Draw suite 真实 GL。
//!
//! 链路：扫描最小测试插件的 interact 变体（org.oak.test-plugin.interact，
//! describe 期声明 kOfxImageEffectPluginPropOverlayInteractV2 → 指向
//! mainEntryInteract）→ `new_interact`（发 kOfxActionNewInteract）→
//! describe → create_instance → pen/key/idle 事件 → draw（宿主
//! gl_bridge 保持 GL current，插件经 Draw suite setColour + 原生 GL 画
//! 已知色块）→ 回读断言。
//!
//! 断言即"真实调用到插件"的证据：插件把每次 action 的实参写入
//! `OAK_TEST_PLUGIN_INTERACT_MARKER` 指向的标记文件（unix 下
//! `va_list` 由 stdarg.h 承担），测试逐行断言；GL 断言另读回 FBO 像素
//! 比对插件绘制颜色（与 CPU 路径可区分）。
//!
//! 门：事件用例不依赖 GL，任何平台可跑；draw 用例要求 macOS +
//! `OAK_GPU_TESTS`（GL 验收约定，CI 一律跳过）。

mod common;

use std::path::PathBuf;

use oak_ofx_plugin::suites::{interact::Interact, status};

const INTERACT_PLUGIN_ID: &str = "org.oak.test-plugin.interact";
const BASE_PLUGIN_ID: &str = "org.oak.test-plugin";
/// 插件把 interact action 记录写入该环境变量指向的文件。
const MARKER_ENV: &str = "OAK_TEST_PLUGIN_INTERACT_MARKER";

/// 扫描测试插件 + 注册节点（幂等；不可用 → false = skip）。
fn scan_and_register() -> bool {
	let Some(dir) = common::test_plugin_scan_dir() else {
		common::skip("最小测试插件未构建");
		return false;
	};
	if oak_ofx_plugin::host::Host::global().cache.scan_path(&dir).is_err() {
		common::skip("测试插件扫描失败");
		return false;
	}
	oak_ofx_plugin::node_factory::register_plugin_nodes();
	true
}

/// 独立标记文件路径（临时目录，进程 id 防串）。
fn marker_path(tag: &str) -> PathBuf {
	std::env::temp_dir().join(format!(
		"oak-interact-{}-{}.log",
		std::process::id(),
		tag
	))
}

/// 读标记文件全部行。
fn read_marker(path: &PathBuf) -> Vec<String> {
	std::fs::read_to_string(path)
		.unwrap_or_default()
		.lines()
		.map(|l| l.to_string())
		.collect()
}

/// 无 interact 的插件：new_interact → None（main entry 对
/// kOfxActionNewInteract 返回 ReplyDefault 且未声明 overlay 入口）。
#[test]
fn base_plugin_has_no_interact() {
	common::with_host(|| {
		if !scan_and_register() {
			return;
		}
		let inst = oak_ofx_plugin::host::Host::global()
			.create_instance(BASE_PLUGIN_ID, None)
			.expect("base 插件实例应可建");
		assert!(
			inst.value.new_interact().is_none(),
			"无 interact 的插件 new_interact 应为 None"
		);
		oak_ofx_plugin::host::Host::global().shutdown();
	});
}

/// 生命周期 + 事件调用面：new_interact → describe → create → pen/key/
/// idle → destroy。逐条断言插件侧真实记录与参数。
#[test]
fn interact_lifecycle_and_events() {
	common::with_host(|| {
		if !scan_and_register() {
			return;
		}
		let marker = marker_path("events");
		let _ = std::fs::remove_file(&marker);
		unsafe { std::env::set_var(MARKER_ENV, &marker) };

		let inst = oak_ofx_plugin::host::Host::global()
			.create_instance(INTERACT_PLUGIN_ID, None)
			.expect("interact 变体实例应可建");

		let interact_arc = inst
			.value
			.new_interact()
			.expect("interact 变体应创建 interact");
		let interact: &Interact = &interact_arc;
		assert_eq!(interact.describe(), status::OK);
		assert_eq!(interact.create_instance(), status::OK);

		// 事件：坐标为视口像素；pixelScale 默认 1 → canonical == 视口。
		assert_eq!(interact.pen_motion((10.0, 20.0), true, 5.0), status::OK);
		assert_eq!(interact.pen_down((30.0, 40.0), 5.0), status::OK);
		assert_eq!(interact.pen_up((30.0, 40.0), 5.0), status::OK);
		assert_eq!(
			interact.key_down(oak_ofx_plugin::host::KEY_A, "a", 5.0),
			status::OK
		);
		assert_eq!(interact.key_up(oak_ofx_plugin::host::KEY_A, "a", 5.0), status::OK);
		assert_eq!(interact.idle(), status::OK);
		interact.destroy();

		unsafe { std::env::remove_var(MARKER_ENV) };
		let lines = read_marker(&marker);
		let _ = std::fs::remove_file(&marker);

		// 生命周期序列（按序）。
		let seq = ["new_interact", "describe", "create", "destroy"];
		let pos: Vec<Option<usize>> = seq
			.iter()
			.map(|s| lines.iter().position(|l| l == s))
			.collect();
		assert!(
			pos.iter().all(|p| p.is_some()),
			"标记文件缺生命周期动作：{lines:?}"
		);
		let pos: Vec<usize> = pos.into_iter().map(|p| p.unwrap()).collect();
		assert!(
			pos.windows(2).all(|w| w[0] < w[1]),
			"生命周期顺序应为 new_interact→describe→create→destroy：{lines:?}"
		);

		// pen 事件：真实参数（视口位置/canonical/压力；C %g 无尾零）。
		assert!(
			lines.iter().any(|l| l == "pen_motion vp=10,20 canon=10,20 pressure=1"),
			"pen_motion 参数不符：{lines:?}"
		);
		assert!(
			lines.iter().any(|l| l == "pen_down vp=30,40 canon=30,40 pressure=1"),
			"pen_down 参数不符：{lines:?}"
		);
		assert!(
			lines.iter().any(|l| l == "pen_up vp=30,40 canon=30,40 pressure=0"),
			"pen_up 参数不符：{lines:?}"
		);

		// key 事件：真实 keySym/keyString。
		assert!(
			lines.iter().any(|l| l == "key_down sym=97 str=a"),
			"key_down 参数不符：{lines:?}"
		);
		assert!(
			lines.iter().any(|l| l == "key_up sym=97 str=a"),
			"key_up 参数不符：{lines:?}"
		);
		assert!(
			lines.iter().any(|l| l == "idle"),
			"idle 未记录：{lines:?}"
		);

		oak_ofx_plugin::host::Host::global().shutdown();
	});
}

/// 返回值如实透传：插件对 Escape 的 key_down 返回 kOfxStatReplyDefault
/// （未处理），宿主原样返回 14。
#[test]
fn interact_passthrough_plugin_status() {
	common::with_host(|| {
		if !scan_and_register() {
			return;
		}
		let marker = marker_path("status");
		let _ = std::fs::remove_file(&marker);
		unsafe { std::env::set_var(MARKER_ENV, &marker) };

		let inst = oak_ofx_plugin::host::Host::global()
			.create_instance(INTERACT_PLUGIN_ID, None)
			.expect("interact 变体实例应可建");
		let interact = inst.value.new_interact().expect("interact 应可建");

		// Escape 的 key_down：插件返回 ReplyDefault → 宿主透传。
		let st = interact.key_down(oak_ofx_plugin::host::KEY_ESCAPE, "", 0.0);
		assert_eq!(st, status::REPLY_DEFAULT, "插件 Escape → ReplyDefault 应透传");

		unsafe { std::env::remove_var(MARKER_ENV) };
		let lines = read_marker(&marker);
		let _ = std::fs::remove_file(&marker);
		assert!(
			lines.iter().any(|l| l == "key_down sym=65307 str="),
			"Escape key_down 应真实到达插件：{lines:?}"
		);

		oak_ofx_plugin::host::Host::global().shutdown();
	});
}

/// 实例销毁连带销毁 interact（同一生命周期）：drop 实例 → 插件收到
/// destroy（interact 的 kOfxActionDestroyInstance）。
#[test]
fn instance_destroy_cleans_up_interact() {
	common::with_host(|| {
		if !scan_and_register() {
			return;
		}
		let marker = marker_path("destroy");
		let _ = std::fs::remove_file(&marker);
		unsafe { std::env::set_var(MARKER_ENV, &marker) };

		let inst = oak_ofx_plugin::host::Host::global()
			.create_instance(INTERACT_PLUGIN_ID, None)
			.expect("interact 变体实例应可建");
		let _interact = inst.value.new_interact().expect("interact 应可建");

		// drop 实例（Arc 归零 → Drop → notify_destroy → interact destroy）。
		drop(inst);

		unsafe { std::env::remove_var(MARKER_ENV) };
		let lines = read_marker(&marker);
		let _ = std::fs::remove_file(&marker);
		assert!(
			lines.iter().any(|l| l == "destroy"),
			"实例销毁应连带 interact destroy：{lines:?}"
		);

		oak_ofx_plugin::host::Host::global().shutdown();
	});
}

/// draw 端到端（真实 GL）：宿主 acquire → 建输出纹理/FBO → 调 draw →
/// 插件 glClear 暗背景 + Draw suite setColour(0.9,0.1,0.2,1) +
/// draw(Rectangle 10..30) → 回读断言矩形区域颜色与背景色。
///
/// 同时断言 Draw suite 状态真实往返：setColour/getColour 在插件侧返回
/// OK（记录在标记文件）。
#[test]
fn interact_draw_renders_plugin_colours() {
	common::with_host(|| {
		if !cfg!(target_os = "macos") {
			common::skip("GL 桥仅 macOS 实现");
			return;
		}
		if !common::gpu_available() {
			common::skip("GL 测试需 OAK_GPU_TESTS");
			return;
		}
		if !oak_ofx_plugin::gl_bridge::gl_available() {
			common::skip("本机无可用 GL 上下文");
			return;
		}
		if !scan_and_register() {
			return;
		}
		let marker = marker_path("draw");
		let _ = std::fs::remove_file(&marker);
		unsafe { std::env::set_var(MARKER_ENV, &marker) };

		let inst = oak_ofx_plugin::host::Host::global()
			.create_instance(INTERACT_PLUGIN_ID, None)
			.expect("interact 变体实例应可建");
		let interact = inst.value.new_interact().expect("interact 应可建");

		let (w, h) = (64i32, 64i32);
		let params = oak_ofx_plugin::render::VideoParams {
			width: w,
			height: h,
			format: oak_ofx_plugin::render::PIXEL_FORMAT_F32,
			..Default::default()
		};
		// 一次 acquire 覆盖 FBO 装配 → draw（内部嵌套 acquire）→ 回读。
		let _guard = oak_ofx_plugin::gl_bridge::acquire().expect("acquire 应成功");
		let tex = oak_ofx_plugin::gl_bridge::create_output_texture(w, h, &params)
			.expect("输出纹理应可建");
		let fbo = oak_ofx_plugin::gl_bridge::create_fbo(tex, w, h).expect("FBO 应完整");
		oak_ofx_plugin::gl_bridge::bind_fbo(fbo);
		oak_ofx_plugin::gl_bridge::set_viewport(w, h);

		let st = interact.draw((64.0, 64.0), (1.0, 1.0), 5.0, None);
		assert_eq!(st, status::OK, "draw 应返回插件 OK");

		let img = oak_ofx_plugin::gl_bridge::read_pixels_to_image(w, h, &params).expect("回读应成功");
		oak_ofx_plugin::gl_bridge::delete_fbo(fbo);
		oak_ofx_plugin::gl_bridge::delete_gl_texture(tex);
		drop(_guard);

		unsafe { std::env::remove_var(MARKER_ENV) };
		let lines = read_marker(&marker);
		let _ = std::fs::remove_file(&marker);

		// Draw suite 状态真实往返（插件侧记录）。
		assert!(
			lines.iter().any(|l| {
				l.starts_with("draw vp=64x64") && l.contains("setcol_st=0")
					&& l.contains("setcol=0.9,0.1,0.2,1")
					&& l.contains("getcol_st=0") && l.contains("getcol=0,0,0,1")
					&& l.contains("draw_st=0")
			}),
			"draw 记录不符（setcol/getcol/draw 应 OK）：{lines:?}"
		);

		// 像素断言：矩形区域 (10..30)² 的 GL 像素 → 翻转后 frame y 33..53。
		// F32 RGBA 每像素 16 字节（紧凑行）。
		let stride = (w as usize) * 16;
		let px = |x: usize, y: usize| -> [f32; 4] {
			let p = &img.pixels()[y * stride + x * 16..y * stride + x * 16 + 16];
			[
				f32::from_ne_bytes(p[0..4].try_into().unwrap()),
				f32::from_ne_bytes(p[4..8].try_into().unwrap()),
				f32::from_ne_bytes(p[8..12].try_into().unwrap()),
				f32::from_ne_bytes(p[12..16].try_into().unwrap()),
			]
		};
		// 角上（rect 外）应为插件清屏暗背景。
		let corner = px(0, 0);
		for (i, v) in [0.05f32, 0.05, 0.05, 1.0].iter().enumerate() {
			assert!(
				(corner[i] - v).abs() < 1e-4,
				"角像素[{i}] = {}，期望暗背景 {v}",
				corner[i]
			);
		}
		// rect 内部（frame (20,40) = GL (20,23)）应为 setColour 颜色。
		let inside = px(20, 40);
		for (i, v) in [0.9f32, 0.1, 0.2, 1.0].iter().enumerate() {
			assert!(
				(inside[i] - v).abs() < 1e-4,
				"矩形内像素[{i}] = {}，期望 {v}",
				inside[i]
			);
		}
		// 整帧只应有两种颜色（背景 + 矩形），矩形覆盖 ~400 像素。
		let (mut rect_n, mut clear_n) = (0usize, 0usize);
		for y in 0..h as usize {
			for x in 0..w as usize {
				let c = px(x, y);
				let is_rect = (0..4).all(|i| (c[i] - [0.9, 0.1, 0.2, 1.0][i]).abs() < 1e-3);
				let is_clear = (0..4).all(|i| (c[i] - [0.05, 0.05, 0.05, 1.0][i]).abs() < 1e-3);
				if is_rect {
					rect_n += 1;
				} else if is_clear {
					clear_n += 1;
				} else {
					panic!("意外颜色像素 ({x},{y}): {c:?}");
				}
			}
		}
		assert!(
			(rect_n as i32 - 400).abs() <= 16,
			"矩形应覆盖 ~400 像素，got {rect_n}"
		);
		assert_eq!(rect_n + clear_n, (w * h) as usize, "整帧应为背景+矩形两色");

		oak_ofx_plugin::host::Host::global().shutdown();
	});
}
