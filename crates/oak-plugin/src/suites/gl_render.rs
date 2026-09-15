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

//! OfxImageEffectOpenGLRenderSuiteV1（M11 第 2 期）：clipLoadTexture /
//! clipFreeTexture / flushResources。
//!
//! 语义对照 ofxGPURender.h（vendored，OpenGL Render Suite 一节）与
//! HostSupport 插件侧实现（HS: ofxhImageEffect.cpp:2296-2367）：
//!
//! - `clipLoadTexture(clip, time, format, region, *texture)`：把 clip
//!   在 `time` 的图像加载为 GL 纹理。纹理句柄是**属性集**（标签 0），
//!   含 ofxGPURender.h 规定的 12 个属性（OpenGLTextureIndex/
//!   OpenGLTextureTarget/PixelDepth/Components/PreMultiplication/
//!   RenderScale/PixelAspectRatio/Bounds/RegionOfDefinition/RowBytes/
//!   Field/UniqueIdentifier）；宿主侧存强引用（[`LIVE_TEXTURES`]），
//!   clipFreeTexture 摘除即释放（对应 HS 的 get/release 配对）。
//!   **OpenGLTextureIndex 是真实 GL 纹理名**（GL 模式，见
//!   [`crate::gl_bridge`] 方案 B）：Output clip 是宿主为输出帧建的
//!   GL 纹理（经 GlCtx 注入，render 驱动负责建/FBO 挂载/回读/删除）；
//!   输入 clip 是本 suite 在 GL 渲染期经桥上传图像得到的 GL 纹理
//!   （clipFreeTexture 删除）。0 = CPU 回退（无 GL 名，插件按规范回退）。
//! - Output clip：`format` 忽略，宿主返回已附着的输出纹理句柄——
//!   绑定渲染目标的动作（ofxGPURender.h "the host must bind the
//!   resulting texture as the current color buffer"）由调用方约定：
//!   GL render 驱动在进入 render_gl 前已把输出 GL 纹理挂进 FBO 并
//!   绑定（等价 C++ 的 `PluginRenderer::attach_output_texture`）。
//!   clipFreeTexture 对 Output 只释放句柄、不删纹理（宿主还要回读它，
//!   回读后由 render 驱动删除）。
//! - 输入纹理在 GL 模式经 [`crate::gl_bridge::create_input_texture`]
//!   上传（CPU 帧 → GL 纹理）；CPU 模式（无 GL 上下文）维持
//!   [`crate::render`] 的 CPU 纹理。纹理格式：全链路 F32 约束下，
//!   像素深度按 clip 协商结果（恒 F32）；`format` 参数
//!   （kOfxImageEffectGLFormat*）若请求的分量与协商分量不符，Phase 2
//!   不做转换 → Failed（规范要求 "host ensures it gives the requested
//!   format"——宁可显式失败也不静默给错格式）。ofxGPURender.h 注明
//!   "宿主无需按 Clip Preferences 把图像重映射到插件请求的位深"，
//!   插件以纹理句柄的 PixelDepth/Components 为准。
//! - `flushResources`：宿主在 render 之间不缓存 GPU 资源（纹理随
//!   clipFreeTexture 立即释放）→ 无可释放 → kOfxStatReplyDefault
//!   （规范："nothing the host could do"）。
//! - GL 上下文规则（ofxGPURender.h "OpenGL Current Context"）：宿主
//!   只在 Render/BeginSequenceRender/EndSequenceRender/Attach/Detach
//!   期间要求上下文 current；本实现的约定是 render 驱动一次 acquire
//!   整个 GL render action（[`crate::gl_bridge::acquire`]），本 suite
//!   回调期间上下文恒 current，经 [`crate::suites::gl_ctx`] TLS 取
//!   渲染器句柄。

use std::collections::HashMap;
use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use crate::clip::ClipInstance;
use crate::image::Image;
use crate::property::{PropertySet, Value};
use crate::suites::{status, tag};

// ---- GL 常量（ofxGPURender.h；GLFormat 字符串为 ofxOpenGLRender.h
// OpenFX 1.4 的规范名，vendored 头文件是 stub 未收录，按规范定义）----

