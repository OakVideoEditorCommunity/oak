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

//! Effect-library presentation and SDK effect/parameter adapters.

use quercus_engine_rs::edit::EditCommand;
use quercus_engine_rs::node::NodeId;

use super::graphops::ProjectRef;

/// Returns a node's effect-input id through the SDK.
///
/// The effect input is the input that carries a node's effect chain.
/// `project` identifies the open project and `node` the potential host;
/// `None` when there is no session for `project` or the node has no effect
/// input, i.e. it cannot host effects.
pub fn effect_input_of(project: &ProjectRef, node: NodeId) -> Option<String> {
	let session = super::real::editor_session_for_project(project).ok()?;
	session
		.project()
		.effect_input_id(quercus_engine_rs::node::NodeId::from_raw(node.raw()))
}

/// Returns a host's effect chain, closest to source first.
///
/// `project` identifies the open project and `host` the node whose chain is
/// read (usually a clip). The result is the engine's ordered effect-node list
/// in signal order, with ids the inspector cannot map dropped. Empty when
/// there is no session for `project` or the host cannot host effects.
pub fn chain(project: &ProjectRef, host: NodeId) -> Vec<NodeId> {
	let Ok(session) = super::real::editor_session_for_project(project) else {
		return Vec::new();
	};
	session
		.project()
		.effect_chain(quercus_engine_rs::node::NodeId::from_raw(host.raw()))
		.into_iter()
		.filter_map(|node| super::graphops::id_of(node.raw()))
		.collect()
}

/// Returns whether an effect node is enabled through the SDK.
///
/// `project` identifies the open project and `node` the effect instance. The
/// engine's enabled flag is what the effect stack's toggle shows; `false` when
/// there is no session for `project` or the engine reports the node as
/// disabled (including nodes that are not effect instances).
pub fn is_enabled(project: &ProjectRef, node: NodeId) -> bool {
	super::real::editor_session_for_project(project)
		.ok()
		.is_some_and(|session| {
			session
				.project()
				.effect_is_enabled(quercus_engine_rs::node::NodeId::from_raw(node.raw()))
		})
}

/// Returns an OpenFX instance id without transferring ownership.
///
/// `project` identifies the open project and `node` the effect instance;
/// `None` when there is no session for `project` or the node is not a live
/// OpenFX instance. The returned id is the engine's raw registry handle, not a
/// new reference — it must not be used after the engine destroys the instance.
pub fn plugin_instance_handle(project: &ProjectRef, node: NodeId) -> Option<u64> {
	let session = super::real::editor_session_for_project(project).ok()?;
	session
		.project()
		.plugin_instance_handle(quercus_engine_rs::node::NodeId::from_raw(node.raw()))
}

/// Built-in effect-group keys in presentation order.
pub const GROUP_KEYS: &[&str] = &[
	"color",
	"filter",
	"distort",
	"keying",
	"generator",
	"math",
	"time",
	"channel",
	"transition",
	"general",
];

/// Maps an SDK node category to its presentation group key.
///
/// `category` is the factory entry's category; the result is one of
/// [`GROUP_KEYS`], or `"general"` for categories without a dedicated group
/// (callers also use `"general"` when the entry has no category at all).
pub fn category_group_key(category: quercus_engine_rs::effect::NodeCategory) -> &'static str {
	use quercus_engine_rs::effect::NodeCategory as Category;
	match category {
		Category::Color => "color",
		Category::Filter => "filter",
		Category::Distort => "distort",
		Category::Keying => "keying",
		Category::Generator => "generator",
		Category::Math => "math",
		_ => "general",
	}
}

/// Localizes a built-in effect-group key.
///
/// `group_key` is one of [`GROUP_KEYS`]; the matching
/// `effect_library.group.*` translation is returned. OpenFX subcategories are
/// not built-in keys and pass through unchanged.
pub fn group_label(group_key: &str) -> String {
	match group_key {
		"color" => crate::i18n::tr("effect_library.group.color"),
		"filter" => crate::i18n::tr("effect_library.group.filter"),
		"distort" => crate::i18n::tr("effect_library.group.distort"),
		"keying" => crate::i18n::tr("effect_library.group.keying"),
		"generator" => crate::i18n::tr("effect_library.group.generator"),
		"math" => crate::i18n::tr("effect_library.group.math"),
		"time" => crate::i18n::tr("effect_library.group.time"),
		"channel" => crate::i18n::tr("effect_library.group.channel"),
		"transition" => crate::i18n::tr("effect_library.group.transition"),
		"general" => crate::i18n::tr("effect_library.group.general"),
		other => other,
	}
	.to_string()
}

fn node_name_key(type_id: &str) -> String {
	format!("node.{}.name", type_suffix(type_id))
}

