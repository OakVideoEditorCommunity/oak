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

//! OfxTimeLineSuite v1：时间查询转发到当前渲染上下文
//! （[`crate::suites::RenderCtx`]，frame range 来自 clip 桥）。
//! 参照 HS: ofxhImageEffect.cpp gTimelineSuite。
//!
//! 无渲染上下文（渲染外调用）→ 回退 app 注入的活动 viewer 时间源
//! （[`set_active_viewer_provider`]，阶段 6a 注入点）；也未注入则
//! 时间 0 / 时间域 (0,0) 的 headless 默认。gotoTime 第 1 期无时间线
//! 驱动 → OK no-op（渲染时间由驱动固定，见
//! [`crate::suites::set_render_ctx`]）。

use std::ffi::{c_double, c_int, c_void};
use std::sync::{Arc, Mutex, OnceLock};

use crate::suites::{render_ctx, status};

// ---------------------------------------------------------------------------
// app 注入点：活动 viewer 时间源
// ---------------------------------------------------------------------------

/// 活动 viewer 的时间信息（timeline suite 渲染外回退源的最小集）。
#[derive(Clone, Copy, Debug)]
pub struct ViewerTimeInfo {
	/// viewer 当前时间（秒）。
	pub time: f64,
	/// 时间域下界（秒）。
	pub range_min: f64,
	/// 时间域上界（秒）。
	pub range_max: f64,
}

/// 活动 viewer 提供器：返回 None 表示当前无活动 viewer。
pub type ActiveViewerProvider = Arc<dyn Fn() -> Option<ViewerTimeInfo> + Send + Sync>;

static ACTIVE_VIEWER: OnceLock<Mutex<Option<ActiveViewerProvider>>> = OnceLock::new();

fn viewer_slot() -> &'static Mutex<Option<ActiveViewerProvider>> {
	ACTIVE_VIEWER.get_or_init(|| Mutex::new(None))
}

/// 注册/清除活动 viewer 提供器（app 接线点；覆盖式）。
pub fn set_active_viewer_provider(provider: Option<ActiveViewerProvider>) {
	*viewer_slot().lock().unwrap_or_else(|e| e.into_inner()) = provider;
}

fn active_viewer() -> Option<ViewerTimeInfo> {
	let provider = viewer_slot()
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.clone()?;
	provider()
}

/// 函数表布局（OfxTimeLineSuiteV1）。
#[repr(C)]
pub struct TimeLineSuiteV1 {
	/// getTime
	pub get_time: unsafe extern "C" fn(*mut c_void, *mut c_double) -> c_int,
	/// gotoTime
	pub goto_time: unsafe extern "C" fn(*mut c_void, c_double) -> c_int,
	/// getTimeBounds
	pub get_time_bounds: unsafe extern "C" fn(*mut c_void, *mut c_double, *mut c_double) -> c_int,
}

/// 公共入口模板：handle 空检查 + panic 兜底。
unsafe fn caught(handle: *mut c_void, f: impl FnOnce() -> c_int) -> c_int {
	std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
		if handle.is_null() {
			return status::ERR_BAD_HANDLE;
		}
		f()
	}))
	.unwrap_or(status::FAILED)
}

unsafe extern "C" fn timeline_get_time(handle: *mut c_void, time: *mut c_double) -> c_int {
	unsafe {
		caught(handle, || {
			if time.is_null() {
				return status::ERR_VALUE;
			}
			// 渲染上下文 → 活动 viewer → 0（headless 默认）。
			*time = render_ctx()
				.map(|c| c.time)
				.or_else(|| active_viewer().map(|v| v.time))
				.unwrap_or(0.0);
			status::OK
		})
	}
}

/// gotoTime：第 1 期宿主不随插件移动时间线（渲染时间由驱动固定），
/// OK no-op。
unsafe extern "C" fn timeline_goto_time(handle: *mut c_void, _time: c_double) -> c_int {
	unsafe { caught(handle, || status::OK) }
}

unsafe extern "C" fn timeline_get_time_bounds(
	handle: *mut c_void,
	min: *mut c_double,
	max: *mut c_double,
) -> c_int {
	unsafe {
		caught(handle, || {
			if min.is_null() || max.is_null() {
				return status::ERR_VALUE;
			}
			// 渲染上下文 → 活动 viewer → (0,0)（headless 默认）。
			let r = render_ctx()
				.map(|c| (c.range.min, c.range.max))
				.or_else(|| active_viewer().map(|v| (v.range_min, v.range_max)))
				.unwrap_or((0.0, 0.0));
			*min = r.0;
			*max = r.1;
			status::OK
		})
	}
}

/// 静态函数表实例。
pub fn suite_v1() -> &'static TimeLineSuiteV1 {
	static SUITE: std::sync::OnceLock<TimeLineSuiteV1> = std::sync::OnceLock::new();
	SUITE.get_or_init(|| TimeLineSuiteV1 {
		get_time: timeline_get_time,
		goto_time: timeline_goto_time,
		get_time_bounds: timeline_get_time_bounds,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::instance::{OfxRangeD, RenderScale};
	use crate::suites::{set_render_ctx, RenderCtx};

	#[test]
	fn queries_forward_to_render_ctx() {
		let s = suite_v1();
		let handle = 0x10usize as *mut c_void;
		let mut t = 0.0;
		let mut min = 0.0;
		let mut max = 0.0;

		// 无上下文：0 / (0,0)。
		unsafe {
			assert_eq!((s.get_time)(handle, &mut t), status::OK);
			assert_eq!(t, 0.0);
			assert_eq!((s.get_time_bounds)(handle, &mut min, &mut max), status::OK);
			assert_eq!((min, max), (0.0, 0.0));
		}

		// 有上下文：转发。
		set_render_ctx(Some(RenderCtx {
			time: 42.5,
			scale: RenderScale { x: 1.0, y: 1.0 },
			range: OfxRangeD {
				min: 10.0,
				max: 200.0,
			},
		}));
		unsafe {
			assert_eq!((s.get_time)(handle, &mut t), status::OK);
			assert_eq!(t, 42.5);
			assert_eq!((s.get_time_bounds)(handle, &mut min, &mut max), status::OK);
			assert_eq!((min, max), (10.0, 200.0));
			assert_eq!((s.goto_time)(handle, 99.0), status::OK);
		}
		set_render_ctx(None);

		// 空 handle / 空 out。
		unsafe {
			assert_eq!(
				(s.get_time)(std::ptr::null_mut(), &mut t),
				status::ERR_BAD_HANDLE
			);
			assert_eq!(
				(s.get_time)(handle, std::ptr::null_mut()),
				status::ERR_VALUE
			);
		}
	}
}
