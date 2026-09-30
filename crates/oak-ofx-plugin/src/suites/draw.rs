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

//! OfxDrawSuiteV1（OFX 1.5，vendored ofxDrawSuite.h）：宿主绘制套件。
//!
//! 插件在 `kOfxInteractActionDraw` 内经 `kOfxInteractPropDrawContext`
//! 取到绘制上下文（本模块的 [`DrawContext`]），再调 suite 函数画图：
//! getColour / setColour / setLineWidth / setLineStipple / draw /
//! drawText。
//!
//! ## 上下文模型
//!
//! - [`DrawContext`] 在每次 draw action 前由宿主创建、经存活表
//!   [`LIVE_CONTEXTS`] 注册（句柄 = 堆地址；对应 [`crate::suites::tag`]
//!   的句柄纪律），draw 返回后摘除。插件只在 draw action 内持有上下文
//!   ——ofxDrawSuite.h 各函数文档注明 "failure, e.g. if function is
//!   called outside kOfxInteractActionDraw" → 存活表查不到即
//!   kOfxStatFailed。
//! - 状态（colour / lineWidth / stipple / viewport / pixelScale）保存在
//!   [`DrawContext`]，draw action 之间不共享。
//! - GL 命令要求上下文 current：宿主在调 `kOfxInteractActionDraw` 前经
//!   [`crate::gl_bridge::acquire`] 保持 current（draw action 全期）。
//!
//! ## 绘制实现（真实 GL）
//!
//! gl_bridge 的 CGL 上下文是 **3.2 core profile**（无固定管线，
//! glBegin 不可用）——本套件的 draw 用最小着色器 + VAO/VBO 真实渲染：
//! 正交投影把 canonical 坐标映射到 NDC（canonical 宽 = viewport /
//! pixelScale），非不透明色按 "over" 合成（ofxDrawSuite.h setColour
//! 文档要求）。非 macOS 无 GL 桥 → kOfxStatFailed。
//!
//! ## drawText
//!
//! 本宿主无字体光栅化器（GL 内画字形需要字库/曲线上采样，超出 Phase
//! 1 范围），如实返回 kOfxStatErrUnsupported，不假装画了字。

use std::collections::HashMap;
use std::ffi::{c_char, c_float, c_int, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{LazyLock, Mutex};

use crate::suites::status;

/// `OfxStandardColour`（ofxDrawSuite.h:37-46）枚举值。
pub mod std_colour {
	/// kOfxStandardColourOverlayBackground。
	pub const BACKGROUND: i32 = 0;
	/// kOfxStandardColourOverlayActive。
	pub const ACTIVE: i32 = 1;
	/// kOfxStandardColourOverlaySelected。
	pub const SELECTED: i32 = 2;
	/// kOfxStandardColourOverlayDeselected。
	pub const DESELECTED: i32 = 3;
	/// kOfxStandardColourOverlayMarqueeFG。
	pub const MARQUEE_FG: i32 = 4;
	/// kOfxStandardColourOverlayMarqueeBG。
	pub const MARQUEE_BG: i32 = 5;
	/// kOfxStandardColourOverlayText。
	pub const TEXT: i32 = 6;
}

/// `OfxDrawLineStipplePattern`（ofxDrawSuite.h:49-56）枚举值。
pub mod stipple {
	/// kOfxDrawLineStipplePatternSolid。
	pub const SOLID: i32 = 0;
	/// kOfxDrawLineStipplePatternDot。
	pub const DOT: i32 = 1;
	/// kOfxDrawLineStipplePatternDash。
	pub const DASH: i32 = 2;
	/// kOfxDrawLineStipplePatternAltDash。
	pub const ALT_DASH: i32 = 3;
	/// kOfxDrawLineStipplePatternDotDash。
	pub const DOT_DASH: i32 = 4;
}

/// `OfxDrawPrimitive`（ofxDrawSuite.h:60-68）枚举值。
pub mod primitive {
	/// kOfxDrawPrimitiveLines（n 点画 n/2 条独立线段）。
	pub const LINES: i32 = 0;
	/// kOfxDrawPrimitiveLineStrip。
	pub const LINE_STRIP: i32 = 1;
	/// kOfxDrawPrimitiveLineLoop。
	pub const LINE_LOOP: i32 = 2;
	/// kOfxDrawPrimitiveRectangle（轴对齐实心矩形，2 对角点）。
	pub const RECTANGLE: i32 = 3;
	/// kOfxDrawPrimitivePolygon（实心 n 边形）。
	pub const POLYGON: i32 = 4;
	/// kOfxDrawPrimitiveEllipse（2 对角点包围盒内的轴对齐椭圆**线框**）。
	pub const ELLIPSE: i32 = 5;
}

/// `OfxRGBAColourF`（ofxPixels.h）：Draw suite 的颜色值类型。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OfxRGBAColourF {
	/// 红。
	pub r: f32,
	/// 绿。
	pub g: f32,
	/// 蓝。
	pub b: f32,
	/// alpha。
	pub a: f32,
}

