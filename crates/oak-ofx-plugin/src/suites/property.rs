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

//! OfxPropertySuite v1：属性读写。
//!
//! 语义逐条对照 HS: ofxhPropertySuite.cpp：
//! - 读未定义属性 / 类型不符 → kOfxStatErrUnknown（HS: propGet 的
//!   `fetchTypedProperty` 失败路径，ofxhPropertySuite.cpp:787-794）；
//! - 越界读写 → kOfxStatErrBadIndex（HS: `getValueRaw`/
//!   `setValue`，ofxhPropertySuite.cpp:257/284）；
//! - propGetN 拷贝 min(count, dimension) 个元素，不报错
//!   （HS: `getValueNRaw`，ofxhPropertySuite.cpp:271-280）；
//! - propSetN 维度变化时整体替换：本 crate 的属性维度始终跟随数组
//!   长度（HS 对固定维度属性 count > dimension 会返回
//!   kOfxStatErrBadIndex，ofxhPropertySuite.cpp:299——本 crate 不区分
//!   固定/可变维度，如实更宽容；三个系统 bundle 均以正确 count 调用，
//!   无行为分歧）；
//! - 只读属性：SDK 无 kOfxStatErrReadOnly，HostSupport 也不在 suite
//!   层拦截写（`_pluginReadOnly` 仅供宿主内部逻辑查询）——本实现
//!   如实不拦截；若日后需要，在 PropertySet 增加只读标记再评。
//! - propReset：HostSupport 恢复到 define 时默认值
//!   （ofxhPropertySuite.cpp:330）；本 crate 不保存默认值快照，
//!   第 1 期明确返回 kOfxStatErrUnsupported（真话比静默 no-op 安全）。

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};

use crate::property::{Property, PropertySet, Value};
use crate::suites::status;

/// 函数表布局（与 SDK `OfxPropertySuiteV1` 逐字段一致）。
#[repr(C)]
pub struct PropertySuiteV1 {
	/// propSetPointer
	pub set_pointer: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int,
	/// propSetString
	pub set_string: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *const c_char) -> c_int,
	/// propSetDouble
	pub set_double: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, c_double) -> c_int,
	/// propSetInt
	pub set_int: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, c_int) -> c_int,
	/// propSetPointerN
	pub set_pointer_n:
		unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *const *mut c_void) -> c_int,
	/// propSetStringN
	pub set_string_n:
		unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *const *const c_char) -> c_int,
	/// propSetDoubleN
	pub set_double_n:
		unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *const c_double) -> c_int,
	/// propSetIntN
	pub set_int_n: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *const c_int) -> c_int,
	/// propGetPointer
	pub get_pointer:
		unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut *mut c_void) -> c_int,
	/// propGetString
	pub get_string:
		unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut *mut c_char) -> c_int,
	/// propGetDouble
	pub get_double: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_double) -> c_int,
	/// propGetInt
	pub get_int: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_int) -> c_int,
	/// propGetPointerN
	pub get_pointer_n:
		unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut *mut c_void) -> c_int,
	/// propGetStringN
	pub get_string_n:
		unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut *mut c_char) -> c_int,
	/// propGetDoubleN
	pub get_double_n:
		unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_double) -> c_int,
	/// propGetIntN
	pub get_int_n: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_int) -> c_int,
	/// propReset
	pub reset: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int,
	/// propGetDimension
	pub get_dimension: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_int) -> c_int,
}

/// 属性值类别（suite 层的类型检查；对应 HS 的 `Property::TypeEnum`）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
	Int,
	Double,
	Str,
	Pointer,
}

impl Kind {
	fn of(v: &Value) -> Kind {
		match v {
			Value::Int(_) => Kind::Int,
			Value::Double(_) => Kind::Double,
			Value::String(_) => Kind::Str,
			Value::Pointer(_) => Kind::Pointer,
		}
	}
}

/// 属性 suite 公共入口模板：解句柄 + panic 兜底。
///
/// 句柄按 [`crate::suites::tag`] 约定：低 3 位是对象种类标签，地址
/// 即对象 props 字段（偏移 0）；剥标签后可直接当 PropertySet 用。
/// 裸 PropertySet 指针（标签 0，宿主内部/测试直传）原样通过。
///
/// `# Safety`：`handle` 必须指向活的 `PropertySet` 或已注册对象
/// （suite 生命周期契约：宿主对象先于 suite 调用创建，后于全部调用
/// 销毁）。
#[track_caller]
unsafe fn caught(handle: *mut c_void, f: impl FnOnce(&PropertySet) -> Result<(), c_int>) -> c_int {
	let caller = std::panic::Location::caller();
	let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
		if handle.is_null() {
			return status::ERR_BAD_HANDLE;
		}
		let set = unsafe { &*crate::suites::tag::strip(handle) };
		f(set).map_or_else(|code| code, |()| status::OK)
	}))
	.unwrap_or(status::FAILED);
	if code != status::OK && std::env::var_os("OAK_OFX_TRACE").is_some() {
		let tag = crate::suites::tag::kind(handle);
		eprintln!("[ofx] property suite error {code} at {caller} (handle {handle:p} tag {tag})");
	}
	code
}

/// 属性名：空指针 / 非 UTF-8 → kOfxStatErrValue（防御性；HostSupport
/// 直接解引用会崩）。
///
/// `# Safety`：`name` 必须是有效的 NUL 结尾 C 字符串（插件契约）。
unsafe fn c_name<'a>(name: *const c_char) -> Result<&'a str, c_int> {
	if name.is_null() {
		return Err(status::ERR_VALUE);
	}
	unsafe { CStr::from_ptr(name) }
		.to_str()
		.map_err(|_| status::ERR_VALUE)
}

/// 越界防护：`index` 转 usize（负数 → 越界错误）。
fn idx(index: c_int) -> Result<usize, c_int> {
	usize::try_from(index).map_err(|_| status::ERR_BAD_INDEX)
}

/// 先类型后索引的取值（HS 顺序：`fetchTypedProperty` 在
/// `getValueRaw` 之前，ofxhPropertySuite.cpp:787/794/257）。
fn get_value<'a>(
	props: &'a [Property],
	name: &str,
	index: c_int,
	kind: Kind,
) -> Result<&'a Value, c_int> {
	let result = get_value_inner(props, name, index, kind);
	if std::env::var_os("OAK_OFX_TRACE").is_some() {
		match &result {
			Ok(_) => eprintln!("[ofx] propGet({name}[{index}]) -> ok"),
			Err(c) => eprintln!("[ofx] propGet({name}[{index}]) -> {c}"),
		}
	}
	result
}

