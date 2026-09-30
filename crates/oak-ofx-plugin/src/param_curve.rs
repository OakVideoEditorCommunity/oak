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

//! 参数化曲线模型（OfxParametricParameterSuite 的宿主侧数据与求值）。
//!
//! 曲线 = 按 key 升序排列的控制点列表，每个控制点携带 Hermite 切线
//! （slope）。suite（[`crate::suites::parametric`]）只暴露 key/value
//! 编辑入口，slope 一律由 [`Curve::recompute_slopes`] 自动计算——
//! 内部点用两侧差分（centered finite difference），端点用单侧差分；
//! 字段保持 `pub`，便于测试/宿主显式编辑斜率。
//!
//! 求值 = 分段三次 Hermite（见 [`Curve::evaluate`]）。这是 OFX 规范
//! 建议宿主采用的"曲线编辑器式"表示（ofxParametricParam.h 的
//! parametric 参数文档），与贝塞尔形式等价（Hermite 切线 × 段长即
//! 贝塞尔控制臂）。

/// 曲线控制点：`key`（parametric 位置，定义域由
/// `OfxParamPropParametricRange` 限定）、`value`（求值结果）、
/// `slope`（该点处的一阶导数，三次 Hermite 的切线）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlPoint {
	/// parametric 位置（x）。
	pub key: f64,
	/// 求值结果（y）。
	pub value: f64,
	/// 一阶导数（自动差分 / 显式编辑）。
	pub slope: f64,
}

impl ControlPoint {
	/// 按 key/value 构造（slope 由 [`Curve::recompute_slopes`] 填入）。
	pub fn new(key: f64, value: f64) -> Self {
		Self {
			key,
			value,
			slope: 0.0,
		}
	}
}

/// 一条参数曲线：控制点按 key 升序（不变量；由本模块的修改接口
/// 维护）。
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
	/// 控制点序列（key 升序，无重复 key）。
	pub points: Vec<ControlPoint>,
}

impl Curve {
	/// 空曲线（`DeleteAllControlPoints` 后的状态；求值退化为恒等，
	/// 见 [`Curve::evaluate`]）。
	pub fn empty() -> Self {
		Self {
			points: Vec::new(),
		}
	}

	/// 恒等曲线：`{(lo, lo), (hi, hi)}` + 自动 slope —— parametric
	/// 参数的默认 default（ofxParametricParam.h："The default default
	/// value of a parametric curve is to be an identity lookup"）。
	/// `lo`/`hi` 来自 `OfxParamPropParametricRange`（默认 (0,1)）。
	pub fn identity(lo: f64, hi: f64) -> Self {
		let mut c = Self {
			points: vec![
				ControlPoint::new(lo, lo),
				ControlPoint::new(hi, hi),
			],
		};
		c.recompute_slopes();
		c
	}

	/// 由 key/value 对构造（slope 自动差分；按传入顺序——调用方
	/// 保证升序，或经 [`Curve::upsert`] 逐点插入）。
	pub fn from_pairs(pairs: &[(f64, f64)]) -> Self {
		let mut c = Self {
			points: pairs
				.iter()
				.map(|&(k, v)| ControlPoint::new(k, v))
				.collect(),
		};
		c.recompute_slopes();
		c
	}

	/// 控制点数。
	pub fn len(&self) -> usize {
		self.points.len()
	}

	/// 是否为空（`DeleteAllControlPoints` 后）。
	pub fn is_empty(&self) -> bool {
		self.points.is_empty()
	}

	/// 第 `i` 个控制点（越界 → None）。
	pub fn nth(&self, i: usize) -> Option<&ControlPoint> {
		self.points.get(i)
	}

	/// 重算全部 slope：内部点中心差分（两侧跨距），端点单侧差分；
	/// 单点/空曲线无差分 → slope 0。修改 key/value 后必须调用，
	/// 否则 Hermite 切线滞后于控制点几何。
	pub fn recompute_slopes(&mut self) {
		let n = self.points.len();
		for i in 0..n {
			self.points[i].slope = slope_at(&self.points, i);
		}
	}