/// `OfxPointD`（ofxCore.h:819）：canonical 坐标点。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OfxPointD {
	/// x。
	pub x: f64,
	/// y。
	pub y: f64,
}

/// Draw suite 上下文：一次 draw action 期间插件的绘制状态。
///
/// 宿主在 [`crate::suites::interact::Interact::draw`] 内创建并注册，
/// draw 返回后摘除。`viewport`/`pixel_scale` 在创建时快照（canonical
/// → NDC 正交投影用）；颜色/线宽/虚线样式由插件经 suite 函数写入。
pub struct DrawContext {
	/// 当前绘制颜色（RGBA；setColour 写入，draw 时作为着色器 uniform）。
	pub colour: [f32; 4],
	/// 当前线宽（setLineWidth；GL 线绘制 glLineWidth）。
	pub line_width: f32,
	/// 当前虚线样式（setLineStipple；`stipple::*` 枚举）。
	pub stipple: i32,
	/// 视口尺寸（像素；创建时快照）。
	pub viewport: (f64, f64),
	/// canonical→屏幕像素比例（创建时快照）。
	pub pixel_scale: (f64, f64),
}

impl DrawContext {
	/// 新上下文（默认颜色不透明黑、线宽 1、实线）。
	pub fn new(viewport: (f64, f64), pixel_scale: (f64, f64)) -> Self {
		Self {
			colour: [0.0, 0.0, 0.0, 1.0],
			line_width: 1.0,
			stipple: stipple::SOLID,
			viewport,
			pixel_scale,
		}
	}
}

/// 存活绘制上下文表：draw action 期间注册（地址 → 盒值）。
/// 插件只在 draw 内持有句柄，draw 返回即摘除（同
/// [`crate::suites::gl_render`] 的 LIVE_TEXTURES 模式）。
static LIVE_CONTEXTS: LazyLock<Mutex<HashMap<usize, Box<DrawContext>>>> =
	LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock() -> std::sync::MutexGuard<'static, HashMap<usize, Box<DrawContext>>> {
	LIVE_CONTEXTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// 注册上下文并返回句柄（宿主 draw action 调用；句柄写入 inArgs 的
/// kOfxInteractPropDrawContext）。
pub(crate) fn make_context(viewport: (f64, f64), pixel_scale: (f64, f64)) -> *mut c_void {
	let ctx = Box::new(DrawContext::new(viewport, pixel_scale));
	let addr = &*ctx as *const DrawContext as usize;
	lock().insert(addr, ctx);
	addr as *mut c_void
}

/// 摘除上下文（draw action 返回后调用；后续 suite 调用 → Failed）。
pub(crate) fn drop_context(handle: *mut c_void) {
	lock().remove(&(handle as usize));
}

/// 公共入口模板：panic 兜底。
fn caught(f: impl FnOnce() -> Result<(), c_int>) -> c_int {
	catch_unwind(AssertUnwindSafe(f)).map_or_else(
		|_| status::FAILED,
		|r| r.map_or_else(|c| c, |()| status::OK),
	)
}

/// 上下文句柄解析（存活表反查；draw action 外 → Failed，ofxDrawSuite.h
/// 的 "outside kOfxInteractActionDraw" 语义）。
fn resolve(handle: *mut c_void) -> Result<&'static DrawContext, c_int> {
	if handle.is_null() {
		return Err(status::ERR_BAD_HANDLE);
	}
	let addr = handle as usize;
	lock()
		.get(&addr)
		.map(|b| unsafe { &*(&**b as *const DrawContext) })
		.ok_or(status::FAILED)
}

