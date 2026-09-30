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

//! ofxColour（M11 §4）：宿主 OCIO 能力宣告、实例协商属性、输入 clip
//! 工作空间（ACEScg）、GetOutputColourspace 往返（偏好采纳 +
//! 交叉引用解析 + 输出写回）。
//!
//! 单库化后经 Rust API 直连（`Host::global().create_instance`），不再
//! 经已删除的 `oakplugin_host_scan`/`oakplugin_instance_create` C ABI。

mod common;

use std::sync::Arc;

use oak_ofx_plugin::handle::RefBox;
use oak_ofx_plugin::host::Host;
use oak_ofx_plugin::instance::Instance;
use oak_ofx_plugin::property::Value;

const TEST_PLUGIN_ID: &str = "org.oak.test-plugin";

/// 扫描并创建实例；不可用返回 None（skip）。
fn scan_and_create(id: &str) -> Option<Arc<RefBox<Instance>>> {
	if common::test_plugin_scan_dir().is_none() {
		common::skip("最小测试插件未构建");
		return None;
	}
	let dir = common::test_plugin_scan_dir().unwrap();
	let host = Host::global();
	host.cache.scan_path(&dir).ok()?;
	host.create_instance(id, None).ok()
}

fn prop_str(props: &oak_ofx_plugin::property::PropertySet, name: &str) -> Option<String> {
	match props.get(name, 0)? {
		Value::String(s) => Some(s.to_string_lossy().into_owned()),
		_ => None,
	}
}

/// 宿主能力宣告：OCIO 模式 + native 配置列表 + GL 支持。
#[test]
fn host_declares_ocio_capability() {
	let host = Host::global();
	assert_eq!(
		prop_str(&host.props, "OfxImageEffectPropColourManagementStyle").as_deref(),
		Some("OfxImageEffectColourManagementOCIO")
	);
	assert_eq!(
		host.props
			.dimension("OfxImageEffectPropColourManagementAvailableConfigs"),
		1,
		"可用配置至少含 ofx-native-v1.5_aces-v1.3_ocio-v2.3"
	);
	assert_eq!(
		prop_str(&host.props, "OfxImageEffectPropOpenGLRenderSupported").as_deref(),
		Some("true")
	);
}

/// 实例期协商属性：style/config/OCIOConfig 已写；输入 clip 色彩空间
/// = 工作空间 ACEScg；输出 clip 未设（待 GetOutputColourspace）。
#[test]
fn instance_and_clip_colourspace_props() {
	common::with_host(|| {
		let Some(inst) = scan_and_create(TEST_PLUGIN_ID) else {
			return;
		};
		assert_eq!(
			prop_str(&inst.value.props, "OfxImageEffectPropColourManagementStyle").as_deref(),
			Some("OfxImageEffectColourManagementOCIO")
		);
		assert_eq!(
			prop_str(
				&inst.value.props,
				"OfxImageEffectPropColourManagementConfig"
			)
			.as_deref(),
			Some("ofx-native-v1.5_aces-v1.3_ocio-v2.3")
		);
		assert_eq!(
			prop_str(&inst.value.props, "OfxImageEffectPropOCIOConfig").as_deref(),
			Some("ocio://default")
		);
		let source = inst
			.value
			.clips
			.iter()
			.find(|c| c.name == "Source")
			.unwrap();
		assert_eq!(
			prop_str(&source.props, "OfxImageClipPropColourspace").as_deref(),
			Some("ACEScg"),
			"输入 clip 色彩空间 = 工作空间"
		);
		let output = inst
			.value
			.clips
			.iter()
			.find(|c| c.name == "Output")
			.unwrap();
		assert!(
			prop_str(&output.props, "OfxImageClipPropColourspace")
				.map(|s| s.is_empty())
				.unwrap_or(true),
			"输出 clip 色彩空间在 GetOutputColourspace 前未设"
		);
	});
}

/// GetOutputColourspace 往返：无偏好 → 插件回 "OfxColourspace_Source"
/// 交叉引用 → 宿主解析为 Source 的实际色彩空间（ACEScg）并写回输出。
#[test]
fn get_output_colourspace_cross_reference() {
	common::with_host(|| {
		let Some(inst) = scan_and_create(TEST_PLUGIN_ID) else {
			return;
		};
		let cs = inst
			.value
			.get_output_colourspace(&[])
			.expect("GetOutputColourspace 应成功");
		assert_eq!(cs, "ACEScg", "交叉引用 OfxColourspace_Source → ACEScg");
		inst.value.set_output_colourspace(&cs);
		let output = inst
			.value
			.clips
			.iter()
			.find(|c| c.name == "Output")
			.unwrap();
		assert_eq!(
			prop_str(&output.props, "OfxImageClipPropColourspace").as_deref(),
			Some("ACEScg"),
			"输出 clip 已写回解析后的色彩空间"
		);
	});
}

/// GetOutputColourspace 偏好采纳：插件优先取宿主偏好的第一个色彩
/// 空间。
#[test]
fn get_output_colourspace_preferred() {
	common::with_host(|| {
		let Some(inst) = scan_and_create(TEST_PLUGIN_ID) else {
			return;
		};
		let cs = inst
			.value
			.get_output_colourspace(&["ACEScg".to_string(), "linear".to_string()])
			.expect("GetOutputColourspace 应成功");
		assert_eq!(cs, "ACEScg", "偏好列表第一个被采纳");
	});
}

/// 交叉引用解析的边界：未知 clip 引用原样返回；非交叉引用原样返回。
#[test]
fn resolve_colourspace_edge_cases() {
	common::with_host(|| {
		let Some(inst) = scan_and_create(TEST_PLUGIN_ID) else {
			return;
		};
		assert_eq!(
			inst.value
				.resolve_colourspace("OfxColourspace_NoSuchClip".to_string()),
			"OfxColourspace_NoSuchClip",
			"未知 clip 引用保持原样"
		);
		assert_eq!(
			inst.value.resolve_colourspace("ACEScg".to_string()),
			"ACEScg",
			"非交叉引用原样返回"
		);
	});
}