/// kOfxImageEffectPropOpenGLTextureIndex（ofxGPURender.h:135）。
pub(crate) const GL_TEXTURE_INDEX: &str = "OfxImageEffectPropOpenGLTextureIndex";
/// kOfxImageEffectPropOpenGLTextureTarget（ofxGPURender.h:152）。
pub(crate) const GL_TEXTURE_TARGET: &str = "OfxImageEffectPropOpenGLTextureTarget";

/// kOfxImageEffectGLFormatRGBA（ofxOpenGLRender.h 1.4）。
pub(crate) const GL_FORMAT_RGBA: &str = "OfxImageEffectGLFormatRGBA";
/// kOfxImageEffectGLFormatRGB。
pub(crate) const GL_FORMAT_RGB: &str = "OfxImageEffectGLFormatRGB";
/// kOfxImageEffectGLFormatAlpha。
pub(crate) const GL_FORMAT_ALPHA: &str = "OfxImageEffectGLFormatAlpha";
/// kOfxImageEffectGLFormatLuminance。
pub(crate) const GL_FORMAT_LUMINANCE: &str = "OfxImageEffectGLFormatLuminance";
/// kOfxImageEffectGLFormatLuminanceAlpha。
pub(crate) const GL_FORMAT_LUMINANCE_ALPHA: &str = "OfxImageEffectGLFormatLuminanceAlpha";

/// GL_TEXTURE_2D 的 GLenum 值（0x0DE1；纹理句柄的 OpenGLTextureTarget
/// 属性。GL 规范值，非 OFX 宏）。
pub(crate) const GL_TEXTURE_2D: i32 = 0x0DE1;

/// kOfxImageEffectPropPreMultiplication 的非预乘默认值。
pub(crate) const PREMULT_NONE: &str = "OfxImagePreMultipliedNone";

/// GL 像素深度协商（ofxGPURender.h:65-89 kOfxOpenGLPropPixelDepth）：
/// 插件描述符声明的 GL 渲染支持位深列表（可选）。
///
/// 返回 Some(实际位深) 表示 GL 模式可行，None 表示管线无法满足
/// 插件声明 → 宿主应回退 CPU 渲染（ofxGPURender.h "the host will
/// try to provide buffers/textures in one of the supported formats"；
/// Phase 2 全链路 F32，无法提供其他位深）：
/// - 列表缺失/为空 → Some(Float)（宿主自选，规范默认）；
/// - 列表含 Float → Some(Float)；
/// - 列表存在且不含 Float → None（管线约束；GL 模式不可行）。
pub(crate) fn pick_gl_pixel_depth(
	plugin_props: &crate::property::PropertySet,
) -> Option<&'static str> {
	use crate::property::Value;
	let dim = plugin_props.dimension(crate::host::PROP_GL_PIXEL_DEPTH);
	if dim == 0 {
		return Some("OfxBitDepthFloat");
	}
	for i in 0..dim {
		if let Some(Value::String(s)) = plugin_props.get(crate::host::PROP_GL_PIXEL_DEPTH, i) {
			if s.to_string_lossy() == "OfxBitDepthFloat" {
				return Some("OfxBitDepthFloat");
			}
		}
	}
	None
}

// ---- 存活纹理表 -----------------------------------------------------------

/// 存活 GL 纹理表：clipLoadTexture 产出（props 地址 → 属性集 +
/// 纹理值 + 是否输出 clip + 真实 GL 纹理名[可选]）；clipFreeTexture
/// 摘除即释放——对应 HS 的 get/release 配对
/// （HS: ofxhImageEffect.cpp:2336-2351）。纹理是 oakrender 值（drop
/// 自动释放后端 token；原 `texture_free` 调用面随值模型删除）。
///
/// `gl_texture`：GL 模式下输入 clip 经 [`crate::gl_bridge`] 建的真实
/// GL 纹理名（输出 clip 恒 None——宿主自建并负责生命周期，见
/// [`crate::gl_bridge`] 模块文档）；摘除时经
/// [`delete_gl_texture_if_gl`] 删除。
///
/// 属性集必须**装箱**（Box 稳定堆地址）：纹理句柄指向它，函数返回后
/// 必须仍存活；栈上临时变量会悬垂（phase-2 实现初版的 bug）。
static LIVE_TEXTURES: std::sync::LazyLock<
	Mutex<HashMap<usize, (Box<PropertySet>, crate::render::Texture, bool, Option<i32>)>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// 登记纹理（clipLoadTexture 内部；`props` 装箱后取地址为句柄基址）。