	/// 三次 Hermite 求值。
	///
	/// - 空曲线：恒等（f(x) = x）——与"默认 default 是恒等查找"
	///   一致，`DeleteAllControlPoints` 后曲线退化为中性恒等，无端点
	///   可钳制，直接返回 x；
	/// - 单点曲线：常数（该点 value）；
	/// - x 在首/末 key 之外：钳制到端点值（任务/规范：越界 key
	///   钳制到端点值）。
	///
	/// 数学：段 [x0, x1] 上令 t = (x - x0)/(x1 - x0)，h = x1 - x0，
	/// 三次 Hermite 基函数
	///   h00 = 2t³ - 3t² + 1,  h01 = -2t³ + 3t²
	///   h10 = t³ - 2t² + t,   h11 = t³ - t²
	/// f(x) = h00·y0 + h01·y1 + h·(h10·m0 + h11·m1)。
	/// slope 为 1 的恒等曲线恰退化为 f(x) = x（端点钳制外）。
	pub fn evaluate(&self, x: f64) -> f64 {
		let n = self.points.len();
		if n == 0 {
			return x;
		}
		if n == 1 {
			return self.points[0].value;
		}
		if x <= self.points[0].key {
			return self.points[0].value;
		}
		if x >= self.points[n - 1].key {
			return self.points[n - 1].value;
		}
		// 定位所在段：points[i].key <= x < points[i+1].key。
		let i = self.points.partition_point(|p| p.key <= x) - 1;
		let a = self.points[i];
		let b = self.points[i + 1];
		let h = b.key - a.key;
		if h <= 0.0 {
			return a.value; // 防御：重复 key（不变量保证不出现）
		}
		let t = (x - a.key) / h;
		let t2 = t * t;
		let t3 = t2 * t;
		let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
		let h01 = -2.0 * t3 + 3.0 * t2;
		let h10 = t3 - 2.0 * t2 + t;
		let h11 = t3 - t2;
		h00 * a.value + h01 * b.value + h * (h10 * a.slope + h11 * b.slope)
	}

	/// 放入 (key, value)：同 key 已存在 → 覆盖其值（返回 false）；
	/// 否则按 key 升序插入（返回 true）。随后重算 slope。
	/// 这是 Add 与 SetNth 的公共落点（SetNth 先移除再落点）。
	pub fn upsert(&mut self, key: f64, value: f64) -> bool {
		if let Some(p) = self.points.iter_mut().find(|p| p.key == key) {
			p.value = value;
			self.recompute_slopes();
			return false;
		}
		let idx = self.points.partition_point(|p| p.key < key);
		self.points.insert(idx, ControlPoint::new(key, value));
		self.recompute_slopes();
		true
	}

	/// 改第 `nth` 个控制点为 (key, value)。key 变化可能破坏有序性
	/// （ofxParametricParam.h：SetNthControlPoint 的 key 可前移/后移
	/// 到其他点之前/之后）——先移除再按 key 插入；新 key 撞上其他
	/// 点的 key 时按覆盖语义处理。nth 越界 → Err(())。
	pub fn set_nth(&mut self, nth: usize, key: f64, value: f64) -> Result<(), ()> {
		if nth >= self.points.len() {
			return Err(());
		}
		self.points.remove(nth);
		self.upsert(key, value);
		Ok(())
	}

	/// 删第 `nth` 个控制点（nth 越界 → Err(())）；随后重算 slope。
	pub fn delete_nth(&mut self, nth: usize) -> Result<(), ()> {
		if nth >= self.points.len() {
			return Err(());
		}
		self.points.remove(nth);
		self.recompute_slopes();
		Ok(())
	}

	/// 删除全部控制点（`DeleteAllControlPoints`）。
	pub fn clear(&mut self) {
		self.points.clear();
	}
}

