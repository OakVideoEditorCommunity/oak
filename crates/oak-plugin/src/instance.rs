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

//! 插件实例：action 调用面。
//!
//! 对应 C++ 的 `OlivePluginInstance`。语义参照：
//! HS: ofxhImageEffect.cpp `Instance`（action 调用序列与参数集组装）。

use std::ffi::CString;
use std::sync::Arc;

use crate::clip::ClipInstance;
use crate::host::Plugin;
use crate::param::ParamSetInstance;
use crate::property::PropertySet;

/// paramEditBegin/End 的编辑事务状态（undo 分组）。对应 C++
/// oliveplugininstance.cpp 的 `edit_depth_`/`edit_command_`：
/// 事务内的多次参数回写合并为一条 multi 命令，editEnd 时整体
/// redo 后释放。
#[repr(C)]
pub struct EditTransaction {
	/// 嵌套深度（editBegin/End 必须配对）。
	depth: i32,
	/// 事务累积的 multi 命令（None = 尚无子命令）。
	multi: Option<oak_undo::undocommand::UndoCommand>,
	/// 事务内回写次数（标签计数）。
	param_count: i32,
	/// 第一条回写的标签。
	first_label: String,
}

impl EditTransaction {
	/// 空事务（无嵌套、无累积命令）。
	pub fn new() -> Self {
		Self {
			depth: 0,
			multi: None,
			param_count: 0,
			first_label: String::new(),
		}
	}
}

/// 时间范围（OFX 规范双精度秒）。
#[derive(Clone, Copy, Debug, Default)]
pub struct OfxRangeD {
	/// 起始（含）。
	pub min: f64,
	/// 结束（含）。
	pub max: f64,
}

/// 矩形（规范坐标，double）。
#[derive(Clone, Copy, Debug, Default)]
pub struct OfxRectD {
	/// 左。
	pub x1: f64,
	/// 上。
	pub y1: f64,
	/// 右。
	pub x2: f64,
	/// 下。
	pub y2: f64,
}

/// 像素比/场序等渲染参量（render action 的 in_args 子集）。
#[derive(Clone, Copy, Debug)]
pub struct RenderScale {
	/// x 方向渲染比例。
	pub x: f64,
	/// y 方向渲染比例。
	pub y: f64,
}

/// 插件实例。`Arc<RefBox<Instance>>` 管理生命周期（`RefBox` 为 facade
/// 边界类型，见 [`crate::handle`]）；param 桥按 props 地址映射反查
/// （[`crate::suites::param`]），节点绑定经 [`crate::node`] 身份注册表。
///
/// `#[repr(C)]` + props 在偏移 0（句柄约定，见 [`crate::suites::tag`]；
/// 实例期 effect/param-set handle 即 `&props`）。
#[repr(C)]
pub struct Instance {
	/// 实例级属性集（偏移 0，句柄约定）。
	pub props: PropertySet,
	/// 所属插件。
	pub plugin: Arc<Plugin>,
	/// 上下文（kOfxImageEffectContext*）。
	pub context: String,
	/// 参数实例集（createInstance 后由 describe 产物实例化）。
	pub params: ParamSetInstance,
	/// clip 实例集（含 "Output"；Box 保证句柄地址稳定）。
	pub clips: Vec<Box<ClipInstance>>,
	/// 绑定的 oaknode 节点身份（param 桥写入；0 = 未绑定）。
	pub node_identity: std::sync::atomic::AtomicUsize,
	/// destroyInstance action 是否已通知（析构幂等门；Drop 驱动；
	/// 公开：宿主 shutdown 与测试都要置位）。
	pub destroyed: std::sync::atomic::AtomicBool,
	/// beginSequenceRender 的时间域（timeline getTimeBounds 用；
	/// endSequenceRender 清除）。
	pub sequence_range: std::sync::Mutex<Option<OfxRangeD>>,
	/// facade 注册的进度回调（oakplugin_instance_set_progress_cb）。
	pub progress_cb: std::sync::Mutex<Option<(crate::progress::ProgressFn, usize)>>,
	/// facade 取消标记（oakplugin_instance_cancel）。
	pub cancel: std::sync::atomic::AtomicBool,
	/// paramEditBegin/End 的编辑事务（undo 分组）。
	pub edit: std::sync::Mutex<EditTransaction>,
	/// 实例级渲染串行化（对应 C++ `OlivePluginInstance::mutex`；
	/// pluginrenderer.cpp:1436-1444 的 instance_lock——渲染路径非
	/// 线程安全，并发 render 必须互斥）。
	pub render_lock: std::sync::Mutex<()>,
	/// 关联的 interact（`new_interact` 创建；与实例同生命周期，
	/// notify_destroy 时连带销毁）。
	pub interact: std::sync::Mutex<Option<std::sync::Arc<crate::suites::interact::Interact>>>,
}

/// 实例销毁路径：先通知 destroyInstance action，再摘除 param 回写登记。
/// （`Arc<RefBox<Instance>>` 归零时 Drop 触发——action 通知必须在
/// 对象析构前发出。）
impl Drop for Instance {
	fn drop(&mut self) {
		if !self
			.destroyed
			.swap(true, std::sync::atomic::Ordering::Relaxed)
		{
			self.notify_destroy();
		}
		crate::suites::param::unregister_params_of(&self.props as *const _ as usize);
	}
}

impl Instance {
	/// 绑定 oaknode 节点（C++ `set_node_handle` 的 Rust 侧；装配期由
	/// facade/测试调用）。`identity` 为 [`crate::node::register_node`]
	/// 返回的 oaknode 节点身份（[`oak_node::id::NodeId::identity`] 的
	/// 打包值，经 [`crate::node::node_from_identity`] 反查）；
	/// 0 解除绑定。
	pub fn bind_node(&self, identity: usize) {
		self.node_identity
			.store(identity, std::sync::atomic::Ordering::Relaxed);
	}

