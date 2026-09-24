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

//! GL 互操作桥（方案 B 落地：离屏 GL 上下文 + 纹理/FBO + 回读）。
//!
//! ## 背景（阶段 6a spike 结论，已落地）
//!
//! OFX 的 `OpenGLRender` 扩展要求宿主把 clip 纹理以 **GL 纹理名**
//! （`kOfxImageEffectPropOpenGLTextureIndex`）递给插件，插件直接画进
//! 输出纹理。oak 的渲染后端是 wgpu（macOS 为 Metal），Metal 与 GL
//! 纹理名之间没有共享命名空间（方案 A 不可行，详见 0.1 版的 spike
//! 文档）。本模块实现**方案 B**：
//!
//! ```text
//! 宿主自建原生离屏 GL 上下文（macOS = CGL）
//!   → 为每帧建输出 GL 纹理 + FBO（插件 render 画进附着纹理）
//!   → 插件经 GL suite 取到真实纹理名（OpenGLTextureIndex）
//!   → render 返回后 glReadPixels 回读为 CPU 帧
//!   → 按目标帧格式转换（F32/RGBA8）装帧
//! ```
//!
//! 渲染跑在 oak-worker 进程内（M15 进程隔离），GL 上下文在 worker
//! 进程内创建即可；主进程无需 GL。
//!
//! ## 上下文生命周期与线程模型
//!
//! - **进程级共享单例**：`OnceLock` 惰性创建（`imp::context`），一次
//!   创建、整个进程共用（CGL 上下文可在任意线程置为 current，但不能
//!   同时）。
//! - **串行化**：[`acquire`] 取全局 `Mutex` 并把上下文置为当前线程
//!   current；返回 [`GlGuard`]，drop 时清 current 并放锁。OFX 插件
//!   渲染已由实例锁（`Instance::render_lock`）串行化，但不同实例可在
//!   不同线程渲染——GL 侧必须再串行一次，保证同一时刻只有一条线程
//!   持有 current 上下文。
//! - **current 的跨调用保持**：ofxGPURender.h 要求宿主在 Render/
//!   Begin/EndSequenceRender 期间让上下文保持 current（插件在 suite
//!   调用之间直接发 GL 命令）。因此 render 驱动**一次** acquire 整个
//!   render action（含 suite 回调），[`GlGuard`] 跨整个 action 持有；
//!   本模块其余函数假定上下文已 current（guard 已持有），不再加锁。
//! - **同线程可重入**：[`acquire`] 在同一线程已持外层 guard 时只递增
//!   线程局部嵌套深度（上下文已 current，不重取锁）；最外层 guard
//!   drop 时才清 current 放锁。interact draw 的宿主测试需要"一次
//!   acquire 覆盖 FBO 装配 → 插件绘制 → 回读"而 `Interact::draw`
//!   内部又要自行 acquire（宿主侧每次 action 前都要保证 current）——
//!   嵌套 acquire 免去死锁。
//! - **清理**：进程退出时 `OnceLock` 里的 [`GlContext`] drop，销毁
//!   CGL 上下文与像素格式。
//!
//! ## 平台
//!
//! - **macOS（真实实现）**：Core OpenGL（`OpenGL.framework`，
//!   `#[link(name = "OpenGL", kind = "framework")]`，无需新 crate）。
//!   像素格式：3.2 core profile、offline renderer 允许（无 GPU 的无头
//!   环境可回退软件渲染）、24 位颜色 + 8 位 alpha。离屏渲染（FBO）
//!   无需 drawable，不 swap buffer。
//! - **Linux（预留 EGL）**：`cfg(target_os = "linux")` 走 stub，全部
//!   返回失败/`false`（use_opengl 决策据此回退 CPU）。接线点：
//!   `imp` 换成 EGL（`eglGetDisplay/eglCreateContext` + GLES3），
//!   FFI 同构，GL 命令一致。
//! - **Windows（预留 WGL）**：同理，stub。接线点：`wglCreateContext`
//!   + `wglMakeCurrent`，FFI 同构。
//!
//! ## 回读格式转换（真实正确）
//!
//! 输出 GL 纹理的内部格式按目标帧格式选择（管线 F32 → `GL_RGBA32F`，
//! U8 目标 → `GL_RGBA8`）。`glReadPixels` 按纹理类型回读（`GL_FLOAT`
//! 或 `GL_UNSIGNED_BYTE`），垂直翻转（GL 原点左下 vs 帧缓冲原点左上），
//! U8 按 `v/255.0` 归一化到 F32——与 CPU 路径输出格式（F32 RGBA）
//! 一致，经 `write_output_frame` 装配。

use std::sync::Arc;

use crate::image::Image;
use crate::render::VideoParams;

// ---- 对外 API（平台无关签名；实现按平台 cfg 分派）----------------------

/// Whether the TEST build may touch the real CGL context on macOS. The
/// headless CI runner cannot exercise Apple's GL reliably (parallel unit
/// tests have segfaulted inside the driver), so the test build behaves as
/// if GL were unavailable unless `OAK_GPU_TESTS` asks for it — the same
/// gate the integration GL tests use. Release builds are untouched.
///
/// Every public entry point below obeys the gate: the suites fabricate
/// contexts and texture names when they drive the GL paths without a
/// real driver, so even a "context is current" call must not reach CGL.
#[cfg(all(test, target_os = "macos"))]
fn real_gl_gated() -> bool {
	std::env::var_os("OAK_GPU_TESTS").is_none()
}