// ---- 曲线集 ↔ JSON（节点输入 / 工程序列化的值载荷）--------------------
//
// 格式（确定性、紧凑，无空白）：
//   {"curves":[[{"key":0.0,"value":0.0,"slope":1.0},...],...]}
// 外层按 dimension 顺序一维一条曲线；每维一个控制点对象，字段序固定
// key/value/slope。数值用 Rust 最短往返格式化（`{}`）。非有限值 JSON
// 无标准写法，取对称 token：NaN → `null`，+Inf → `"inf"`，
// -Inf → `"-inf"`（字符串字面量；解析器对称处理，有限值恒为裸数字）。
// 解析器容忍空白与字段乱序；结构不符 → None（调用方按字符串族静默
// 路径处理）。手写实现，不引入 serde（crate 无该依赖）。

/// 曲线集 → JSON（节点输入默认值 / 插件回写节点输入的载荷）。
pub fn curves_to_json(curves: &[Curve]) -> String {
	let mut out =
		String::with_capacity(8 + 12 * curves.iter().map(|c| c.len()).sum::<usize>());
	out.push_str("{\"curves\":[");
	for (ci, c) in curves.iter().enumerate() {
		if ci > 0 {
			out.push(',');
		}
		out.push('[');
		for (pi, p) in c.points.iter().enumerate() {
			if pi > 0 {
				out.push(',');
			}
			out.push_str("{\"key\":");
			push_num(&mut out, p.key);
			out.push_str(",\"value\":");
			push_num(&mut out, p.value);
			out.push_str(",\"slope\":");
			push_num(&mut out, p.slope);
			out.push('}');
		}
		out.push(']');
	}
	out.push_str("]}");
	out
}

/// JSON → 曲线集（格式见 [`curves_to_json`]；结构不符 → None）。
pub fn curves_from_json(text: &str) -> Option<Vec<Curve>> {
	let mut p = JsonParser {
		bytes: text.as_bytes(),
		pos: 0,
	};
	p.skip_ws();
	if !p.eat(b'{') {
		return None;
	}
	p.skip_ws();
	if p.parse_string()?.as_str() != "curves" {
		return None;
	}
	p.skip_ws();
	if !p.eat(b':') {
		return None;
	}
	p.skip_ws();
	let mut curves = Vec::new();
	if p.eat(b'[') {
		loop {
			p.skip_ws();
			if p.eat(b']') {
				break;
			}
			curves.push(p.parse_curve()?);
			p.skip_ws();
			if p.eat(b',') {
				continue;
			}
			if p.eat(b']') {
				break;
			}
			return None;
		}
	} else {
		return None;
	}
	p.skip_ws();
	if !p.eat(b'}') {
		return None;
	}
	p.skip_ws();
	if p.pos != p.bytes.len() {
		return None;
	}
	Some(curves)
}

/// 单个数值 → JSON token（有限值裸数字；NaN/±Inf 见模块文档）。
fn push_num(out: &mut String, v: f64) {
	if v.is_nan() {
		out.push_str("null");
	} else if v == f64::INFINITY {
		out.push_str("\"inf\"");
	} else if v == f64::NEG_INFINITY {
		out.push_str("\"-inf\"");
	} else {
		out.push_str(&format!("{v}"));
	}
}

/// 手写 JSON 解析器（仅本格式的子集；字段名无转义引号，控制点字段
/// 均为固定标识符）。
struct JsonParser<'a> {
	bytes: &'a [u8],
	pos: usize,
}

