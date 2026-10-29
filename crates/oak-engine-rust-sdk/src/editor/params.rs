//! Owned parameter metadata, values, properties and keyframe snapshots.

use std::ffi::c_void;

use crate::codec::string_out;
use crate::editor::EditorSession;
use crate::node::{NodeId, NodeValue};
use crate::types::Rational;

mod raw;
#[cfg(test)]
mod tests;

use raw::*;

/// Declared parameter value vocabulary. Unknown future discriminants are
/// preserved instead of being collapsed into a valid built-in type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterValueType {
	/// No value.
	None,
	/// An integer value.
	Int,
	/// A floating-point value.
	Float,
	/// A rational-seconds value.
	Rational,
	/// A boolean value.
	Boolean,
	/// An RGBA color.
	Color,
	/// A 4x4 matrix.
	Matrix,
	/// A text value.
	Text,
	/// A texture sample.
	Texture,
	/// An audio sample buffer.
	Samples,
	/// A two-component vector.
	Vec2,
	/// A three-component vector.
	Vec3,
	/// A four-component vector.
	Vec4,
	/// An integer combo selection.
	Combo,
	/// A string combo selection.
	StrCombo,
	/// Video stream parameters.
	VideoParams,
	/// Audio stream parameters.
	AudioParams,
	/// Opaque binary data.
	Binary,
	/// A push-button action.
	PushButton,
	/// A node reference.
	NodeRef,
	/// A parametric curve.
	Parametric,
	/// A value of a type not modeled here.
	Other(i32),
}

impl ParameterValueType {
	fn from_raw(raw: i32) -> Self {
		match raw {
			0 => Self::None,
			1 => Self::Int,
			2 => Self::Float,
			3 => Self::Rational,
			4 => Self::Boolean,
			5 => Self::Color,
			6 => Self::Matrix,
			7 => Self::Text,
			10 => Self::Texture,
			11 => Self::Samples,
			12 => Self::Vec2,
			13 => Self::Vec3,
			14 => Self::Vec4,
			16 => Self::Combo,
			17 => Self::StrCombo,
			18 => Self::VideoParams,
			19 => Self::AudioParams,
			21 => Self::Binary,
			22 => Self::PushButton,
			23 => Self::NodeRef,
			24 => Self::Parametric,
			other => Self::Other(other),
		}
	}
}

/// POD and/or string representation of a parameter value.
#[derive(Clone, Debug)]
pub struct ParameterValue {
	/// The `pod` field.
	pub pod: Option<NodeValue>,
	/// The `text` field.
	pub text: Option<String>,
}

/// One input property such as a combo option, UI group or numeric bound.
#[derive(Clone, Debug)]
pub struct ParameterProperty {
	/// The `key` field.
	pub key: String,
	/// The `value` field.
	pub value: ParameterValue,
}

/// Interpolation to the next keyframe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyframeInterpolation {
	/// Holds the previous value until the next key.
	Hold,
	/// Interpolates linearly between keys.
	Linear,
	/// Interpolates with Bezier handles.
	Bezier,
}

/// One owned keyframe with relative bezier handles.
#[derive(Clone, Debug)]
pub struct ParameterKeyframe {
	/// The `time` field.
	pub time: Rational,
	/// The `value` field.
	pub value: ParameterValue,
	/// The `interpolation` field.
	pub interpolation: KeyframeInterpolation,
	/// The `bezier_in` field.
	pub bezier_in: (f64, f64),
	/// The `bezier_out` field.
	pub bezier_out: (f64, f64),
}

/// One visible declared parameter and all of its element data.
#[derive(Clone, Debug)]
pub struct Parameter {
	/// The `input_id` field.
	pub input_id: String,
	/// The `label` field.
	pub label: String,
	/// The `value_type` field.
	pub value_type: ParameterValueType,
	/// The `flags` field.
	pub flags: u32,
	/// The `properties` field.
	pub properties: Vec<ParameterProperty>,
	/// Static values keyed by element; scalar inputs use element `-1`.
	pub values: Vec<(i32, ParameterValue)>,
	/// Keyframe tracks keyed by element, sorted as stored by the domain track.
	pub keyframes: Vec<(i32, Vec<ParameterKeyframe>)>,
}

impl Parameter {
	/// Returns the static scalar value for an ordinary parameter.
	pub fn scalar_value(&self) -> Option<&ParameterValue> {
		self.values.iter().find(|(element, _)| *element == -1).map(|(_, value)| value)
	}

	/// Returns the keyframe track for `element`, if one exists.
	pub fn track(&self, element: i32) -> Option<&[ParameterKeyframe]> {
		self.keyframes
			.iter()
			.find(|(index, _)| *index == element)
			.map(|(_, keys)| keys.as_slice())
	}
}

/// Immutable parameter state for one node at `revision`.
#[derive(Clone, Debug)]
pub struct ParameterSnapshot {
	/// The `revision` field.
	pub revision: i64,
	/// The `params` field.
	pub params: Vec<Parameter>,
}