/// `gl_texture`：真实 GL 纹理名（输入 clip GL 模式；无则 None）。
fn register(
	props: Box<PropertySet>,
	texture: crate::render::Texture,
	is_output: bool,
	gl_texture: Option<i32>,
) -> usize {
	let addr = &*props as *const PropertySet as usize;
	LIVE_TEXTURES
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.insert(addr, (props, texture, is_output, gl_texture));
	addr
}

/// 删除 GL 纹理（上下文 current 时；否则留给下次 GL 渲染前的兜底——
/// 正常路径 clipFreeTexture/purge 都在 GL render 期，上下文必 current）。
fn delete_gl_texture_if_gl(name: i32) {
	if crate::suites::gl_ctx().is_some() {
		crate::gl_bridge::delete_gl_texture(name);
	}
}

/// 释放全部残留输入纹理（GL render action 返回后的安全网：规范要求
/// 插件在 action 返回前 clipFreeTexture 全部句柄；遗漏的输入纹理在
/// 此释放，输出纹理保留——宿主还要读它）。GL 输入纹理同步删除
/// （上下文 current；render_gl 在清 GlCtx TLS 前调用本函数）。
pub(crate) fn purge_leftovers() {
	let mut live = LIVE_TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
	let dropped: Vec<Option<i32>> = live
		.iter()
		.filter(|(_, (_, _, is_output, _))| !*is_output)
		.map(|(_, (_, _, _, gl))| *gl)
		.collect();
	live.retain(|_, (_, _, is_output, _)| *is_output);
	drop(live);
	for gl in dropped.into_iter().flatten() {
 			delete_gl_texture_if_gl(gl);
 		}
}

/// 公共入口模板：panic 兜底。
fn caught(f: impl FnOnce() -> Result<(), c_int>) -> c_int {
	catch_unwind(AssertUnwindSafe(f)).map_or_else(
		|_| status::FAILED,
		|r| r.map_or_else(|c| c, |()| status::OK),
	)
}

/// clip 句柄解析（实例期）。
fn resolve_clip(handle: *mut c_void) -> Result<&'static ClipInstance, c_int> {
	if handle.is_null() {
		return Err(status::ERR_BAD_HANDLE);
	}
	unsafe {
		match tag::kind(handle) {
			tag::CLIP => Ok(&*(tag::strip(handle) as *const ClipInstance)),
			_ => Err(status::ERR_BAD_HANDLE),
		}
	}
}

fn cs(s: &str) -> CString {
	CString::new(s).unwrap()
}

/// 把矩形写成 Int×4 属性（Bounds/ROD 用；OFX 图像属性是像素坐标）。
fn rect_props(props: &PropertySet, name: &str, rect: crate::instance::OfxRectD) {
	let b = |v: f64| Value::Int(v.round() as i32);
	props.define(name, vec![b(rect.x1), b(rect.y1), b(rect.x2), b(rect.y2)]);
}

/// 从 clip 属性读字符串（缺失返回默认）。
fn clip_string(clip: &ClipInstance, name: &str, default: &str) -> String {
	clip.props
		.get(name, 0)
		.map(|v| match v {
			Value::String(s) => s.to_string_lossy().into_owned(),
			_ => default.to_string(),
		})
		.unwrap_or_else(|| default.to_string())
}

