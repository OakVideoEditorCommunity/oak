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

//! handle.rs 的契约测试：`RefBox` 容器。
//!
//! 单库化（M14 R5）后 crate 内部不再传 CHandle——原 CHandle 装拆
//! （`make_owned`/`make_borrowed`/`get`）、panic 兜底（`guard`/
//! `guard_handle`/`guard_void`）与身份注册表（`Registry`）均已删除。
//! [`oak_ofx_plugin::handle::RefBox`] 仅作为 `Host::create_instance` 的
//! 边界返回类型保留（`Arc<RefBox<Instance>>`，oakengine test_support
//! 消费）；此处验证其基本契约。每个测试只验一条规则，命名即规约。

mod common;

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use oak_ofx_plugin::handle::RefBox;

/// 析构标志：以"被析构次数"断言对象的销毁时机（Arc 生命周期语义的
/// 行为探针）。
struct DropFlag(Arc<AtomicUsize>);

impl Drop for DropFlag {
	fn drop(&mut self) {
		self.0.fetch_add(1, Ordering::Relaxed);
	}
}

/// 值可经 `.value` 字段取回（create_instance 返回后调用方的访问方式）。
#[test]
fn refbox_exposes_value_by_field() {
	let boxed = RefBox {
		refs: AtomicU32::new(1),
		value: 7u32,
	};
	assert_eq!(boxed.value, 7);
}

/// 生命周期完全由外层 Arc 管理：`refs` 只以固定值 1 构造（无 thunk
/// 增减），Arc 归零时内含对象恰好析构一次；弱引用在 Arc 存活期间
/// 有效、归零后失效（host 的活跃实例表即 Weak 列表）。
#[test]
fn refbox_value_drops_exactly_once_when_arc_reaches_zero() {
	let drops = Arc::new(AtomicUsize::new(0));
	let boxed = Arc::new(RefBox {
		refs: AtomicU32::new(1),
		value: DropFlag(drops.clone()),
	});

	let weak = Arc::downgrade(&boxed);
	assert!(weak.upgrade().is_some());
	assert_eq!(drops.load(Ordering::Relaxed), 0);

	drop(boxed);
	assert_eq!(drops.load(Ordering::Relaxed), 1);
	assert!(weak.upgrade().is_none());
}