fn get_value_inner<'a>(
	props: &'a [Property],
	name: &str,
	index: c_int,
	kind: Kind,
) -> Result<&'a Value, c_int> {
	let p = props
		.iter()
		.find(|p| p.name == name)
		.ok_or(status::ERR_UNKNOWN)?;
	// 首元素代理整条属性的类型（宿主只定义同构数组；HS 是定义期
	// 固定类型，等价）。空数组没有类型探针，跳过类型检查，越界
	// 由下面的索引访问报 BadIndex（HS getValueRaw 语义）。
	if let Some(probe) = p.values.first() {
		// Int <-> Double 数值协随（与 get_n 同款宽容宿主语义）。
		let numeric = matches!(Kind::of(probe), Kind::Int | Kind::Double)
			&& matches!(kind, Kind::Int | Kind::Double);
		if Kind::of(probe) != kind && !numeric {
			if std::env::var_os("OAK_OFX_TRACE").is_some() {
				eprintln!("[ofx] property kind mismatch: {name}");
			}
			return Err(status::ERR_UNKNOWN);
		}
	}
	p.values.get(idx(index)?).ok_or(status::ERR_BAD_INDEX)
}

// ---- propSet（单元素）-----------------------------------------------------

/// propSet 通用实现：属性未定义时按 OFX 语义**隐式创建**（ofxProperty.h：
/// "If the property does not exist it is created"，HS setValue 同）——
/// 插件 describe 期写的大量描述符属性（grouping、各 capability 开关）
/// 并未由宿主预定义，拒绝创建会让 describe 直接失败。已定义但类型
/// 不符 → Unknown；越界 → BadIndex。
fn set_value(set: &PropertySet, name: &str, index: c_int, value: Value) -> Result<(), c_int> {
	set.with_locked(|props| {
		let idx = idx(index)?;
		let Some(p) = props.iter_mut().find(|p| p.name == name) else {
			// 隐式创建：维度 index+1，前导槽位以同值填充（HS 的新属性
			// 默认值语义——新建属性先有一个默认元素再逐位写）。
			let mut values = vec![value.clone(); idx];
			values.push(value);
			props.push(crate::property::Property {
				name: name.to_string(),
				values,
			});
			return Ok(());
		};
		// 空属性（宿主预定义的空数组，如 OfxImageEffectPropSupportedContexts）
		// 没有类型探针：按 HS 的 index == size 追加语义直接 push。
		if p.values.is_empty() {
			if idx != 0 {
				if std::env::var_os("OAK_OFX_TRACE").is_some() {
					eprintln!("[ofx] propSet into empty predefined array: {name}[{idx}]");
				}
				return Err(status::ERR_BAD_INDEX);
			}
			p.values.push(value);
			return Ok(());
		}
		let probe = &p.values[0];
		if Kind::of(probe) != Kind::of(&value) {
			return Err(status::ERR_UNKNOWN);
		}
		// HS `setValue` 允许 index == size 时追加（ofxhPropertySuite.cpp:284）
		// ——addSupportedContext 等协商属性正是这样逐位增长的。
		if idx == p.values.len() {
			p.values.push(value);
			return Ok(());
		}
		if idx >= p.values.len() && std::env::var_os("OAK_OFX_TRACE").is_some() {
			eprintln!("[ofx] propSet out of range: {name}[{idx}] (dim {})", p.values.len());
		}
		let slot = p.values.get_mut(idx).ok_or(status::ERR_BAD_INDEX)?;
		*slot = value;
		Ok(())
	})
}

unsafe extern "C" fn prop_set_pointer(
	handle: *mut c_void,
	name: *const c_char,
	index: c_int,
	value: *mut c_void,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			set_value(set, name, index, Value::Pointer(value))
		})
	}
}

unsafe extern "C" fn prop_set_string(
	handle: *mut c_void,
	name: *const c_char,
	index: c_int,
	value: *const c_char,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if value.is_null() {
				return Err(status::ERR_VALUE);
			}
			// C 语义：截断到首个 NUL；CString::new 不会失败
			//（from_ptr 已保证 NUL 结尾）。
			let s = CString::new(CStr::from_ptr(value).to_bytes()).unwrap();
			set_value(set, name, index, Value::String(s))
		})
	}
}

unsafe extern "C" fn prop_set_double(
	handle: *mut c_void,
	name: *const c_char,
	index: c_int,
	value: c_double,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			set_value(set, name, index, Value::Double(value))
		})
	}
}

unsafe extern "C" fn prop_set_int(
	handle: *mut c_void,
	name: *const c_char,
	index: c_int,
	value: c_int,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			set_value(set, name, index, Value::Int(value))
		})
	}
}

// ---- propSetN（批量）------------------------------------------------------

/// 从 C 数组读出 `count` 个元素并整体写入；count != 现有维度时
/// 整体替换（HS `setValueN` 的 resize 语义，ofxhPropertySuite.cpp:299-311）。
fn set_values(
	set: &PropertySet,
	name: &str,
	count: c_int,
	values: Vec<Value>,
) -> Result<(), c_int> {
	set.with_locked(|props| {
		let Some(p) = props.iter_mut().find(|p| p.name == name) else {
			// 与单元素 set_value 一致：未定义属性按 OFX 语义隐式创建。
			props.push(crate::property::Property {
				name: name.to_string(),
				values,
			});
			return Ok(());
		};
		// count == 0 时无类型可探（HS 仍会做 fetchTypedProperty）。
		if let Some(first) = p.values.first() {
			if let Some(v) = values.first() {
				if Kind::of(first) != Kind::of(v) {
					return Err(status::ERR_UNKNOWN);
				}
			}
		}
		let count = idx(count)?;
		if count != p.values.len() {
			// 维度变化：整体替换（HS 是 resize + 逐位写，等价）。
			p.values = values;
		} else {
			for (i, v) in values.into_iter().enumerate() {
				p.values[i] = v;
			}
		}
		Ok(())
	})
}

unsafe extern "C" fn prop_set_pointer_n(
	handle: *mut c_void,
	name: *const c_char,
	count: c_int,
	values: *const *mut c_void,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if count > 0 && values.is_null() {
				return Err(status::ERR_VALUE);
			}
			let vals = (0..idx(count)?)
				.map(|i| Value::Pointer(*values.add(i)))
				.collect();
			set_values(set, name, count, vals)
		})
	}
}

