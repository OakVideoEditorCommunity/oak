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

//! 进度上报（Progress suite 的宿主侧）。
//!
//! 对应 C++ 的 `PluginProgressReporter`。进度/取消经 facade 注册的
//! 回调出 crate（M9 的 facade 回调模式）。取消是粘滞的：一旦回调
//! 答 false，本报告器的 [`is_cancelled`]（image effect suite 的
//! abort 查询）持续为真。
//!
//! ## app 注入点（阶段 6a）
//!
//! facade C 回调之外，app 可经 [`set_reporter_factory`] 注册一个
//! 工厂：渲染未装 C 回调时，Progress suite 的 progressStart 携
//! (label, message) 从工厂现造一个 [`UiProgressReporter`]（如进度
//! 对话框），progressUpdate 转发给它，返回 false 即取消。对应 C++
//! `PluginProgressDialogReporter` 的创建路径。

use std::ffi::c_int;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// 进度 UI 报告器（app 实现：对话框、状态栏等）。`update` 返回
/// false 表示用户请求取消（映射 kOfxStatReplyNo）。
pub trait UiProgressReporter: Send {
	/// 上报进度（0.0..=1.0）；false = 取消。
	fn update(&mut self, progress: f64) -> bool;
	/// progressEnd 通知（默认 no-op；app 可据此关闭进度 UI / 把完成
	/// 事件转发过 IPC——worker 侧的进度回传依赖它）。
	fn end(&mut self) {}
}

/// 报告器工厂：progressStart 携 (label, message) 调用，现造一个
/// [`UiProgressReporter`]。
pub type ReporterFactory =
	Arc<dyn Fn(&str, &str) -> Box<dyn UiProgressReporter> + Send + Sync>;

static REPORTER_FACTORY: OnceLock<Mutex<Option<ReporterFactory>>> = OnceLock::new();

fn factory_slot() -> &'static Mutex<Option<ReporterFactory>> {
	REPORTER_FACTORY.get_or_init(|| Mutex::new(None))
}

/// 注册/清除进度 UI 工厂（app 接线点；覆盖式）。
pub fn set_reporter_factory(factory: Option<ReporterFactory>) {
	*factory_slot().lock().unwrap_or_else(|e| e.into_inner()) = factory;
}

pub(crate) fn reporter_factory() -> Option<ReporterFactory> {
	factory_slot()
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.clone()
}

/// 是否已注册 UI 工厂（render 路径据此决定是否装静默报告器）。
pub fn has_reporter_factory() -> bool {
	reporter_factory().is_some()
}

/// 进度回调：签名与 `include/plugin/instance.h` 的
/// `oakplugin_progress_fn` 逐字一致——`(progress, userdata)`，
/// 返回非 0 表示应中止（骨架声明为 `(userdata, progress) -> bool`，
/// 与头文件不符，以此为准）。
pub type ProgressFn = unsafe extern "C" fn(progress: f64, userdata: *mut std::ffi::c_void) -> c_int;

/// 进度上报器。render 路径持有一份，Progress suite 的
/// progressStart/Update/End 转发到这里。
pub struct ProgressReporter {
	callback: Option<ProgressFn>,
	userdata: usize,
	/// UI 报告器（progressStart 经 [`reporter_factory`] 现造；
	/// C 回调优先，无回调时才用）。
	ui: Mutex<Option<Box<dyn UiProgressReporter>>>,
	/// 取消标志（粘滞；update 返回 false 时置位）。
	cancelled: AtomicBool,
}

impl ProgressReporter {
	/// 无回调（渲染静默进行；progressStart 仍可能经工厂装上 UI
	/// 报告器）。
	pub fn silent() -> Self {
		Self {
			callback: None,
			userdata: 0,
			ui: Mutex::new(None),
			cancelled: AtomicBool::new(false),
		}
	}

	/// 带回调。
	///
	/// # Safety
	/// `userdata` 的生命周期由注册方保证。
	pub unsafe fn new(callback: ProgressFn, userdata: *mut std::ffi::c_void) -> Self {
		Self {
			callback: Some(callback),
			// usize 存（裸指针破坏 Send 推导；值语义不变）。
			userdata: userdata as usize,
			ui: Mutex::new(None),
			cancelled: AtomicBool::new(false),
		}
	}

	/// progressStart 钩子：无 C 回调且工厂已注册时，现造 UI 报告器
	/// （已装过则不重复造——progressStart 可嵌套括号）。
	pub(crate) fn install_ui(&self, label: &str, message: &str) {
		if self.callback.is_some() {
			return;
		}
		let mut slot = self.ui.lock().unwrap_or_else(|e| e.into_inner());
		if slot.is_some() {
			return;
		}
		if let Some(factory) = reporter_factory() {
			*slot = Some(factory(label, message));
		}
	}

	/// 报告进度；返回 false 表示应取消（映射
	/// kOfxStatReplyNo/action 失败由调用点决定）。
	pub fn update(&self, progress: f64) -> bool {
		match self.callback {
			Some(cb) => {
				// 头文件契约：非 0 = 中止。
				let abort = unsafe { cb(progress, self.userdata as *mut std::ffi::c_void) } != 0;
				if abort {
					self.cancelled.store(true, Ordering::Relaxed);
				}
				!abort
			}
			None => {
				let mut slot = self.ui.lock().unwrap_or_else(|e| e.into_inner());
				match slot.as_mut() {
					Some(ui) => {
						let keep_going = ui.update(progress);
						if !keep_going {
							self.cancelled.store(true, Ordering::Relaxed);
						}
						keep_going
					}
					None => true,
				}
			}
		}
	}

	/// 本次渲染是否已被取消（image effect suite 的 abort 透传）。
	pub(crate) fn is_cancelled(&self) -> bool {
		self.cancelled.load(Ordering::Relaxed)
	}

	/// progressEnd 钩子：转发给 UI 报告器（无报告器时 no-op）。
	pub(crate) fn end_ui(&self) {
		let mut slot = self.ui.lock().unwrap_or_else(|e| e.into_inner());
		if let Some(ui) = slot.as_mut() {
			ui.end();
		}
	}
}