impl<'a> JsonParser<'a> {
	fn skip_ws(&mut self) {
		while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
			self.pos += 1;
		}
	}

	fn eat(&mut self, byte: u8) -> bool {
		if self.bytes.get(self.pos) == Some(&byte) {
			self.pos += 1;
			true
		} else {
			false
		}
	}

	/// 原样匹配一段字节（字段名；不做字符串语义）。
	fn eat_str(&mut self, want: &[u8]) -> bool {
		if self.bytes[self.pos..].starts_with(want) {
			self.pos += want.len();
			true
		} else {
			false
		}
	}

	/// 引号字符串（无转义；字段名/`"inf"` token 用）。
	fn parse_string(&mut self) -> Option<String> {
		if !self.eat(b'"') {
			return None;
		}
		let start = self.pos;
		while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
			self.pos += 1;
		}
		let s = std::str::from_utf8(&self.bytes[start..self.pos]).ok()?.to_string();
		if !self.eat(b'"') {
			return None;
		}
		Some(s)
	}

	/// 数值 token：`null` → NaN；`"inf"`/`"-inf"` → ±∞；否则裸数字
	/// （`f64::from_str`，上溢 → ±∞，与 `{}` 最短往返格式对称）。
	fn parse_num(&mut self) -> Option<f64> {
		if self.eat_str(b"null") {
			return Some(f64::NAN);
		}
		if self.bytes.get(self.pos) == Some(&b'"') {
			return match self.parse_string()?.as_str() {
				"inf" => Some(f64::INFINITY),
				"-inf" => Some(f64::NEG_INFINITY),
				_ => None,
			};
		}
		let start = self.pos;
		while self.pos < self.bytes.len()
			&& matches!(
				self.bytes[self.pos],
				b'-' | b'+' | b'.' | b'0'..=b'9' | b'e' | b'E'
			)
		{
			self.pos += 1;
		}
		if self.pos == start {
			return None;
		}
		std::str::from_utf8(&self.bytes[start..self.pos])
			.ok()?
			.parse()
			.ok()
	}

	/// 一条曲线：`[` 控制点* `]`（控制点按 key 升序存储，解析按原序）。
	fn parse_curve(&mut self) -> Option<Curve> {
		if !self.eat(b'[') {
			return None;
		}
		let mut points = Vec::new();
		loop {
			self.skip_ws();
			if self.eat(b']') {
				break;
			}
			points.push(self.parse_point()?);
			self.skip_ws();
			if self.eat(b',') {
				continue;
			}
			if self.eat(b']') {
				break;
			}
			return None;
		}
		Some(Curve { points })
	}

	/// 一个控制点：`{` ("name" `:` 数值)* `}`（字段乱序可；未知字段
	/// 拒绝）。
	fn parse_point(&mut self) -> Option<ControlPoint> {
		if !self.eat(b'{') {
			return None;
		}
		let mut key = None;
		let mut value = None;
		let mut slope = None;
		loop {
			self.skip_ws();
			if self.eat(b'}') {
				break;
			}
			let name = self.parse_string()?;
			self.skip_ws();
			if !self.eat(b':') {
				return None;
			}
			self.skip_ws();
			let v = self.parse_num()?;
			match name.as_str() {
				"key" => key = Some(v),
				"value" => value = Some(v),
				"slope" => slope = Some(v),
				_ => return None,
			}
			self.skip_ws();
			if self.eat(b',') {
				continue;
			}
			if self.eat(b'}') {
				break;
			}
			return None;
		}
		Some(ControlPoint {
			key: key?,
			value: value?,
			slope: slope.unwrap_or(0.0),
		})
	}
}

/// 第 `i` 点处的差分斜率（不重算，供 [`Curve::recompute_slopes`]）。
/// 内部点：中心差分 (y_{i+1} - y_{i-1}) / (x_{i+1} - x_{i-1})；
/// 端点：单侧差分；单点/空曲线：0。防御重复 key 时除零 → 0。
fn slope_at(points: &[ControlPoint], i: usize) -> f64 {
	let n = points.len();
	if n < 2 {
		return 0.0;
	}
	let (a, b) = if i == 0 {
		(&points[0], &points[1])
	} else if i == n - 1 {
		(&points[n - 2], &points[n - 1])
	} else {
		(&points[i - 1], &points[i + 1])
	};
	let dx = b.key - a.key;
	if dx == 0.0 {
		return 0.0;
	}
	(b.value - a.value) / dx
}