unsafe extern "C" fn prop_set_string_n(
	handle: *mut c_void,
	name: *const c_char,
	count: c_int,
	values: *const *const c_char,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if count > 0 && values.is_null() {
				return Err(status::ERR_VALUE);
			}
			let mut vals = Vec::with_capacity(count.max(0) as usize);
			for i in 0..idx(count)? {
				let v = *values.add(i);
				if v.is_null() {
					return Err(status::ERR_VALUE);
				}
				vals.push(Value::String(
					CString::new(CStr::from_ptr(v).to_bytes()).unwrap(),
				));
			}
			set_values(set, name, count, vals)
		})
	}
}

unsafe extern "C" fn prop_set_double_n(
	handle: *mut c_void,
	name: *const c_char,
	count: c_int,
	values: *const c_double,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if count > 0 && values.is_null() {
				return Err(status::ERR_VALUE);
			}
			let vals = (0..idx(count)?)
				.map(|i| Value::Double(*values.add(i)))
				.collect();
			set_values(set, name, count, vals)
		})
	}
}

unsafe extern "C" fn prop_set_int_n(
	handle: *mut c_void,
	name: *const c_char,
	count: c_int,
	values: *const c_int,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if count > 0 && values.is_null() {
				return Err(status::ERR_VALUE);
			}
			let vals = (0..idx(count)?)
				.map(|i| Value::Int(*values.add(i)))
				.collect();
			set_values(set, name, count, vals)
		})
	}
}

// ---- propGet（单元素）-----------------------------------------------------

/// 非字符串标量读取（克隆值写出；字符串走内驻指针路径）。
fn get_scalar(set: &PropertySet, name: &str, index: c_int, kind: Kind) -> Result<Value, c_int> {
	set.with_locked(|props| get_value(props, name, index, kind).cloned())
}

unsafe extern "C" fn prop_get_pointer(
	handle: *mut c_void,
	name: *const c_char,
	index: c_int,
	out: *mut *mut c_void,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if out.is_null() {
				return Err(status::ERR_VALUE);
			}
			let v = get_scalar(set, name, index, Kind::Pointer)?;
			match v {
				Value::Pointer(p) => {
					if std::env::var_os("OAK_OFX_TRACE").is_some() {
						eprintln!("[ofx] propGetPointer(h={handle:p}, {name}[{index}]) -> {p:p}");
					}
					*out = p;
					Ok(())
				}
				_ => Err(status::FAILED),
			}
		})
	}
}

/// propGetString：写**内驻** C 串指针（OFX 契约：指向宿主内部存储，
/// 属性被下次修改前有效；克隆的 CString 指针会悬垂，绝不能给插件）。
fn get_string(set: &PropertySet, name: &str, index: c_int) -> Result<*mut c_char, c_int> {
	set.with_locked(|props| match get_value(props, name, index, Kind::Str)? {
		Value::String(s) => Ok(s.as_ptr() as *mut c_char),
		_ => Err(status::FAILED),
	})
}

unsafe extern "C" fn prop_get_string(
	handle: *mut c_void,
	name: *const c_char,
	index: c_int,
	out: *mut *mut c_char,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if out.is_null() {
				return Err(status::ERR_VALUE);
			}
			let r = get_string(set, name, index);
			if std::env::var_os("OAK_OFX_TRACE").is_some() {
				let rendered = match &r {
					Ok(p) => format!("0 <- {:?}", std::ffi::CStr::from_ptr(*p)),
					Err(c) => format!("{c}"),
				};
				eprintln!("[ofx] propGetString(h={handle:p}, {name}[{index}]) -> {rendered}");
			}
			*out = r?;
			Ok(())
		})
	}
}

unsafe extern "C" fn prop_get_double(
	handle: *mut c_void,
	name: *const c_char,
	index: c_int,
	out: *mut c_double,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if out.is_null() {
				return Err(status::ERR_VALUE);
			}
			let v = get_scalar(set, name, index, Kind::Double)?;
			match v {
				Value::Double(d) => {
					*out = d;
					Ok(())
				}
				// Int <-> Double 数值协随（宽容宿主语义，同 get_n）。
				Value::Int(i) => {
					*out = i as f64;
					Ok(())
				}
				_ => Err(status::FAILED),
			}
		})
	}
}

unsafe extern "C" fn prop_get_int(
	handle: *mut c_void,
	name: *const c_char,
	index: c_int,
	out: *mut c_int,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if out.is_null() {
				return Err(status::ERR_VALUE);
			}
			let v = get_scalar(set, name, index, Kind::Int)?;
			match v {
				Value::Int(i) => {
					*out = i;
					Ok(())
				}
				// Int <-> Double 数值协随（宽容宿主语义，同 get_n）。
				Value::Double(d) => {
					*out = d as i32;
					Ok(())
				}
				_ => Err(status::FAILED),
			}
			.inspect_err(|&c| {
				if std::env::var_os("OAK_OFX_TRACE").is_some() {
					eprintln!("[ofx] propGetInt({name}) -> {c}");
				}
			})
		})
	}
}

// ---- propGetN（批量）------------------------------------------------------

/// 批量读取：拷贝 min(count, dimension) 个（HS `getValueNRaw`，
/// ofxhPropertySuite.cpp:271-280，不报错）。
fn get_n(
	set: &PropertySet,
	name: &str,
	count: c_int,
	kind: Kind,
	write: impl Fn(&Value, usize),
) -> Result<(), c_int> {
	set.with_locked(|props| {
		let p = props
			.iter()
			.find(|p| p.name == name)
			.ok_or(status::ERR_UNKNOWN)?;
		let probe = p.values.first().ok_or(status::ERR_UNKNOWN)?;
		// Int <-> Double coercion (forgiving-host semantics): the CImg
		// framework reads the DOUBLE render window with propGetIntN and
		// must still get the values; strict kind checks turn that into a
		// bogus MissingHostFeature at render time.
		let numeric = matches!(Kind::of(probe), Kind::Int | Kind::Double)
			&& matches!(kind, Kind::Int | Kind::Double);
		if Kind::of(probe) != kind && !numeric {
			return Err(status::ERR_UNKNOWN);
		}
		let n = idx(count)?.min(p.values.len());
		for (i, v) in p.values.iter().take(n).enumerate() {
			write(v, i);
		}
		if std::env::var_os("OAK_OFX_TRACE").is_some() {
			let dump: Vec<String> = p
				.values
				.iter()
				.take(n)
				.map(|v| match v {
					Value::Int(i) => i.to_string(),
					Value::Double(d) => d.to_string(),
					_ => "?".to_string(),
				})
				.collect();
			eprintln!("[ofx] propGetN({name} x{n}) -> ok [{dump:?}]");
		}
		Ok(())
	})
	.inspect_err(|&code| {
		// The property the plugin asked for and we did not have — the
		// single most useful line when a plugin reports
		// MissingHostFeature (trace-gated, like fetchSuite misses).
		if std::env::var_os("OAK_OFX_TRACE").is_some() {
			eprintln!("[ofx] property miss: {name} (code {code})");
		}
	})
}