/// Builds the i18n key for one input label of a node type.
///
/// `type_id` is the full node type id — only its last dot-separated segment is
/// used (see `type_suffix`) — and `input_id` is the SDK input key; the result
/// is `node.<type>.input.<input>`.
fn input_name_key(type_id: &str, input_id: &str) -> String {
	format!("node.{}.input.{input_id}", type_suffix(type_id))
}

fn type_suffix(type_id: &str) -> &str {
	type_id.rsplit('.').next().unwrap_or(type_id)
}

/// Enumerates the effects that can be added to a clip's chain.
///
/// Collects the SDK factory entries flagged as video effects and not hidden
/// from the create menu (localizing built-in names), then appends the dynamic
/// (OpenFX) entries, whose subcategory becomes the group. The list is sorted
/// by built-in group order ([`GROUP_KEYS`]) with unknown groups after them,
/// then case-insensitively by name; it is what the effect library pane lists.
pub fn addable_effects() -> Vec<super::engine::EffectEntry> {
	use super::engine::EffectEntry;
	use quercus_engine_rs::effect::{NODE_FLAG_DONT_SHOW_IN_CREATE_MENU, NODE_FLAG_VIDEO_EFFECT};
	let mut out = Vec::new();
	for meta in quercus_engine_rs::effect::factory_entries() {
		if meta.flags & NODE_FLAG_VIDEO_EFFECT != 0
			&& meta.flags & NODE_FLAG_DONT_SHOW_IN_CREATE_MENU == 0
		{
			let name = if meta.name.is_empty() {
				meta.type_id.clone()
			} else {
				meta.name.clone()
			};
			let name = crate::i18n::tr_or(&node_name_key(&meta.type_id), &name);
			let group = meta.category.map(category_group_key).unwrap_or("general");
			out.push(EffectEntry {
				type_id: meta.type_id,
				name,
				group: Some(group.to_string()),
			});
		}
	}
	for meta in quercus_engine_rs::effect::dynamic_factory_entries() {
		let name = if meta.name.is_empty() {
			meta.type_id.clone()
		} else {
			meta.name
		};
		out.push(EffectEntry {
			type_id: meta.type_id,
			name,
			group: Some(meta.sub_category),
		});
	}
	out.sort_by(|a, b| {
		let ga = a.group.as_deref().unwrap_or("");
		let gb = b.group.as_deref().unwrap_or("");
		match (group_rank(ga), group_rank(gb)) {
			(Some(ra), Some(rb)) => ra.cmp(&rb),
			(Some(_), None) => std::cmp::Ordering::Less,
			(None, Some(_)) => std::cmp::Ordering::Greater,
			(None, None) => ga.cmp(gb),
		}
		.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
	});
	out
}

fn group_rank(group: &str) -> Option<usize> {
	GROUP_KEYS.iter().position(|key| *key == group)
}