	/// paramEditBegin：进入编辑事务（可嵌套；首次进入重置事务状态）。
	pub fn edit_begin(&self) {
		let mut e = self.edit.lock().unwrap_or_else(|e| e.into_inner());
		e.depth += 1;
		if e.depth == 1 {
			e.multi = None;
			e.param_count = 0;
			e.first_label.clear();
		}
	}

	/// paramEditEnd：退出编辑事务；最外层结束时把累积的 multi 命令
	/// redo 生效并释放（本 crate 无 undo 栈——命令在 oakundo 侧
	/// 提交，见 C++ `submit_undo_command` 的 fallback 分支）。
	pub fn edit_end(&self) {
		let mut e = self.edit.lock().unwrap_or_else(|e| e.into_inner());
		if e.depth > 0 {
			e.depth -= 1;
		}
		if e.depth == 0 {
			if let Some(mut multi) = e.multi.take() {
				multi.redo_now();
				// drop(multi)：释放 multi 与子命令（命令值语义）。
				drop(multi);
			}
			e.param_count = 0;
			e.first_label.clear();
		}
	}

	/// 是否处于编辑事务内（param 桥回写据此决定打包成 multi）。
	pub fn in_edit(&self) -> bool {
		self.edit.lock().unwrap_or_else(|e| e.into_inner()).depth > 0
	}

	/// 事务感知的 undo 提交（param 桥回写用；对应 C++
	/// oliveplugininstance.cpp:390-414 `submit_undo_command`）：
	/// 编辑事务内并入 multi（子命令立即 redo 生效，值即时可见），
	/// 否则单命令 redo 后释放。
	pub(crate) fn submit_undo_command(&self, mut cmd: oak_undo::undocommand::UndoCommand, label: &str) {
		if self.in_edit() {
			let mut e = self.edit.lock().unwrap_or_else(|e| e.into_inner());
			if e.multi.is_none() {
				e.multi = Some(oak_undo::undocommand::UndoCommand::multi());
			}
			e.param_count += 1;
			if e.first_label.is_empty() {
				e.first_label = label.to_string();
			}
			cmd.redo_now();
			e.multi
				.as_mut()
				.expect("multi 已在上面初始化")
				.multi_add_child(cmd);
		} else {
			cmd.redo_now();
			drop(cmd);
		}
	}