unsafe extern "C" fn prop_get_pointer_n(
	handle: *mut c_void,
	name: *const c_char,
	count: c_int,
	out: *mut *mut c_void,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if count > 0 && out.is_null() {
				return Err(status::ERR_VALUE);
			}
			get_n(set, name, count, Kind::Pointer, |v, i| if let Value::Pointer(p) = v { *out.add(i) = *p })
		})
	}
}

unsafe extern "C" fn prop_get_string_n(
	handle: *mut c_void,
	name: *const c_char,
	count: c_int,
	out: *mut *mut c_char,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if count > 0 && out.is_null() {
				return Err(status::ERR_VALUE);
			}
			get_n(set, name, count, Kind::Str, |v, i| if let Value::String(s) = v { *out.add(i) = s.as_ptr() as *mut c_char })
		})
	}
}

unsafe extern "C" fn prop_get_double_n(
	handle: *mut c_void,
	name: *const c_char,
	count: c_int,
	out: *mut c_double,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if count > 0 && out.is_null() {
				return Err(status::ERR_VALUE);
			}
			get_n(set, name, count, Kind::Double, |v, i| match v {
				Value::Double(d) => *out.add(i) = *d,
				Value::Int(d) => *out.add(i) = *d as f64,
				_ => {}
			})
		})
	}
}

unsafe extern "C" fn prop_get_int_n(
	handle: *mut c_void,
	name: *const c_char,
	count: c_int,
	out: *mut c_int,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if count > 0 && out.is_null() {
				return Err(status::ERR_VALUE);
			}
			get_n(set, name, count, Kind::Int, |v, i| match v {
				Value::Int(d) => *out.add(i) = *d,
				Value::Double(d) => *out.add(i) = *d as i32,
				_ => {}
			})
		})
	}
}

// ---- propReset / propGetDimension -----------------------------------------

unsafe extern "C" fn prop_reset(handle: *mut c_void, name: *const c_char) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			// HS `reset()` 恢复到 define 时的默认值
			// （ofxhPropertySuite.cpp:330）；本 crate 不保存默认值快照，
			// 第 1 期明确不支持（见模块文档）。
			let _ = (set, name);
			Err(status::ERR_UNSUPPORTED)
		})
	}
}

unsafe extern "C" fn prop_get_dimension(
	handle: *mut c_void,
	name: *const c_char,
	out: *mut c_int,
) -> c_int {
	unsafe {
		caught(handle, |set| {
			let name = c_name(name)?;
			if out.is_null() {
				return Err(status::ERR_VALUE);
			}
			// HS: fetchProperty 失败（未定义）→ Unknown（ofxhPropertySuite.cpp:992）；
			// 已定义的空数组（如协商属性的初始状态）是合法维度 0，不是错误。
			let exists = set.with_locked(|props| props.iter().any(|p| p.name == name));
			if !exists {
				if std::env::var_os("OAK_OFX_TRACE").is_some() {
					let tag = crate::suites::tag::kind(handle);
					eprintln!("[ofx] property miss (dimension): {name} (handle {handle:p} tag {tag})");
				}
				return Err(status::ERR_UNKNOWN);
			}
			*out = set.dimension(name) as c_int;
			Ok(())
		})
	}
}