/// The error gated entry points report in tests (see [`real_gl_gated`]).
#[cfg(all(test, target_os = "macos"))]
fn gl_gated_error() -> crate::error::Error {
	crate::error::Error::Failed(
		"gl_bridge: real GL is gated behind OAK_GPU_TESTS in tests".to_string(),
	)
}

/// GL 上下文是否可用（离屏上下文可创建）。use_opengl 决策用它当
/// "目标纹理有有效 GL 名" 的门：桥能为目标帧建出真实 GL 纹理 ⟺
/// 上下文可用。惰性创建，幂等。
pub fn gl_available() -> bool {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return false;
	}
	imp::gl_available()
}

/// 取得进程级 GL 上下文并把本线程置为 current（全局串行）。
/// 返回的 guard 在 drop 时清 current 并放锁。失败（无 GL / 上下文
/// 创建失败）→ 调用方回退 CPU。
pub fn acquire() -> crate::error::Result<GlGuard> {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return Err(gl_gated_error());
	}
	imp::acquire()
}

/// 创建输出 GL 纹理（尺寸 = 目标帧，内部格式按 `params.format`：
/// F32 → RGBA32F，U8 → RGBA8），返回真实 GL 纹理名。
///
/// # Safety
///
/// 要求 GL 上下文已 current（[`GlGuard`] 持有期间调用）。
pub fn create_output_texture(w: i32, h: i32, params: &VideoParams) -> crate::error::Result<i32> {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return Err(gl_gated_error());
	}
	imp::create_output_texture(w, h, params)
}

/// 创建输入 GL 纹理（RGBA32F）并把 F32 像素数据上传，返回真实 GL
/// 纹理名。`pixels` 必须为 `w*h*4*4` 字节（紧凑 RGBA F32，行优先）。
///
/// # Safety
///
/// 要求 GL 上下文已 current（[`GlGuard`] 持有期间调用）。
pub fn create_input_texture(w: i32, h: i32, pixels: &[u8]) -> crate::error::Result<i32> {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return Err(gl_gated_error());
	}
	imp::create_input_texture(w, h, pixels)
}

/// 删除 GL 纹理。
///
/// # Safety
///
/// 要求 GL 上下文已 current。
pub fn delete_gl_texture(name: i32) {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return;
	}
	imp::delete_gl_texture(name);
}

/// 创建 FBO 并把 `tex` 挂为颜色附件（绑定并校验完整性）。
/// 返回 FBO 名（当前已绑定）。失败时已清理。
///
/// # Safety
///
/// 要求 GL 上下文已 current。
pub fn create_fbo(tex: i32, w: i32, h: i32) -> crate::error::Result<i32> {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return Err(gl_gated_error());
	}
	imp::create_fbo(tex, w, h)
}

/// 绑定 FBO（0 = 系统帧缓冲）。
///
/// # Safety
///
/// 要求 GL 上下文已 current。
pub fn bind_fbo(fbo: i32) {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return;
	}
	imp::bind_fbo(fbo);
}

/// 删除 FBO。
///
/// # Safety
///
/// 要求 GL 上下文已 current。
pub fn delete_fbo(fbo: i32) {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return;
	}
	imp::delete_fbo(fbo);
}

/// 设置视口（像素坐标）。
///
/// # Safety
///
/// 要求 GL 上下文已 current。
pub fn set_viewport(w: i32, h: i32) {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return;
	}
	imp::set_viewport(w, h);
}

/// 回读当前绑定 FBO 并装配成 F32 RGBA 图像（垂直翻转、U8 归一化）。
/// 返回的 Image 与 CPU 路径输出格式一致，可直接交
/// [`crate::render_driver::write_output_frame`]。
///
/// # Safety
///
/// 要求 GL 上下文已 current、目标 FBO 仍绑定。
pub fn read_pixels_to_image(w: i32, h: i32, params: &VideoParams) -> crate::error::Result<Arc<Image>> {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return Err(gl_gated_error());
	}
	imp::read_pixels_to_image(w, h, params)
}

/// 设置 clear 颜色（FBO 测试/失败紫屏用）。
///
/// # Safety
///
/// 要求 GL 上下文已 current。
pub fn gl_clear_color(r: f32, g: f32, b: f32, a: f32) {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return;
	}
	imp::gl_clear_color(r, g, b, a);
}

/// 清除颜色缓冲（FBO 测试用；插件正常路径自行发 GL 命令）。
///
/// # Safety
///
/// 要求 GL 上下文已 current。
pub fn gl_clear() {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return;
	}
	imp::gl_clear();
}

/// 当前 GL 实现的版本串（真实上下文证据；测试用）。
pub fn gl_version_string() -> Option<String> {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return None;
	}
	imp::gl_version_string()
}

/// GL 上下文句柄（持有全局锁 + 上下文 current；drop 时清 current 放锁）。
pub struct GlGuard {
	_inner: imp::GuardInner,
}

/// 当前线程是否持有 GL current（嵌套 guard 内也算；[`acquire`] 成功
/// 后为 true）。无 GL 的调用方（Draw suite 在 draw action 外被调）据此
/// 不触 GL 命令——macOS 无 current 上下文时调 GL 是未定义行为。
pub fn is_current() -> bool {
	#[cfg(all(test, target_os = "macos"))]
	if real_gl_gated() {
		return false;
	}
	imp::is_current()
}