/// Converts an SDK parameter snapshot into inspector-owned DTOs.
///
/// `snapshot` is the effect's parameter state reported by the editor engine
/// and `type_id` its node type id. Labels are localized for built-in effects
/// (present in the factory table) via the `node.<type>.input.<input>` key;
/// dynamic/OpenFX effects keep the SDK label. The font-family input gains one
/// `combo_option` per installed family when the SDK supplies none, and the
/// known multi-line text inputs are marked `multiline`. Each parameter's
/// scalar value is decoded into an [`super::engine::EffectValue`] and its
/// properties are carried over verbatim, so the result is fully owned and
/// safe to keep after the snapshot is dropped.
pub fn params_from_sdk(
	snapshot: &quercus_engine_rs::editor::ParameterSnapshot,
	type_id: &str,
) -> Vec<super::engine::EffectParam> {
	use super::engine::{EffectValue, EffectValueType};
	use quercus_engine_rs::editor::{ParameterValue, ParameterValueType};

	/// Maps an SDK [`ParameterValueType`] to the inspector's value type.
	///
	/// `value` is the parameter's declared type. Variants without a distinct
	/// inspector representation (`None` and the open-ended `Other`) collapse
	/// to `EffectValueType::None`; every concrete kind maps one-to-one.
	fn value_type(value: ParameterValueType) -> EffectValueType {
		use EffectValueType as Out;
		use ParameterValueType as In;
		match value {
			In::None | In::Other(_) => Out::None,
			In::Int => Out::Int,
			In::Float => Out::Float,
			In::Rational => Out::Rational,
			In::Boolean => Out::Boolean,
			In::Color => Out::Color,
			In::Matrix => Out::Matrix,
			In::Text => Out::Text,
			In::Texture => Out::Texture,
			In::Samples => Out::Samples,
			In::Vec2 => Out::Vec2,
			In::Vec3 => Out::Vec3,
			In::Vec4 => Out::Vec4,
			In::Combo => Out::Combo,
			In::StrCombo => Out::StrCombo,
			In::VideoParams => Out::VideoParams,
			In::AudioParams => Out::AudioParams,
			In::Binary => Out::Binary,
			In::PushButton => Out::PushButton,
			In::NodeRef => Out::NodeRef,
			In::Parametric => Out::Parametric,
		}
	}

	/// Decodes an SDK POD value into an inspector [`EffectValue`].
	///
	/// `value.kind` is the SDK node-value discriminant: 1 int, 2 float, 3 bool
	/// (non-zero is true), 5 color (4 floats), 6/7/8 vec2/vec3/vec4, 16 combo
	/// (integer index). Any other kind becomes `EffectValue::Unsupported`
	/// carrying the kind number, which the inspector renders as read-only
	/// instead of guessing a shape.
	fn pod_value(value: quercus_engine_rs::node::NodeValue) -> EffectValue {
		match value.kind {
			1 => EffectValue::Int(value.num),
			2 => EffectValue::Float(value.f[0]),
			3 => EffectValue::Boolean(value.num != 0),
			5 => EffectValue::Color(value.f),
			6 => EffectValue::Vec2([value.f[0], value.f[1]]),
			7 => EffectValue::Vec3([value.f[0], value.f[1], value.f[2]]),
			8 => EffectValue::Vec4(value.f),
			16 => EffectValue::Combo(value.num),
			kind => EffectValue::Unsupported(format!("POD kind {kind}")),
		}
	}

	fn property_value(value: &ParameterValue) -> EffectValue {
		if let Some(text) = &value.text {
			return EffectValue::Text(text.clone());
		}
		value.pod.map(pod_value).unwrap_or(EffectValue::None)
	}

	/// Converts one SDK [`ParameterValue`] into the inspector's
	/// [`EffectValue`].
	///
	/// `kind` decides the shape: `PushButton` always yields
	/// `EffectValue::PushButton` (the stored value is irrelevant), text values
	/// become `StrCombo` for `ParameterValueType::StrCombo` and `Text`
	/// otherwise, and packed values are decoded by [`pod`]. A value with
	/// neither text nor POD data yields `EffectValue::None`.
	fn parameter_value(value: &ParameterValue, kind: ParameterValueType) -> EffectValue {
		if kind == ParameterValueType::PushButton {
			return EffectValue::PushButton;
		}
		if let Some(text) = &value.text {
			return if kind == ParameterValueType::StrCombo {
				EffectValue::StrCombo(text.clone())
			} else {
				EffectValue::Text(text.clone())
			};
		}
		value.pod.map(pod_value).unwrap_or(EffectValue::None)
	}

	let builtin = quercus_engine_rs::effect::factory_entries()
		.iter()
		.any(|entry| entry.type_id == type_id);
	snapshot
		.params
		.iter()
		.map(|param| {
			let display_name = if builtin {
				crate::i18n::tr_or(&input_name_key(type_id, &param.input_id), &param.label)
			} else {
				param.label.clone()
			};
			let mut properties: Vec<(String, EffectValue)> = param
				.properties
				.iter()
				.map(|property| (property.key.clone(), property_value(&property.value)))
				.collect();
			if param.input_id == "font_family_in"
				&& !properties.iter().any(|(key, _)| key == "combo_option")
			{
				properties.extend(
					super::textengine::font_families()
						.into_iter()
						.map(|name| ("combo_option".into(), EffectValue::Text(name))),
				);
			}
			if matches!(param.value_type, ParameterValueType::Text)
				&& matches!(param.input_id.as_str(), "plain_text_in" | "text_in")
				&& !properties.iter().any(|(key, _)| key == "multiline")
			{
				properties.push(("multiline".into(), EffectValue::Boolean(true)));
			}
			super::engine::EffectParam {
				input_id: param.input_id.clone(),
				display_name,
				value_type: value_type(param.value_type),
				value: param
					.scalar_value()
					.map(|value| parameter_value(value, param.value_type))
					.unwrap_or(EffectValue::None),
				flags: param.flags,
				properties,
			}
		})
		.collect()
}