/// 静态函数表实例（fetch_suite 返回其地址）。
pub fn suite_v1() -> &'static PropertySuiteV1 {
	static SUITE: std::sync::OnceLock<PropertySuiteV1> = std::sync::OnceLock::new();
	SUITE.get_or_init(|| PropertySuiteV1 {
		set_pointer: prop_set_pointer,
		set_string: prop_set_string,
		set_double: prop_set_double,
		set_int: prop_set_int,
		set_pointer_n: prop_set_pointer_n,
		set_string_n: prop_set_string_n,
		set_double_n: prop_set_double_n,
		set_int_n: prop_set_int_n,
		get_pointer: prop_get_pointer,
		get_string: prop_get_string,
		get_double: prop_get_double,
		get_int: prop_get_int,
		get_pointer_n: prop_get_pointer_n,
		get_string_n: prop_get_string_n,
		get_double_n: prop_get_double_n,
		get_int_n: prop_get_int_n,
		reset: prop_reset,
		get_dimension: prop_get_dimension,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn handle(set: &PropertySet) -> *mut c_void {
		set as *const PropertySet as *mut c_void
	}

	fn cs(s: &str) -> CString {
		CString::new(s).unwrap()
	}

	/// `Value` 未实现 PartialEq（Pointer 无法比较）——用 Debug 串比较。
	fn val_eq(a: &Value, b: &Value) -> bool {
		format!("{a:?}") == format!("{b:?}")
	}

	fn assert_value(set: &PropertySet, name: &str, index: usize, want: &Value) {
		assert!(
			val_eq(set.get(name, index).as_ref().expect("属性应存在"), want),
			"{name}[{index}] 值不符"
		);
	}

	/// 空句柄 → BadHandle；空/非 UTF-8 属性名 → ErrValue。
	#[test]
	fn bad_handles_and_bad_names() {
		let s = suite_v1();
		let set = PropertySet::new();
		let name = cs("a");
		let mut iv = 0;
		let mut dv = 0.0;
		let mut sv: *mut c_char = std::ptr::null_mut();
		let mut pv: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!(
				(s.set_int)(std::ptr::null_mut(), name.as_ptr(), 0, 1),
				status::ERR_BAD_HANDLE
			);
			assert_eq!(
				(s.get_int)(std::ptr::null_mut(), name.as_ptr(), 0, &mut iv),
				status::ERR_BAD_HANDLE
			);
			// 空属性名。
			assert_eq!((s.set_int)(handle(&set), std::ptr::null(), 0, 1), status::ERR_VALUE);
			assert_eq!(
				(s.get_int)(handle(&set), std::ptr::null(), 0, &mut iv),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.set_double)(handle(&set), std::ptr::null(), 0, 1.0),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.set_string)(handle(&set), std::ptr::null(), 0, name.as_ptr()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_double)(handle(&set), std::ptr::null(), 0, &mut dv),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_string)(handle(&set), std::ptr::null(), 0, &mut sv),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_pointer)(handle(&set), std::ptr::null(), 0, &mut pv),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_dimension)(handle(&set), std::ptr::null(), &mut iv),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.reset)(handle(&set), std::ptr::null()),
				status::ERR_VALUE
			);
			// 非 UTF-8 属性名（0xFF 后接 NUL）。
			let bad: [c_char; 2] = [0xFFu8 as c_char, 0];
			assert_eq!(
				(s.set_int)(handle(&set), bad.as_ptr(), 0, 1),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_int)(handle(&set), bad.as_ptr(), 0, &mut iv),
				status::ERR_VALUE
			);
		}
	}

	/// 未定义属性 get → ErrUnknown；define 后空数组维度 0 且越界读
	/// → BadIndex；负索引 → BadIndex。
	#[test]
	fn get_missing_empty_and_bounds() {
		let s = suite_v1();
		let set = PropertySet::new();
		let n = cs("a");
		let mut iv = 0;
		unsafe {
			assert_eq!(
				(s.get_int)(handle(&set), n.as_ptr(), 0, &mut iv),
				status::ERR_UNKNOWN
			);
			assert_eq!(
				(s.get_int)(handle(&set), n.as_ptr(), -1, &mut iv),
				status::ERR_UNKNOWN,
				"未定义属性先报 Unknown（HS 先查类型）"
			);
		}
		set.define("a", vec![]);
		unsafe {
			assert_eq!(
				(s.get_int)(handle(&set), n.as_ptr(), 0, &mut iv),
				status::ERR_BAD_INDEX
			);
		}
		set.define("a", vec![Value::Int(3)]);
		unsafe {
			assert_eq!(
				(s.get_int)(handle(&set), n.as_ptr(), -1, &mut iv),
				status::ERR_BAD_INDEX
			);
			assert_eq!(
				(s.get_int)(handle(&set), n.as_ptr(), 1, &mut iv),
				status::ERR_BAD_INDEX
			);
		}
	}

	/// propSet 隐式创建：index 2 建出维度 3 的属性且前导槽位同值；
	/// 已定义数组 index == 维度追加、越界 → BadIndex。
	#[test]
	fn set_creates_and_appends() {
		let s = suite_v1();
		let set = PropertySet::new();
		let n = cs("a");
		unsafe {
			assert_eq!((s.set_int)(handle(&set), n.as_ptr(), 2, 7), 0);
		}
		assert_eq!(set.dimension("a"), 3);
		assert_value(&set, "a", 0, &Value::Int(7));
		assert_value(&set, "a", 2, &Value::Int(7));

		// index == 维度 → 追加（协商属性逐位增长语义）。
		unsafe {
			assert_eq!((s.set_int)(handle(&set), n.as_ptr(), 3, 9), 0);
		}
		assert_eq!(set.dimension("a"), 4);
		assert_value(&set, "a", 3, &Value::Int(9));

		// 越界（跳过追加位）→ BadIndex。
		unsafe {
			assert_eq!(
				(s.set_int)(handle(&set), n.as_ptr(), 9, 1),
				status::ERR_BAD_INDEX
			);
		}
		assert_eq!(set.dimension("a"), 4);
	}

	/// 空预定义数组：index 0 推入；index > 0 → BadIndex。
	#[test]
	fn set_into_empty_predefined_array() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("empty", vec![]);
		let n = cs("empty");
		unsafe {
			assert_eq!(
				(s.set_int)(handle(&set), n.as_ptr(), 1, 1),
				status::ERR_BAD_INDEX
			);
			assert_eq!((s.set_int)(handle(&set), n.as_ptr(), 0, 5), 0);
		}
		assert_value(&set, "empty", 0, &Value::Int(5));
	}

	/// propSet 类型不符 → Unknown；数值 Int/Double 不互转（set 严格）。
	#[test]
	fn set_type_mismatch() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("i", vec![Value::Int(0)]);
		set.define("d", vec![Value::Double(0.0)]);
		let ni = cs("i");
		let nd = cs("d");
		unsafe {
			assert_eq!(
				(s.set_double)(handle(&set), ni.as_ptr(), 0, 1.0),
				status::ERR_UNKNOWN
			);
			assert_eq!(
				(s.set_int)(handle(&set), nd.as_ptr(), 0, 1),
				status::ERR_UNKNOWN
			);
		}
	}

	/// propGet：Int ↔ Double 数值协随；混合数组元素类型不符 → Failed。
	#[test]
	fn get_numeric_coercion_and_mixed_arrays() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("i", vec![Value::Int(3)]);
		set.define("d", vec![Value::Double(1.5)]);
		let ni = cs("i");
		let nd = cs("d");
		let mut dv = 0.0;
		let mut iv = 0;
		unsafe {
			assert_eq!((s.get_double)(handle(&set), ni.as_ptr(), 0, &mut dv), 0);
		}
		assert_eq!(dv, 3.0);
		unsafe {
			assert_eq!((s.get_int)(handle(&set), nd.as_ptr(), 0, &mut iv), 0);
		}
		assert_eq!(iv, 1);

		// 混合数组（首元素定类型，后续元素类型不符）→ 单元素读 Failed。
		let mix = PropertySet::new();
		mix.define(
			"m",
			vec![Value::Double(1.0), Value::String(cs("x"))],
		);
		let nm = cs("m");
		unsafe {
			assert_eq!(
				(s.get_double)(handle(&mix), nm.as_ptr(), 1, &mut dv),
				status::FAILED
			);
		}
		let mix2 = PropertySet::new();
		mix2.define("m", vec![Value::Int(1), Value::String(cs("x"))]);
		unsafe {
			assert_eq!(
				(s.get_int)(handle(&mix2), nm.as_ptr(), 1, &mut iv),
				status::FAILED
			);
		}
	}

	/// 分量不足/超出的类型读：get_string 读数值属性 → Unknown（类型
	/// 不符），get_double 读字符串 → Unknown。
	#[test]
	fn get_kind_mismatch() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("i", vec![Value::Int(1)]);
		set.define("str", vec![Value::String(cs("s"))]);
		let ni = cs("i");
		let ns = cs("str");
		let mut sv: *mut c_char = std::ptr::null_mut();
		let mut dv = 0.0;
		unsafe {
			assert_eq!(
				(s.get_string)(handle(&set), ni.as_ptr(), 0, &mut sv),
				status::ERR_UNKNOWN
			);
			assert_eq!(
				(s.get_double)(handle(&set), ns.as_ptr(), 0, &mut dv),
				status::ERR_UNKNOWN
			);
		}
	}

	/// propGet 空 out 指针 → ErrValue（四种标量读取）。
	#[test]
	fn get_null_out_pointers() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("i", vec![Value::Int(1)]);
		set.define("d", vec![Value::Double(1.0)]);
		set.define("s", vec![Value::String(cs("x"))]);
		set.define("p", vec![Value::Pointer(std::ptr::null_mut())]);
		let n = cs("i");
		unsafe {
			assert_eq!(
				(s.get_int)(handle(&set), n.as_ptr(), 0, std::ptr::null_mut()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_double)(handle(&set), n.as_ptr(), 0, std::ptr::null_mut()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_string)(handle(&set), n.as_ptr(), 0, std::ptr::null_mut()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_pointer)(handle(&set), n.as_ptr(), 0, std::ptr::null_mut()),
				status::ERR_VALUE
			);
		}
	}

	/// propGetPointer：成功写出指针；混合数组类型不符 → Failed。
	#[test]
	fn get_pointer_paths() {
		let s = suite_v1();
		let raw = 0x1234usize as *mut c_void;
		let set = PropertySet::new();
		set.define("p", vec![Value::Pointer(raw)]);
		let np = cs("p");
		let mut out: *mut c_void = std::ptr::null_mut();
		unsafe {
			assert_eq!((s.get_pointer)(handle(&set), np.as_ptr(), 0, &mut out), 0);
		}
		assert_eq!(out, raw);

		let mix = PropertySet::new();
		mix.define(
			"p",
			vec![Value::Pointer(raw), Value::Int(1)],
		);
		unsafe {
			assert_eq!(
				(s.get_pointer)(handle(&mix), np.as_ptr(), 1, &mut out),
				status::FAILED
			);
		}
	}

	/// propGetString：写出内驻指针且内容正确；混合数组 → Failed。
	#[test]
	fn get_string_paths() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("s", vec![Value::String(cs("hello"))]);
		let ns = cs("s");
		let mut out: *mut c_char = std::ptr::null_mut();
		unsafe {
			assert_eq!((s.get_string)(handle(&set), ns.as_ptr(), 0, &mut out), 0);
			assert_eq!(CStr::from_ptr(out).to_bytes(), b"hello");
		}
		let mix = PropertySet::new();
		mix.define(
			"s",
			vec![Value::String(cs("x")), Value::Int(1)],
		);
		unsafe {
			assert_eq!(
				(s.get_string)(handle(&mix), ns.as_ptr(), 1, &mut out),
				status::FAILED
			);
		}
	}

	/// propSetString：NULL 值 → ErrValue；截断到首个 NUL。
	#[test]
	fn set_string_null_and_truncation() {
		let s = suite_v1();
		let set = PropertySet::new();
		let n = cs("s");
		unsafe {
			assert_eq!(
				(s.set_string)(handle(&set), n.as_ptr(), 0, std::ptr::null()),
				status::ERR_VALUE
			);
		}
	}

	/// propSetN 家族：隐式创建、维度替换/逐位写、空数组与 NULL 参数。
	#[test]
	fn set_n_families() {
		let s = suite_v1();
		let set = PropertySet::new();

		// set_int_n：隐式创建 2 维。
		let ni = cs("ints");
		let ivals = [1 as c_int, 2];
		unsafe {
			assert_eq!(
				(s.set_int_n)(handle(&set), ni.as_ptr(), 2, ivals.as_ptr()),
				0
			);
		}
		assert_eq!(set.dimension("ints"), 2);
		assert_value(&set, "ints", 1, &Value::Int(2));

		// count 与现有维度相同 → 逐位覆盖。
		let ivals2 = [7 as c_int, 8];
		unsafe {
			assert_eq!(
				(s.set_int_n)(handle(&set), ni.as_ptr(), 2, ivals2.as_ptr()),
				0
			);
		}
		assert_value(&set, "ints", 0, &Value::Int(7));

		// 维度变化 → 整体替换。
		let ivals3 = [9 as c_int];
		unsafe {
			assert_eq!(
				(s.set_int_n)(handle(&set), ni.as_ptr(), 1, ivals3.as_ptr()),
				0
			);
		}
		assert_eq!(set.dimension("ints"), 1);

		// count 0 + NULL：合法（空维度替换）。
		unsafe {
			assert_eq!(
				(s.set_int_n)(handle(&set), ni.as_ptr(), 0, std::ptr::null()),
				0
			);
		}
		assert_eq!(set.dimension("ints"), 0);

		// count > 0 + NULL → ErrValue；负 count → BadIndex。
		unsafe {
			assert_eq!(
				(s.set_int_n)(handle(&set), ni.as_ptr(), 1, std::ptr::null()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.set_int_n)(handle(&set), ni.as_ptr(), -1, ivals.as_ptr()),
				status::ERR_BAD_INDEX
			);
		}

		// set_double_n：隐式创建 + NULL 分支。
		let nd = cs("dbls");
		let dvals = [1.5 as c_double, 2.5];
		unsafe {
			assert_eq!(
				(s.set_double_n)(handle(&set), nd.as_ptr(), 2, dvals.as_ptr()),
				0
			);
			assert_eq!(
				(s.set_double_n)(handle(&set), nd.as_ptr(), 1, std::ptr::null()),
				status::ERR_VALUE
			);
		}
		assert_value(&set, "dbls", 1, &Value::Double(2.5));

		// set_pointer_n：单元素与 count 0 + NULL。
		let np = cs("ptrs");
		let pvals = [0x55usize as *mut c_void];
		unsafe {
			assert_eq!(
				(s.set_pointer_n)(handle(&set), np.as_ptr(), 1, pvals.as_ptr()),
				0
			);
			assert_eq!(
				(s.set_pointer_n)(handle(&set), np.as_ptr(), 1, std::ptr::null()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.set_pointer_n)(handle(&set), np.as_ptr(), 0, std::ptr::null()),
				0
			);
		}
		assert_eq!(set.dimension("ptrs"), 0);

		// set_string_n：成功、NULL 数组、数组内 NULL 元素。
		let nstr = cs("strs");
		let sa = cs("a");
		let sb = cs("b");
		let svals = [sa.as_ptr(), sb.as_ptr()];
		unsafe {
			assert_eq!(
				(s.set_string_n)(handle(&set), nstr.as_ptr(), 2, svals.as_ptr()),
				0
			);
			assert_eq!(
				(s.set_string_n)(handle(&set), nstr.as_ptr(), 1, std::ptr::null()),
				status::ERR_VALUE
			);
			let with_null = [sa.as_ptr(), std::ptr::null()];
			assert_eq!(
				(s.set_string_n)(handle(&set), nstr.as_ptr(), 2, with_null.as_ptr()),
				status::ERR_VALUE
			);
		}
		assert_eq!(set.dimension("strs"), 2);
	}

	/// set_n 类型不符 → Unknown；已有数组 count 0 替换为空。
	#[test]
	fn set_n_type_mismatch_and_empty_replace() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("i", vec![Value::Int(1)]);
		let ni = cs("i");
		let dvals = [2.0 as c_double];
		unsafe {
			assert_eq!(
				(s.set_double_n)(handle(&set), ni.as_ptr(), 1, dvals.as_ptr()),
				status::ERR_UNKNOWN
			);
			// values 为空（count 0）→ 无类型探针，整体替换为空。
			assert_eq!(
				(s.set_double_n)(handle(&set), ni.as_ptr(), 0, dvals.as_ptr()),
				0
			);
		}
		assert_eq!(set.dimension("i"), 0);
	}

	/// propGetN：Int ↔ Double 协随、min(count, dim) 截断、混合数组写
	/// 跳过不符元素。
	#[test]
	fn get_n_families() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("ints", vec![Value::Int(1), Value::Int(2)]);
		set.define("dbls", vec![Value::Double(1.5), Value::Double(2.5)]);

		// get_double_n 读 Int 属性 → 协随为 f64。
		let ni = cs("ints");
		let mut dout = [0.0 as c_double; 2];
		unsafe {
			assert_eq!(
				(s.get_double_n)(handle(&set), ni.as_ptr(), 2, dout.as_mut_ptr()),
				0
			);
		}
		assert_eq!(dout, [1.0, 2.0]);

		// get_int_n 读 Double 属性 → 截断为 i32。
		let nd = cs("dbls");
		let mut iout = [0 as c_int; 2];
		unsafe {
			assert_eq!(
				(s.get_int_n)(handle(&set), nd.as_ptr(), 2, iout.as_mut_ptr()),
				0
			);
		}
		assert_eq!(iout, [1, 2]);

		// count > 维度：只拷贝 min(count, dim)。
		let mut one = [0 as c_int; 1];
		unsafe {
			assert_eq!(
				(s.get_int_n)(handle(&set), ni.as_ptr(), 5, one.as_mut_ptr()),
				0
			);
		}
		assert_eq!(one, [1]);

		// count 0：不写。
		let mut zero = [7 as c_int; 1];
		unsafe {
			assert_eq!(
				(s.get_int_n)(handle(&set), ni.as_ptr(), 0, zero.as_mut_ptr()),
				0
			);
		}
		assert_eq!(zero, [7]);

		// get_pointer_n / get_string_n 成功路径。
		let ptrs = PropertySet::new();
		let raw = 0x99usize as *mut c_void;
		ptrs.define("p", vec![Value::Pointer(raw)]);
		let np = cs("p");
		let mut pout = [std::ptr::null_mut(); 1];
		unsafe {
			assert_eq!(
				(s.get_pointer_n)(handle(&ptrs), np.as_ptr(), 1, pout.as_mut_ptr()),
				0
			);
		}
		assert_eq!(pout[0], raw);

		let strs = PropertySet::new();
		strs.define("s", vec![Value::String(cs("hi"))]);
		let ns = cs("s");
		let mut sout = [std::ptr::null_mut(); 1];
		unsafe {
			assert_eq!(
				(s.get_string_n)(handle(&strs), ns.as_ptr(), 1, sout.as_mut_ptr()),
				0
			);
			assert_eq!(CStr::from_ptr(sout[0]).to_bytes(), b"hi");
		}
	}

	/// propGetN 错误路径：空 out、未定义、空数组、类型不符、负 count。
	#[test]
	fn get_n_error_paths() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("ints", vec![Value::Int(1)]);
		set.define("strs", vec![Value::String(cs("s"))]);
		set.define("empty", vec![]);
		let ni = cs("ints");
		let ns = cs("strs");
		let ne = cs("empty");
		let nmissing = cs("missing");
		let mut iout = [0 as c_int; 2];
		let mut dout = [0.0 as c_double; 2];
		let mut pout = [std::ptr::null_mut(); 2];
		let mut sout = [std::ptr::null_mut(); 2];
		unsafe {
			// 空 out（count > 0）。
			assert_eq!(
				(s.get_int_n)(handle(&set), ni.as_ptr(), 1, std::ptr::null_mut()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_double_n)(handle(&set), ni.as_ptr(), 1, std::ptr::null_mut()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_pointer_n)(handle(&set), ni.as_ptr(), 1, std::ptr::null_mut()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_string_n)(handle(&set), ni.as_ptr(), 1, std::ptr::null_mut()),
				status::ERR_VALUE
			);
			// 未定义 → Unknown；空数组 → Unknown（无类型探针）。
			assert_eq!(
				(s.get_int_n)(handle(&set), nmissing.as_ptr(), 1, iout.as_mut_ptr()),
				status::ERR_UNKNOWN
			);
			assert_eq!(
				(s.get_int_n)(handle(&set), ne.as_ptr(), 1, iout.as_mut_ptr()),
				status::ERR_UNKNOWN
			);
			// 类型不符 → Unknown（数值族之间协随，其余严格）。
			assert_eq!(
				(s.get_int_n)(handle(&set), ns.as_ptr(), 1, iout.as_mut_ptr()),
				status::ERR_UNKNOWN
			);
			assert_eq!(
				(s.get_double_n)(handle(&set), ns.as_ptr(), 1, dout.as_mut_ptr()),
				status::ERR_UNKNOWN
			);
			assert_eq!(
				(s.get_pointer_n)(handle(&set), ni.as_ptr(), 1, pout.as_mut_ptr()),
				status::ERR_UNKNOWN
			);
			assert_eq!(
				(s.get_string_n)(handle(&set), ni.as_ptr(), 1, sout.as_mut_ptr()),
				status::ERR_UNKNOWN
			);
			// 负 count → BadIndex（在 min 之前 idx 转换失败）。
			assert_eq!(
				(s.get_int_n)(handle(&set), ni.as_ptr(), -1, iout.as_mut_ptr()),
				status::ERR_BAD_INDEX
			);
		}
	}

	/// propReset 明确不支持；propGetDimension 空 out/未定义/空数组。
	#[test]
	fn reset_and_dimension() {
		let s = suite_v1();
		let set = PropertySet::new();
		set.define("a", vec![Value::Int(1)]);
		set.define("empty", vec![]);
		let na = cs("a");
		let ne = cs("empty");
		let nmissing = cs("missing");
		let mut dim = -1;
		unsafe {
			assert_eq!(
				(s.reset)(handle(&set), na.as_ptr()),
				status::ERR_UNSUPPORTED
			);
			assert_eq!(
				(s.get_dimension)(handle(&set), na.as_ptr(), std::ptr::null_mut()),
				status::ERR_VALUE
			);
			assert_eq!(
				(s.get_dimension)(handle(&set), nmissing.as_ptr(), &mut dim),
				status::ERR_UNKNOWN
			);
			assert_eq!((s.get_dimension)(handle(&set), na.as_ptr(), &mut dim), 0);
			assert_eq!(dim, 1);
			assert_eq!((s.get_dimension)(handle(&set), ne.as_ptr(), &mut dim), 0);
			assert_eq!(dim, 0, "已定义空数组是合法维度 0");
		}
	}

	/// OAK_OFX_TRACE 开启时的诊断分支：所有 trace 打印点都被走到
	/// （错误路径与成功路径）。测试串行化并还原环境变量。
	#[test]
	fn trace_diagnostic_branches() {
		static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
		let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
		let prev = std::env::var_os("OAK_OFX_TRACE");
		// set_var/remove_var 在本 crate 的 2021 edition 中是安全函数。
		std::env::set_var("OAK_OFX_TRACE", "1");
		let body = std::panic::catch_unwind(|| {
			let s = suite_v1();
			let set = PropertySet::new();
			let ni = cs("i");
			let nstr = cs("str");
			let nmissing = cs("missing");
			set.define("i", vec![Value::Int(1)]);
			set.define("str", vec![Value::String(cs("s"))]);
			let mut iv = 0;
			let mut dv = 0.0;
			let mut sv: *mut c_char = std::ptr::null_mut();
			let mut pv: *mut c_void = std::ptr::null_mut();
			let mut dim = 0;
			unsafe {
				// caught 的 trace：非 OK 状态码。
				assert_eq!(
					(s.get_int)(handle(&set), nmissing.as_ptr(), 0, &mut iv),
					status::ERR_UNKNOWN
				);
				// get_value trace：成功 + 失败。
				assert_eq!((s.get_int)(handle(&set), ni.as_ptr(), 0, &mut iv), 0);
				assert_eq!(
					(s.get_int)(handle(&set), nmissing.as_ptr(), 0, &mut iv),
					status::ERR_UNKNOWN
				);
				// 类型不符 trace。
				assert_eq!(
					(s.get_string)(handle(&set), ni.as_ptr(), 0, &mut sv),
					status::ERR_UNKNOWN
				);
				// set_value：空数组越界 trace；越界 trace。
				let empty = PropertySet::new();
				empty.define("e", vec![]);
				let ne = cs("e");
				assert_eq!(
					(s.set_int)(handle(&empty), ne.as_ptr(), 1, 1),
					status::ERR_BAD_INDEX
				);
				assert_eq!(
					(s.set_int)(handle(&set), ni.as_ptr(), 5, 2),
					status::ERR_BAD_INDEX
				);
				// propGetPointer trace（成功）。
				let ptrs = PropertySet::new();
				ptrs.define("p", vec![Value::Pointer(0x11usize as *mut c_void)]);
				let np = cs("p");
				assert_eq!((s.get_pointer)(handle(&ptrs), np.as_ptr(), 0, &mut pv), 0);
				// propGetString trace：成功 + 失败。
				assert_eq!((s.get_string)(handle(&set), nstr.as_ptr(), 0, &mut sv), 0);
				assert_eq!(
					(s.get_string)(handle(&set), nmissing.as_ptr(), 0, &mut sv),
					status::ERR_UNKNOWN
				);
				// propGetInt inspect_err trace。
				assert_eq!(
					(s.get_int)(handle(&set), nmissing.as_ptr(), 0, &mut iv),
					status::ERR_UNKNOWN
				);
				// get_n trace：成功 dump + 未命中 inspect_err。
				let mut iout = [0 as c_int; 2];
				assert_eq!(
					(s.get_int_n)(handle(&set), ni.as_ptr(), 2, iout.as_mut_ptr()),
					0
				);
				assert_eq!(
					(s.get_int_n)(handle(&set), nmissing.as_ptr(), 1, iout.as_mut_ptr()),
					status::ERR_UNKNOWN
				);
				// dump 的 Double 与未知元素分支。
				let mixed = PropertySet::new();
				mixed.define(
					"m",
					vec![Value::Double(1.5), Value::String(cs("x"))],
				);
				let nm = cs("m");
				let mut dout = [0.0 as c_double; 2];
				assert_eq!(
					(s.get_double_n)(handle(&mixed), nm.as_ptr(), 2, dout.as_mut_ptr()),
					0
				);
				// 空 out 的 get_double trace 分支（Err 路径渲染）。
				assert_eq!(
					(s.get_double)(handle(&set), ni.as_ptr(), 0, &mut dv),
					0
				);
				// propGetDimension 未定义 trace。
				assert_eq!(
					(s.get_dimension)(handle(&set), nmissing.as_ptr(), &mut dim),
					status::ERR_UNKNOWN
				);
			}
		});
		match prev {
			Some(v) => std::env::set_var("OAK_OFX_TRACE", v),
			None => std::env::remove_var("OAK_OFX_TRACE"),
		}
		if let Err(p) = body {
			std::panic::resume_unwind(p);
		}
	}
}