// ---- 平台实现 ----------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
	use super::*;
	use crate::image::{BitDepth, Components};
	use std::ffi::{c_char, c_void};
	use std::sync::{Arc, Mutex, OnceLock};

	// ---- OpenGL 常量（GL 规范值，非 OFX 宏）----
	const GL_TEXTURE_2D: u32 = 0x0DE1;
	const GL_RGBA: u32 = 0x1908;
	const GL_UNSIGNED_BYTE: u32 = 0x1401;
	const GL_FLOAT: u32 = 0x1406;
	const GL_RGBA32F: u32 = 0x8814;
	const GL_RGBA8: u32 = 0x8058;
	const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
	const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
	const GL_NEAREST: u32 = 0x2600;
	const GL_TEXTURE_WRAP_S: u32 = 0x2802;
	const GL_TEXTURE_WRAP_T: u32 = 0x2803;
	const GL_CLAMP_TO_EDGE: u32 = 0x812F;
	const GL_FRAMEBUFFER: u32 = 0x8D40;
	const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
	const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
	const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
	const GL_PACK_ALIGNMENT: u32 = 0x0D05;
	const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;
	const GL_VERSION: u32 = 0x1F02;
	const GL_NO_ERROR: u32 = 0;

	// CGL 像素格式属性（CGLPixelFormatAttribute，CGL.h）。
	const K_CGL_PFA_OPENGL_PROFILE: i32 = 99;
	const K_CGLOGLP_VERSION_3_2_CORE: i32 = 0x3200;
	const K_CGL_PFA_ALLOW_OFFLINE_RENDERERS: i32 = 96;
	const K_CGL_PFA_COLOR_SIZE: i32 = 8;
	const K_CGL_PFA_ALPHA_SIZE: i32 = 11;

	// macOS：CGL + OpenGL 的 FFI 声明（直接链接 OpenGL.framework）。
	//
	// # Safety
	//
	// 全部是系统 framework 导出函数；指针参数语义见各函数注释。
	// CGL* 类型用 `*mut c_void` 表示不透明对象指针（CGLContextObj /
	// CGLPixelFormatObj）。
	#[link(name = "OpenGL", kind = "framework")]
	unsafe extern "C" {
		/// 按属性列表选像素格式（成功返回 kCGLNoError=0；`npix` 是
		/// CGL 要求返回的像素格式计数，本桥只取格式对象，可丢弃——
		/// 但实参必须给足，否则是未定义行为）。
		fn CGLChoosePixelFormat(attrs: *const i32, pix: *mut *mut c_void, npix: *mut i32) -> i32;
		/// 建上下文（share = NULL 无共享组）。返回 kCGLNoError=0。
		fn CGLCreateContext(pix: *mut c_void, share: *mut c_void, ctx: *mut *mut c_void) -> i32;
		fn CGLDestroyPixelFormat(pix: *mut c_void);
		fn CGLDestroyContext(ctx: *mut c_void);
		/// 置为当前线程的 current 上下文（NULL = 清）。返回 kCGLNoError=0。
		fn CGLSetCurrentContext(ctx: *mut c_void) -> i32;

		fn glGenTextures(n: i32, textures: *mut u32);
		fn glDeleteTextures(n: i32, textures: *const u32);
		fn glBindTexture(target: u32, texture: u32);
		fn glTexParameteri(target: u32, pname: u32, param: i32);
		fn glPixelStorei(pname: u32, param: i32);
		/// 分配/上传 2D 纹理图像。`pixels` 为 NULL 时只分配。
		fn glTexImage2D(
			target: u32,
			level: i32,
			internalformat: i32,
			width: i32,
			height: i32,
			border: i32,
			format: u32,
			type_: u32,
			pixels: *const c_void,
		);
		fn glGenFramebuffers(n: i32, framebuffers: *mut u32);
		fn glDeleteFramebuffers(n: i32, framebuffers: *const u32);
		fn glBindFramebuffer(target: u32, framebuffer: u32);
		fn glFramebufferTexture2D(target: u32, attachment: u32, textarget: u32, texture: u32, level: i32);
		fn glCheckFramebufferStatus(target: u32) -> u32;
		fn glViewport(x: i32, y: i32, width: i32, height: i32);
		fn glClearColor(r: f32, g: f32, b: f32, a: f32);
		fn glClear(mask: u32);
		fn glReadPixels(
			x: i32,
			y: i32,
			width: i32,
			height: i32,
			format: u32,
			type_: u32,
			pixels: *mut c_void,
		);
		fn glGetError() -> u32;
		/// 返回当前上下文的信息字符串（非空 = 上下文真实可用）。
		fn glGetString(name: u32) -> *const c_char;
	}

	/// CGL 上下文单例（OnceLock 惰性创建；Send+Sync 见下）。
	pub struct GlContext {
		ctx: *mut c_void,
		pixel_format: *mut c_void,
	}

	// SAFETY: CGL 上下文对象可在任意线程经 CGLSetCurrentContext 置为
	// current，但不能同时被两条线程使用——本模块用全局 Mutex 串行化
	// 一切置 current 与 GL 调用，指针只在线程间传递（Arc 共享）而从不
	// 并发解引用；Drop 只在进程退出（单线程）执行。Send/Sync 因此安全。
	unsafe impl Send for GlContext {}
	unsafe impl Sync for GlContext {}

	impl Drop for GlContext {
		fn drop(&mut self) {
			// SAFETY: 进程退出期单线程；先清 current 再销毁对象，顺序
			// 满足 CGL 文档（context 必须先取消 current 再 destroy）。
			unsafe {
				CGLSetCurrentContext(std::ptr::null_mut());
				if !self.ctx.is_null() {
					CGLDestroyContext(self.ctx);
				}
				if !self.pixel_format.is_null() {
					CGLDestroyPixelFormat(self.pixel_format);
				}
			}
		}
	}

	static GL_CONTEXT: OnceLock<Result<Arc<GlContext>, String>> = OnceLock::new();

	/// GL 全局串行锁：同一时刻只有一条线程把共享上下文置为 current。
	static GL_LOCK: Mutex<()> = Mutex::new(());

	fn lock() -> std::sync::MutexGuard<'static, ()> {
		GL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
	}

	fn err(msg: &str) -> crate::error::Error {
		crate::error::Error::Failed(format!("gl_bridge: {msg}"))
	}

	/// 惰性创建进程级 CGL 上下文（幂等）。
	fn context() -> crate::error::Result<&'static Arc<GlContext>> {
		let res = GL_CONTEXT.get_or_init(create_context);
		match res {
			Ok(c) => Ok(c),
			Err(e) => Err(err(&format!("CGL 上下文创建失败：{e}"))),
		}
	}

	fn create_context() -> Result<Arc<GlContext>, String> {
		// SAFETY: CGLChoosePixelFormat 把像素格式对象写入 pix；属性表
		// 以 0 结尾（kCGLPFAOpenGLProfile 之后是 profile 值、kCGLPFA
		// AllowOfflineRenderers 后跟 1；见 CGL.h 的 attr 配对约定）。
		let attrs = [
			K_CGL_PFA_OPENGL_PROFILE,
			K_CGLOGLP_VERSION_3_2_CORE,
			K_CGL_PFA_ALLOW_OFFLINE_RENDERERS,
			1,
			K_CGL_PFA_COLOR_SIZE,
			24,
			K_CGL_PFA_ALPHA_SIZE,
			8,
			0,
		];
		unsafe {
			let mut pix: *mut c_void = std::ptr::null_mut();
			// npix 是 CGL 的 out 参数（像素格式计数）；本桥不需要，但
			// 必须给足实参（CGL 会写它）。
			let mut npix: i32 = 0;
			let st = CGLChoosePixelFormat(attrs.as_ptr(), &mut pix, &mut npix);
			if st != 0 || pix.is_null() {
				return Err(format!("CGLChoosePixelFormat 失败：{st}"));
			}
			let mut ctx: *mut c_void = std::ptr::null_mut();
			// share = NULL：无共享组（本进程唯一 GL 使用方，无需与其他
			// 上下文共享命名空间）。
			let st = CGLCreateContext(pix, std::ptr::null_mut(), &mut ctx);
			if st != 0 || ctx.is_null() {
				CGLDestroyPixelFormat(pix);
				return Err(format!("CGLCreateContext 失败：{st}"));
			}
			Ok(Arc::new(GlContext { ctx, pixel_format: pix }))
		}
	}

	/// guard 持有期：全局锁 + 上下文 current。
	pub struct GuardInner {
		/// 最外层 guard 持有全局锁；同线程嵌套 guard 为 None（只递增
		/// 深度，见 [`acquire`]）。
		_lock: Option<std::sync::MutexGuard<'static, ()>>,
		_ctx: Arc<GlContext>,
	}

	// 当前线程的 acquire 嵌套深度。>0 = 本线程已持外层 guard（上下文
	// 已 current）；嵌套 acquire 只递增深度，不重取锁。
	thread_local! {
		static ACQUIRE_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
	}

	pub fn is_current() -> bool {
		ACQUIRE_DEPTH.with(|d| d.get() > 0)
	}

	pub fn acquire() -> crate::error::Result<GlGuard> {
		let ctx = context()?.clone();
		let depth = ACQUIRE_DEPTH.with(|d| d.get());
		if depth > 0 {
			// 嵌套：上下文已 current（外层 guard 持有期），只递增深度。
			ACQUIRE_DEPTH.with(|d| d.set(depth + 1));
			return Ok(GlGuard {
				_inner: GuardInner {
					_lock: None,
					_ctx: ctx,
				},
			});
		}
		let guard = lock();
		// SAFETY: ctx 是有效 CGLContextObj；置 current 后本线程可发 GL
		// 命令。失败返回非 0。
		let st = unsafe { CGLSetCurrentContext(ctx.ctx) };
		if st != 0 {
			drop(guard);
			return Err(err(&format!("CGLSetCurrentContext 失败：{st}")));
		}
		ACQUIRE_DEPTH.with(|d| d.set(1));
		Ok(GlGuard {
			_inner: GuardInner {
				_lock: Some(guard),
				_ctx: ctx,
			},
		})
	}

	impl Drop for GuardInner {
		fn drop(&mut self) {
			let depth = ACQUIRE_DEPTH.with(|d| d.get());
			if depth <= 1 {
				// 最外层：清 current（NULL）并释放全局锁。LIFO 栈纪律
				// 保证外层 drop 前所有嵌套 guard 已 drop（深度归 1），
				// 故 _lock 必为 Some。
				ACQUIRE_DEPTH.with(|d| d.set(0));
				// SAFETY: 清 current（NULL）后放锁；见模块文档的线程模型。
				unsafe {
					CGLSetCurrentContext(std::ptr::null_mut());
				}
				drop(self._lock.take());
			} else {
				// 嵌套 guard drop：只递减深度，不清 current/不放锁。
				ACQUIRE_DEPTH.with(|d| d.set(depth - 1));
			}
		}
	}

	pub fn gl_available() -> bool {
		context().is_ok()
	}

	/// GL 错误检查（测试断言用；返回最近错误或 GL_NO_ERROR）。
	fn check_error(what: &str) -> crate::error::Result<()> {
		// SAFETY: glGetError 无参数，读取当前上下文的错误状态。
		let e = unsafe { glGetError() };
		if e != GL_NO_ERROR {
			Err(err(&format!("{what} 失败：GL error 0x{e:x}")))
		} else {
			Ok(())
		}
	}

	/// 目标帧格式 → 输出 GL 纹理格式。
	/// 返回 (internal format, readback type, bytes per channel)。
	fn gl_texture_format(params: &VideoParams) -> crate::error::Result<(u32, u32, usize)> {
		match params.format {
			f if f == crate::render::PIXEL_FORMAT_F32 => Ok((GL_RGBA32F, GL_FLOAT, 4)),
			f if f == crate::render::PIXEL_FORMAT_U8 => Ok((GL_RGBA8, GL_UNSIGNED_BYTE, 1)),
			f => Err(err(&format!(
				"目标帧格式 {f} 不支持 GL 输出（仅 F32/U8）"
			))),
		}
	}

	/// 建纹理并设采样参数（FBO 完整性要求非 mipmap 的 min filter 与
	/// clamp-to-edge）。
	fn new_texture(w: i32, h: i32, internal: u32) -> crate::error::Result<u32> {
		if w <= 0 || h <= 0 {
			return Err(err("纹理尺寸非法"));
		}
		unsafe {
			let mut tex: u32 = 0;
			glGenTextures(1, &mut tex);
			if tex == 0 {
				return Err(err("glGenTextures 失败"));
			}
			glBindTexture(GL_TEXTURE_2D, tex);
			glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST as i32);
			glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST as i32);
			glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as i32);
			glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as i32);
			glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
			// internalformat 是 GLenum（无符号）；C 签名里 glTexImage2D
			// 的 internalformat 实为 GLint——此处值 < 2^31 安全。
			glTexImage2D(
				GL_TEXTURE_2D,
				0,
				internal as i32,
				w,
				h,
				0,
				GL_RGBA,
				GL_FLOAT,
				std::ptr::null(),
			);
			glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
			glBindTexture(GL_TEXTURE_2D, 0);
			check_error("glTexImage2D")?;
			Ok(tex)
		}
	}

	pub fn create_output_texture(
		w: i32,
		h: i32,
		params: &VideoParams,
	) -> crate::error::Result<i32> {
		let (internal, _, _) = gl_texture_format(params)?;
		new_texture(w, h, internal).map(|t| t as i32)
	}

	pub fn create_input_texture(w: i32, h: i32, pixels: &[u8]) -> crate::error::Result<i32> {
		let tex = new_texture(w, h, GL_RGBA32F)?;
		// 上传 F32 RGBA 像素（紧凑，行优先；先确保 unpack alignment=1）。
		let expected = (w as usize) * (h as usize) * 4 * 4;
		if pixels.len() != expected {
			let _ = delete_gl_texture(tex as i32);
			return Err(err(&format!(
				"输入像素长度 {} 与 w*h*4*4={expected} 不符",
				pixels.len()
			)));
		}
		unsafe {
			glBindTexture(GL_TEXTURE_2D, tex);
			glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
			glTexImage2D(
				GL_TEXTURE_2D,
				0,
				GL_RGBA32F as i32,
				w,
				h,
				0,
				GL_RGBA,
				GL_FLOAT,
				pixels.as_ptr() as *const c_void,
			);
			glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
			glBindTexture(GL_TEXTURE_2D, 0);
		}
		check_error("输入纹理上传")?;
		Ok(tex as i32)
	}

	pub fn delete_gl_texture(name: i32) {
		if name <= 0 {
			return;
		}
		// SAFETY: name 是之前 glGenTextures 产出的纹理名；删除后不可
		// 再用（本桥保证不再引用）。要求上下文 current。
		unsafe {
			let t = name as u32;
			glDeleteTextures(1, &t);
		}
	}

	pub fn create_fbo(tex: i32, _w: i32, _h: i32) -> crate::error::Result<i32> {
		if tex <= 0 {
			return Err(err("FBO 附着纹理无效"));
		}
		unsafe {
			let mut fbo: u32 = 0;
			glGenFramebuffers(1, &mut fbo);
			if fbo == 0 {
				return Err(err("glGenFramebuffers 失败"));
			}
			glBindFramebuffer(GL_FRAMEBUFFER, fbo);
			glFramebufferTexture2D(
				GL_FRAMEBUFFER,
				GL_COLOR_ATTACHMENT0,
				GL_TEXTURE_2D,
				tex as u32,
				0,
			);
			let status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
			if status != GL_FRAMEBUFFER_COMPLETE {
				glDeleteFramebuffers(1, &fbo);
				glBindFramebuffer(GL_FRAMEBUFFER, 0);
				return Err(err(&format!("FBO 不完整：0x{status:x}")));
			}
			Ok(fbo as i32)
		}
	}

	pub fn bind_fbo(fbo: i32) {
		// SAFETY: 绑定 0（系统帧缓冲）或本桥创建的 FBO 名。
		unsafe {
			glBindFramebuffer(GL_FRAMEBUFFER, fbo as u32);
		}
	}

	pub fn delete_fbo(fbo: i32) {
		if fbo <= 0 {
			return;
		}
		// SAFETY: fbo 是本桥 glGenFramebuffers 的产物；删除前先解除绑定。
		unsafe {
			glBindFramebuffer(GL_FRAMEBUFFER, 0);
			let f = fbo as u32;
			glDeleteFramebuffers(1, &f);
		}
	}

	pub fn set_viewport(w: i32, h: i32) {
		// SAFETY: 视口是 GL 上下文状态，直接写。
		unsafe {
			glViewport(0, 0, w, h);
		}
	}

	/// 回读原始像素（紧凑、行优先、GL 原点左下）。
	fn read_pixels(w: i32, h: i32, format: u32, type_: u32, bpc: usize) -> crate::error::Result<Vec<u8>> {
		let size = (w as usize) * (h as usize) * 4 * bpc;
		let mut buf = vec![0u8; size];
		unsafe {
			glPixelStorei(GL_PACK_ALIGNMENT, 1);
			glReadPixels(0, 0, w, h, format, type_, buf.as_mut_ptr() as *mut c_void);
			glPixelStorei(GL_PACK_ALIGNMENT, 4);
		}
		check_error("glReadPixels")?;
		Ok(buf)
	}

	pub fn read_pixels_to_image(
		w: i32,
		h: i32,
		params: &VideoParams,
	) -> crate::error::Result<Arc<Image>> {
		let (_, gl_type, bpc) = gl_texture_format(params)?;
		let raw = read_pixels(w, h, GL_RGBA, gl_type, bpc)?;
		let (w, h) = (w as usize, h as usize);
		let mut img = Image::allocate(
			BitDepth::Float,
			Components::Rgba,
			crate::instance::OfxRectD {
				x1: 0.0,
				y1: 0.0,
				x2: w as f64,
				y2: h as f64,
			},
		);
		let dst = img.pixels_mut();
		let src_row = w * 4 * bpc;
		let dst_row = w * 4 * 4;
		for y in 0..h {
			// 垂直翻转：GL 读回原点在左下，帧缓冲原点在左上。
			let src_y = h - 1 - y;
			let src = &raw[src_y * src_row..(src_y + 1) * src_row];
			let d = &mut dst[y * dst_row..(y + 1) * dst_row];
			for x in 0..w {
				let (r, g, b, a): (f32, f32, f32, f32) = if bpc == 4 {
					let p = &src[x * 16..x * 16 + 16];
					(
						f32::from_ne_bytes(p[0..4].try_into().unwrap()),
						f32::from_ne_bytes(p[4..8].try_into().unwrap()),
						f32::from_ne_bytes(p[8..12].try_into().unwrap()),
						f32::from_ne_bytes(p[12..16].try_into().unwrap()),
					)
				} else {
					let p = &src[x * 4..x * 4 + 4];
					(
						p[0] as f32 / 255.0,
						p[1] as f32 / 255.0,
						p[2] as f32 / 255.0,
						p[3] as f32 / 255.0,
					)
				};
				let o = x * 16;
				d[o..o + 4].copy_from_slice(&r.to_ne_bytes());
				d[o + 4..o + 8].copy_from_slice(&g.to_ne_bytes());
				d[o + 8..o + 12].copy_from_slice(&b.to_ne_bytes());
				d[o + 12..o + 16].copy_from_slice(&a.to_ne_bytes());
			}
		}
		Ok(Arc::new(img))
	}

	pub fn gl_clear_color(r: f32, g: f32, b: f32, a: f32) {
		// SAFETY: 写当前上下文的 clear 颜色状态。
		unsafe {
			glClearColor(r, g, b, a);
		}
	}

	pub fn gl_clear() {
		// SAFETY: 清除当前绑定帧缓冲的颜色缓冲。
		unsafe {
			glClear(GL_COLOR_BUFFER_BIT);
		}
	}

	pub fn gl_version_string() -> Option<String> {
		// SAFETY: glGetString 返回当前上下文内驻字符串；拷贝成 Rust
		// String 后再返回，不持指针。
		unsafe {
			let p = glGetString(GL_VERSION);
			if p.is_null() {
				None
			} else {
				Some(std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
			}
		}
	}
}