/// 可变访问（resolve 的写路径；draw 期单线程，存活保证同 resolve）。
fn resolve_mut(handle: *mut c_void) -> Result<(), c_int> {
	resolve(handle).map(|_| ())
}

// ---- 宿主标准颜色板（getColour）----

/// 宿主标准颜色板：按 `OfxStandardColour` 枚举序（ofxDrawSuite.h:37-46）。
const PALETTE: [[f32; 4]; 7] = [
	// BACKGROUND：宿主视口背景（本宿主 Phase 1 无合成背景，黑）。
	[0.0, 0.0, 0.0, 1.0],
	// ACTIVE：进行中/激活的 overlay 元素。
	[1.0, 0.84, 0.0, 1.0],
	// SELECTED：选中元素。
	[0.0, 1.0, 0.0, 1.0],
	// DESELECTED：未选中元素。
	[1.0, 1.0, 1.0, 0.5],
	// MARQUEE_FG：框选前景。
	[1.0, 1.0, 1.0, 1.0],
	// MARQUEE_BG：框选背景。
	[0.0, 0.0, 0.0, 0.5],
	// TEXT：overlay 文本。
	[1.0, 1.0, 1.0, 1.0],
];

unsafe extern "C" fn get_colour(
	ctx: *mut c_void,
	std_colour: c_int,
	out: *mut OfxRGBAColourF,
) -> c_int {
	caught(|| {
		if out.is_null() {
			return Err(status::ERR_VALUE);
		}
		let _ = resolve(ctx)?;
		let c = PALETTE
			.get(std_colour as usize)
			.ok_or(status::ERR_VALUE)?;
		unsafe {
			*out = OfxRGBAColourF {
				r: c[0],
				g: c[1],
				b: c[2],
				a: c[3],
			};
		}
		Ok(())
	})
}

unsafe extern "C" fn set_colour(ctx: *mut c_void, colour: *const OfxRGBAColourF) -> c_int {
	caught(|| {
		if colour.is_null() {
			return Err(status::ERR_VALUE);
		}
		resolve_mut(ctx)?;
		let v = unsafe { &*colour };
		let addr = ctx as usize;
		let mut live = lock();
		let slot = live.get_mut(&addr).expect("resolve 已保证存活");
		slot.colour = [v.r, v.g, v.b, v.a];
		Ok(())
	})
}

unsafe extern "C" fn set_line_width(ctx: *mut c_void, width: c_float) -> c_int {
	caught(|| {
		resolve_mut(ctx)?;
		let addr = ctx as usize;
		let mut live = lock();
		let slot = live.get_mut(&addr).expect("resolve 已保证存活");
		slot.line_width = width;
		Ok(())
	})
}

unsafe extern "C" fn set_line_stipple(ctx: *mut c_void, pattern: c_int) -> c_int {
	caught(|| {
		if !(0..=stipple::DOT_DASH).contains(&pattern) {
			return Err(status::ERR_VALUE);
		}
		resolve_mut(ctx)?;
		let addr = ctx as usize;
		let mut live = lock();
		let slot = live.get_mut(&addr).expect("resolve 已保证存活");
		slot.stipple = pattern;
		Ok(())
	})
}

unsafe extern "C" fn draw(
	ctx: *mut c_void,
	prim: c_int,
	points: *const OfxPointD,
	count: c_int,
) -> c_int {
	caught(|| {
		if !(0..=primitive::ELLIPSE).contains(&prim) {
			return Err(status::ERR_VALUE);
		}
		if count < 0 || (count > 0 && points.is_null()) {
			return Err(status::ERR_VALUE);
		}
		// 每原语的合法点数（ofxDrawSuite.h draw 文档）。
		let valid = match prim {
			primitive::LINES => count >= 2 && count % 2 == 0,
			primitive::LINE_STRIP | primitive::LINE_LOOP => count >= 2,
			primitive::RECTANGLE | primitive::ELLIPSE => count == 2,
			primitive::POLYGON => count >= 3,
			_ => false,
		};
		if !valid {
			return Err(status::ERR_VALUE);
		}
		let c = resolve(ctx)?;
		let pts: Vec<OfxPointD> = (0..count as usize)
			.map(|i| unsafe { *points.add(i) })
			.collect();
		// 真实 GL 渲染（macOS 3.2 core；非 macOS stub → Failed）。
		gl_imp::render(c, prim, &pts)
	})
}

