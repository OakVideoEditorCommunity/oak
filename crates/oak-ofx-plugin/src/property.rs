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

//! OFX 属性集（Property Suite 的宿主侧存储）。
//!
//! 每个 OFX 对象（host/plugin/descriptor/instance/clip/image/param）
//! 都挂一个 [`PropertySet`]。属性是多维数组，按类型存取。
//! 参照：HS: ofxhProperty.cpp（GetSuite 的读写语义：越界返回
//! kOfxStatErrBadIndex，类型不符返回 kOfxStatErrBadHandle）。

use std::sync::Mutex;

use crate::error::{Error, Result};

/// 属性值（单元素）。字符串以 OFX 的 `char*` 语义持有（host 拥有，
/// 插件借用）。
#[derive(Clone, Debug)]
pub enum Value {
	/// 32 位整型。
	Int(i32),
	/// 双精度。
	Double(f64),
	/// UTF-8 字符串（OFX 侧为 NUL 结尾 char*）。
	String(std::ffi::CString),
	/// 不透明指针（如 OfxImageEffectHandle 互指）。
	Pointer(*mut std::ffi::c_void),
}

// 裸指针默认禁 Send/Sync；但属性集语义把指针当作**不透明令牌**：
// 跨线程只搬运/比较值，解引用永远是 suite 层的 unsafe 责任（OFX
// multithread suite 本就要求插件线程可读写宿主图像缓冲——指针的
// 跨线程传递是规范语义，而非逃逸）。Mutex 包裹后属性集整体可共享。
// 与 Rust 安全模型不冲突：安全代码只能拿到 `&Value`，无法解引用指针。
unsafe impl Send for Value {}
unsafe impl Sync for Value {}

/// 一个属性：名字 + 多维值数组。
#[derive(Clone, Debug)]
pub struct Property {
	/// OFX 属性名（kOfxProp*；拥有型——协商期的动态名
	/// "OfxImageClipPropComponents_<clip>" 需要，`&'static str`
	/// 无法表达）。
	pub name: String,
	/// 元素数组；维度 = len。
	pub values: Vec<Value>,
}

/// 属性集。线程安全（内部 Mutex）；OFX 对象的 `*Handle` 即指向它的
/// 包装。
pub struct PropertySet {
	props: Mutex<Vec<Property>>,
}

/// 深拷贝（持锁克隆内部数组；createInstance 时描述符属性 → 实例
/// 属性需要）。
impl Clone for PropertySet {
	fn clone(&self) -> Self {
		let props = lock(&self.props);
		Self {
			props: Mutex::new(props.clone()),
		}
	}
}

impl PropertySet {
	/// 空集。
	pub fn new() -> Self {
		Self {
			props: Mutex::new(Vec::new()),
		}
	}

	/// 定义（或整体替换）一个属性。已存在同名属性时替换其值数组。
	///
	/// 对应 C++ `Set::addProperty` 的替换语义（HS: ofxhPropertySuite.cpp:462）
	/// 与 `PropertyTemplate::setValueN` 的整体写数组语义；属性不存在时
	/// 新建（维度和值都取自 `values`）。
	pub fn define(&self, name: &str, values: Vec<Value>) {
		let mut props = lock(&self.props);
		if let Some(p) = props.iter_mut().find(|p| p.name == name) {
			p.values = values;
		} else {
			props.push(Property {
				name: name.to_string(),
				values,
			});
		}
	}

	/// 单元素便捷定义。
	pub fn set_one(&self, name: &str, value: Value) {
		self.define(name, vec![value]);
	}

	/// 读取第 `index` 个元素；属性不存在或越界返回 `None`。
	///
	/// 越界对应 C++ 的 kOfxStatErrBadIndex
	/// （HS: ofxhPropertySuite.cpp:257 `getValueRaw`），此处以 Option 表达。
	pub fn get(&self, name: &str, index: usize) -> Option<Value> {
		let props = lock(&self.props);
		props
			.iter()
			.find(|p| p.name == name)
			.and_then(|p| p.values.get(index).cloned())
	}

	/// 覆盖第 `index` 个元素的值（不改变维度）；失败返回
	/// [`crate::error::Error::NotFound`]。
	///
	/// 维度固定是刻意为之：与 C++ `setValue` 的自动扩容不同，这里
	/// 扩容只能经 [`PropertySet::define`] 显式进行（维度语义由定义方
	/// 掌控，避免插件意外撑大数组）。
	pub fn set_at(&self, name: &str, index: usize, value: Value) -> Result<()> {
		let mut props = lock(&self.props);
		let p = props
			.iter_mut()
			.find(|p| p.name == name)
			.ok_or(Error::NotFound)?;
		let slot = p.values.get_mut(index).ok_or(Error::NotFound)?;
		*slot = value;
		Ok(())
	}

	/// 维度（属性不存在为 0）。
	pub fn dimension(&self, name: &str) -> usize {
		lock(&self.props)
			.iter()
			.find(|p| p.name == name)
			.map_or(0, |p| p.values.len())
	}

	/// 删除属性；不存在为 no-op。
	pub fn remove(&self, name: &str) {
		lock(&self.props).retain(|p| p.name != name);
	}

	/// 遍历快照（dump/快照测试用）。
	pub fn snapshot(&self) -> Vec<Property> {
		lock(&self.props).clone()
	}

	/// suite 层专用：持锁访问原始存储。
	///
	/// 属性 suite 需要"先类型检查再按索引读写、返回内驻字符串指针"
	/// 等跨多次读写的一致性语义（逐条对照 HS: ofxhProperty.cpp），
	/// 公开 API 无法在不重复加锁的前提下表达；此入口把 `Vec<Property>`
	/// 在单一临界区内交给 suite 实现。仅本 crate 可见。
	pub(crate) fn with_locked<R>(&self, f: impl FnOnce(&mut Vec<Property>) -> R) -> R {
		let mut props = lock(&self.props);
		f(&mut props)
	}
}

/// 取锁。毒锁（本 crate 代码在持锁时 panic）时接管其内部状态继续
/// 使用——属性集的数据本身总是完好的，宁可继续也不让一次 panic
/// 级联成后续所有 FFI 调用失败。
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
	m.lock().unwrap_or_else(|e| e.into_inner())
}