/// 非 macOS：GL 桥 stub（Linux 预留 EGL、Windows 预留 WGL 的接线点；
/// 全部失败/false，use_opengl 决策回退 CPU）。
#[cfg(not(target_os = "macos"))]
mod imp {
	use super::*;

	/// Linux 接线点：替换为 EGL（eglGetDisplay → eglInitialize →
	/// eglChooseConfig → eglCreateContext → eglMakeCurrent）；Windows
	/// 替换为 WGL（wglCreateContext/wglMakeCurrent）。签名与 macOS
	/// 实现同构。
	pub struct GuardInner;

	pub fn gl_available() -> bool {
		false
	}

	pub fn is_current() -> bool {
		false
	}

	pub fn acquire() -> crate::error::Result<GlGuard> {
		Err(crate::error::Error::Failed("gl_bridge: 平台无 GL 实现（Linux/Windows stub）".into()))
	}

	pub fn create_output_texture(_w: i32, _h: i32, _p: &VideoParams) -> crate::error::Result<i32> {
		Err(crate::error::Error::Failed("gl_bridge: 平台无 GL 实现".into()))
	}

	pub fn create_input_texture(_w: i32, _h: i32, _p: &[u8]) -> crate::error::Result<i32> {
		Err(crate::error::Error::Failed("gl_bridge: 平台无 GL 实现".into()))
	}