unsafe extern "C" fn draw_text(
	ctx: *mut c_void,
	text: *const c_char,
	pos: *const OfxPointD,
	_alignment: c_int,
) -> c_int {
	caught(|| {
		if text.is_null() || pos.is_null() {
			return Err(status::ERR_VALUE);
		}
		let _ = resolve(ctx)?;
		// 确认文本有效（UTF-8）；随后如实拒绝（无字体光栅化器）。
		let _ = unsafe { CStr::from_ptr(text) }
			.to_str()
			.map_err(|_| status::ERR_VALUE)?;
		Err(status::ERR_UNSUPPORTED)
	})
}

/// 函数表布局（与 SDK `OfxDrawSuiteV1` 逐字段一致；ofxDrawSuite.h:85-177）。
#[repr(C)]
pub struct DrawSuiteV1 {
	/// getColour：宿主标准颜色板取色。
	pub get_colour: unsafe extern "C" fn(*mut c_void, c_int, *mut OfxRGBAColourF) -> c_int,
	/// setColour：设置后续绘制颜色。
	pub set_colour: unsafe extern "C" fn(*mut c_void, *const OfxRGBAColourF) -> c_int,
	/// setLineWidth：设置后续线宽。
	pub set_line_width: unsafe extern "C" fn(*mut c_void, c_float) -> c_int,
	/// setLineStipple：设置后续虚线样式。
	pub set_line_stipple: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
	/// draw：绘制原语（点数组，canonical 坐标）。
	pub draw: unsafe extern "C" fn(*mut c_void, c_int, *const OfxPointD, c_int) -> c_int,
	/// drawText：绘制文本（Phase 1 返回 kOfxStatErrUnsupported）。
	pub draw_text: unsafe extern "C" fn(*mut c_void, *const c_char, *const OfxPointD, c_int) -> c_int,
}