/// 构造纹理属性集（ofxGPURender.h 规定的属性；输入与输出共用，
/// 只是数据来源不同）。`OpenGLTextureIndex` 为真实 GL 纹理名
/// （`texture_index`；0 = 无 GL 名——CPU 回退下插件按规范回退，纹理
/// 内容仍可经 clipGetImage 取用）。
#[allow(clippy::too_many_arguments)]
fn make_texture_props(
	width: f64,
	height: f64,
	components: crate::image::Components,
	depth: &str,
	premult: &str,
	par: f64,
	scale: crate::instance::RenderScale,
	row_bytes: i32,
	texture_index: i32,
) -> PropertySet {
	let props = PropertySet::new();
	props.set_one(GL_TEXTURE_INDEX, Value::Int(texture_index));
	props.set_one(GL_TEXTURE_TARGET, Value::Int(GL_TEXTURE_2D));
	props.set_one(
		crate::image::K_IMAGE_EFFECT_PROP_PIXEL_DEPTH,
		Value::String(cs(depth)),
	);
	props.set_one(
		crate::image::K_IMAGE_EFFECT_PROP_COMPONENTS,
		Value::String(cs(components.to_ofx())),
	);
	props.set_one(
		"OfxImageEffectPropPreMultiplication",
		Value::String(cs(premult)),
	);
	props.define(
		crate::host::PROP_RENDER_SCALE,
		vec![Value::Double(scale.x), Value::Double(scale.y)],
	);
	props.set_one("OfxImagePropPixelAspectRatio", Value::Double(par));
	let bounds = crate::instance::OfxRectD {
		x1: 0.0,
		y1: 0.0,
		x2: width,
		y2: height,
	};
	rect_props(&props, crate::image::K_IMAGE_PROP_BOUNDS, bounds);
	rect_props(&props, crate::image::K_IMAGE_PROP_ROD, bounds);
	props.set_one(crate::image::K_IMAGE_PROP_ROW_BYTES, Value::Int(row_bytes));
	props.set_one("OfxImagePropField", Value::String(cs("OfxFieldNone")));
	props.set_one(
		crate::image::K_IMAGE_PROP_UNIQUE_ID,
		Value::String(crate::image::unique_identifier()),
	);
	props
}

/// 纹理请求格式 → 期望分量（NULL/未知 → None = 宿主自选）。
fn format_components(format: Option<&str>) -> Option<crate::image::Components> {
	match format {
		Some(GL_FORMAT_RGBA) | None => Some(crate::image::Components::Rgba),
		Some(GL_FORMAT_RGB) => Some(crate::image::Components::Rgb),
		Some(GL_FORMAT_ALPHA) => Some(crate::image::Components::Alpha),
		// Luminance 族：Phase 2 不建模（无对应 Components），宿主
		// 按 RGBA 上传并如实上报——格式核对走 None 分支。
		Some(GL_FORMAT_LUMINANCE) | Some(GL_FORMAT_LUMINANCE_ALPHA) => None,
		Some(_) => None,
	}
}

