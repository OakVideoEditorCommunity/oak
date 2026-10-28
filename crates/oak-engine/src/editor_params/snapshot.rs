//! Domain-side capture of declared parameters and their animation data.

use oak_node::id::NodeId;
use oak_node::node::ENABLED_INPUT;
use oak_node::value::{NodeValue, OakNodeValue, ValueType};

use crate::coretypes::OakRational;
use crate::editor_commands::ProjectRef;
use crate::handle::NativeMirror;

/// POD and string representations of one value. Non-POD types use `text`.
#[derive(Clone, Debug, Default)]
pub struct ParamValue {
	pub pod: Option<OakNodeValue>,
	pub text: Option<String>,
}

/// One named input property, including combo options and UI grouping metadata.
#[derive(Clone, Debug)]
pub struct ParamProperty {
	pub key: String,
	pub value: ParamValue,
}

/// One keyframe on an (input, element) track.
#[derive(Clone, Debug)]
pub struct ParamKeyframe {
	pub time: OakRational,
	pub value: ParamValue,
	/// 0 = hold, 1 = linear, 2 = cubic bezier.
	pub interpolation: i32,
	pub bezier_in: (f64, f64),
	pub bezier_out: (f64, f64),
}

/// One declared parameter and all of its current values and animation tracks.
#[derive(Clone, Debug)]
pub struct Param {
	pub input_id: String,
	pub label: String,
	pub value_type: i32,
	pub flags: u32,
	pub properties: Vec<ParamProperty>,
	pub values: Vec<(i32, ParamValue)>,
	pub tracks: Vec<(i32, Vec<ParamKeyframe>)>,
}

/// Immutable parameter state for one node at one undo revision.
#[derive(Clone, Debug)]
pub(crate) struct ParamSnapshot {
	pub revision: i64,
	pub params: Vec<Param>,
}

/// Converts a domain value without erasing string-carried values.
fn value(declared: ValueType, value: &NodeValue) -> ParamValue {
	let text = match value {
		NodeValue::Text(value) | NodeValue::StrCombo(value) => Some(value.clone()),
		_ => None,
	};
	ParamValue {
		pod: OakNodeValue::from_node_value(declared, value).ok(),
		text,
	}
}

/// Captures visible parameters, excluding stream inputs, hidden inputs and the
/// structural enabled toggle. Behavior labels and combo strings are preserved;
/// application language-pack substitution remains an app concern.
pub(crate) fn build(project: &ProjectRef, node: NodeId) -> Option<ParamSnapshot> {
	let revision = oak_undo::global::index().unwrap_or(0);
	let guard = project.lock().unwrap_or_else(|e| e.into_inner());
	let entry = guard.graph.get(node)?;
	let behavior = &entry.behavior;
	let core = &entry.core;
	let params = core
		.inputs
		.iter()
		.filter(|input| {
			!matches!(input.value_type, ValueType::Texture | ValueType::Samples | ValueType::Matrix)
				&& input.flags & oak_node::input::flags::HIDDEN == 0
				&& input.id != ENABLED_INPUT
		})
		.map(|input| {
			let behavior_name = behavior.input_name(&input.id);
			let label = if behavior_name != input.id {
				behavior_name.to_string()
			} else {
				input.display_name.clone()
			};
			let mut properties: Vec<ParamProperty> = input
				.properties
				.iter()
				.map(|(key, value)| ParamProperty {
					key: key.clone(),
					value: self::value(input.value_type, value),
				})
				.collect();
			if input.value_type == ValueType::Combo
				&& !properties.iter().any(|property| property.key == "combo_option")
			{
				properties.extend(behavior.input_combo_strings(&input.id).into_iter().map(|option| {
					ParamProperty {
						key: "combo_option".into(),
						value: ParamValue {
							pod: None,
							text: Some(option.to_string()),
						},
					}
				}));
			}
			let mut values: Vec<(i32, ParamValue)> = core
				.standard_values
				.iter()
				.filter(|((id, _), _)| id == &input.id)
				.map(|((_, element), value)| {
					(*element, self::value(input.value_type, value))
				})
				.collect();
			if !values.iter().any(|(element, _)| *element == -1) {
				values.push((-1, self::value(input.value_type, &input.default)));
			}
			values.sort_by_key(|(element, _)| *element);
			let tracks = core
				.keyframes
				.iter()
				.filter(|(id, _, _)| id == &input.id)
				.map(|(_, element, track)| {
					let keys = track
						.keys()
						.iter()
						.map(|key| ParamKeyframe {
							time: OakRational::from_native(&key.time),
							value: self::value(input.value_type, &key.value),
							interpolation: match key.interpolation {
								oak_node::keyframe::Interpolation::Hold => 0,
								oak_node::keyframe::Interpolation::Linear => 1,
								oak_node::keyframe::Interpolation::Bezier => 2,
							},
							bezier_in: key.bezier_in,
							bezier_out: key.bezier_out,
						})
						.collect();
					(*element, keys)
				})
				.collect();
			Param {
				input_id: input.id.clone(),
				label,
				value_type: match input.value_type {
					ValueType::NodeRef => 23,
					ValueType::Parametric => 24,
					other => other.to_cpp_discriminant(),
				},
				flags: input.flags,
				properties,
				values,
				tracks,
			}
		})
		.collect();
	Some(ParamSnapshot { revision, params })
}
