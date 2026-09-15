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

//! Volume audio effect (C++ `src/node/src/audio/volume/volume.{h,cpp}`,
//! `olive::VolumeNode`).

use crate::factory::NodeMeta;
use crate::node::{Category, NodeBehavior, NodeCore};
use crate::value::{NodeValue, ValueType};

/// Samples input id (C++ `k_samples_input`). Type: samples; flags:
/// not-keyframable; this is the node's effect input
/// (`set_effect_input(k_samples_input)`).
pub const SAMPLES_INPUT: &str = "samples_in";

/// Volume input id (C++ `k_volume_input`). Type: float; default `1.0`;
/// properties: `min = 0.0`, `view = decibel`.
pub const VOLUME_INPUT: &str = "volume_in";

/// Volume effect node. Multiplies every channel of a sample buffer by a
/// gain factor.
///
/// The C++ class derives from `MathNodeBase` (for the shared
/// `process_samples_internal` helper used with `k_op_multiply`) but
/// declares no own data members, so this is a unit-like struct; the
/// multiply-by-scalar sample math will be shared with
/// `super::mathbase` when the base module lands.
pub struct VolumeNode;

impl NodeBehavior for VolumeNode {
	/// Human-readable name (C++ `name()`).
	fn name(&self) -> &str {
		"Volume"
	}

	/// Stable type id (C++ `id()`).
	fn type_id(&self) -> &str {
		"org.olivevideoeditor.Olive.volume"
	}

	/// Categories (C++ `category()`).
	fn categories(&self) -> &[Category] {
		&[Category::Filter]
	}

	/// Description (C++ `description()`).
	fn description(&self) -> &str {
		"Adjusts the volume of an audio source."
	}

	/// Localized input names (C++ `retranslate()`): `samples_in` ->
	/// "Samples", `volume_in` -> "Volume".
	fn input_name<'a>(&self, id: &'a str) -> &'a str {
		match id {
			SAMPLES_INPUT => "Samples",
			VOLUME_INPUT => "Volume",
			_ => id,
		}
	}

	/// Evaluate outputs (C++ `value()`): unallocated samples input ->
	/// push nothing; allocated samples with a static volume input ->
	/// apply the gain immediately and push the transformed samples,
	/// skipping the transform when the gain is effectively 1.0 (C++
	/// preserves the `!qFuzzyCompare(volume, 1.0)` double semantics:
	/// `abs(volume - 1.0) * 1e12 > min(abs(volume), 1.0)`); allocated
	/// samples with a non-static volume input -> build a `SampleJob`
	/// over `samples_in` + `volume_in` and push it as a samples value.
	///
	/// The Rust model has no `SampleJob` payload: the dynamic case
	/// pushes the input samples through unchanged and the audio
	/// renderer applies the per-index gain via [`Self::process_samples`]
	/// (`// CPP-PARITY: volume.cpp` `value()`).
	fn value(
		&self,
		core: &NodeCore,
		inputs: &crate::value::NodeValueRow,
		time: oak_core::Rational,
		table: &mut crate::value::NodeValueTable,
	) {
		let buffer = match inputs.get(SAMPLES_INPUT) {
			Some(NodeValue::Samples(b)) if b.is_allocated() => b.clone(),
			_ => return,
		};

		if core.is_input_static(inputs, VOLUME_INPUT, -1) {
			let volume = core.value_at_time(VOLUME_INPUT, -1, time).to_double();

			// Same semantics as `!qFuzzyCompare(volume, 1.0)` (double
			// overload): NOT fuzzy-equal -> transform.
			if (volume - 1.0).abs() * 1e12 > volume.abs().min(1.0) {
				let mut transformed = buffer;
				transformed.transform_volume(volume);
				table.push(ValueType::Samples, NodeValue::Samples(transformed), None);
			} else {
				table.push(ValueType::Samples, NodeValue::Samples(buffer), None);
			}
		} else {
			// Dynamic volume input: deferred sample job.
			table.push(ValueType::Samples, NodeValue::Samples(buffer), None);
		}
	}

	/// Process a span of samples (C++ `process_samples()`): delegates to
	/// `MathNodeBase::process_samples_internal` with `k_op_multiply`,
	/// i.e. each output sample is the input sample multiplied by the
	/// `volume_in` value. The C++ signature receives the input buffer
	/// and a sample index; the Rust trait instead hands over a time
	/// `range` and the destination buffer, so the whole output span is
	/// filled here.
	fn process_samples(
		&self,
		core: &NodeCore,
		inputs: &crate::value::NodeValueRow,
		range: oak_core::TimeRange,
		output: &mut crate::value::SampleBuffer,
	) {
		let input = match inputs.get(SAMPLES_INPUT) {
			Some(NodeValue::Samples(b)) => b,
			_ => return,
		};
		let volume = match inputs.get(VOLUME_INPUT) {
			Some(v) => v.to_double(),
			None => core
				.value_at_time(VOLUME_INPUT, -1, range.in_())
				.to_double(),
		};
		for c in 0..output.channels {
			for i in 0..output.sample_count {
				let v = input.sample_value(c, i);
				output.set_sample_value(c, i, v * volume);
			}
		}
	}

	/// Deep copy (C++ `copy()`).
	fn duplicate(&self, _core: &NodeCore) -> Option<Box<dyn NodeBehavior>> {
		Some(Box::new(VolumeNode))
	}
}

