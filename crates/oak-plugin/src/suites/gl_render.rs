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

	/// GL 渲染 suite 的存活表是进程全局状态：本模块所有触碰它的用例
	/// 经此锁串行（否则 purge_leftovers 会踩其他用例的登记条目）。
	fn suite_lock() -> std::sync::MutexGuard<'static, ()> {
		static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
		LOCK.lock().unwrap_or_else(|e| e.into_inner())
	}

	fn clip(name: &str) -> ClipInstance {
		ClipInstance::from_descriptor(&crate::descriptor::ClipDescriptor::new(name))
	}

	fn clip_handle(c: &ClipInstance) -> *mut c_void {
		tag::make(&c.props as *const PropertySet, tag::CLIP)
	}

	fn f32_frame(w: i32, h: i32, fill: f32) -> crate::render::Frame {
		let params = crate::render::VideoParams {
			width: w,
			height: h,
			format: crate::render::PIXEL_FORMAT_F32,
			..Default::default()
		};
		let mut frame = crate::render::Frame::new();
		frame.set_video_params(params);
		assert!(frame.allocate());
		for c in frame.data.as_chunks_mut::<4>().0 {
			c.copy_from_slice(&fill.to_le_bytes());
		}
		frame
	}

	fn f32_texture(w: i32, h: i32, fill: f32) -> crate::render::Texture {
		crate::render::Texture::wrap_frame(f32_frame(w, h, fill))
	}

	struct FakeGpu;

	impl oak_core::backend::GpuContextLike for FakeGpu {
		fn kind(&self) -> oak_core::backend::BackendKind {
			oak_core::backend::BackendKind::Cpu
		}
		fn destroy_texture(&self, _token: u64) {}
		fn upload(
			&self,
			_token: u64,
			_frame: &oak_core::texture::Frame,
		) -> oak_render::error::Result<()> {
			Ok(())
		}
		fn download(&self, _token: u64) -> oak_render::error::Result<oak_core::texture::Frame> {
			Ok(oak_core::texture::Frame::new())
		}
		fn blit(
			&self,
			_src: u64,
			_dst: u64,
			_processor: Option<&oak_core::color::ColorProcessor>,
		) -> oak_render::error::Result<()> {
			Ok(())
		}
	}

	fn gl_ctx(output: crate::render::Texture, index: Option<i32>) -> crate::suites::GlCtx {
		crate::suites::GlCtx {
			renderer: std::sync::Arc::new(FakeGpu),
			output_texture: output,
			gl_pixel_depth: "OfxBitDepthFloat",
			output_gl_texture: index,
		}
	}

	/// 从 clipLoadTexture 写出的句柄读属性集。
	fn handle_props(handle: *mut c_void) -> PropertySet {
		assert_eq!(tag::kind(handle), tag::PROPERTY_SET, "纹理句柄标签 0");
		unsafe { (*(tag::strip(handle))).clone() }
	}

	// `Value` 未实现 PartialEq（Pointer 无法比较）——按种类解包断言。

	fn prop_int(props: &PropertySet, name: &str, index: usize) -> i32 {
		match props.get(name, index) {
			Some(Value::Int(i)) => i,
			other => panic!("{name}[{index}] 应为 Int，got {other:?}"),
		}
	}

	fn prop_double(props: &PropertySet, name: &str, index: usize) -> f64 {
		match props.get(name, index) {
			Some(Value::Double(d)) => d,
			other => panic!("{name}[{index}] 应为 Double，got {other:?}"),
		}
	}

	fn prop_string(props: &PropertySet, name: &str, index: usize) -> String {
		match props.get(name, index) {
			Some(Value::String(s)) => s.to_string_lossy().into_owned(),
			other => panic!("{name}[{index}] 应为 String，got {other:?}"),
		}
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

		// 列表存在但元素非字符串 → 跳过，不含 Float → None。
		let props5 = PropertySet::new();
		props5.define(crate::host::PROP_GL_PIXEL_DEPTH, vec![Value::Int(4)]);
		assert_eq!(pick_gl_pixel_depth(&props5), None);
	}

	/// 纹理请求格式 → 期望分量矩阵（NULL/未知 → 宿主自选）。
	#[test]
	fn format_components_matrix() {
		use crate::image::Components;
		assert_eq!(format_components(Some(GL_FORMAT_RGBA)), Some(Components::Rgba));
		assert_eq!(format_components(None), Some(Components::Rgba));
		assert_eq!(format_components(Some(GL_FORMAT_RGB)), Some(Components::Rgb));
		assert_eq!(format_components(Some(GL_FORMAT_ALPHA)), Some(Components::Alpha));
		assert_eq!(format_components(Some(GL_FORMAT_LUMINANCE)), None);
		assert_eq!(format_components(Some(GL_FORMAT_LUMINANCE_ALPHA)), None);
		assert_eq!(format_components(Some("bogus")), None);
	}

	/// make_texture_props：ofxGPURender.h 规定的 12 个属性齐全且值正确。
	#[test]
	fn make_texture_props_fields() {
		let props = make_texture_props(
			4.0,
			2.0,
			crate::image::Components::Rgb,
			"OfxBitDepthFloat",
			PREMULT_NONE,
			1.5,
			crate::instance::RenderScale { x: 2.0, y: 3.0 },
			48,
			7,
		);
		assert_eq!(prop_int(&props, GL_TEXTURE_INDEX, 0), 7);
		assert_eq!(prop_int(&props, GL_TEXTURE_TARGET, 0), GL_TEXTURE_2D);
		assert_eq!(
			prop_string(&props, crate::image::K_IMAGE_EFFECT_PROP_PIXEL_DEPTH, 0),
			"OfxBitDepthFloat"
		);
		assert_eq!(
			prop_string(&props, crate::image::K_IMAGE_EFFECT_PROP_COMPONENTS, 0),
			"OfxImageComponentRGB"
		);
		assert_eq!(
			prop_string(&props, "OfxImageEffectPropPreMultiplication", 0),
			PREMULT_NONE
		);
		assert_eq!(prop_double(&props, crate::host::PROP_RENDER_SCALE, 0), 2.0);
		assert_eq!(prop_double(&props, crate::host::PROP_RENDER_SCALE, 1), 3.0);
		assert_eq!(
			prop_double(&props, "OfxImagePropPixelAspectRatio", 0),
			1.5
		);
		for (i, want) in [0, 0, 4, 2].iter().enumerate() {
			assert_eq!(
				prop_int(&props, crate::image::K_IMAGE_PROP_BOUNDS, i),
				*want,
				"Bounds[{i}]"
			);
			assert_eq!(
				prop_int(&props, crate::image::K_IMAGE_PROP_ROD, i),
				*want,
				"RoD[{i}]"
			);
		}
		assert_eq!(prop_int(&props, crate::image::K_IMAGE_PROP_ROW_BYTES, 0), 48);
		assert_eq!(
			prop_string(&props, "OfxImagePropField", 0),
			"OfxFieldNone"
		);
		assert!(matches!(
			props.get(crate::image::K_IMAGE_PROP_UNIQUE_ID, 0),
			Some(Value::String(_))
		));
	}

	/// rect_props：Double 矩形四舍五入为 Int×4（负半轴远离零）。
	#[test]
	fn rect_props_rounds_to_int() {
		let props = PropertySet::new();
		rect_props(
			&props,
			"r",
			crate::instance::OfxRectD {
				x1: 1.4,
				y1: -0.6,
				x2: 3.5,
				y2: 2.49,
			},
		);
		for (i, want) in [1, -1, 4, 2].iter().enumerate() {
			assert_eq!(prop_int(&props, "r", i), *want, "r[{i}]");
		}
	}

	/// clip_string / clip_par：字符串与非字符串回退、PAR 缺失默认 1.0。
	#[test]
	fn clip_string_and_par_fallbacks() {
		let c = clip("Source");
		assert_eq!(clip_string(&c, "missing", "def"), "def");
		assert_eq!(clip_par(&c), 1.0);

		c.props.set_one("s", Value::String(cs("xyz")));
		assert_eq!(clip_string(&c, "s", "def"), "xyz");
		c.props.set_one("s2", Value::Int(3));
		assert_eq!(clip_string(&c, "s2", "def"), "def");

		c.props
			.set_one("OfxImagePropPixelAspectRatio", Value::Double(2.0));
		assert_eq!(clip_par(&c), 2.0);
		c.props
			.set_one("OfxImagePropPixelAspectRatio", Value::Int(2));
		assert_eq!(clip_par(&c), 1.0, "非 Double 回退 1.0");
	}

	/// texture_size / image_width / image_height。
	#[test]
	fn texture_size_and_image_dims() {
		assert_eq!(texture_size(&crate::render::Texture::dummy()), (0.0, 0.0));
		assert_eq!(texture_size(&f32_texture(3, 2, 0.0)), (3.0, 2.0));

		let img = Image::allocate(
			crate::image::BitDepth::Float,
			crate::image::Components::Rgba,
			crate::instance::OfxRectD {
				x1: 0.0,
				y1: 0.0,
				x2: 5.0,
				y2: 4.0,
			},
		);
		assert_eq!(image_width(&img), 5.0);
		assert_eq!(image_height(&img), 4.0);
	}

	/// resolve_clip：空句柄/非 CLIP 标签 → BadHandle；CLIP 标签有效。
	#[test]
	fn resolve_clip_handle_rules() {
		assert!(matches!(
			resolve_clip(std::ptr::null_mut()),
			Err(status::ERR_BAD_HANDLE)
		));
		let props = PropertySet::new();
		let wrong = tag::make(&props as *const PropertySet, tag::PROPERTY_SET);
		assert!(matches!(resolve_clip(wrong), Err(status::ERR_BAD_HANDLE)));
		let c = clip("Source");
		assert!(resolve_clip(clip_handle(&c)).is_ok());
	}

	/// clipLoadTexture：入口防御（空 out / 空 clip / 非 clip 句柄 /
	/// 无 GL 上下文）。
	#[test]
	fn clip_load_texture_entry_guards() {
		let _lock = suite_lock();
		crate::suites::set_gl_ctx(None);
		let s = suite_v1();
		let c = clip("Source");
		let name = cs(GL_FORMAT_RGBA);
		let mut sentinel = 0u8;
		let mut out: *mut c_void = &mut sentinel as *mut u8 as *mut c_void;
		unsafe {
			// 空 out → BadHandle（且不触碰返回值）。
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&c),
					0.0,
					name.as_ptr(),
					std::ptr::null(),
					std::ptr::null_mut(),
				),
				status::ERR_BAD_HANDLE
			);
			// 空 clip 句柄 → BadHandle。
			assert_eq!(
				(s.clip_load_texture)(
					std::ptr::null_mut(),
					0.0,
					name.as_ptr(),
					std::ptr::null(),
					&mut out,
				),
				status::ERR_BAD_HANDLE
			);
			assert!(out.is_null(), "入口先把 out 置空");
			// 非 clip 标签 → BadHandle。
			let wrong = tag::make(&c.props as *const PropertySet, tag::PROPERTY_SET);
			assert_eq!(
				(s.clip_load_texture)(wrong, 0.0, name.as_ptr(), std::ptr::null(), &mut out),
				status::ERR_BAD_HANDLE
			);
			// 无 GL 上下文 → MissingHostFeature（GL suite 只在 GL 渲染期可达）。
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&c),
					0.0,
					name.as_ptr(),
					std::ptr::null(),
					&mut out,
				),
				status::ERR_MISSING_HOST_FEATURE
			);
		}
	}

	/// clipLoadTexture(Output)：忽略 format，返回已附着的输出纹理句柄
	/// （真实 GL 名经 OpenGLTextureIndex 上报）；free 只摘句柄。
	#[test]
	fn clip_load_texture_output_clip() {
		let _lock = suite_lock();
		let s = suite_v1();
		let out_clip = clip("Output");
		let tex = f32_texture(4, 2, 0.25);
		crate::suites::set_gl_ctx(Some(gl_ctx(tex, Some(9))));
		let rgb = cs(GL_FORMAT_RGB); // Output 忽略请求格式。
		let mut handle: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&out_clip),
					0.0,
					rgb.as_ptr(),
					std::ptr::null(),
					&mut handle,
				),
				status::OK
			);
		}
		let props = handle_props(handle);
		assert_eq!(prop_int(&props, GL_TEXTURE_INDEX, 0), 9);
		assert_eq!(prop_int(&props, GL_TEXTURE_TARGET, 0), GL_TEXTURE_2D);
		assert_eq!(
			prop_string(&props, crate::image::K_IMAGE_EFFECT_PROP_COMPONENTS, 0),
			"OfxImageComponentRGBA"
		);
		assert_eq!(prop_int(&props, crate::image::K_IMAGE_PROP_BOUNDS, 2), 4);
		// 行字节 = w × RGBA × F32；OpenGLTextureIndex=0 变体（CPU 回退）。
		assert_eq!(
			prop_int(&props, crate::image::K_IMAGE_PROP_ROW_BYTES, 0),
			4 * 4 * 4
		);
		assert!(LIVE_TEXTURES.lock().unwrap().contains_key(&(tag::strip(handle) as usize)));

		// free：输出 clip 只释放句柄、不删纹理（宿主还要回读）。
		unsafe {
			assert_eq!((s.clip_free_texture)(handle), status::OK);
			// 二次释放 / 空句柄 → BadHandle。
			assert_eq!((s.clip_free_texture)(handle), status::ERR_BAD_HANDLE);
			assert_eq!(
				(s.clip_free_texture)(std::ptr::null_mut()),
				status::ERR_BAD_HANDLE
			);
		}
		assert!(!LIVE_TEXTURES.lock().unwrap().contains_key(&(tag::strip(handle) as usize)));

		// output_gl_texture = None → 索引 0（CPU 回退语义）。
		crate::suites::set_gl_ctx(Some(gl_ctx(f32_texture(2, 2, 0.0), None)));
		let mut h0: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&out_clip),
					0.0,
					std::ptr::null(),
					std::ptr::null(),
					&mut h0,
				),
				status::OK
			);
		}
		assert_eq!(prop_int(&handle_props(h0), GL_TEXTURE_INDEX, 0), 0);
		unsafe {
			assert_eq!((s.clip_free_texture)(h0), status::OK);
		}
		crate::suites::set_gl_ctx(None);
	}

	/// clipLoadTexture(Output)：附着纹理为 dummy/0 尺寸 → BadHandle。
	#[test]
	fn clip_load_texture_output_rejects_empty_texture() {
		let _lock = suite_lock();
		let s = suite_v1();
		let out_clip = clip("Output");
		crate::suites::set_gl_ctx(Some(gl_ctx(crate::render::Texture::dummy(), None)));
		let mut handle: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&out_clip),
					0.0,
					std::ptr::null(),
					std::ptr::null(),
					&mut handle,
				),
				status::ERR_BAD_HANDLE
			);
		}
		crate::suites::set_gl_ctx(None);
	}

	/// clipLoadTexture(输入 clip)：整帧限制、格式匹配、协商属性与
	/// 释放；GL 不可用时 OpenGLTextureIndex=0（CPU 回退）。
	#[test]
	fn clip_load_texture_input_clip() {
		let _lock = suite_lock();
		let s = suite_v1();
		let c = clip("Source");
		c.set_input_texture(Some(f32_texture(2, 2, 0.5)), 0.0);
		crate::suites::set_gl_ctx(Some(gl_ctx(crate::render::Texture::dummy(), None)));

		// 子区域请求（region 非空）→ Failed（Phase 2 仅整帧）。
		let region = crate::instance::OfxRectD::default();
		unsafe {
			let mut h: *mut c_void = std::ptr::null_mut();
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&c),
					0.0,
					std::ptr::null(),
					&region as *const crate::instance::OfxRectD as *const c_void,
					&mut h,
				),
				status::FAILED
			);
		}

		// 未挂输入纹理 → Failed（fetch_image NotFound 映射）。
		let empty = clip("Source");
		unsafe {
			let mut h: *mut c_void = std::ptr::null_mut();
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&empty),
					0.0,
					std::ptr::null(),
					std::ptr::null(),
					&mut h,
				),
				status::FAILED
			);
		}

		// 分量不符：请求 RGB 但图像 RGBA（协商默认）→ Failed。
		let rgb = cs(GL_FORMAT_RGB);
		unsafe {
			let mut h: *mut c_void = std::ptr::null_mut();
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&c),
					0.0,
					rgb.as_ptr(),
					std::ptr::null(),
					&mut h,
				),
				status::FAILED
			);
		}

		// 成功（format=NULL）：纹理句柄属性齐全。
		let mut handle: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&c),
					0.0,
					std::ptr::null(),
					std::ptr::null(),
					&mut handle,
				),
				status::OK
			);
		}
		let props = handle_props(handle);
		assert_eq!(
			prop_string(&props, crate::image::K_IMAGE_EFFECT_PROP_COMPONENTS, 0),
			"OfxImageComponentRGBA"
		);
		assert_eq!(
			prop_string(&props, crate::image::K_IMAGE_EFFECT_PROP_PIXEL_DEPTH, 0),
			"OfxBitDepthFloat"
		);
		assert_eq!(
			prop_int(&props, crate::image::K_IMAGE_PROP_ROW_BYTES, 0),
			2 * 4 * 4
		);
		assert_eq!(prop_int(&props, GL_TEXTURE_TARGET, 0), GL_TEXTURE_2D);
		// 非 GL（Linux/Windows stub）→ 索引 0；macOS 有 GL 时 > 0。
		let index = match props.get(GL_TEXTURE_INDEX, 0) {
			Some(Value::Int(i)) => i,
			other => panic!("OpenGLTextureIndex 应为 Int，got {other:?}"),
		};
		if crate::gl_bridge::gl_available() {
			assert!(index > 0, "GL 可用时输入纹理应有真实 GL 名");
		} else {
			assert_eq!(index, 0, "无 GL 时索引为 0");
		}

		// 请求 RGBA（显式）与请求 Luminance 族（None = 宿主自选）都成功。
		let rgba = cs(GL_FORMAT_RGBA);
		let lum = cs(GL_FORMAT_LUMINANCE);
		let lum_a = cs(GL_FORMAT_LUMINANCE_ALPHA);
		let mut h2: *mut c_void = std::ptr::null_mut();
		let mut h3: *mut c_void = std::ptr::null_mut();
		let mut h4: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.clip_load_texture)(clip_handle(&c), 0.0, rgba.as_ptr(), std::ptr::null(), &mut h2),
				status::OK
			);
			assert_eq!(
				(s.clip_load_texture)(clip_handle(&c), 0.0, lum.as_ptr(), std::ptr::null(), &mut h3),
				status::OK
			);
			assert_eq!(
				(s.clip_load_texture)(clip_handle(&c), 0.0, lum_a.as_ptr(), std::ptr::null(), &mut h4),
				status::OK
			);
			// 非 UTF-8 format 串按 None 处理（宿主自选）→ OK。
			let bad = CString::new(vec![0xFFu8]).unwrap();
			let mut h5: *mut c_void = std::ptr::null_mut();
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&c),
					0.0,
					bad.as_ptr(),
					std::ptr::null(),
					&mut h5,
				),
				status::OK
			);
			// free 输入纹理：摘表（GL 模式同时删真实 GL 纹理）。
			assert_eq!((s.clip_free_texture)(h2), status::OK);
			assert_eq!((s.clip_free_texture)(h3), status::OK);
			assert_eq!((s.clip_free_texture)(h4), status::OK);
			assert_eq!((s.clip_free_texture)(h5), status::OK);
			assert_eq!((s.clip_free_texture)(handle), status::OK);
		}
		crate::suites::set_gl_ctx(None);
	}

	/// clipLoadTexture：渲染上下文的 scale 进入纹理属性；0 尺寸图像
	/// → Failed。
	#[test]
	fn clip_load_texture_uses_render_scale_and_rejects_zero_image() {
		let _lock = suite_lock();
		let s = suite_v1();

		// 有 render ctx：scale 写入纹理句柄的 RenderScale 属性。
		let c = clip("Source");
		c.set_input_texture(Some(f32_texture(2, 2, 0.0)), 0.0);
		crate::suites::set_render_ctx(Some(crate::suites::RenderCtx {
			time: 1.0,
			scale: crate::instance::RenderScale { x: 2.0, y: 2.0 },
			range: crate::instance::OfxRangeD { min: 0.0, max: 1.0 },
		}));
		crate::suites::set_gl_ctx(Some(gl_ctx(crate::render::Texture::dummy(), None)));
		let mut handle: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&c),
					0.0,
					std::ptr::null(),
					std::ptr::null(),
					&mut handle,
				),
				status::OK
			);
		}
		let props = handle_props(handle);
		assert_eq!(prop_double(&props, crate::host::PROP_RENDER_SCALE, 0), 2.0);
		unsafe {
			assert_eq!((s.clip_free_texture)(handle), status::OK);
		}
		crate::suites::set_render_ctx(None);

		// 0×0 帧（非 dummy：data 非空）→ image_width == 0 → Failed。
		let zero = clip("Source");
		let params = crate::render::VideoParams {
			format: crate::render::PIXEL_FORMAT_F32,
			..Default::default()
		};
		let mut frame = crate::render::Frame::new();
		frame.set_video_params(params);
		frame.data = vec![0u8; 16];
		zero.set_input_texture(Some(crate::render::Texture::wrap_frame(frame)), 0.0);
		let mut h: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&zero),
					0.0,
					std::ptr::null(),
					std::ptr::null(),
					&mut h,
				),
				status::FAILED
			);
		}
		crate::suites::set_gl_ctx(None);
	}

	/// purge_leftovers：释放残留输入句柄、保留输出句柄（宿主还要回读）。
	#[test]
	fn purge_leftovers_keeps_outputs() {
		let _lock = suite_lock();
		let s = suite_v1();
		let input = clip("Source");
		input.set_input_texture(Some(f32_texture(2, 2, 0.0)), 0.0);
		let out_clip = clip("Output");
		crate::suites::set_gl_ctx(Some(gl_ctx(f32_texture(2, 2, 0.0), Some(1))));
		let mut input_handle: *mut c_void = std::ptr::null_mut();
		let mut out_handle: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&input),
					0.0,
					std::ptr::null(),
					std::ptr::null(),
					&mut input_handle,
				),
				status::OK
			);
			assert_eq!(
				(s.clip_load_texture)(
					clip_handle(&out_clip),
					0.0,
					std::ptr::null(),
					std::ptr::null(),
					&mut out_handle,
				),
				status::OK
			);
		}
		assert!(LIVE_TEXTURES.lock().unwrap().contains_key(&(tag::strip(input_handle) as usize)));
		assert!(LIVE_TEXTURES.lock().unwrap().contains_key(&(tag::strip(out_handle) as usize)));

		purge_leftovers();
		let live = LIVE_TEXTURES.lock().unwrap();
		assert!(
			!live.contains_key(&(tag::strip(input_handle) as usize)),
			"输入句柄应被兜底释放"
		);
		assert!(
			live.contains_key(&(tag::strip(out_handle) as usize)),
			"输出句柄应保留"
		);
		drop(live);
		unsafe {
			assert_eq!((s.clip_free_texture)(out_handle), status::OK);
		}
		crate::suites::set_gl_ctx(None);
	}

	/// delete_gl_texture_if_gl：无 GL 上下文 no-op；有上下文时经桥删除
	/// （GL 不可用环境下桥为 stub，仅验证不 panic）。
	#[test]
	fn delete_gl_texture_if_gl_branches() {
		let _lock = suite_lock();
		crate::suites::set_gl_ctx(None);
		delete_gl_texture_if_gl(3);
		crate::suites::set_gl_ctx(Some(gl_ctx(crate::render::Texture::dummy(), None)));
		delete_gl_texture_if_gl(4);
		crate::suites::set_gl_ctx(None);
	}

	/// flushResources：宿主不缓存 GPU 资源 → ReplyDefault。
	#[test]
	fn flush_resources_replies_default() {
		unsafe {
			assert_eq!((suite_v1().flush_resources)(), status::REPLY_DEFAULT);
		}
	}
}
