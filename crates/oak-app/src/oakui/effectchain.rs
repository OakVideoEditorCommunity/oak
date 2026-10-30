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
pub fn effect_input_of(project: &ProjectRef, node: NodeId) -> Option<String> {
	let session = super::real::editor_session_for_project(project).ok()?;
	session
		.project()
		.effect_input_id(quercus_engine_rs::node::NodeId::from_raw(node.raw()))
}

/// Returns a host's effect chain, closest to source first.
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

/// Returns whether an effect is enabled through the SDK.
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

/// Maps an SDK node category to its presentation group.
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

/// Localizes a built-in group key; OpenFX subcategories pass through.
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

fn input_name_key(type_id: &str, input_id: &str) -> String {
	format!("node.{}.input.{input_id}", type_suffix(type_id))
}

fn type_suffix(type_id: &str) -> &str {
	type_id.rsplit('.').next().unwrap_or(type_id)
}

/// Enumerates effects that can be added to a clip's chain.
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
pub fn params_from_sdk(
	snapshot: &quercus_engine_rs::editor::ParameterSnapshot,
	type_id: &str,
) -> Vec<super::engine::EffectParam> {
	use super::engine::{EffectValue, EffectValueType};
	use quercus_engine_rs::editor::{ParameterValue, ParameterValueType};

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
pub fn set_sdk_input_value(
	session: &quercus_engine_rs::editor::EditorSession,
	node: quercus_engine_rs::node::NodeId,
	input_id: &str,
	value: super::engine::EffectValue,
) -> Result<(), String> {
	use super::engine::EffectValue as In;
	use quercus_engine_rs::node::NodeValue;
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

/// Returns repeated `combo_option` text properties.
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

/// Returns repeated `combo_value` text properties.
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

/// Returns the `ui_group` / `ui_page` section of a parameter.
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