/// Writes one app parameter value through the session's native command ABI.
///
/// `session` is the open editor session, `node` the effect node, `input_id`
/// the SDK input key and `value` the inspector DTO value to store. POD kinds
/// become `EditCommand::SetNodeStandardValue` (element `-1`, i.e. the whole
/// input), text and string-combo values become `SetNodeTextValue`, and
/// `PushButton` becomes `PushNodeButton` (a trigger that stores nothing). The
/// command is executed immediately, so the edit lands on the undo stack as one
/// entry.
///
/// # Errors
///
/// Returns a message when the value is not writable through this DTO
/// (`None`, `Binary`, `Unsupported`), or the session's error when the command
/// fails.
pub fn set_sdk_input_value(
	session: &quercus_engine_rs::editor::EditorSession,
	node: quercus_engine_rs::node::NodeId,
	input_id: &str,
	value: super::engine::EffectValue,
) -> Result<(), String> {
	use super::engine::EffectValue as In;
	use quercus_engine_rs::node::NodeValue;
	/// Builds a POD node value of `kind` with the scalar `num` and up to four
	/// float components.
	///
	/// `values` fills the leading slots of `NodeValue::f`; the remaining slots
	/// keep their defaults. Used by [`set_sdk_input_value`] to hand the
	/// inspector DTO value to the native command ABI.
	///
	/// # Panics
	///
	/// Panics when `values` has more than four entries (the size of
	/// `NodeValue::f`).
	fn pod(kind: i32, num: i64, values: &[f64]) -> NodeValue {
		let mut value = NodeValue {
			kind,
			num,
			..NodeValue::default()
		};
		value.f[..values.len()].copy_from_slice(values);
		value
	}
	let command = match value {
		In::Int(value) => EditCommand::SetNodeStandardValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value: pod(1, value, &[]),
		},
		In::Float(value) => EditCommand::SetNodeStandardValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value: pod(2, 0, &[value]),
		},
		In::Color(value) => EditCommand::SetNodeStandardValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value: pod(5, 0, &value),
		},
		In::Text(value) | In::StrCombo(value) => EditCommand::SetNodeTextValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value,
		},
		In::Boolean(value) => EditCommand::SetNodeStandardValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value: pod(3, value as i64, &[]),
		},
		In::Vec2(value) => EditCommand::SetNodeStandardValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value: pod(6, 0, &value),
		},
		In::Vec3(value) => EditCommand::SetNodeStandardValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value: pod(7, 0, &value),
		},
		In::Vec4(value) => EditCommand::SetNodeStandardValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value: pod(8, 0, &value),
		},
		In::Combo(value) => EditCommand::SetNodeStandardValue {
			node,
			input_id: input_id.into(),
			element: -1,
			value: pod(16, value, &[]),
		},
		In::PushButton => EditCommand::PushNodeButton {
			node,
			input_id: input_id.into(),
		},
		In::None => return Err("a null parameter value is not writable".into()),
		In::Binary(_) => return Err("binary parameters are not writable through this DTO".into()),
		In::Unsupported(_) => return Err("unsupported parameter values are read-only".into()),
	};
	session.execute(command).map(|_| ())
}

/// Returns the `combo_option` text properties of a parameter.
///
/// Combo parameters carry one repeated `combo_option` per selectable entry
/// (the localized label shown in the combo, index-parallel to the
/// `combo_value` entries from [`combo_values`]); non-text properties are
/// ignored, so the order is the SDK's declared order.
pub fn combo_options(p: &super::engine::EffectParam) -> Vec<String> {
	p.properties
		.iter()
		.filter(|(key, _)| key == "combo_option")
		.filter_map(|(_, value)| match value {
			super::engine::EffectValue::Text(text) => Some(text.clone()),
			_ => None,
		})
		.collect()
}

/// Returns the `combo_value` text properties of a parameter.
///
/// Combo parameters carry one repeated `combo_value` per option (the machine
/// value stored in the node, index-parallel to the `combo_option` labels from
/// [`combo_options`]); non-text properties are ignored, so the order is the
/// SDK's declared order.
pub fn combo_values(p: &super::engine::EffectParam) -> Vec<String> {
	p.properties
		.iter()
		.filter(|(key, _)| key == "combo_value")
		.filter_map(|(_, value)| match value {
			super::engine::EffectValue::Text(text) => Some(text.clone()),
			_ => None,
		})
		.collect()
}

/// Returns the OpenFX `ui_group` / `ui_page` section a parameter belongs to.
///
/// `p` is an inspector DTO produced by [`params_from_sdk`]; both values come
/// from its repeated text properties. The pair is `(ui_group, ui_page)` with a
/// missing side represented as an empty string, and `None` only when the
/// parameter has neither property (callers then render it in a flat list).
pub fn ui_section_of(p: &super::engine::EffectParam) -> Option<(String, String)> {
	let group = p
		.properties
		.iter()
		.find(|(key, _)| key == "ui_group")
		.and_then(|(_, value)| match value {
			super::engine::EffectValue::Text(text) => Some(text.clone()),
			_ => None,
		});
	let page = p
		.properties
		.iter()
		.find(|(key, _)| key == "ui_page")
		.and_then(|(_, value)| match value {
			super::engine::EffectValue::Text(text) => Some(text.clone()),
			_ => None,
		});
	match (group, page) {
		(None, None) => None,
		(group, page) => Some((group.unwrap_or_default(), page.unwrap_or_default())),
	}
}
