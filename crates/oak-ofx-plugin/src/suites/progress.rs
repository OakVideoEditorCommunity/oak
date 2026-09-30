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

//! OfxProgressSuite v1/v2：进度 → 当前渲染的
//! [`crate::progress::ProgressReporter`]。
//!
//! 定位方式：TLS（render 驱动在调用插件 action 前
//! [`set_current`]，action 返回后清除）——进度只在渲染期内有意义，
//! 不设全局状态（progress.rs 文档）。
//! 语义：update 返回 false（取消）→ kOfxStatReplyNo
//! （HS: ofxhImageEffect.cpp gProgressSuite）；无报告器（渲染外
//! 调用）→ 静默 OK。start/end 为括号，OK。

use std::ffi::{c_char, c_double, c_int, c_void};

use crate::progress::ProgressReporter;
use crate::suites::status;

thread_local! {
	static CURRENT: std::cell::RefCell<Option<ProgressReporter>> = const { std::cell::RefCell::new(None) };
}

/// 设置/清除当前报告器（render 驱动调用；公开：render 驱动在
/// host 层，测试也直接注入）。
pub fn set_current(reporter: Option<ProgressReporter>) {
	CURRENT.with(|c| *c.borrow_mut() = reporter);
}

/// 读取当前报告器的取消状态（image effect suite 的 abort 用）。
pub(crate) fn is_cancelled() -> bool {
	CURRENT.with(|c| c.borrow().as_ref().is_some_and(|r| r.is_cancelled()))
}

/// 进度更新：取消 → kOfxStatReplyNo；无报告器/未取消 → OK。
fn progress_update(progress: f64) -> c_int {
	let cancelled = CURRENT.with(|c| match c.borrow().as_ref() {
		Some(r) => !r.update(progress),
		None => false,
	});
	if cancelled {
		status::REPLY_NO
	} else {
		status::OK
	}
}

/// 公共入口模板：panic 兜底。
fn caught(f: impl FnOnce() -> c_int) -> c_int {
	std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(status::FAILED)
}

/// 函数表布局（OfxProgressSuiteV1）。
#[repr(C)]
pub struct ProgressSuiteV1 {
	/// progressStart
	pub start: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int,
	/// progressUpdate（返回非 kOfxStatOK 即取消）
	pub update: unsafe extern "C" fn(*mut c_void, c_double) -> c_int,
	/// progressEnd
	pub end: unsafe extern "C" fn(*mut c_void) -> c_int,
}

/// 函数表布局（OfxProgressSuiteV2：start 带 label + message）。
#[repr(C)]
pub struct ProgressSuiteV2 {
	/// progressStart
	pub start: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
	/// progressUpdate
	pub update: unsafe extern "C" fn(*mut c_void, c_double) -> c_int,
	/// progressEnd
	pub end: unsafe extern "C" fn(*mut c_void) -> c_int,
}

/// C 字符串解码（空指针/非法 UTF-8 → 空串）。
unsafe fn decode<'a>(p: *const c_char) -> &'a str {
	if p.is_null() {
		return "";
	}
	unsafe { std::ffi::CStr::from_ptr(p) }
		.to_str()
		.unwrap_or("")
}

/// progressStart：括号起点；携 label（v2 另携 message）经
/// [`crate::progress`] 工厂现造 UI 报告器（无工厂/已有报告器时
/// no-op），状态恒 OK。
unsafe extern "C" fn progress_start_v1(_handle: *mut c_void, label: *const c_char) -> c_int {
	caught(|| {
		let label = unsafe { decode(label) }.to_string();
		CURRENT.with(|c| {
			if let Some(r) = c.borrow().as_ref() {
				r.install_ui(&label, "");
			}
		});
		status::OK
	})
}

unsafe extern "C" fn progress_update_v1(_handle: *mut c_void, progress: c_double) -> c_int {
	caught(|| progress_update(progress))
}

unsafe extern "C" fn progress_end_v1(_handle: *mut c_void) -> c_int {
	caught(|| {
		CURRENT.with(|c| {
			if let Some(r) = c.borrow().as_ref() {
				r.end_ui();
			}
		});
		status::OK
	})
}

unsafe extern "C" fn progress_start_v2(
	_handle: *mut c_void,
	label: *const c_char,
	message: *const c_char,
) -> c_int {
	caught(|| {
		let label = unsafe { decode(label) }.to_string();
		let message = unsafe { decode(message) }.to_string();
		CURRENT.with(|c| {
			if let Some(r) = c.borrow().as_ref() {
				r.install_ui(&label, &message);
			}
		});
		status::OK
	})
}

unsafe extern "C" fn progress_update_v2(_handle: *mut c_void, progress: c_double) -> c_int {
	caught(|| progress_update(progress))
}

unsafe extern "C" fn progress_end_v2(_handle: *mut c_void) -> c_int {
	caught(|| status::OK)
}

/// 静态函数表实例（v1）。
pub fn suite_v1() -> &'static ProgressSuiteV1 {
	static SUITE: std::sync::OnceLock<ProgressSuiteV1> = std::sync::OnceLock::new();
	SUITE.get_or_init(|| ProgressSuiteV1 {
		start: progress_start_v1,
		update: progress_update_v1,
		end: progress_end_v1,
	})
}

/// 静态函数表实例（v2）。
pub fn suite_v2() -> &'static ProgressSuiteV2 {
	static SUITE: std::sync::OnceLock<ProgressSuiteV2> = std::sync::OnceLock::new();
	SUITE.get_or_init(|| ProgressSuiteV2 {
		start: progress_start_v2,
		update: progress_update_v2,
		end: progress_end_v2,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// 取消型出口（头文件契约：非 0 = 中止）。
	unsafe extern "C" fn cancel_cb(_p: f64, _userdata: *mut c_void) -> c_int {
		1
	}

	/// 静默：update 恒 OK；带取消回调：update → REPLY_NO 且 abort
	/// 查询为真（粘滞）。
	#[test]
	fn update_cancel_and_abort() {
		// 无报告器（渲染外）：静默 OK。
		set_current(None);
		let s = suite_v1();
		unsafe {
			assert_eq!((s.update)(std::ptr::null_mut(), 0.5), status::OK);
			assert_eq!(
				(s.start)(std::ptr::null_mut(), std::ptr::null()),
				status::OK
			);
			assert_eq!((s.end)(std::ptr::null_mut()), status::OK);
		}

		// 带取消回调：update → REPLY_NO；is_cancelled 为真。
		set_current(Some(unsafe {
			ProgressReporter::new(cancel_cb, std::ptr::null_mut())
		}));
		unsafe {
			assert_eq!((s.update)(std::ptr::null_mut(), 0.5), status::REPLY_NO);
		}
		assert!(is_cancelled());
		set_current(None);
		assert!(!is_cancelled());
	}
}
