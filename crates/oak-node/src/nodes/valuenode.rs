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

//! Constant-value generator node (C++
//! `src/node/src/input/value/valuenode.{h,cpp}`, `olive::ValueNode`).

use crate::factory::NodeMeta;
use crate::node::{Category, NodeBehavior, NodeCore};
use crate::value::ValueType;

/// Type selector input id (C++ `k_type_input`). Type: combo; default
/// `0` (the first entry of [`SUPPORTED_TYPES`]); flags: not-connectable,
/// not-keyframable. Combo strings are the pretty data-type names of the
/// supported types (built in `retranslate()`).
pub const TYPE_INPUT: &str = "type_in";

/// Value input id (C++ `k_value_input`). Type: initially
/// `SUPPORTED_TYPES[0]` (float), switched by `input_value_changed` when
/// `type_in` changes; default: empty variant; flags: not-connectable.
pub const VALUE_INPUT: &str = "value_in";

/// Selectable value types (C++ `k_supported_types`). The C++ list is:
/// float, int, rational, vec2, vec3, vec4, color, text, matrix, font,
/// boolean. The Rust [`ValueType`] enum has no `Matrix` or `Font`
/// variants, so those two entries are omitted here (a behavioral gap to
/// resolve when matrix/font values land in `value.rs`).
pub const SUPPORTED_TYPES: &[ValueType] = &[
	ValueType::Float,
	ValueType::Int,
	ValueType::Rational,
	ValueType::Vec2,
	ValueType::Vec3,
	ValueType::Vec4,
	ValueType::Color,
	ValueType::Text,
	ValueType::Boolean,
];

/// Value node. Holds a single typed constant that can be connected to
/// other nodes' inputs. The C++ class has no own members (its state
/// lives entirely in the two inputs), so this is a unit-like struct.
pub struct ValueNode;

impl NodeBehavior for ValueNode {
	/// Human-readable name (C++ `name()`).
	fn name(&self) -> &str {
		"Value"
	}

	/// Stable type id (C++ `id()`).
	fn type_id(&self) -> &str {
		"org.olivevideoeditor.Olive.value"
	}

	/// Categories (C++ `category()` returns
	/// `{ k_category_generator }`).
	fn categories(&self) -> &[Category] {
		&[Category::Generator]
	}

	/// Description (C++ `description()`).
	fn description(&self) -> &str {
		"Create a single value that can be connected to various other inputs."
	}

	/// Localized input names (C++ `retranslate()`): `type_in` ->
	/// "Type", `value_in` -> "Value". The C++ override also sets the
	/// `type_in` combo strings to the pretty names of the supported
	/// types — that part has no trait surface and is noted here only.
	fn input_name<'a>(&self, id: &'a str) -> &'a str {
		match id {
			TYPE_INPUT => "Type",
			VALUE_INPUT => "Value",
			_ => id,
		}
	}

	/// Combo input option labels (C++ `retranslate()` /
	/// `set_combo_box_strings`): `type_in` -> the pretty data-type names
	/// of [`SUPPORTED_TYPES`] in order — "Float", "Integer", "Rational",
	/// "Vector 2D", "Vector 3D", "Vector 4D", "Color", "Text", "Boolean"
	/// (matching the Rust list, which omits the C++ `k_matrix`/`k_font`
	/// entries).
	fn input_combo_strings(&self, id: &str) -> Vec<&'static str> {
		match id {
			TYPE_INPUT => vec![
				"Float",
				"Integer",
				"Rational",
				"Vector 2D",
				"Vector 3D",
				"Vector 4D",
				"Color",
				"Text",
				"Boolean",
			],
			_ => Vec::new(),
		}
	}

	/// Evaluate outputs (C++ `value()`): pushes the `value_in` value
	/// onto the table unchanged.
	fn value(
		&self,
		core: &NodeCore,
		inputs: &crate::value::NodeValueRow,
		time: oak_core::Rational,
		table: &mut crate::value::NodeValueTable,
	) {
		let _ = inputs;
		let v = core.value_at_time(VALUE_INPUT, -1, time);
		table.push(v.value_type(), v, None);
	}

	/// Input value changed (C++ `InputValueChangedEvent()`): when
	/// `type_in` changes, sets the data type of `value_in` to
	/// `SUPPORTED_TYPES[type index]`; then defers to the base-class
	/// behavior.
	fn input_value_changed(&mut self, core: &mut NodeCore, input: &str, element: i32) {
		if input == TYPE_INPUT && element == -1 {
			let idx = core.standard_value(TYPE_INPUT, -1).to_double() as usize;
			if let Some(ty) = SUPPORTED_TYPES.get(idx) {
				if let Some(value_in) = core.get_input_mut(VALUE_INPUT) {
					value_in.value_type = *ty;
				}
			}
		}
	}

	/// Deep copy (C++ `copy()` via `NODE_DEFAULT_FUNCTIONS`).
	fn duplicate(&self, _core: &NodeCore) -> Option<Box<dyn NodeBehavior>> {
		Some(Box::new(ValueNode))
	}
}