/// Constructor (C++ `VolumeNode::VolumeNode()`): adds `samples_in`
/// (samples, not-keyframable) and `volume_in` (float, default 1.0,
/// min/view properties documented on the constant), sets the
/// audio-effect flag and makes `samples_in` the effect input.
pub fn create() -> (NodeCore, Box<dyn NodeBehavior>) {
	let mut core = NodeCore::new();

	let mut samples = crate::input::Input::new(SAMPLES_INPUT, ValueType::Samples, NodeValue::None);
	samples.flags |= crate::input::flags::NOT_KEYFRAMABLE;
	core.add_input(samples);

	let mut volume =
		crate::input::Input::new(VOLUME_INPUT, ValueType::Float, NodeValue::Float(1.0));
	volume.properties = vec![
		("min".to_string(), NodeValue::Float(0.0)),
		("view".to_string(), NodeValue::Text("decibel".into())),
	];
	core.add_input(volume);

	core.flags |= crate::node::flags::AUDIO_EFFECT;
	core.effect_input = SAMPLES_INPUT.to_string();

	(core, Box::new(VolumeNode))
}

/// Register this node type (C++ `k_audio_volume` in
/// `factory.cpp::create_from_factory_index`).
pub fn register(meta: &mut Vec<NodeMeta>) {
	meta.push(NodeMeta {
		type_id: "org.olivevideoeditor.Olive.volume",
		name: "Volume",
		categories: &[Category::Filter],
		create,
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::value::NodeValueTable;
	use oak_core::{Rational, SampleFormat, TimeRange};

	fn planar(channels: usize, count: usize, values: &[f64]) -> crate::value::SampleBuffer {
		let mut buf = crate::value::SampleBuffer {
			format: SampleFormat::F32Planar,
			channels,
			sample_count: count,
			data: vec![0u8; channels * count * 4],
		};
		for c in 0..channels {
			for i in 0..count {
				buf.set_sample_value(c, i, values[c * count + i]);
			}
		}
		buf
	}

	#[test]
	fn input_names() {
		let n = VolumeNode;
		assert_eq!(n.input_name(SAMPLES_INPUT), "Samples");
		assert_eq!(n.input_name(VOLUME_INPUT), "Volume");
		assert_eq!(n.input_name("other"), "other");
	}

	#[test]
	fn create_wires_inputs_and_flags() {
		let (core, behavior) = create();
		assert_eq!(behavior.name(), "Volume");
		assert_eq!(behavior.type_id(), "org.olivevideoeditor.Olive.volume");
		assert!(core.get_input(SAMPLES_INPUT).is_some());
		assert_eq!(
			core.get_input(SAMPLES_INPUT).unwrap().flags & crate::input::flags::NOT_KEYFRAMABLE,
			crate::input::flags::NOT_KEYFRAMABLE
		);
		assert_eq!(
			core.get_input(VOLUME_INPUT).unwrap().default,
			NodeValue::Float(1.0)
		);
		assert_eq!(core.effect_input, SAMPLES_INPUT);
		assert_ne!(core.flags & crate::node::flags::AUDIO_EFFECT, 0);
	}

	#[test]
	fn value_unallocated_pushes_nothing() {
		let (core, behavior) = create();
		let mut table = NodeValueTable::default();
		let inputs = std::collections::BTreeMap::new();
		behavior.value(&core, &inputs, Rational::new(0, 1), &mut table);
		assert!(table.is_empty());
	}

	#[test]
	fn value_static_volume_applies_gain() {
		let (mut core, behavior) = create();
		core.set_standard_value(VOLUME_INPUT, -1, NodeValue::Float(0.5));
		let buf = planar(1, 2, &[1.0, 2.0]);
		let inputs = std::collections::BTreeMap::from([(
			SAMPLES_INPUT.to_string(),
			NodeValue::Samples(buf.clone()),
		)]);
		let mut table = NodeValueTable::default();
		behavior.value(&core, &inputs, Rational::new(0, 1), &mut table);
		let out = match table.get(ValueType::Samples).unwrap() {
			NodeValue::Samples(s) => s,
			_ => panic!("samples"),
		};
		assert_eq!(out.sample_value(0, 0), 0.5);
		assert_eq!(out.sample_value(0, 1), 1.0);
		// The input buffer is not mutated.
		assert_eq!(buf.sample_value(0, 0), 1.0);
	}

	#[test]
	fn value_static_volume_unity_passes_through() {
		let (mut core, behavior) = create();
		core.set_standard_value(VOLUME_INPUT, -1, NodeValue::Float(1.0));
		let buf = planar(1, 2, &[1.0, 2.0]);
		let inputs = std::collections::BTreeMap::from([(
			SAMPLES_INPUT.to_string(),
			NodeValue::Samples(buf.clone()),
		)]);
		let mut table = NodeValueTable::default();
		behavior.value(&core, &inputs, Rational::new(0, 1), &mut table);
		let out = match table.get(ValueType::Samples).unwrap() {
			NodeValue::Samples(s) => s,
			_ => panic!("samples"),
		};
		assert_eq!(out.sample_value(0, 0), 1.0);
		assert_eq!(out.sample_value(0, 1), 2.0);
	}

	#[test]
	fn value_dynamic_volume_pushes_through() {
		let (mut core, behavior) = create();
		// Keyframing the volume input makes it non-static.
		core.keyframe_track_mut(VOLUME_INPUT, -1)
			.set_key(crate::keyframe::Keyframe {
				time: Rational::new(0, 1),
				value: NodeValue::Float(0.5),
				interpolation: crate::keyframe::Interpolation::Linear,
				bezier_in: (0.0, 0.0),
				bezier_out: (0.0, 0.0),
			});
		let buf = planar(1, 2, &[1.0, 2.0]);
		let inputs = std::collections::BTreeMap::from([(
			SAMPLES_INPUT.to_string(),
			NodeValue::Samples(buf.clone()),
		)]);
		let mut table = NodeValueTable::default();
		behavior.value(&core, &inputs, Rational::new(0, 1), &mut table);
		let out = match table.get(ValueType::Samples).unwrap() {
			NodeValue::Samples(s) => s,
			_ => panic!("samples"),
		};
		// Unchanged: the renderer applies the gain via process_samples.
		assert_eq!(out.sample_value(0, 0), 1.0);
		assert_eq!(out.sample_value(0, 1), 2.0);
	}

	#[test]
	fn process_samples_fills_output() {
		let (mut core, behavior) = create();
		core.set_standard_value(VOLUME_INPUT, -1, NodeValue::Float(2.0));
		let buf = planar(2, 2, &[1.0, 2.0, 3.0, 4.0]);
		let inputs = std::collections::BTreeMap::from([
			(SAMPLES_INPUT.to_string(), NodeValue::Samples(buf)),
			(VOLUME_INPUT.to_string(), NodeValue::Float(2.0)),
		]);
		let mut out = planar(2, 2, &[0.0; 4]);
		behavior.process_samples(
			&core,
			&inputs,
			TimeRange::new(Rational::new(0, 1), Rational::new(2, 1)),
			&mut out,
		);
		assert_eq!(out.sample_value(0, 0), 2.0);
		assert_eq!(out.sample_value(0, 1), 4.0);
		assert_eq!(out.sample_value(1, 0), 6.0);
		assert_eq!(out.sample_value(1, 1), 8.0);
	}

	#[test]
	fn duplicate_clones() {
		let (core, behavior) = create();
		let dup = behavior.duplicate(&core).unwrap();
		assert_eq!(dup.name(), "Volume");
		assert_eq!(dup.type_id(), "org.olivevideoeditor.Olive.volume");
	}
}
