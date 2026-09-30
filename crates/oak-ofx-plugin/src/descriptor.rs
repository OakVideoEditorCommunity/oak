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

//! describe 的产物：效果描述符与 clip 描述符。
//!
//! 参照：HS: ofxhImageEffect.cpp `Descriptor`（describe action 期间
//! 插件可读写的属性容器）。
//!
//! 句柄约定（[`crate::suites::tag`]）：`props` 在偏移 0 且
//! `#[repr(C)]`；`params`/`clips` 元素装箱（Vec 重分配不移动对象）。

use std::ffi::CString;

use crate::param::ParamDef;
use crate::property::{PropertySet, Value};

fn cs(s: &str) -> CString {
	CString::new(s).unwrap()
}

// ---- clip 描述符属性（ofxImageEffect.h / ofxCore.h）----

/// kOfxImageClipPropOptional。
pub(crate) const CLIP_OPTIONAL: &str = "OfxImageClipPropOptional";
/// kOfxImageClipPropIsMask。
pub(crate) const CLIP_IS_MASK: &str = "OfxImageClipPropIsMask";
/// kOfxImageClipPropFieldExtraction。
pub(crate) const CLIP_FIELD_EXTRACTION: &str = "OfxImageClipPropFieldExtraction";
/// kOfxImageEffectPropSupportedComponents。
pub(crate) const CLIP_SUPPORTED_COMPONENTS: &str = "OfxImageEffectPropSupportedComponents";
/// kOfxImageEffectPropTemporalClipAccess。
pub(crate) const CLIP_TEMPORAL_ACCESS: &str = "OfxImageEffectPropTemporalClipAccess";
/// kOfxImageEffectPropSupportsTiles。
pub(crate) const CLIP_SUPPORTS_TILES: &str = "OfxImageEffectPropSupportsTiles";
/// kOfxImageFieldDoubled（FieldExtraction 默认值）。
pub(crate) const CLIP_FIELD_DOUBLED: &str = "OfxImageFieldDoubled";
/// kOfxTypeClip。
pub(crate) const CLIP_TYPE: &str = "OfxTypeClip";

/// clip 描述符（describe 期间由插件定义）。
/// `#[repr(C)]` + props 在偏移 0（句柄约定；describe 期 clip handle
/// 即 &props，与实例期 [`crate::clip::ClipInstance`] 共用 CLIP 标签）。
#[repr(C)]
pub struct ClipDescriptor {
	/// 描述属性：label、可选性（kOfxImageClipPropOptional）、支持分量
	/// （kOfxImageEffectPropSupportedComponents）、field 支持等。
	pub props: PropertySet,
	/// clip 名（如 "Source"/"Output"）。
	pub name: String,
}

impl ClipDescriptor {
	/// 按名构建（镜像 HS ofxhClip.cpp `ClipDescriptor::ClipDescriptor`
	/// + clipDescriptorStuffs 属性表，ofxhClip.cpp:22-38）。公开：
	/// 测试构造 clip 实例（GL suite 往返）。
	pub fn new(name: &str) -> Self {
		let props = PropertySet::new();
		props.set_one(crate::param::PROP_TYPE, Value::String(cs(CLIP_TYPE)));
		props.set_one(crate::param::PROP_NAME, Value::String(cs(name)));
		props.set_one(crate::param::PROP_LABEL, Value::String(cs(name)));
		props.set_one(crate::param::PROP_SHORT_LABEL, Value::String(cs("")));
		props.set_one(crate::param::PROP_LONG_LABEL, Value::String(cs("")));
		props.define(CLIP_SUPPORTED_COMPONENTS, vec![]);
		props.set_one(CLIP_TEMPORAL_ACCESS, Value::Int(0));
		props.set_one(CLIP_OPTIONAL, Value::Int(0));
		props.set_one(CLIP_IS_MASK, Value::Int(0));
		props.set_one(CLIP_FIELD_EXTRACTION, Value::String(cs(CLIP_FIELD_DOUBLED)));
		// kOfxImageClipPropFieldOrder（ofxhClip.cpp 的 clip 描述符必备
		// 属性；ofxs `Clip::getFieldOrder()` 无默认值强读——Mirror 这类
		// 场感知插件缺它直接 MissingHostFeature）。取值是
		// kOfxImageField* 族（ofxImageEffect.h:1288），默认无场。
		props.set_one(
			"OfxImageClipPropFieldOrder",
			Value::String(cs("OfxFieldNone")),
		);
		props.set_one(CLIP_SUPPORTS_TILES, Value::Int(1));
		// ofxColour（M11 §4）：clip 色彩空间属性族。Colourspace 由宿主
		// 在实例化时写入（输入 clip = 工作空间 ACEScg）；Preferred 由
		// 插件在 GetClipPreferences 写（宿主侧预置空数组）。
		props.set_one(crate::host::PROP_CLIP_COLOURSPACE, Value::String(cs("")));
		props.define(crate::host::PROP_CLIP_PREFERRED_COLOURSPACES, vec![]);
		Self {
			props,
			name: name.to_string(),
		}
	}
}

/// 效果描述符。
/// `#[repr(C)]` + props 在偏移 0（句柄约定：describe 期 effect/
/// param-set handle 即 &props）。
#[repr(C)]
pub struct EffectDescriptor {
	/// 效果级属性：label、描述、分组、单帧/时间域标记等。
	pub props: PropertySet,
	/// 参数定义集（describe 期间插件经 Param suite 填充；
	/// 值类型见 [`crate::param::ParamDef`]；Box 保证句柄地址稳定）。
	pub params: Vec<Box<ParamDef>>,
	/// clip 定义集（含 "Output"；Box 保证句柄地址稳定）。
	pub clips: Vec<Box<ClipDescriptor>>,
}

impl EffectDescriptor {
	/// 空描述符（describe action 前由 host 创建并预置根属性）。
	pub fn new() -> Self {
		Self {
			props: PropertySet::new(),
			params: Vec::new(),
			clips: Vec::new(),
		}
	}

	/// 按名找 clip。
	pub fn clip(&self, name: &str) -> Option<&ClipDescriptor> {
		self.clips
			.iter()
			.find(|c| c.name == name)
			.map(|b| b.as_ref())
	}

	/// 按名找参数定义。
	pub fn param(&self, name: &str) -> Option<&ParamDef> {
		self.params
			.iter()
			.find(|p| p.name == name)
			.map(|b| b.as_ref())
	}
}