	/// getClipPreferences action，返回协商结果（输出 clip 的分量/
	/// 位深/像素比/field 与帧率）。协商顺序逐行对照
	/// HS: ofxhImageEffect.cpp `Instance::getClipPreferences`——
	/// 当年调试重灾区，实现时必须附行号注释。
	///
	/// 协商流程（HS: ofxhImageEffect.cpp:1686-1740）：
	/// 1. out args 预置 clipPrefsStuffs（frameRate=1、premult、
	///    fieldOrder、continuousSamples、frameVarying，HS:1697-1710）；
	/// 2. 预置 per-clip 的 "OfxImageClipPropComponents_<name>" 等
	///    （默认 RGBA/Float/PAR=1，HS:1717-1727）；
	/// 3. 调 action（out args 即协商产物，HS:1708-1718）；
	/// 4. 回灌：per-clip 分量/位深/像素比写回 clip 实例属性，
	///    输出帧率/fielding/premult 记入实例（HS:1721-1740）。
	pub fn get_clip_preferences(&self) -> crate::error::Result<ClipPreferences> {
		use crate::host::{
			ACTION_GET_CLIP_PREFERENCES, CLIP_PREF_COMPONENTS, CLIP_PREF_DEPTH, CLIP_PREF_PAR,
			PROP_CONTINUOUS_SAMPLES, PROP_FIELD_ORDER, PROP_FRAME_RATE, PROP_FRAME_VARYING,
			PROP_PREMULT,
		};
		use crate::property::Value;

		// 1. clipPrefsStuffs（HS:1697-1706 的默认值）。
		let out = PropertySet::new();
		out.set_one(PROP_FRAME_RATE, Value::Double(1.0));
		out.set_one(PROP_PREMULT, Value::String(CString::new("").unwrap()));
		out.set_one(PROP_FIELD_ORDER, Value::String(CString::new("").unwrap()));
		out.set_one(PROP_CONTINUOUS_SAMPLES, Value::Int(0));
		out.set_one(PROP_FRAME_VARYING, Value::Int(0));

		// 2. per-clip 预置（HS:1717-1727；phase 1 全链路 F32+RGBA）。
		for clip in &self.clips {
			out.set_one(
				&crate::host::clip_pref_prop(CLIP_PREF_COMPONENTS, &clip.name),
				Value::String(CString::new("OfxImageComponentRGBA").unwrap()),
			);
			out.set_one(
				&crate::host::clip_pref_prop(CLIP_PREF_DEPTH, &clip.name),
				Value::String(CString::new("OfxBitDepthFloat").unwrap()),
			);
			out.set_one(
				&crate::host::clip_pref_prop(CLIP_PREF_PAR, &clip.name),
				Value::Double(1.0),
			);
		}

		// 3. action。
		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let empty = PropertySet::new();
		Self::add_action_arg_compat_props(&empty);
		if std::env::var_os("OAK_OFX_TRACE").is_some() {
			eprintln!("[probe-args] getClipPreferences in_args {:p} out {:p}", &empty, &out);
		}
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_GET_CLIP_PREFERENCES, inst_handle, &empty, &out)
		};
		if stat != crate::suites::status::OK && stat != crate::suites::status::REPLY_DEFAULT {
			return Err(crate::error::Error::Failed(format!(
				"getClipPreferences 失败：{stat}"
			)));
		}

		// 4. 回灌（HS:1721-1740：每 clip 的协商结果写回实例属性）。
		let mut output_components = "OfxImageComponentRGBA".to_string();
		let mut output_bit_depth = "OfxBitDepthFloat".to_string();
		let mut output_par = 1.0;
		for clip in &self.clips {
			let comp_name = crate::host::clip_pref_prop(CLIP_PREF_COMPONENTS, &clip.name);
			let depth_name = crate::host::clip_pref_prop(CLIP_PREF_DEPTH, &clip.name);
			let par_name = crate::host::clip_pref_prop(CLIP_PREF_PAR, &clip.name);
			let comps = out
				.get(&comp_name, 0)
				.map(|v| match v {
					Value::String(s) => s.to_string_lossy().into_owned(),
					_ => "OfxImageComponentRGBA".to_string(),
				})
				.unwrap_or_else(|| "OfxImageComponentRGBA".to_string());
			let depth = out
				.get(&depth_name, 0)
				.map(|v| match v {
					Value::String(s) => s.to_string_lossy().into_owned(),
					_ => "OfxBitDepthFloat".to_string(),
				})
				.unwrap_or_else(|| "OfxBitDepthFloat".to_string());
			let par = out
				.get(&par_name, 0)
				.and_then(|v| match v {
					Value::Double(d) => Some(d),
					_ => None,
				})
				.unwrap_or(1.0);
			clip.props.set_one(
				crate::image::K_IMAGE_EFFECT_PROP_COMPONENTS,
				Value::String(CString::new(comps.clone()).unwrap()),
			);
			clip.props.set_one(
				crate::image::K_IMAGE_EFFECT_PROP_PIXEL_DEPTH,
				Value::String(CString::new(depth.clone()).unwrap()),
			);
			clip.props
				.set_one("OfxImagePropPixelAspectRatio", Value::Double(par));
			if clip.name == "Output" {
				output_components = comps;
				output_bit_depth = depth;
				output_par = par;
			}
		}

		let frame_rate = out
			.get(PROP_FRAME_RATE, 0)
			.and_then(|v| match v {
				Value::Double(d) => Some(d),
				_ => None,
			})
			.unwrap_or(24.0);
		let field = out
			.get(PROP_FIELD_ORDER, 0)
			.map(|v| match v {
				Value::String(s) => s.to_string_lossy().into_owned(),
				_ => String::new(),
			})
			.unwrap_or_default();

		Ok(ClipPreferences {
			output_components,
			output_bit_depth,
			pixel_aspect_ratio: output_par,
			frame_rate,
			field,
		})
	}

	/// getRegionOfDefinition action（HS: ofxhImageEffect.cpp:1087-1130：
	/// in args = time + renderScale；out args = RegionOfDefinition）。
	pub fn get_region_of_definition(
		&self,
		time: f64,
		scale: RenderScale,
	) -> crate::error::Result<OfxRectD> {
		use crate::host::{ACTION_GET_ROD, PROP_RENDER_SCALE, PROP_ROD, PROP_TIME};
		use crate::property::Value;

		let in_args = PropertySet::new();
		in_args.set_one(PROP_TIME, Value::Double(time));
		in_args.define(
			PROP_RENDER_SCALE,
			vec![Value::Double(scale.x), Value::Double(scale.y)],
		);
		Self::add_action_arg_compat_props(&in_args);
		if std::env::var_os("OAK_OFX_TRACE").is_some() {
			eprintln!("[probe-args] getRod in_args {:p}", &in_args);
		}
		// out args 预定义（HS 行为：宿主先建属性表，插件只管写——
		// 缺失属性上插件的 propSet 会失败被忽略）。
		let out = PropertySet::new();
		out.define(
			PROP_ROD,
			vec![
				Value::Double(0.0),
				Value::Double(0.0),
				Value::Double(0.0),
				Value::Double(0.0),
			],
		);

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_GET_ROD, inst_handle, &in_args, &out)
		};
		if stat != crate::suites::status::OK && stat != crate::suites::status::REPLY_DEFAULT {
			return Err(crate::error::Error::Failed(format!("getRoD 失败：{stat}")));
		}
		read_rect(&out, PROP_ROD).ok_or(crate::error::Error::Failed("getRoD 无输出".into()))
	}

	/// getRegionsOfInterest action：输入 clip 的 RoI 写入 out_args
	/// 属性集，返回值即各 clip 的 RoI 列表（与 `clips` 顺序一致）。
	///
	/// 约定名 "OfxImageEffectPropRegionOfInterest_<clipname>"（HS:
	/// ofxhImageEffect.cpp getRegionsOfInterest 的 per-clip 前缀）。
	pub fn get_regions_of_interest(
		&self,
		time: f64,
		scale: RenderScale,
		region: OfxRectD,
	) -> crate::error::Result<Vec<OfxRectD>> {
		use crate::host::{ACTION_GET_ROI, PROP_RENDER_SCALE, PROP_ROI, PROP_TIME};
		use crate::property::Value;

		let in_args = PropertySet::new();
		in_args.set_one(PROP_TIME, Value::Double(time));
		in_args.define(
			PROP_RENDER_SCALE,
			vec![Value::Double(scale.x), Value::Double(scale.y)],
		);
		Self::add_action_arg_compat_props(&in_args);
		if std::env::var_os("OAK_OFX_TRACE").is_some() {
			eprintln!("[probe-args] getRoi in_args {:p}", &in_args);
		}
		in_args.define(
			PROP_ROI,
			vec![
				Value::Double(region.x1),
				Value::Double(region.y1),
				Value::Double(region.x2),
				Value::Double(region.y2),
			],
		);
		// out args 预定义 per-clip ROI 属性（HS 约定名前缀）。
		let out = PropertySet::new();
		for clip in &self.clips {
			let name = format!("OfxImageEffectPropRegionOfInterest_{}", clip.name);
			out.define(
				&name,
				vec![
					Value::Double(0.0),
					Value::Double(0.0),
					Value::Double(0.0),
					Value::Double(0.0),
				],
			);
		}

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_GET_ROI, inst_handle, &in_args, &out)
		};
		if stat != crate::suites::status::OK && stat != crate::suites::status::REPLY_DEFAULT {
			return Err(crate::error::Error::Failed(format!("getRoI 失败：{stat}")));
		}
		let mut rois = Vec::with_capacity(self.clips.len());
		for clip in &self.clips {
			let name = format!("OfxImageEffectPropRegionOfInterest_{}", clip.name);
			let rect = read_rect(&out, &name)
				// 未设置 → 默认整个 region（OFX 语义：插件可不写）。
				.unwrap_or(region);
			rois.push(rect);
		}
		Ok(rois)
	}

	/// isIdentity action：返回 Some((time, input_clip_name)) 表示本帧
	/// OFX 1.4+/Natron 框架对 action in-args 的强读属性：缺了会被抛成
	/// PropertyUnknownToHost → MissingHostFeature（紫帧根因）。每个
	/// action in-args 都带上。
	fn add_action_arg_compat_props(in_args: &PropertySet) {
		use crate::property::Value;
		// NatronOfxPropNativeOverlays：Natron 支持库强读（0 = 宿主不
		// 支持原生 overlay，插件走自己的回退）。OfxImageEffectPropRenderPlanes
		// 不放 in-args：Natron 框架按**字符串**读它，存在但类型是 Int
		// 反而会抛 PropertyUnknownToHost（缺失它才容忍）——它只属于
		// 图像/clip 属性（OFX 1.4 语义在 clipGetImagePlane 一侧）。
		in_args.set_one("NatronOfxPropNativeOverlays", Value::Int(0));
	}

	/// 直接透传该输入 clip；None 表示需要真正 render。
	///
	/// 参照 HS: ofxhImageEffect.cpp:1378-1450：in args = time + scale +
	/// renderWindow + fieldToRender；out args = kOfxImageEffectPropIsIdentity
	/// （输入 clip 名）+ kOfxPropTime（透传时间，可改）。
	pub fn is_identity(&self, time: f64) -> crate::error::Result<Option<(f64, String)>> {
		use crate::host::{
			ACTION_IS_IDENTITY, PROP_FIELD_TO_RENDER, PROP_IS_IDENTITY, PROP_RENDER_SCALE,
			PROP_RENDER_WINDOW, PROP_TIME,
		};
		use crate::property::Value;

		let in_args = PropertySet::new();
		in_args.set_one(PROP_TIME, Value::Double(time));
		in_args.define(
			PROP_RENDER_SCALE,
			vec![Value::Double(1.0), Value::Double(1.0)],
		);
		in_args.define(
			PROP_RENDER_WINDOW,
			vec![
				Value::Int(0),
				Value::Int(0),
				Value::Int(0),
				Value::Int(0),
			],
		);
		in_args.set_one(
			PROP_FIELD_TO_RENDER,
			Value::String(CString::new("OfxFieldNone").unwrap()),
		);
		in_args.set_one("NatronOfxPropNativeOverlays", Value::Int(0));
		// out args 预定义：IsIdentity（String）+ Time（Double）。
		let out = PropertySet::new();
		out.set_one(PROP_IS_IDENTITY, Value::String(CString::new("").unwrap()));
		out.set_one(PROP_TIME, Value::Double(time));

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		if std::env::var_os("OAK_OFX_TRACE").is_some() {
			eprintln!("[ofx] isIdentity in_args {:p} out_args {:p}", &in_args, &out);
		}
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_IS_IDENTITY, inst_handle, &in_args, &out)
		};
		if stat != crate::suites::status::OK && stat != crate::suites::status::REPLY_DEFAULT {
			// A failed isIdentity is NOT fatal: the C++ plugin renderer
			// never calls this action at all (plugins that error on it —
			// the CImg suite answers MissingHostFeature — simply render
			// normally). Treat any non-OK as "not identity" and let the
			// real render decide.
			if std::env::var_os("OAK_OFX_TRACE").is_some() {
				eprintln!("[ofx] isIdentity stat {stat}: proceeding to render");
			}
			return Ok(None);
		}
		let identity = out.get(PROP_IS_IDENTITY, 0);
		match identity {
			Some(Value::String(s)) if !s.is_empty() => {
				let clip_name = s.to_string_lossy().into_owned();
				let t = out
					.get(PROP_TIME, 0)
					.and_then(|v| match v {
						Value::Double(d) => Some(d),
						_ => None,
					})
					.unwrap_or(time);
				Ok(Some((t, clip_name)))
			}
			_ => Ok(None),
		}
	}

	/// render action 的 in args 组装（CPU/GL 共用；GL 模式加
	/// kOfxImageEffectPropOpenGLEnabled=1，ofxGPURender.h:117）。
	fn render_in_args(
		time: f64,
		scale: RenderScale,
		window: OfxRectD,
		gl_enabled: bool,
	) -> PropertySet {
		use crate::host::{
			PROP_FIELD_TO_RENDER, PROP_INTERACTIVE_RENDER, PROP_NO_SPATIAL_AWARENESS,
			PROP_RENDER_QUALITY_DRAFT, PROP_RENDER_SCALE, PROP_RENDER_WINDOW,
			PROP_SEQUENTIAL_RENDER, PROP_TIME,
		};
		use crate::property::Value;

		let in_args = PropertySet::new();
		in_args.set_one(PROP_TIME, Value::Double(time));
		in_args.define(
			PROP_RENDER_SCALE,
			vec![Value::Double(scale.x), Value::Double(scale.y)],
		);
		// RenderWindow 必须是 Int x4（支持库 ofxsPropertyValidation 对
		// render in-args 的强制类型；像素坐标本就是整型——Double 会校验
		// 失败成 kOfxStatFailed）。
		in_args.define(
			PROP_RENDER_WINDOW,
			vec![
				Value::Int(window.x1.round() as i32),
				Value::Int(window.y1.round() as i32),
				Value::Int(window.x2.round() as i32),
				Value::Int(window.y2.round() as i32),
			],
		);
		in_args.set_one(
			PROP_FIELD_TO_RENDER,
			Value::String(CString::new("OfxFieldNone").unwrap()),
		);
		in_args.set_one(PROP_SEQUENTIAL_RENDER, Value::Int(0));
		in_args.set_one(PROP_INTERACTIVE_RENDER, Value::Int(0));
		in_args.set_one(PROP_RENDER_QUALITY_DRAFT, Value::Int(0));
		in_args.set_one(PROP_NO_SPATIAL_AWARENESS, Value::Int(0));
		// Natron 支持库对 NatronOfxPropNativeOverlays 的强读（0 = 宿主
		// 不支持原生 overlay——插件据此走自己的回退路径）。
		in_args.set_one("NatronOfxPropNativeOverlays", Value::Int(0));
		if gl_enabled {
			in_args.set_one(crate::host::PROP_GL_ENABLED, Value::Int(1));
		}
		in_args
	}

	/// render action（CPU 路径）。`output` 为已按协商格式分配的输出
	/// 图像（Arc：插件经 clipGetImage(Output) 取用，render 驱动与
	/// suite 各自持强引用）；输入 clip 的图像由调用方经
	/// [`ClipInstance::fetch_image`] 备好。GL 路径见
	/// [`Instance::render_gl`]。
	///
	/// 声明原为 `&mut Image`：输出图像需与 suite 共享强引用（TLS +
	/// LIVE_IMAGES 表），&mut 无法表达——改为 `Arc<Image>`。
	pub fn render(
		&self,
		time: f64,
		scale: RenderScale,
		window: OfxRectD,
		output: std::sync::Arc<crate::image::Image>,
	) -> crate::error::Result<()> {
		use crate::host::ACTION_RENDER;

		let in_args = Self::render_in_args(time, scale, window, false);

		// facade 取消标记：render 入口即短路（多帧循环的帧间取消）。
		if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
			return Err(crate::error::Error::Failed("已取消".into()));
		}

		// 渲染上下文（TLS）：timeline/progress/clipGetImage 读取。
		let range = self
			.sequence_range
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.unwrap_or(OfxRangeD { min: 0.0, max: 0.0 });
		crate::suites::set_render_ctx(Some(crate::suites::RenderCtx { time, scale, range }));
		crate::suites::set_current_output(Some(output.clone()));

		// 进度报告器（facade 回调优先；无回调而 app 注册了 UI 工厂
		// 时装静默报告器，progressStart 再经工厂现造 UI 报告器）。
		if let Some((cb, userdata)) = *self
			.progress_cb
			.lock()
			.unwrap_or_else(|e| e.into_inner())
		{
			crate::suites::progress::set_current(Some(unsafe {
				crate::progress::ProgressReporter::new(cb, userdata as *mut std::ffi::c_void)
			}));
		} else if crate::progress::has_reporter_factory() {
			crate::suites::progress::set_current(Some(
				crate::progress::ProgressReporter::silent(),
			));
		}

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let out = PropertySet::new();
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_RENDER, inst_handle, &in_args, &out)
		};

		// 清理 TLS（action 返回后渲染上下文必须消失）。
		crate::suites::set_current_output(None);
		crate::suites::set_render_ctx(None);
		crate::suites::progress::set_current(None);

		if stat != crate::suites::status::OK {
			return Err(crate::error::Error::Failed(format!("render 失败：{stat}")));
		}
		Ok(())
	}

	/// GL render action（M11 §4；ofxGPURender.h 的 Render 动作 GL
	/// 模式）。与 [`Instance::render`]（CPU）并存。
	///
	/// 前置契约（render 驱动遵守）：调用方已把 `renderer` 的 GL
	/// 上下文置为 current（ofxGPURender.h "OpenGL Current Context"：
	/// 宿主只在 Render/Begin/EndSequenceRender/Attach/Detach 期间
	/// 要求上下文 current——本实现的约定是 oakrender 的 PluginJob
	/// 路径在进入前做好），且 `output_texture` 已附着为渲染器输出
	/// 目标（等价 C++ `PluginRenderer::attach_output_texture`）。
	/// `output_gl_texture` 为宿主为输出帧建的真实 GL 纹理名（GL 模式
	/// 下 clipLoadTexture(Output) 的 OpenGLTextureIndex 以它为准）；
	/// None = CPU 回退语义。
	///
	/// action 序列：kOfxActionOpenGLContextAttached → render（in args
	/// 带 kOfxImageEffectPropOpenGLEnabled=1）→
	/// kOfxActionOpenGLContextDetached（ofxGPURender.h:345-371；
	/// attach/detach 必须配对）。渲染结果留在 GL 输出纹理上（插件
	/// 直接画进附着目标），宿主经 glReadPixels 回读（render 驱动
	/// 负责，见 [`crate::gl_bridge`]）；GL 模式下
	/// clipGetImage(Output) 不可用（插件按规范走 OpenGL suite——
	/// ofxGPURender.h "the effect SHOULD access all its images through
	/// the OpenGL suite"）。render 返回前对未释放的输入 GL 纹理做
	/// 兜底清理（[`crate::suites::gl_render::purge_leftovers`]）。
	pub fn render_gl(
		&self,
		time: f64,
		scale: RenderScale,
		window: OfxRectD,
		renderer: crate::render::Renderer,
		output_texture: crate::render::Texture,
		output_gl_texture: Option<i32>,
	) -> crate::error::Result<()> {
		use crate::host::ACTION_RENDER;

		let in_args = Self::render_in_args(time, scale, window, true);

		if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
			return Err(crate::error::Error::Failed("已取消".into()));
		}

		let range = self
			.sequence_range
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.unwrap_or(OfxRangeD { min: 0.0, max: 0.0 });
		crate::suites::set_render_ctx(Some(crate::suites::RenderCtx { time, scale, range }));
		// GL 纹理位深协商（插件 kOfxOpenGLPropPixelDepth → 管线 F32；
		// 调用方已按协商门（render 驱动）决定走 GL）。
		let gl_pixel_depth =
			crate::suites::gl_render::pick_gl_pixel_depth(&self.plugin.descriptor.props)
				.unwrap_or("OfxBitDepthFloat");
		crate::suites::set_gl_ctx(Some(crate::suites::GlCtx {
			renderer,
			output_texture,
			gl_pixel_depth,
			output_gl_texture,
		}));
		// GL 模式无 CPU 输出图像（current_output 保持 None）。

		if let Some((cb, userdata)) = *self
			.progress_cb
			.lock()
			.unwrap_or_else(|e| e.into_inner())
		{
			crate::suites::progress::set_current(Some(unsafe {
				crate::progress::ProgressReporter::new(cb, userdata as *mut std::ffi::c_void)
			}));
		} else if crate::progress::has_reporter_factory() {
			crate::suites::progress::set_current(Some(
				crate::progress::ProgressReporter::silent(),
			));
		}

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let empty = PropertySet::new();
		// attach（失败仍继续——插件可忽略；规范允许 ReplyDefault）。
		let stat = unsafe {
			self.plugin.call_action(
				crate::host::ACTION_GL_CONTEXT_ATTACHED,
				inst_handle,
				&empty,
				&empty,
			)
		};
		if stat != crate::suites::status::OK && stat != crate::suites::status::REPLY_DEFAULT {
			crate::suites::set_gl_ctx(None);
			crate::suites::set_render_ctx(None);
			crate::suites::progress::set_current(None);
			return Err(crate::error::Error::Failed(format!(
				"OpenGLContextAttached 失败：{stat}"
			)));
		}

		let out = PropertySet::new();
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_RENDER, inst_handle, &in_args, &out)
		};

		// detach（必须与 attach 配对；ofxGPURender.h:345-346）。
		unsafe {
			self.plugin.call_action(
				crate::host::ACTION_GL_CONTEXT_DETACHED,
				inst_handle,
				&empty,
				&empty,
			)
		};

		// 兜底：插件遗漏 clipFreeTexture 的输入纹理在此释放。
		crate::suites::gl_render::purge_leftovers();
		crate::suites::set_gl_ctx(None);
		crate::suites::set_render_ctx(None);
		crate::suites::progress::set_current(None);

		if stat != crate::suites::status::OK {
			return Err(crate::error::Error::Failed(format!(
				"GL render 失败：{stat}"
			)));
		}
		Ok(())
	}

	/// GetOutputColourspace action（M11 §4；ofxColour.h:243-283）：
	/// 宿主给插件一份偏好色彩空间列表，插件回写输出 clip 的色彩
	/// 空间（可为 "OfxColourspace_<clip>" 交叉引用）。
	///
	/// 返回解析后的输出色彩空间：交叉引用解析为所引 clip 的实际
	/// 色彩空间（ofxColour.h:142-141 的跨 clip 引用约定）；
	/// REPLY_DEFAULT → 第一个输入 clip 的色彩空间（ofxColour.h:278-279）。
	/// 调用方（render 驱动）随后把结果写回输出 clip（
	/// [`Instance::set_output_colourspace`]）。
	pub fn get_output_colourspace(&self, preferred: &[String]) -> crate::error::Result<String> {
		use crate::host::{
			ACTION_GET_OUTPUT_COLOURSPACE, PROP_CLIP_COLOURSPACE, PROP_CLIP_PREFERRED_COLOURSPACES,
		};
		use crate::property::Value;

		let in_args = PropertySet::new();
		let values: Vec<Value> = preferred
			.iter()
			.map(|s| Value::String(CString::new(s.as_str()).unwrap()))
			.collect();
		in_args.define(PROP_CLIP_PREFERRED_COLOURSPACES, values);
		let out = PropertySet::new();
		out.set_one(
			PROP_CLIP_COLOURSPACE,
			Value::String(CString::new("").unwrap()),
		);

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_GET_OUTPUT_COLOURSPACE, inst_handle, &in_args, &out)
		};
		if stat == crate::suites::status::OK {
			let value = out
				.get(PROP_CLIP_COLOURSPACE, 0)
				.map(|v| match v {
					Value::String(s) => s.to_string_lossy().into_owned(),
					_ => String::new(),
				})
				.unwrap_or_default();
			if value.is_empty() {
				return Err(crate::error::Error::Failed(
					"GetOutputColourspace 未回写色彩空间".into(),
				));
			}
			Ok(self.resolve_colourspace(value))
		} else if stat == crate::suites::status::REPLY_DEFAULT {
			// 插件未实现 → 用第一个输入 clip 的色彩空间（规范默认）。
			let first_input = self
				.clips
				.iter()
				.find(|c| c.name != "Output")
				.map(|c| {
					c.props
						.get(PROP_CLIP_COLOURSPACE, 0)
						.map(|v| match v {
							Value::String(s) => s.to_string_lossy().into_owned(),
							_ => String::new(),
						})
						.unwrap_or_default()
				})
				.unwrap_or_default();
			Ok(first_input)
		} else {
			Err(crate::error::Error::Failed(format!(
				"GetOutputColourspace 失败：{stat}"
			)))
		}
	}

	/// 把输出 clip 的色彩空间写回（GetOutputColourspace 后的落点；
	/// ofxColour.h:262-264 "the host must set kOfxImageClipPropColourspace
	/// on the instance's output clip to the value from outArgs"）。
	pub fn set_output_colourspace(&self, colourspace: &str) {
		use crate::property::Value;
		if let Some(output) = self.clips.iter().find(|c| c.name == "Output") {
			output.props.set_one(
				crate::host::PROP_CLIP_COLOURSPACE,
				Value::String(CString::new(colourspace).unwrap()),
			);
		}
	}

	/// 解析 "OfxColourspace_<clip>" 交叉引用为所引 clip 的实际色彩
	/// 空间；非交叉引用原样返回。
	pub fn resolve_colourspace(&self, value: String) -> String {
		const PREFIX: &str = "OfxColourspace_";
		if let Some(clip_name) = value.strip_prefix(PREFIX) {
			self.clips
				.iter()
				.find(|c| c.name == clip_name)
				.map(|c| {
					c.props
						.get(crate::host::PROP_CLIP_COLOURSPACE, 0)
						.map(|v| match v {
							crate::property::Value::String(s) => s.to_string_lossy().into_owned(),
							_ => value.clone(),
						})
						.unwrap_or(value.clone())
				})
				.unwrap_or(value)
		} else {
			value
		}
	}

	/// begin/endSequenceRender 配对（帧序列渲染的前后括号；
	/// HS: ofxhImageEffect.cpp:473-513 的 in args：frameRange +
	/// frameStep + renderScale + sequentialRenderStatus）。GL 模式
	/// 变体 [`Instance::begin_sequence_render_gl`] 额外带
	/// kOfxImageEffectPropOpenGLEnabled（ofxGPURender.h:117）。
	pub fn begin_sequence_render(&self, range: OfxRangeD) -> crate::error::Result<()> {
		self.begin_sequence_inner(range, false)
	}

	/// GL 模式的 beginSequenceRender（in args 带 OpenGLEnabled=1）。
	pub fn begin_sequence_render_gl(&self, range: OfxRangeD) -> crate::error::Result<()> {
		self.begin_sequence_inner(range, true)
	}

	fn begin_sequence_inner(&self, range: OfxRangeD, gl_enabled: bool) -> crate::error::Result<()> {
		use crate::host::ACTION_BEGIN_SEQUENCE;

		let in_args = Self::sequence_in_args(range, gl_enabled);
		self.sequence_range
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.replace(range);

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let out = PropertySet::new();
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_BEGIN_SEQUENCE, inst_handle, &in_args, &out)
		};
		if stat != crate::suites::status::OK {
			return Err(crate::error::Error::Failed(format!(
				"beginSequenceRender 失败：{stat}"
			)));
		}
		Ok(())
	}

	/// 序列 action 的 in args 组装（begin/end 共用；GL 模式加
	/// OpenGLEnabled=1）。
	fn sequence_in_args(range: OfxRangeD, gl_enabled: bool) -> PropertySet {
		use crate::host::{
			PROP_FRAME_RANGE, PROP_FRAME_STEP, PROP_RENDER_SCALE, PROP_SEQUENTIAL_RENDER,
		};
		use crate::property::Value;

		let in_args = PropertySet::new();
		in_args.define(
			PROP_FRAME_RANGE,
			vec![Value::Double(range.min), Value::Double(range.max)],
		);
		in_args.set_one(PROP_FRAME_STEP, Value::Double(1.0));
		in_args.define(
			PROP_RENDER_SCALE,
			vec![Value::Double(1.0), Value::Double(1.0)],
		);
		in_args.set_one(PROP_SEQUENTIAL_RENDER, Value::Int(1));
		if gl_enabled {
			in_args.set_one(crate::host::PROP_GL_ENABLED, Value::Int(1));
		}
		in_args
	}

	/// 见 [`Instance::begin_sequence_render`]。
	pub fn end_sequence_render(&self, range: OfxRangeD) -> crate::error::Result<()> {
		self.end_sequence_inner(range, false)
	}

	/// GL 模式的 endSequenceRender。
	pub fn end_sequence_render_gl(&self, range: OfxRangeD) -> crate::error::Result<()> {
		self.end_sequence_inner(range, true)
	}

	fn end_sequence_inner(&self, range: OfxRangeD, gl_enabled: bool) -> crate::error::Result<()> {
		use crate::host::ACTION_END_SEQUENCE;

		let in_args = Self::sequence_in_args(range, gl_enabled);

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let out = PropertySet::new();
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_END_SEQUENCE, inst_handle, &in_args, &out)
		};
		self.sequence_range
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.take();
		if stat != crate::suites::status::OK {
			return Err(crate::error::Error::Failed(format!(
				"endSequenceRender 失败：{stat}"
			)));
		}
		Ok(())
	}

	/// kOfxActionInstanceChanged 的宿主侧分发（ofxCore.h:405-435 的
	/// inArgs 契约）：
	///
	/// - kOfxPropType = kOfxTypeParameter（参数值变更）
	/// - kOfxPropName = 变更的参数名
	/// - kOfxPropChangeReason = kOfxChange*（UserEdited / PluginEdited /
	///   Time）
	/// - kOfxPropTime = 变更发生时的效果时间（Image Effect 插件专属）
	/// - kOfxImageEffectPropRenderScale = 当前渲染比例
	///
	/// 宿主按下 push button 后以 UserEdited 调用它（push button 无值，
	/// 插件的反应全在 instanceChanged 里；真实插件如 CImg 依赖此动作
	/// 刷新内部状态）。返回非 OK/ReplyDefault 状态码时为 Err。
	pub fn instance_changed(
		&self,
		param_name: &str,
		reason: crate::param::ChangeReason,
		time: f64,
		scale: RenderScale,
	) -> crate::error::Result<()> {
		use crate::host::{
			ACTION_INSTANCE_CHANGED, CHANGE_PLUGIN_EDITED, CHANGE_TIME, CHANGE_USER_EDITED,
			PROP_CHANGE_REASON, PROP_RENDER_SCALE, PROP_TIME,
		};
		use crate::property::Value;

		let in_args = PropertySet::new();
		in_args.set_one(
			crate::param::PROP_TYPE,
			Value::String(CString::new(crate::param::TYPE_PARAMETER).unwrap()),
		);
		in_args.set_one(
			crate::param::PROP_NAME,
			Value::String(CString::new(param_name).unwrap()),
		);
		let reason_str = match reason {
			crate::param::ChangeReason::UserEdited => CHANGE_USER_EDITED,
			crate::param::ChangeReason::PluginEdited => CHANGE_PLUGIN_EDITED,
			crate::param::ChangeReason::TimeChanged => CHANGE_TIME,
		};
		in_args.set_one(
			PROP_CHANGE_REASON,
			Value::String(CString::new(reason_str).unwrap()),
		);
		in_args.set_one(PROP_TIME, Value::Double(time));
		in_args.define(
			PROP_RENDER_SCALE,
			vec![Value::Double(scale.x), Value::Double(scale.y)],
		);

		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let out = PropertySet::new();
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_INSTANCE_CHANGED, inst_handle, &in_args, &out)
		};
		if stat != crate::suites::status::OK && stat != crate::suites::status::REPLY_DEFAULT {
			return Err(crate::error::Error::Failed(format!(
				"instanceChanged 失败：{stat}"
			)));
		}
		Ok(())
	}

	/// 创建关联的 interact（主进程 UI 事件宿主；任务契约的
	/// `new_interact(instance)`）。与 worker 进程里的渲染实例并存——
	/// OFX 允许同一插件多实例，interact 是独立对象（独立 handle），
	/// 经 [`crate::suites::interact::Interact::handle`] 与效果实例区分。
	///
	/// 流程：插件声明的 overlay interact 入口（V2 优先、V1 次之；
	/// ofxImageEffect.h:825/812）作为 interact 的入口——未声明则用插件
	/// main entry（任务契约）→ 建 [`Interact`]（属性表 + tagged handle）
	/// → 向入口发 `kOfxActionNewInteract`。
	///
	/// 返回值：
	/// - `Some(interact)`：创建成功（NewInteract 返回 OK；或插件未实现
	///   NewInteract 但声明了 overlay interact——真实 overlay 插件不认
	///   NewInteract，走 describe/create 序列）。
	/// - `None`：插件无 interact（NewInteract 返回 kOfxStatReplyDefault
	///   且未声明 overlay 入口）或返回错误状态。
	///
	/// 创建后调用方继续 [`Interact::describe`] → [`Interact::create_instance`]
	/// 完成实例化；销毁随实例自动连带（[`Instance::notify_destroy`]）。
	pub fn new_interact(&self) -> Option<std::sync::Arc<crate::suites::interact::Interact>> {
		use crate::host::overlay_interact_entry;
		use crate::suites::interact::Interact;

		let overlay = overlay_interact_entry(&self.plugin.descriptor.props);
		let entry = overlay.unwrap_or(self.plugin.entry);
		let has_overlay = overlay.is_some();

		let effect_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let interact = Interact::new(self.plugin.clone(), entry, effect_handle);

		let empty = PropertySet::new();
		let st = interact.call(crate::host::ACTION_NEW_INTERACT, &empty, &empty);
		let accepted = match st {
			crate::suites::status::OK => true,
			// 插件未实现 NewInteract 但声明了 overlay interact → 仍创建
			//（走 describe/create 的官方序列）。
			crate::suites::status::REPLY_DEFAULT if has_overlay => true,
			_ => false,
		};
		if !accepted {
			return None;
		}
		*self
			.interact
			.lock()
			.unwrap_or_else(|e| e.into_inner()) = Some(interact.clone());
		Some(interact)
	}

	/// 已创建的 interact（None = 未创建）。
	pub fn interact(&self) -> Option<std::sync::Arc<crate::suites::interact::Interact>> {
		self.interact
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.clone()
	}

	/// describe_interact（任务契约）：对已创建的 interact 发
	/// `kOfxActionDescribe`（kOfxActionDescribeInteract）。未创建 →
	/// kOfxStatErrBadHandle。
	pub fn describe_interact(&self) -> i32 {
		match self.interact() {
			Some(i) => i.describe(),
			None => crate::suites::status::ERR_BAD_HANDLE,
		}
	}

	/// 销毁（destroyInstance action）。析构由 `Arc<RefBox<Instance>>`
	/// 归零驱动；此处只做 action 通知，幂等（[`Instance::drop`] 的
	/// `destroyed` 门保证只发一次）。
	pub(crate) fn notify_destroy(&self) {
		use crate::host::ACTION_DESTROY_INSTANCE;
		let inst_handle = crate::suites::tag::make(
			&self.props as *const PropertySet,
			crate::suites::tag::INSTANCE,
		);
		let empty = PropertySet::new();
		// interact 与实例同生命周期：实例销毁前先销毁 interact
		//（ofxInteract.h kOfxActionDestroyInstanceInteract 的 \pre 要求
		// 实例成员尚未销毁）。
		if let Some(i) = self
			.interact
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.take()
		{
			i.destroy();
		}
		// 通知失败只记日志（销毁路径不可回滚）。
		let stat = unsafe {
			self.plugin
				.call_action(ACTION_DESTROY_INSTANCE, inst_handle, &empty, &empty)
		};
		if stat != crate::suites::status::OK && stat != crate::suites::status::REPLY_DEFAULT {
			eprintln!("destroyInstance 通知失败：{stat}");
		}
	}
}

/// 从属性集读矩形（Double×4）。
fn read_rect(props: &PropertySet, name: &str) -> Option<OfxRectD> {
	use crate::property::Value;
	let x1 = match props.get(name, 0)? {
		Value::Double(d) => d,
		_ => return None,
	};
	let y1 = match props.get(name, 1)? {
		Value::Double(d) => d,
		_ => return None,
	};
	let x2 = match props.get(name, 2)? {
		Value::Double(d) => d,
		_ => return None,
	};
	let y2 = match props.get(name, 3)? {
		Value::Double(d) => d,
		_ => return None,
	};
	Some(OfxRectD { x1, y1, x2, y2 })
}

/// getClipPreferences 的协商结果。
#[derive(Clone, Debug)]
pub struct ClipPreferences {
	/// 输出分量（kOfxImageComponentRGBA 等）。
	pub output_components: String,
	/// 输出位深（kOfxBitDepthFloat 等；全链路 F32 下恒为 float）。
	pub output_bit_depth: String,
	/// 像素比。
	pub pixel_aspect_ratio: f64,
	/// 帧率。
	pub frame_rate: f64,
	/// field 处理模式（kOfxImageField*）。
	pub field: String,
}