impl ParameterSnapshot {
	/// Finds a parameter by its stable input ID.
	pub fn get(&self, input_id: &str) -> Option<&Parameter> {
		self.params.iter().find(|param| param.input_id == input_id)
	}
}

struct ParamsHandle(*mut c_void);

impl Drop for ParamsHandle {
	fn drop(&mut self) {
		unsafe { oak_editor_params_release(self.0) };
	}
}

fn convert_value(raw: ValueRaw, text: Option<String>) -> ParameterValue {
	ParameterValue {
		pod: raw.has_pod.then_some(raw.pod),
		text,
	}
}

impl ParameterSnapshot {
	unsafe fn from_handle(handle: ParamsHandle) -> Self {
		let revision = unsafe { oak_editor_params_revision(handle.0) };
		let params = (0..unsafe { oak_editor_params_count(handle.0) }.max(0))
			.filter_map(|index| {
				let mut raw = ParamRaw::default();
				if !unsafe { oak_editor_params_at(handle.0, index, &mut raw) } {
					return None;
				}
				let input_id = string_out(|buf, len| unsafe {
					oak_editor_params_input_at(handle.0, index, buf, len)
				})
				.unwrap_or_default();
				let properties = (0..raw.property_count.max(0))
					.filter_map(|property_index| {
						let mut property = PropertyRaw::default();
						if !unsafe {
							oak_editor_params_property_at(handle.0, index, property_index, &mut property)
						} {
							return None;
						}
						let text = property.value.has_text.then(|| {
							string_out(|buf, len| unsafe {
								oak_editor_params_property_text_at(handle.0, index, property_index, buf, len)
							})
							.unwrap_or_default()
						});
						Some(ParameterProperty {
							key: string_out(|buf, len| unsafe {
								oak_editor_params_property_key_at(handle.0, index, property_index, buf, len)
							})
							.unwrap_or_default(),
							value: convert_value(property.value, text),
						})
					})
					.collect();
				let values = (0..raw.value_count.max(0))
					.filter_map(|element| {
						let element = unsafe { oak_editor_params_value_element_at(handle.0, index, element) };
						let mut value = ValueRaw::default();
						if !unsafe { oak_editor_params_value_at(handle.0, index, element, &mut value) } {
							return None;
						}
						let text = value.has_text.then(|| {
							string_out(|buf, len| unsafe {
								oak_editor_params_value_text_at(handle.0, index, element, buf, len)
							})
							.unwrap_or_default()
						});
						Some((element, convert_value(value, text)))
					})
					.collect();
				let keyframes = (0..raw.track_count.max(0))
					.map(|element| {
						let element = unsafe { oak_editor_params_track_element_at(handle.0, index, element) };
						let keys = (0..unsafe { oak_editor_params_keyframe_count(handle.0, index, element) }.max(0))
							.filter_map(|key_index| {
								let mut key = KeyframeRaw::default();
								if !unsafe {
									oak_editor_params_keyframe_at(handle.0, index, element, key_index, &mut key)
								} {
									return None;
								}
								let text = key.value.has_text.then(|| {
									string_out(|buf, len| unsafe {
										oak_editor_params_keyframe_text_at(
											handle.0,
											index,
											element,
											key_index,
											buf,
											len,
										)
									})
									.unwrap_or_default()
								});
								Some(ParameterKeyframe {
									time: key.time,
									value: convert_value(key.value, text),
									interpolation: match key.interpolation {
										0 => KeyframeInterpolation::Hold,
										2 => KeyframeInterpolation::Bezier,
										_ => KeyframeInterpolation::Linear,
									},
									bezier_in: (key.bezier_in_x, key.bezier_in_y),
									bezier_out: (key.bezier_out_x, key.bezier_out_y),
								})
							})
							.collect::<Vec<_>>();
						(element, keys)
					})
					.filter(|(_, keys)| !keys.is_empty())
					.collect();
				Some(Parameter {
					input_id,
					label: string_out(|buf, len| unsafe {
						oak_editor_params_label_at(handle.0, index, buf, len)
					})
					.unwrap_or_default(),
					value_type: ParameterValueType::from_raw(raw.value_type),
					flags: raw.flags,
					properties,
					values,
					keyframes,
				})
			})
			.collect();
		Self { revision, params }
	}
}

impl EditorSession {
	/// Captures all visible parameters for `node`, including static values,
	/// properties and every currently stored keyframe track. Invalid IDs return `None`.
	pub fn parameters(&self, node: NodeId) -> Option<ParameterSnapshot> {
		if !node.is_valid() {
			return None;
		}
		let ptr = unsafe { oak_editor_params_snapshot(self.ptr, node.raw()) };
		if ptr.is_null() {
			return None;
		}
		Some(unsafe { ParameterSnapshot::from_handle(ParamsHandle(ptr)) })
	}
}
