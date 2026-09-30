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

//! 句柄机制的历史遗留：`RefBox<T>` 容器。
//!
//! 单库化（M14 R5）后 crate 内部不再传 CHandle——原 CHandle 装拆
//! （`make_owned`/`make_borrowed`/`get`）、panic 兜底（`guard`/
//! `guard_handle`/`guard_void`）与身份注册表（`Registry`）均已删除：
//! 前者只被测试使用，后者在 src 无读取方（param 桥实际走
//! [`crate::suites::param`] 的 props 地址映射与 [`crate::node`] 的
//! 身份注册表）。
//!
//! 仅 [`RefBox`] 保留：它是 [`crate::host::Host::create_instance`] 的
//! 返回值容器（`Arc<RefBox<Instance>>`），该类型被 oakengine 的
//! test_support 显式标注消费，是本 crate 在 facade 边界的公开类型。

use std::sync::atomic::AtomicU32;

/// 边界盒：`Arc<RefBox<Instance>>` 的承载类型。
///
/// 原为 C 句柄（`{ctx, addref, release, abi_version}`）背后的堆盒子，
/// addref/release thunk 经 `refs` 计数；装拆删除后 `refs` 不再被读写，
/// 仅以固定值 1 构造（"单个拥有者"语义），生命周期完全由外层
/// `Arc` 管理。
pub struct RefBox<T: ?Sized> {
	/// 引用计数（句柄时代遗留，现无 thunk 读写）。
	pub refs: AtomicU32,
	/// 内含对象。
	pub value: T,
}