/// 函数表实例。
pub fn suite_v1() -> &'static DrawSuiteV1 {
	static SUITE: std::sync::OnceLock<DrawSuiteV1> = std::sync::OnceLock::new();
	SUITE.get_or_init(|| DrawSuiteV1 {
		get_colour,
		set_colour,
		set_line_width,
		set_line_stipple,
		draw,
		draw_text,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// 标准颜色板：getColour 按枚举取宿主色板（越界 → ErrValue）。
	#[test]
	fn get_colour_palette() {
		let ctx = make_context((640.0, 480.0), (1.0, 1.0));
		let mut out = OfxRGBAColourF { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
		unsafe {
			assert_eq!(get_colour(ctx, std_colour::ACTIVE, &mut out), status::OK);
		}
		assert_eq!((out.r, out.g, out.b, out.a), (1.0, 0.84, 0.0, 1.0));
		// 越界枚举 → ErrValue。
		unsafe {
			assert_eq!(get_colour(ctx, 99, &mut out), status::ERR_VALUE);
			assert_eq!(get_colour(ctx, -1, &mut out), status::ERR_VALUE);
			// 空输出指针 → ErrValue。
			assert_eq!(
				get_colour(ctx, std_colour::BACKGROUND, std::ptr::null_mut()),
				status::ERR_VALUE
			);
		}
		drop_context(ctx);
	}

	/// setColour 真实写入状态：再 setLineWidth/setLineStipple 后从存活
	/// 表读回（draw action 期插件视图）。
	#[test]
	fn set_colour_and_line_state() {
		let ctx = make_context((100.0, 100.0), (2.0, 2.0));
		let col = OfxRGBAColourF { r: 0.9, g: 0.1, b: 0.2, a: 1.0 };
		unsafe {
			assert_eq!(set_colour(ctx, &col), status::OK);
			assert_eq!(set_line_width(ctx, 3.5), status::OK);
			assert_eq!(set_line_stipple(ctx, stipple::DASH), status::OK);
			assert_eq!(set_line_stipple(ctx, 99), status::ERR_VALUE);
		}

		let addr = ctx as usize;
		let live = lock();
		let c = live.get(&addr).unwrap();
		assert_eq!(c.colour, [0.9, 0.1, 0.2, 1.0]);
		assert_eq!(c.line_width, 3.5);
		assert_eq!(c.stipple, stipple::DASH);
		drop(live);
		drop_context(ctx);
	}

	/// 摘除后调用 → Failed（"outside kOfxInteractActionDraw" 语义）。
	#[test]
	fn draw_suite_outside_draw_fails() {
		let ctx = make_context((10.0, 10.0), (1.0, 1.0));
		drop_context(ctx);
		let col = OfxRGBAColourF { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
		let mut out = OfxRGBAColourF { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
		let s = std::ffi::CString::new("x").unwrap();
		let pos = OfxPointD { x: 0.0, y: 0.0 };
		let pts = [OfxPointD { x: 0.0, y: 0.0 }, OfxPointD { x: 10.0, y: 10.0 }];
		unsafe {
			assert_eq!(set_colour(ctx, &col), status::FAILED);
			assert_eq!(get_colour(ctx, std_colour::TEXT, &mut out), status::FAILED);
			assert_eq!(set_line_width(ctx, 1.0), status::FAILED);
			assert_eq!(set_line_stipple(ctx, stipple::SOLID), status::FAILED);
			// 合法参数（避开空指针检查）→ 摘除后 resolve 失败 → Failed。
			assert_eq!(draw(ctx, primitive::RECTANGLE, pts.as_ptr(), 2), status::FAILED);
			// drawText 同样 Failed（先 resolve 失败）。
			assert_eq!(draw_text(ctx, s.as_ptr(), &pos, 0), status::FAILED);
		}
	}

	/// draw 原语参数校验（不触 GL）：非法原语/点数 → ErrValue。
	#[test]
	fn draw_primitive_argument_validation() {
		let ctx = make_context((10.0, 10.0), (1.0, 1.0));
		let pts = [OfxPointD { x: 0.0, y: 0.0 }, OfxPointD { x: 10.0, y: 10.0 }];
		unsafe {
			// 非法原语枚举。
			assert_eq!(draw(ctx, 99, pts.as_ptr(), 2), status::ERR_VALUE);
			assert_eq!(draw(ctx, -1, pts.as_ptr(), 2), status::ERR_VALUE);
			// 点数不合法：LINES 奇数点。
			assert_eq!(draw(ctx, primitive::LINES, pts.as_ptr(), 3), status::ERR_VALUE);
			// RECTANGLE 需恰好 2 点。
			assert_eq!(draw(ctx, primitive::RECTANGLE, pts.as_ptr(), 3), status::ERR_VALUE);
			// POLYGON 需 ≥3 点。
			assert_eq!(draw(ctx, primitive::POLYGON, pts.as_ptr(), 2), status::ERR_VALUE);
			// 空指针 + 正点数。
			assert_eq!(draw(ctx, primitive::LINES, std::ptr::null(), 2), status::ERR_VALUE);
			// 合法参数：环境相关的 GL 路径（无 current 上下文时宿主可能
			// 失败）——参数校验已过即可，具体状态不在此断言。
			let st = draw(ctx, primitive::RECTANGLE, pts.as_ptr(), 2);
			assert!(
				st == status::OK || st == status::FAILED,
				"合法参数应 OK（有 GL）或 Failed（无 GL 上下文），got {st}"
			);
		}
		drop_context(ctx);
	}

	/// drawText：有效参数 → Unsupported（如实拒绝）；空指针 → ErrValue。
	#[test]
	fn draw_text_is_honest_unsupported() {
		let ctx = make_context((10.0, 10.0), (1.0, 1.0));
		let s = std::ffi::CString::new("hello").unwrap();
		let pos = OfxPointD { x: 1.0, y: 2.0 };
		unsafe {
			assert_eq!(draw_text(ctx, s.as_ptr(), &pos, 0), status::ERR_UNSUPPORTED);
			assert_eq!(draw_text(ctx, std::ptr::null(), &pos, 0), status::ERR_VALUE);
		}
		drop_context(ctx);
	}
}

// ---- GL 绘制实现（平台分派）--------------------------------------------

#[cfg(target_os = "macos")]
mod gl_imp {
	use super::{DrawContext, OfxPointD};
	use crate::suites::status;
	use std::ffi::c_void;

	// ---- GL 常量（GL 规范值）----
	const GL_FALSE: i32 = 0;
	const GL_FLOAT: u32 = 0x1406;
	const GL_ARRAY_BUFFER: u32 = 0x8892;
	const GL_STATIC_DRAW: u32 = 0x88E4;
	const GL_FRAGMENT_SHADER: u32 = 0x8B30;
	const GL_VERTEX_SHADER: u32 = 0x8B31;
	const GL_COMPILE_STATUS: u32 = 0x8B81;
	const GL_LINK_STATUS: u32 = 0x8B82;
	const GL_TRIANGLES: u32 = 0x0004;
	const GL_LINES: u32 = 0x0001;
	const GL_LINE_STRIP: u32 = 0x0003;
	const GL_LINE_LOOP: u32 = 0x0002;
	const GL_BLEND: u32 = 0x0BE2;
	const GL_SRC_ALPHA: u32 = 0x0302;
	const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
	const GL_NO_ERROR: u32 = 0;

	// # Safety: 全部是 OpenGL.framework 导出函数；参数语义见各函数
	// 注释。调用前提：GL 上下文 current（宿主 draw action 已 acquire）。
	#[link(name = "OpenGL", kind = "framework")]
	unsafe extern "C" {
		fn glGenVertexArrays(n: i32, arrays: *mut u32);
		fn glBindVertexArray(array: u32);
		fn glGenBuffers(n: i32, buffers: *mut u32);
		fn glBindBuffer(target: u32, buffer: u32);
		fn glBufferData(target: u32, size: isize, data: *const c_void, usage: u32);
		fn glCreateShader(type_: u32) -> u32;
		fn glShaderSource(shader: u32, count: i32, string: *const *const i8, length: *const i32);
		fn glCompileShader(shader: u32);
		fn glGetShaderiv(shader: u32, pname: u32, params: *mut i32);
		fn glDeleteShader(shader: u32);
		fn glCreateProgram() -> u32;
		fn glAttachShader(program: u32, shader: u32);
		fn glLinkProgram(program: u32);
		fn glGetProgramiv(program: u32, pname: u32, params: *mut i32);
		fn glUseProgram(program: u32);
		fn glGetAttribLocation(program: u32, name: *const i8) -> i32;
		fn glVertexAttribPointer(
			index: u32,
			size: i32,
			type_: u32,
			normalized: u8,
			stride: i32,
			pointer: *const c_void,
		);
		fn glEnableVertexAttribArray(index: u32);
		fn glGetUniformLocation(program: u32, name: *const i8) -> i32;
		fn glUniformMatrix4fv(location: i32, count: i32, transpose: u8, value: *const f32);
		fn glUniform4f(location: i32, x: f32, y: f32, z: f32, w: f32);
		fn glDrawArrays(mode: u32, first: i32, count: i32);
		fn glLineWidth(width: f32);
		fn glEnable(cap: u32);
		fn glBlendFunc(sfactor: u32, dfactor: u32);
		fn glGetError() -> u32;
	}

	/// 顶点着色器（canonical 坐标 + 颜色 uniform；投影映射到 NDC）。
	const VERT_SRC: &[u8] = b"#version 150\n\
		in vec2 a_pos;\n\
		uniform mat4 u_proj;\n\
		uniform vec4 u_col;\n\
		out vec4 v_col;\n\
		void main() {\n\
		  v_col = u_col;\n\
		  gl_Position = u_proj * vec4(a_pos, 0.0, 1.0);\n\
		}\n\0";

	const FRAG_SRC: &[u8] = b"#version 150\n\
		in vec4 v_col;\n\
		out vec4 frag_color;\n\
		void main() { frag_color = v_col; }\n\0";

	fn compile_shader(kind: u32, src: &[u8]) -> Result<u32, i32> {
		// SAFETY: glCreateShader/glShaderSource/glCompileShader 均为当前
		// 上下文的合法调用；src 是 NUL 结尾串（b"...\0"）。
		let sh = unsafe { glCreateShader(kind) };
		if sh == 0 {
			return Err(status::FAILED);
		}
		unsafe {
			let ptr = src.as_ptr() as *const i8;
			glShaderSource(sh, 1, &ptr, std::ptr::null());
			glCompileShader(sh);
		}
		let mut ok: i32 = 0;
		// SAFETY: glGetShaderiv 写 ok。
		unsafe { glGetShaderiv(sh, GL_COMPILE_STATUS, &mut ok) };
		if ok == GL_FALSE {
			// SAFETY: sh 是本函数 glCreateShader 的产物。
			unsafe { glDeleteShader(sh) };
			return Err(status::FAILED);
		}
		Ok(sh)
	}

	/// 惰性资源组：(program, vao, vbo)。进程级缓存；VAO/VBO 固定复用，
	/// 每次 draw 用 glBufferData 重灌顶点（避免逐次分配/泄漏）。
	fn resources() -> Result<(u32, u32, u32), i32> {
		static RES: std::sync::OnceLock<Result<(u32, u32, u32), i32>> = std::sync::OnceLock::new();
		*RES.get_or_init(|| {
			let vs = match compile_shader(GL_VERTEX_SHADER, VERT_SRC) {
				Ok(s) => s,
				Err(e) => return Err(e),
			};
			let fs = match compile_shader(GL_FRAGMENT_SHADER, FRAG_SRC) {
				Ok(s) => s,
				Err(e) => {
					// SAFETY: vs 是本闭包编译成功的着色器。
					unsafe { glDeleteShader(vs) };
					return Err(e);
				}
			};
			let prog = unsafe { glCreateProgram() };
			if prog == 0 {
				// SAFETY: vs/fs 均有效。
				unsafe {
					glDeleteShader(vs);
					glDeleteShader(fs);
				}
				return Err(status::FAILED);
			}
			unsafe {
				glAttachShader(prog, vs);
				glAttachShader(prog, fs);
				glLinkProgram(prog);
			}
			let mut ok: i32 = 0;
			// SAFETY: glGetProgramiv 写 ok。
			unsafe { glGetProgramiv(prog, GL_LINK_STATUS, &mut ok) };
			// 链接后可删着色器对象（程序保留可执行版本）。
			unsafe {
				glDeleteShader(vs);
				glDeleteShader(fs);
			}
			if ok == GL_FALSE {
				return Err(status::FAILED);
			}
			// SAFETY: 生成并绑定固定 VAO/VBO（3.2 core）。
			let mut vao: u32 = 0;
			let mut vbo: u32 = 0;
			unsafe {
				glGenVertexArrays(1, &mut vao);
				glBindVertexArray(vao);
				glGenBuffers(1, &mut vbo);
				glBindBuffer(GL_ARRAY_BUFFER, vbo);
			}
			Ok((prog, vao, vbo))
		})
	}

	/// canonical→NDC 正交投影（列主序 4x4）。
	fn ortho(cw: f64, ch: f64) -> [f32; 16] {
		let (cw, ch) = (cw.max(1.0) as f32, ch.max(1.0) as f32);
		[
			2.0 / cw, 0.0, 0.0, 0.0,
			0.0, 2.0 / ch, 0.0, 0.0,
			0.0, 0.0, -1.0, 0.0,
			-1.0, -1.0, 0.0, 1.0,
		]
	}

	/// 按原语生成 GL 顶点（canonical 坐标；已按图元类型合法）。
	fn vertices(prim: i32, pts: &[OfxPointD]) -> (u32, Vec<f32>) {
		match prim {
			super::primitive::LINES => (
				GL_LINES,
				pts.iter().flat_map(|p| [p.x as f32, p.y as f32]).collect(),
			),
			super::primitive::LINE_STRIP => (
				GL_LINE_STRIP,
				pts.iter().flat_map(|p| [p.x as f32, p.y as f32]).collect(),
			),
			super::primitive::LINE_LOOP => (
				GL_LINE_LOOP,
				pts.iter().flat_map(|p| [p.x as f32, p.y as f32]).collect(),
			),
			super::primitive::RECTANGLE => {
				let (x1, y1) = (pts[0].x as f32, pts[0].y as f32);
				let (x2, y2) = (pts[1].x as f32, pts[1].y as f32);
				// 两个三角形组成实心矩形（对角点定义）。
				(
					GL_TRIANGLES,
					vec![
						x1, y1, x2, y1, x1, y2,
						x1, y2, x2, y1, x2, y2,
					],
				)
			}
			super::primitive::POLYGON => {
				// 三角扇：0,i,i+1（凸多边形约定）。
				let mut v = Vec::new();
				for i in 1..pts.len() - 1 {
					v.extend_from_slice(&[
						pts[0].x as f32, pts[0].y as f32,
						pts[i].x as f32, pts[i].y as f32,
						pts[i + 1].x as f32, pts[i + 1].y as f32,
					]);
				}
				(GL_TRIANGLES, v)
			}
			super::primitive::ELLIPSE => {
				// 椭圆**线框**（文档），包围盒内 64 段线环。
				let (cx, cy) = ((pts[0].x + pts[1].x) / 2.0, (pts[0].y + pts[1].y) / 2.0);
				let (rx, ry) = (
					(pts[1].x - pts[0].x).abs() / 2.0,
					(pts[1].y - pts[0].y).abs() / 2.0,
				);
				let n = 64;
				let mut v = Vec::with_capacity(n * 2);
				for i in 0..n {
					let a = std::f64::consts::TAU * i as f64 / n as f64;
					v.push((cx + rx * a.cos()) as f32);
					v.push((cy + ry * a.sin()) as f32);
				}
				(GL_LINE_LOOP, v)
			}
			_ => (GL_TRIANGLES, Vec::new()),
		}
	}

	/// 真实渲染：绑定固定 VAO/VBO、重灌顶点、设置 uniform、绘制。
	/// 要求 GL 上下文 current（宿主 draw action 已 acquire）；无 current
	/// 上下文时不发任何 GL 命令（macOS 无 current 调 GL 是未定义行为，
	/// 直接失败）。
	pub fn render(ctx: &DrawContext, prim: i32, pts: &[OfxPointD]) -> Result<(), i32> {
		if !crate::gl_bridge::is_current() {
			return Err(status::FAILED);
		}
		let (prog, vao, vbo) = resources().map_err(|_| status::FAILED)?;
		let (mode, verts) = vertices(prim, pts);
		if verts.is_empty() {
			return Err(status::ERR_VALUE);
		}
		// 合成：非不透明色 "over"（ofxDrawSuite.h setColour 文档）。
		if ctx.colour[3] < 1.0 {
			// SAFETY: 开启混合并设因子（上下文 current）。
			unsafe {
				glEnable(GL_BLEND);
				glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
			}
		}
		// SAFETY: 以下 GL 调用均要求上下文 current（宿主已保证）；对象
		// 名来自 resources()（本进程唯一 GL 使用方）。
		unsafe {
			glUseProgram(prog);
			glBindVertexArray(vao);
			glBindBuffer(GL_ARRAY_BUFFER, vbo);
			glBufferData(
				GL_ARRAY_BUFFER,
				(verts.len() * 4) as isize,
				verts.as_ptr() as *const c_void,
				GL_STATIC_DRAW,
			);

			let a_pos = glGetAttribLocation(prog, b"a_pos\0".as_ptr() as *const i8);
			if a_pos < 0 {
				return Err(status::FAILED);
			}
			glVertexAttribPointer(a_pos as u32, 2, GL_FLOAT, 0, 0, std::ptr::null());
			glEnableVertexAttribArray(a_pos as u32);

			let u_proj = glGetUniformLocation(prog, b"u_proj\0".as_ptr() as *const i8);
			let u_col = glGetUniformLocation(prog, b"u_col\0".as_ptr() as *const i8);
			let cw = ctx.viewport.0 / ctx.pixel_scale.0.max(1e-6);
			let ch = ctx.viewport.1 / ctx.pixel_scale.1.max(1e-6);
			let m = ortho(cw, ch);
			glUniformMatrix4fv(u_proj, 1, 0, m.as_ptr());
			glUniform4f(u_col, ctx.colour[0], ctx.colour[1], ctx.colour[2], ctx.colour[3]);

			if ctx.line_width > 0.0 {
				glLineWidth(ctx.line_width);
			}
			glDrawArrays(mode, 0, (verts.len() / 2) as i32);

			// 解除绑定（对象保留复用）。
			glBindVertexArray(0);
			glBindBuffer(GL_ARRAY_BUFFER, 0);
			glUseProgram(0);
		}
		let err = unsafe { glGetError() };
		if err != GL_NO_ERROR {
			return Err(status::FAILED);
		}
		Ok(())
	}
}

/// 非 macOS：Draw suite 的 GL 绘制 stub（无 GL 桥 → Failed）。
#[cfg(not(target_os = "macos"))]
mod gl_imp {
	use super::{DrawContext, OfxPointD};
	use crate::suites::status;

	pub fn render(_ctx: &DrawContext, _prim: i32, _pts: &[OfxPointD]) -> Result<(), i32> {
		Err(status::FAILED)
	}
}