	pub fn delete_gl_texture(_name: i32) {}

	pub fn create_fbo(_tex: i32, _w: i32, _h: i32) -> crate::error::Result<i32> {
		Err(crate::error::Error::Failed("gl_bridge: 平台无 GL 实现".into()))
	}

	pub fn bind_fbo(_fbo: i32) {}

	pub fn delete_fbo(_fbo: i32) {}

	pub fn set_viewport(_w: i32, _h: i32) {}

	pub fn read_pixels_to_image(
		_w: i32,
		_h: i32,
		_p: &VideoParams,
	) -> crate::error::Result<Arc<Image>> {
		Err(crate::error::Error::Failed("gl_bridge: 平台无 GL 实现".into()))
	}

	pub fn gl_clear_color(_r: f32, _g: f32, _b: f32, _a: f32) {}

	pub fn gl_clear() {}

	pub fn gl_version_string() -> Option<String> {
		None
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::image::{BitDepth, Components};
	use crate::render::VideoParams;

	/// GL 不可用时 skip（无 GPU 无头环境）；可用时返回 guard。
	///
	/// macOS 的真实 CGL 验收与集成 GL 测试同一门禁：CI 无头且用例并行，
	/// 默认只做可用性探测；设 `OAK_GPU_TESTS=1` 才跑真实 GL 用例
	/// （`crates/oak-plugin/tests/gl_render_test.rs` 同约定）。
	fn maybe_gl() -> Option<GlGuard> {
		#[cfg(target_os = "macos")]
		if std::env::var_os("OAK_GPU_TESTS").is_none() {
			println!("SKIP: 真实 GL 桥用例需 OAK_GPU_TESTS（本机 GPU 验收；CI 一律跳过）");
			return None;
		}
		match acquire() {
			Ok(g) => Some(g),
			Err(e) => {
				println!("SKIP: GL 上下文不可用：{e}");
				None
			}
		}
	}

	fn f32_params(w: i32, h: i32) -> VideoParams {
		let mut p = VideoParams::default();
		p.width = w;
		p.height = h;
		p.format = crate::render::PIXEL_FORMAT_F32;
		p
	}

	fn u8_params(w: i32, h: i32) -> VideoParams {
		let mut p = VideoParams::default();
		p.width = w;
		p.height = h;
		p.format = crate::render::PIXEL_FORMAT_U8;
		p
	}

	/// 真实 GL 上下文证据：acquire 后能取到非空版本串。
	#[test]
	fn gl_context_is_real() {
		let Some(_g) = maybe_gl() else {
			return;
		};
		let v = gl_version_string();
		assert!(v.is_some(), "真实 CGL 上下文应返回 GL_VERSION");
		let v = v.unwrap();
		// macOS 上 Apple 的 OpenGL 实现构建于 Metal 之上，GL_VERSION
		// 形如 "4.1 Metal - 90.5"（无头/有头皆然）——只断言非空。
		assert!(!v.trim().is_empty(), "GL_VERSION 应非空，got {v:?}");
		assert!(gl_available(), "上下文可用后 gl_available 应为 true");
	}

	/// 上下文单例与跨线程 current 串行：两条线程轮流 acquire 都能
	/// 拿到真实上下文（不崩、版本一致）。
	#[test]
	fn gl_context_thread_serialized() {
		let mut handles = Vec::new();
		for _ in 0..4 {
			handles.push(std::thread::spawn(|| {
				let Some(_g) = maybe_gl() else {
					return;
				};
				let v = gl_version_string().unwrap_or_default();
				assert!(!v.is_empty());
			}));
		}
		for h in handles {
			h.join().unwrap();
		}
	}

	/// 输出纹理 + FBO + clear + 回读 round-trip（F32 目标）：clear 到
	/// 已知颜色 → glReadPixels 读回，逐像素断言。
	#[test]
	fn fbo_clear_readback_f32() {
		let Some(_g) = maybe_gl() else {
			return;
		};
		let (w, h) = (4i32, 2i32);
		let params = f32_params(w, h);
		let tex = create_output_texture(w, h, &params).expect("输出纹理应可建");
		assert!(tex > 0, "GL 纹理名必须为真实非零 GL 名，got {tex}");
		let fbo = create_fbo(tex, w, h).expect("FBO 应完整");
		bind_fbo(fbo);
		set_viewport(w, h);
		gl_clear_color(0.1, 0.2, 0.3, 1.0);
		gl_clear();
		let img = read_pixels_to_image(w, h, &params).expect("回读应成功");
		delete_fbo(fbo);
		delete_gl_texture(tex);
		assert_eq!(img.components(), Components::Rgba);
		assert_eq!(img.depth(), BitDepth::Float);
		assert_eq!(img.bounds().x2, w as f64);
		assert_eq!(img.bounds().y2, h as f64);
		let px = &img.pixels()[0..16];
		let px_f: Vec<f32> = px.chunks_exact(4).map(|c| f32::from_ne_bytes(c.try_into().unwrap())).collect();
		assert!((px_f[0] - 0.1).abs() < 1e-6, "R={}", px_f[0]);
		assert!((px_f[1] - 0.2).abs() < 1e-6, "G={}", px_f[1]);
		assert!((px_f[2] - 0.3).abs() < 1e-6, "B={}", px_f[2]);
		assert!((px_f[3] - 1.0).abs() < 1e-6, "A={}", px_f[3]);
		// 整帧同色（clear 覆盖全 FBO）。
		let all: Vec<f32> = img.pixels()[..]
			.chunks_exact(4)
			.map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
			.collect();
		assert!(all.chunks(4).all(|px| (px[0] - 0.1).abs() < 1e-6), "整帧应为清屏色");
	}

	/// U8 目标：clear → GL_RGBA8 存储 → 回读归一化为 F32（容差内）。
	#[test]
	fn fbo_clear_readback_u8() {
		let Some(_g) = maybe_gl() else {
			return;
		};
		let (w, h) = (3i32, 3i32);
		let params = u8_params(w, h);
		let tex = create_output_texture(w, h, &params).expect("U8 输出纹理应可建");
		let fbo = create_fbo(tex, w, h).expect("FBO 应完整");
		bind_fbo(fbo);
		set_viewport(w, h);
		gl_clear_color(0.5, 0.25, 0.0, 1.0);
		gl_clear();
		let img = read_pixels_to_image(w, h, &params).expect("U8 回读应成功");
		delete_fbo(fbo);
		delete_gl_texture(tex);
		let px_f: Vec<f32> = img.pixels()[0..16]
			.chunks_exact(4)
			.map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
			.collect();
		// 0.5*255=127.5→128（round），128/255≈0.50196；0.25→64→0.25098。
		assert!((px_f[0] - 0.502).abs() < 0.01, "R={}", px_f[0]);
		assert!((px_f[1] - 0.251).abs() < 0.01, "G={}", px_f[1]);
		assert!(px_f[2].abs() < 0.01, "B={}", px_f[2]);
		assert!((px_f[3] - 1.0).abs() < 0.01, "A={}", px_f[3]);
	}

	/// 输入纹理上传：建 RGBA32F 纹理 + 上传已知像素，读回纹理内容
	/// （经 FBO 附着 + glReadPixels）与源像素一致。
	#[test]
	fn input_texture_upload_roundtrip() {
		let Some(_g) = maybe_gl() else {
			return;
		};
		let (w, h) = (2i32, 2i32);
		let mut px = Vec::new();
		for i in 0..(w * h) as usize {
			for c in 0..4 {
				px.extend_from_slice(&((i * 4 + c) as f32 * 0.1).to_ne_bytes());
			}
		}
		let tex = create_input_texture(w, h, &px).expect("输入纹理应可建");
		let params = f32_params(w, h);
		let fbo = create_fbo(tex, w, h).expect("FBO 应完整");
		bind_fbo(fbo);
		let img = read_pixels_to_image(w, h, &params).expect("回读应成功");
		delete_fbo(fbo);
		delete_gl_texture(tex);
		let got: Vec<f32> = img.pixels()[0..w as usize * h as usize * 16]
			.chunks_exact(4)
			.map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
			.collect();
		let want: Vec<f32> = px
			.chunks_exact(4)
			.map(|c| f32::from_ne_bytes(c.try_into().unwrap()))
			.collect();
		// 源是逐像素相异序列：read_pixels_to_image 垂直翻转（GL 左下 →
		// 帧左上）后，最终图像第 r 行 = 源第 (h-1-r) 行。
		let stride = w as usize * 4;
		for (i, g) in got.iter().enumerate() {
			let row = i / stride;
			let inv_row = h as usize - 1 - row;
			let expected = want[inv_row * stride + i % stride];
			assert!(
				(g - expected).abs() < 1e-6,
				"pixel {i}: got {g} want {expected}"
			);
		}
	}

	/// 同线程可重入 acquire：外层 guard 持有期再 acquire 不死锁，GL
	/// 调用仍可达；嵌套 guard drop 后上下文保持 current（外层仍有效），
	/// 外层 drop 后清 current。模拟 interact draw 的"一次 acquire 覆盖
	/// FBO 装配 → 插件绘制 → 回读"场景。
	#[test]
	fn acquire_is_reentrant_on_same_thread() {
		let Some(_outer) = maybe_gl() else {
			return;
		};
		// 嵌套 acquire（interact::draw 内部路径）。
		let inner = acquire().expect("嵌套 acquire 应成功（同线程重入）");
		let v1 = gl_version_string();
		assert!(v1.is_some(), "嵌套 guard 期间 GL 应仍 current");
		drop(inner);
		// 外层仍持有：GL 仍 current。
		let v2 = gl_version_string();
		assert!(v2.is_some(), "嵌套 guard drop 后外层仍应 current");
		// 再套一层（深度 2）验证深度计数不串。
		{
			let inner2 = acquire().expect("二次嵌套应成功");
			assert!(gl_version_string().is_some());
			drop(inner2);
		}
	}
}