#[cfg(test)]
mod tests {
	use super::*;

	fn close(a: f64, b: f64, eps: f64) -> bool {
		(a - b).abs() <= eps
	}

	/// 恒等曲线：{(0,0),(1,1)} + 自动 slope（1,1）→ Hermite 恰为
	/// f(x) = x（数学上精确，浮点逐位不保证——近似比较）；非默认
	/// range 同样恒等。
	#[test]
	fn identity_curve_evaluates_to_x() {
		let c = Curve::identity(0.0, 1.0);
		assert_eq!(c.len(), 2);
		assert_eq!(
			c.points[0],
			ControlPoint {
				key: 0.0,
				value: 0.0,
				slope: 1.0
			}
		);
		assert_eq!(
			c.points[1],
			ControlPoint {
				key: 1.0,
				value: 1.0,
				slope: 1.0
			}
		);
		for x in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
			assert!(close(c.evaluate(x), x, 1e-12), "identity at {x}");
		}
		// 非默认 range（插件改 OfxParamPropParametricRange 后，
		// 新曲线的默认 default 相应平移）。
		let c = Curve::identity(0.0, 255.0);
		assert!(close(c.evaluate(128.0), 128.0, 1e-12));
	}

	/// 单段抛物线形状：{(0,0),(0.5,0.25),(1,1)}（y = x² 采样）。
	/// 自动 slope = (0.5, 1, 1.5)（端点单侧、中点中心差分）；
	/// 求值精确等于分段三次 Hermite 的解析值（过控制点、两侧
	/// 单调递增、中点手算值）。
	#[test]
	fn parabola_shape_honours_hermite() {
		let c = Curve::from_pairs(&[(0.0, 0.0), (0.5, 0.25), (1.0, 1.0)]);
		assert_eq!(
			c.points.iter().map(|p| p.slope).collect::<Vec<_>>(),
			vec![0.5, 1.0, 1.5]
		);
		// 插值性质：控制点处精确命中。
		assert_eq!(c.evaluate(0.0), 0.0);
		assert_eq!(c.evaluate(0.5), 0.25);
		assert_eq!(c.evaluate(1.0), 1.0);
		// 中点解析值（t = 1/2 段内，精确二进制分数）。
		assert_eq!(c.evaluate(0.125), 0.05078125);
		assert_eq!(c.evaluate(0.25), 0.09375);
		assert_eq!(c.evaluate(0.75), 0.59375);
		// 形状：单调递增；首段在抛物线 y = x² 之上（Hermite 端点
		// 用弦斜率、中点用中心差分 → 首段整体上凸）。
		let xs = [0.1, 0.2, 0.3, 0.4, 0.6, 0.7, 0.8, 0.9];
		for w in xs.windows(2) {
			assert!(c.evaluate(w[0]) < c.evaluate(w[1]));
		}
		assert!(c.evaluate(0.25) > 0.25 * 0.25); // 高于抛物线采样点
	}

	/// 越界钳制：x 在首/末 key 之外 → 端点值。
	#[test]
	fn out_of_range_clamps_to_endpoints() {
		let c = Curve::from_pairs(&[(0.2, 0.3), (0.8, 0.9)]);
		assert_eq!(c.evaluate(-100.0), 0.3);
		assert_eq!(c.evaluate(0.19), 0.3);
		assert_eq!(c.evaluate(0.2), 0.3);
		assert_eq!(c.evaluate(0.81), 0.9);
		assert_eq!(c.evaluate(100.0), 0.9);
	}

	/// 增删点后求值正确：upsert 新点 → 该点命中、重排有序；删除后
	/// 恢复；同 key 覆盖不增点。
	#[test]
	fn upsert_and_delete_keep_evaluation_consistent() {
		let mut c = Curve::from_pairs(&[(0.0, 0.0), (1.0, 1.0)]);
		// 插入中点（返回 true = 新增），求值在其 key 处精确命中。
		assert!(c.upsert(0.3, 0.7));
		assert_eq!(c.len(), 3);
		assert_eq!(c.evaluate(0.3), 0.7);
		// 有序性保持。
		let keys: Vec<f64> = c.points.iter().map(|p| p.key).collect();
		assert_eq!(keys, vec![0.0, 0.3, 1.0]);
		// 同 key 覆盖（返回 false = 覆盖），数量不变。
		assert!(!c.upsert(0.3, 0.5));
		assert_eq!(c.len(), 3);
		assert_eq!(c.evaluate(0.3), 0.5);
		// 删除 → 恢复双点（恒等；非 2 幂 key 处逐位有舍入）。
		c.delete_nth(1).unwrap();
		assert_eq!(c.len(), 2);
		assert!(close(c.evaluate(0.3), 0.3, 1e-12));
		// 越界删除 → Err。
		assert!(c.delete_nth(2).is_err());
	}

	/// set_nth 改 key 后保持有序（点在序列中移动）。
	#[test]
	fn set_nth_reorders_on_key_change() {
		let mut c = Curve::from_pairs(&[(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]);
		// 把第 0 点挪到 0.75（原第 1、2 点之前……之后）。
		c.set_nth(0, 0.75, 0.75).unwrap();
		let pairs: Vec<(f64, f64)> = c
			.points
			.iter()
			.map(|p| (p.key, p.value))
			.collect();
		assert_eq!(pairs, vec![(0.5, 0.5), (0.75, 0.75), (1.0, 1.0)]);
		assert_eq!(c.evaluate(0.75), 0.75);
		// 越界 nth → Err。
		assert!(c.set_nth(3, 0.9, 0.9).is_err());
	}

	/// slope 编辑生效：同为 {(0,0),(1,1)}，自动 slope (1,1) 时
	/// 求值 = 恒等；把 slope 显式压平为 0 后曲线变 S 形缓起缓收
	/// （中点值从 0.25 变 0.15625）。
	#[test]
	fn slope_editing_changes_evaluation() {
		let mut c = Curve::from_pairs(&[(0.0, 0.0), (1.0, 1.0)]);
		assert_eq!(c.evaluate(0.25), 0.25);
		// 显式编辑 slope（宿主/测试路径；suite 无 slope 入口，
		// 但字段公开即契约的一部分）。
		c.points[0].slope = 0.0;
		c.points[1].slope = 0.0;
		assert_eq!(c.evaluate(0.25), 0.15625);
		assert_eq!(c.evaluate(0.5), 0.5);
		assert!(close(c.evaluate(0.25), 0.15625, 1e-12));
	}

	/// 空曲线与单点曲线：恒等 / 常数退化。
	#[test]
	fn degenerate_curves() {
		let c = Curve::empty();
		assert!(c.is_empty());
		assert_eq!(c.evaluate(0.3), 0.3);
		assert_eq!(c.evaluate(5.0), 5.0);
		let c = Curve::from_pairs(&[(0.5, 0.25)]);
		assert_eq!(c.evaluate(0.0), 0.25);
		assert_eq!(c.evaluate(100.0), 0.25);
	}

	/// clear：DeleteAllControlPoints 的模型侧语义。
	#[test]
	fn clear_empties_curve() {
		let mut c = Curve::from_pairs(&[(0.0, 0.0), (1.0, 1.0)]);
		c.clear();
		assert!(c.is_empty());
		assert_eq!(c.evaluate(0.5), 0.5);
	}

	/// JSON 往返：恒等曲线（默认值）序列化形状逐字 + 解析回等值模型。
	#[test]
	fn json_roundtrip_identity() {
		let curves = vec![Curve::identity(0.0, 1.0)];
		let json = curves_to_json(&curves);
		assert_eq!(
			json,
			r#"{"curves":[[{"key":0,"value":0,"slope":1},{"key":1,"value":1,"slope":1}]]}"#
		);
		let back = curves_from_json(&json).expect("应可解析");
		assert_eq!(back, curves);
	}

	/// JSON 往返：多维 + 非平凡形状 + 显式编辑 slope（slope 也是载荷
	/// 的一部分，逐位保留）。
	#[test]
	fn json_roundtrip_multidim_and_slope() {
		let mut c = Curve::from_pairs(&[(0.0, 0.0), (0.5, 0.25), (1.0, 1.0)]);
		c.points[1].slope = 0.0; // 显式编辑（字段公开即契约）
		let curves = vec![c.clone(), Curve::identity(0.0, 255.0)];
		let json = curves_to_json(&curves);
		let back = curves_from_json(&json).expect("应可解析");
		assert_eq!(back.len(), 2);
		assert_eq!(back, curves);
		assert_eq!(back[0].points[1].slope, 0.0);
		assert_eq!(back[1].points[1].key, 255.0);
		// 求值不受序列化影响。
		assert_eq!(back[0].evaluate(0.5), 0.25);
	}

	/// JSON 往返：特殊值（NaN/±Inf，slope 与值均可）与空曲线/空集。
	#[test]
	fn json_roundtrip_special_and_empty() {
		let curves = vec![Curve {
			points: vec![
				ControlPoint {
					key: 0.0,
					value: f64::NAN,
					slope: 0.0,
				},
				ControlPoint {
					key: 1.0,
					value: f64::INFINITY,
					slope: f64::NEG_INFINITY,
				},
			],
		}];
		let json = curves_to_json(&curves);
		assert_eq!(
			json,
			r#"{"curves":[[{"key":0,"value":null,"slope":0},{"key":1,"value":"inf","slope":"-inf"}]]}"#
		);
		let back = curves_from_json(&json).expect("应可解析");
		assert!(back[0].points[0].value.is_nan());
		assert_eq!(back[0].points[1].value, f64::INFINITY);
		assert_eq!(back[0].points[1].slope, f64::NEG_INFINITY);

		// 空曲线（DeleteAll 后）与空集。
		let curves = vec![Curve::empty(), Curve::empty()];
		let json = curves_to_json(&curves);
		assert_eq!(json, r#"{"curves":[[],[]]}"#);
		assert_eq!(curves_from_json(&json).unwrap(), curves);
		assert_eq!(curves_from_json(r#"{"curves":[]}"#).unwrap(), Vec::<Curve>::new());
	}

	/// JSON 解析：容忍空白与字段乱序；结构不符 → None（静默路径）。
	#[test]
	fn json_parser_tolerances() {
		let curves = vec![Curve::from_pairs(&[(0.0, 0.0), (1.0, 1.0)])];
		// 空白 + 字段乱序。
		let spaced = r#"{ "curves" : [ [ { "slope" : 1 , "key" : 0 , "value" : 0 } , { "key" : 1 , "value" : 1 , "slope" : 1 } ] ] }"#;
		assert_eq!(curves_from_json(spaced).unwrap(), curves);
		// 结构不符 → None。
		for bad in [
			"",
			"{}",
			r#"{"curves"}"#,
			r#"{"curve":[]}"#,
			r#"{"curves":[{"key":0}]}"#,
			r#"{"curves":[[{"key":"x"}]]}"#,
			r#"{"curves":[[{"key":0}]]"#,
			r#"{"curves":[[{"key":0,"value":0,"slope":0}]]}junk"#,
		] {
			assert!(curves_from_json(bad).is_none(), "应拒绝：{bad}");
		}
		// 缺 slope 字段 → 0（宽容）。
		let no_slope = r#"{"curves":[[{"key":0,"value":0}]]}"#;
		let back = curves_from_json(no_slope).unwrap();
		assert_eq!(back[0].points[0].slope, 0.0);
	}
}