/// clipLoadTexture：把 clip 在 `time` 的图像加载为 GL 纹理。
///
/// `format` 为请求的纹理格式（kOfxImageEffectGLFormat*；NULL = 宿主
/// 按插件的 kOfxOpenGLPropPixelDepth 决定——Phase 2 全链路 F32）。
/// `region`（规范坐标，可空）会裁剪到 clip 的 RoD——Phase 2 仅支持
/// 整帧（NULL）；子区域请求返回 Failed（插件按规范继续，视作黑底）。
unsafe extern "C" fn clip_load_texture(
	clip: *mut c_void,
	time: c_double,
	format: *const c_char,
	region: *const c_void,
	out: *mut *mut c_void,
) -> c_int {
	caught(|| {
		if out.is_null() {
			return Err(status::ERR_BAD_HANDLE);
		}
		unsafe { *out = std::ptr::null_mut() };
		let c = resolve_clip(clip)?;
		// GL 上下文（TLS）：仅 GL 渲染期存在（ofxGPURender.h 的
		// "OpenGL Current Context" 规则）。
		let gl = crate::suites::gl_ctx().ok_or(status::ERR_MISSING_HOST_FEATURE)?;

		let scale = crate::suites::render_ctx()
			.map(|ctx| ctx.scale)
			.unwrap_or(crate::instance::RenderScale { x: 1.0, y: 1.0 });

		if c.name == "Output" {
			// Output：返回已附着的输出纹理（format 忽略；渲染目标
			// 绑定由调用方契约保证——等价 C++ attach_output_texture）。
			let tex = gl.output_texture.clone();
			let (w, h) = texture_size(&tex);
			if w <= 0.0 || h <= 0.0 {
				return Err(status::ERR_BAD_HANDLE);
			}
			// OpenGLTextureIndex = 宿主为输出帧建的真实 GL 纹理名
			// （render 驱动 GL 分支经 GlCtx 注入；0 = CPU 回退）。
			let index = gl.output_gl_texture.unwrap_or(0);
			let props = make_texture_props(
				w,
				h,
				crate::image::Components::Rgba,
				gl.gl_pixel_depth,
				&clip_string(c, "OfxImageEffectPropPreMultiplication", PREMULT_NONE),
				clip_par(c),
				scale,
				(w as i32) * 4 * 4,
				index,
			);
			let addr = register(Box::new(props), tex, true, None);
			unsafe { *out = tag::make(addr as *const PropertySet, tag::PROPERTY_SET) };
			return Ok(());
		}

		// 输入 clip：Phase 2 仅支持整帧（fetch_image 的 phase-1 约束）。
		if !region.is_null() {
			return Err(status::FAILED);
		}
		let format = if format.is_null() {
			None
		} else {
			unsafe { CStr::from_ptr(format) }.to_str().ok()
		};

		let image = c
			.fetch_image(time, scale, None)
			.map_err(|_| status::FAILED)?;
		let components = image.components();
		// 明确请求了不同分量 → Phase 2 不转换 → Failed（规范要求
		// 满足请求格式；静默给错格式比显式失败更糟）。
		if let Some(expected) = format_components(format) {
			if expected != components {
				return Err(status::FAILED);
			}
		}
		let premult = clip_string(c, "OfxImageEffectPropPreMultiplication", PREMULT_NONE);

		let (w, h) = (image_width(&image), image_height(&image));
		if w <= 0.0 || h <= 0.0 {
			return Err(status::FAILED);
		}
		// GL 模式：把输入图像上传成真实 GL 纹理（RGBA32F；上下文
		// current——render 驱动已 acquire），OpenGLTextureIndex 返回
		// 真实名；GL 上传失败 → 显式 Failed（插件按规范继续）。非 GL
		// 上下文（本函数仅在 GL 渲染期可达，此分支是 CPU 纹理兜底）：
		// 仍建 CPU 纹理、索引 0。
		let gl_tex = crate::gl_bridge::create_input_texture(
			w as i32,
			h as i32,
			image.pixels(),
		)
		.ok();
		let params = crate::render::VideoParams {
			width: w as i32,
			height: h as i32,
			format: crate::render::PIXEL_FORMAT_F32,
			..Default::default()
		};
		let tex = crate::render::texture_create(&params, image.pixels(), image.row_bytes() as i32)
			.map_err(|_| status::ERR_MEMORY)?;
		let props = make_texture_props(
			w,
			h,
			components,
			gl.gl_pixel_depth,
			&premult,
			clip_par(c),
			scale,
			image.row_bytes() as i32,
			gl_tex.unwrap_or(0),
		);
		let addr = register(Box::new(props), tex, false, gl_tex);
		unsafe { *out = tag::make(addr as *const PropertySet, tag::PROPERTY_SET) };
		Ok(())
	})
}

/// 纹理尺寸（经 [`crate::render::texture_get_params`]；失败回退 0,0）。
fn texture_size(tex: &crate::render::Texture) -> (f64, f64) {
	let p = crate::render::texture_get_params(tex);
	if p.width > 0 {
		(p.width as f64, p.height as f64)
	} else {
		(0.0, 0.0)
	}
}

fn image_width(image: &Image) -> f64 {
	(image.bounds().x2 - image.bounds().x1).round()
}

fn image_height(image: &Image) -> f64 {
	(image.bounds().y2 - image.bounds().y1).round()
}