/// Constructor (C++ `ValueNode::ValueNode()`): adds `type_in` and
/// `value_in` with the defaults, flags and properties documented on the
/// constants.
pub fn create() -> (NodeCore, Box<dyn NodeBehavior>) {
	let mut core = NodeCore::new();

	let mut type_input = crate::input::Input::new(
		TYPE_INPUT,
		ValueType::Combo,
		crate::value::NodeValue::Combo(0),
	);
	type_input.flags |= crate::input::flags::NOT_CONNECTABLE | crate::input::flags::NOT_KEYFRAMABLE;
	core.add_input(type_input);

	let mut value_input = crate::input::Input::new(
		VALUE_INPUT,
		SUPPORTED_TYPES[0],
		crate::value::NodeValue::None,
	);
	value_input.flags |= crate::input::flags::NOT_CONNECTABLE;
	core.add_input(value_input);

	(core, Box::new(ValueNode))
}

/// Register this node type (C++ `k_value_node` in
/// `factory.cpp::create_from_factory_index`).
pub fn register(meta: &mut Vec<NodeMeta>) {
	meta.push(NodeMeta {
		type_id: "org.olivevideoeditor.Olive.value",
		name: "Value",
		categories: &[Category::Generator],
		create,
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::node::NodeBehavior;
	use crate::value::{NodeValue, NodeValueTable, ValueType};
	use oak_core::Rational;

	#[test]
	fn input_names() {
		let n = ValueNode;
		assert_eq!(n.input_name(TYPE_INPUT), "Type");
		assert_eq!(n.input_name(VALUE_INPUT), "Value");
	}

	#[test]
	fn create_wires_inputs() {
		let (core, behavior) = create();
		assert_eq!(behavior.type_id(), "org.olivevideoeditor.Olive.value");
		assert_eq!(
			core.get_input(VALUE_INPUT).unwrap().value_type,
			ValueType::Float
		);
		assert_eq!(
			core.get_input(TYPE_INPUT).unwrap().flags & crate::input::flags::NOT_CONNECTABLE,
			crate::input::flags::NOT_CONNECTABLE
		);
	}

	#[test]
	fn value_pushes_standard_value() {
		let (mut core, behavior) = create();
		core.set_standard_value(VALUE_INPUT, -1, NodeValue::Float(3.5));
		let mut table = NodeValueTable::default();
		behavior.value(
			&core,
			&crate::value::NodeValueRow::default(),
			Rational::new(0, 1),
			&mut table,
		);
		assert_eq!(table.get(ValueType::Float), Some(&NodeValue::Float(3.5)));
	}

	#[test]
	fn value_pushes_keyframed_value() {
		let (mut core, behavior) = create();
		core.keyframe_track_mut(VALUE_INPUT, -1)
			.set_key(crate::keyframe::Keyframe {
				time: Rational::new(10, 1),
				value: NodeValue::Float(9.0),
				interpolation: crate::keyframe::Interpolation::Linear,
				bezier_in: (0.0, 0.0),
				bezier_out: (0.0, 0.0),
			});
		let mut table = NodeValueTable::default();
		behavior.value(
			&core,
			&crate::value::NodeValueRow::default(),
			Rational::new(10, 1),
			&mut table,
		);
		assert_eq!(table.get(ValueType::Float), Some(&NodeValue::Float(9.0)));
	}

	#[test]
	fn input_value_changed_switches_type() {
		let (mut core, behavior) = create();
		let mut behavior = behavior;
		// Change type_in to vec2 (index 3) and fire the event.
		core.set_standard_value(TYPE_INPUT, -1, NodeValue::Combo(3));
		behavior.input_value_changed(&mut core, TYPE_INPUT, -1);
		assert_eq!(
			core.get_input(VALUE_INPUT).unwrap().value_type,
			ValueType::Vec2
		);

		// Out-of-range index leaves the type unchanged.
		core.set_standard_value(TYPE_INPUT, -1, NodeValue::Combo(99));
		behavior.input_value_changed(&mut core, TYPE_INPUT, -1);
		assert_eq!(
			core.get_input(VALUE_INPUT).unwrap().value_type,
			ValueType::Vec2
		);
	}

	#[test]
	fn duplicate_clones() {
		let (core, behavior) = create();
		let dup = behavior.duplicate(&core).unwrap();
		assert_eq!(dup.name(), "Value");
	}
}
