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

//! 错误码。与 `include/*/error.h` 逐字对应；项目统一 -MMCCCC 方案
//! （模块号注册表见 include/common/error.h），跨模块透传不翻译。

use thiserror::Error;

/// 成功。
pub const OAKPLUGIN_OK: i32 = 0;
/// 空句柄或非法参数。
pub const OAKPLUGIN_E_INVALID: i32 = -90001;
/// 当前状态不允许该调用（如未扫描就创建实例）。
pub const OAKPLUGIN_E_STATE: i32 = -90002;
/// 底层操作失败（OFX action 返回非 kOfxStatOK、插件入口拒绝等）。
pub const OAKPLUGIN_E_FAILED: i32 = -90003;
/// 索引越界 / 指定标识的插件不存在。
pub const OAKPLUGIN_E_NOT_FOUND: i32 = -90004;
/// 分配失败。
pub const OAKPLUGIN_E_NOMEM: i32 = -90005;

/// crate 内部统一的结果类型；FFI 层把它映射为上述 i32 码。
pub type Result<T> = std::result::Result<T, Error>;

/// crate 内部错误。`code()` 给出对外错误码。
#[derive(Debug, Error)]
pub enum Error {
	/// 空句柄或非法参数。
	#[error("plugin: invalid argument")]
	Invalid,
	/// 状态不允许。
	#[error("plugin: call not valid in the current state")]
	State,
	/// 底层失败，附人类可读上下文（仅日志，不出界）。
	#[error("plugin: operation failed: {0}")]
	Failed(String),
	/// 未找到。
	#[error("plugin: not found")]
	NotFound,
	/// 分配失败。
	#[error("plugin: out of memory")]
	NoMem,
}

impl Error {
	/// 映射为 `include/plugin/error.h` 的错误码。
	pub fn code(&self) -> i32 {
		match self {
			Error::Invalid => OAKPLUGIN_E_INVALID,
			Error::State => OAKPLUGIN_E_STATE,
			Error::Failed(_) => OAKPLUGIN_E_FAILED,
			Error::NotFound => OAKPLUGIN_E_NOT_FOUND,
			Error::NoMem => OAKPLUGIN_E_NOMEM,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// One instance of every variant (data-carrying ones get a sample
	/// payload).
	fn all_errors() -> Vec<Error> {
		vec![
			Error::Invalid,
			Error::State,
			Error::Failed("boom".to_string()),
			Error::NotFound,
			Error::NoMem,
		]
	}

	#[test]
	fn display_is_non_empty_for_every_variant() {
		for e in all_errors() {
			let s = e.to_string();
			assert!(!s.is_empty(), "Display produced an empty message for {e:?}");
		}
	}

	#[test]
	fn error_is_object_safe() {
		// `Box<dyn std::error::Error>` must be constructible for every
		// variant; `source()` stays None (no wrapped downstream error).
		let errors: Vec<Box<dyn std::error::Error>> = all_errors()
			.into_iter()
			.map(|e| Box::new(e) as Box<dyn std::error::Error>)
			.collect();
		for e in &errors {
			assert!(!e.to_string().is_empty());
			assert!(e.source().is_none());
		}
	}

	#[test]
	fn code_is_unaffected_by_trait_impl() {
		assert_eq!(Error::Invalid.code(), OAKPLUGIN_E_INVALID);
		assert_eq!(Error::State.code(), OAKPLUGIN_E_STATE);
		assert_eq!(Error::Failed("boom".to_string()).code(), OAKPLUGIN_E_FAILED);
		assert_eq!(Error::NotFound.code(), OAKPLUGIN_E_NOT_FOUND);
		assert_eq!(Error::NoMem.code(), OAKPLUGIN_E_NOMEM);
	}
}