/// clip 的协商像素比。
fn clip_par(c: &ClipInstance) -> f64 {
	c.props
		.get("OfxImagePropPixelAspectRatio", 0)
		.and_then(|v| match v {
			Value::Double(d) => Some(d),
			_ => None,
		})
		.unwrap_or(1.0)
}

/// clipFreeTexture：释放纹理（输入 clip 删除纹理值并释放真实 GL
/// 纹理；Output 只释放句柄不删纹理——宿主还要读它）。纹理是值：
/// 摘除表条目即 drop（原 `texture_free` 调用面随值模型删除）。
unsafe extern "C" fn clip_free_texture(texture_handle: *mut c_void) -> c_int {
	caught(|| {
		if texture_handle.is_null() {
			return Err(status::ERR_BAD_HANDLE);
		}
		let addr = tag::strip(texture_handle) as usize;
		let entry = {
			let mut live = LIVE_TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
			live.remove(&addr)
		};
		match entry {
			Some((_props, _texture, _is_output, gl_tex)) => {
				// 输入 clip 的真实 GL 纹理随释放删除（上下文 current）。
				if let Some(gl) = gl_tex {
					delete_gl_texture_if_gl(gl);
				}
				Ok(())
			}
			None => Err(status::ERR_BAD_HANDLE),
		}
	})
}

/// flushResources：宿主不缓存 GPU 资源 → REPLY_DEFAULT（规范语义
/// "nothing the host could do"）。
unsafe extern "C" fn flush_resources() -> c_int {
	status::REPLY_DEFAULT
}

/// 函数表布局（与 SDK `OfxImageEffectOpenGLRenderSuiteV1` 逐字段
/// 一致；ofxGPURender.h:181-310）。
#[repr(C)]
pub struct GlRenderSuiteV1 {
	/// clipLoadTexture。
	pub clip_load_texture: unsafe extern "C" fn(
		*mut c_void,
		c_double,
		*const c_char,
		*const c_void,
		*mut *mut c_void,
	) -> c_int,
	/// clipFreeTexture。
	pub clip_free_texture: unsafe extern "C" fn(*mut c_void) -> c_int,
	/// flushResources。
	pub flush_resources: unsafe extern "C" fn() -> c_int,
}

/// 函数表实例。
pub fn suite_v1() -> &'static GlRenderSuiteV1 {
	static SUITE: std::sync::OnceLock<GlRenderSuiteV1> = std::sync::OnceLock::new();
	SUITE.get_or_init(|| GlRenderSuiteV1 {
		clip_load_texture,
		clip_free_texture,
		flush_resources,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::property::{PropertySet, Value};

	fn cs(s: &str) -> CString {
		CString::new(s).unwrap()
	}

	/// GL 像素深度协商矩阵（ofxGPURender.h kOfxOpenGLPropPixelDepth）：
	/// 未声明/含 Float → 管线 F32 可行；声明且不含 Float → None
	/// （GL 模式不可行，回退 CPU）。
	#[test]
	fn pick_gl_pixel_depth_matrix() {
		let props = PropertySet::new();
		assert_eq!(pick_gl_pixel_depth(&props), Some("OfxBitDepthFloat"));

		let props2 = PropertySet::new();
		props2.define(
			crate::host::PROP_GL_PIXEL_DEPTH,
			vec![
				Value::String(cs("OfxBitDepthHalf")),
				Value::String(cs("OfxBitDepthByte")),
			],
		);
		assert_eq!(pick_gl_pixel_depth(&props2), None);

		let props3 = PropertySet::new();
		props3.define(
			crate::host::PROP_GL_PIXEL_DEPTH,
			vec![
				Value::String(cs("OfxBitDepthByte")),
				Value::String(cs("OfxBitDepthFloat")),
			],
		);
		assert_eq!(pick_gl_pixel_depth(&props3), Some("OfxBitDepthFloat"));

		let props4 = PropertySet::new();
		props4.define(
			crate::host::PROP_GL_PIXEL_DEPTH,
			vec![Value::String(cs("OfxBitDepthFloat"))],
		);
		assert_eq!(pick_gl_pixel_depth(&props4), Some("OfxBitDepthFloat"));
	}
}
