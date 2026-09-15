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

//! Pan audio effect (C++ `src/node/src/audio/pan/pan.{h,cpp}`,
//! `olive::PanNode`).

use crate::factory::NodeMeta;
use crate::node::{Category, NodeBehavior, NodeCore};
use crate::value::{NodeValue, ValueType};

/// Samples input id (C++ `k_samples_input`). Type: samples; flags:
/// not-keyframable; this is the node's effect input
/// (`set_effect_input(k_samples_input)`).
pub const SAMPLES_INPUT: &str = "samples_in";

/// Panning input id (C++ `k_panning_input`). Type: float; default `0.0`;
/// properties: `min = -1.0`, `max = 1.0`, `view = percentage`.
pub const PANNING_INPUT: &str = "panning_in";

/// Pan effect node. Attenuates the left or right channel of a stereo
/// sample buffer according to a -1..1 panning value.
///
/// The C++ class keeps `NodeInput *samples_input_` /
/// `NodeInput *panning_input_` back-pointers to its own inputs; in Rust
/// inputs live in [`NodeCore`], so there are no own fields.
pub struct PanNode;

impl NodeBehavior for PanNode {
	/// Human-readable name (C++ `name()`).
	fn name(&self) -> &str {
		"Pan"
	}

	/// Stable type id (C++ `id()`).
	fn type_id(&self) -> &str {
		"org.olivevideoeditor.Olive.pan"
	}

	/// Categories (C++ `category()`).
	fn categories(&self) -> &[Category] {
		&[Category::Filter]
	}

	/// Description (C++ `description()`).
	fn description(&self) -> &str {
		"Adjust the stereo panning of an audio source."
	}

	/// Localized input names (C++ `retranslate()`): `samples_in` ->
	/// "Samples", `panning_in` -> "Pan".
	fn input_name<'a>(&self, id: &'a str) -> &'a str {
		match id {
			SAMPLES_INPUT => "Samples",
			PANNING_INPUT => "Pan",
			_ => id,
		}
	}

	/// Evaluate outputs (C++ `value()`): unallocated samples input ->
	/// push nothing; non-stereo (channel count != 2) samples -> push the
	/// input through unchanged; stereo with a static panning input ->
	/// apply the attenuation immediately (pan > 0 scales channel 0 by
	/// `1.0 - pan`, pan < 0 scales channel 1 by `1.0 + pan`, pan == 0
	/// leaves the buffer untouched) and push the transformed samples;
	/// stereo with a non-static panning input -> build a `SampleJob`
	/// over `samples_in` + `panning_in` and push it as a samples value.
	///
	/// The Rust model has no `SampleJob` payload: the dynamic case
	/// pushes the input samples through unchanged and the audio
	/// renderer applies the per-index pan via [`Self::process_samples`]
	/// (`// CPP-PARITY: pan.cpp` `value()`).
	fn value(
		&self,
		core: &NodeCore,
		inputs: &crate::value::NodeValueRow,
		time: oak_core::Rational,
		table: &mut crate::value::NodeValueTable,
	) {
		let samples = match inputs.get(SAMPLES_INPUT) {
			Some(NodeValue::Samples(b)) if b.is_allocated() => b.clone(),
			_ => return,
		};

		// This node is only compatible with stereo audio.
		if samples.channels != 2 {
			table.push(ValueType::Samples, NodeValue::Samples(samples), None);
			return;
		}

		if core.is_input_static(inputs, PANNING_INPUT, -1) {
			let pan_volume = core.value_at_time(PANNING_INPUT, -1, time).to_double();
			if pan_volume != 0.0 {
				let mut transformed = samples;
				if pan_volume > 0.0 {
					transformed.transform_volume_for_channel(0, 1.0 - pan_volume);
				} else {
					transformed.transform_volume_for_channel(1, 1.0 + pan_volume);
				}
				table.push(ValueType::Samples, NodeValue::Samples(transformed), None);
			} else {
				table.push(ValueType::Samples, NodeValue::Samples(samples), None);
			}
		} else {
			// Dynamic panning input: deferred sample job.
			table.push(ValueType::Samples, NodeValue::Samples(samples), None);
		}
	}

	/// Process a span of samples (C++ `process_samples()`): copies every
	/// input channel to the output at the current index, then applies the
	/// pan — pan > 0 attenuates output channel 0 by `1.0 - pan`, pan < 0
	/// attenuates output channel 1 by `1.0 - abs(pan)`, pan == 0 is a
	/// straight copy. The C++ signature receives the input buffer and a
	/// sample index; the Rust trait instead hands over a time `range` and
	/// the destination buffer, so the whole output span is filled here.
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
		let pan_val = match inputs.get(PANNING_INPUT) {
			Some(v) => v.to_double(),
			None => core
				.value_at_time(PANNING_INPUT, -1, range.in_())
				.to_double(),
		};

		for c in 0..output.channels {
			for i in 0..output.sample_count {
				let v = input.sample_value(c, i);
				output.set_sample_value(c, i, v);
			}
		}

		if pan_val > 0.0 {
			for i in 0..output.sample_count {
				let v = output.sample_value(0, i);
				output.set_sample_value(0, i, v * (1.0 - pan_val));
			}
		} else if pan_val < 0.0 {
			for i in 0..output.sample_count {
				let v = output.sample_value(1, i);
				output.set_sample_value(1, i, v * (1.0 - pan_val.abs()));
			}
		}
	}

	/// Deep copy (C++ `copy()`).
	fn duplicate(&self, _core: &NodeCore) -> Option<Box<dyn NodeBehavior>> {
		Some(Box::new(PanNode))
	}
}

/// Constructor (C++ `PanNode::PanNode()`): adds `samples_in` (samples,
/// not-keyframable) and `panning_in` (float, default 0.0, min/max/view
/// properties documented on the constant), sets the audio-effect flag
/// and makes `samples_in` the effect input.
pub fn create() -> (NodeCore, Box<dyn NodeBehavior>) {
	let mut core = NodeCore::new();

	let mut samples = crate::input::Input::new(SAMPLES_INPUT, ValueType::Samples, NodeValue::None);
	samples.flags |= crate::input::flags::NOT_KEYFRAMABLE;
	core.add_input(samples);

	let mut panning =
		crate::input::Input::new(PANNING_INPUT, ValueType::Float, NodeValue::Float(0.0));
	panning.properties = vec![
		("min".to_string(), NodeValue::Float(-1.0)),
		("max".to_string(), NodeValue::Float(1.0)),
		("view".to_string(), NodeValue::Text("percentage".into())),
	];
	core.add_input(panning);

	core.flags |= crate::node::flags::AUDIO_EFFECT;
	core.effect_input = SAMPLES_INPUT.to_string();

	(core, Box::new(PanNode))
}

/// Register this node type (C++ `k_audio_panning` in
/// `factory.cpp::create_from_factory_index`).
pub fn register(meta: &mut Vec<NodeMeta>) {
	meta.push(NodeMeta {
		type_id: "org.olivevideoeditor.Olive.pan",
		name: "Pan",
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
		let n = PanNode;
		assert_eq!(n.input_name(SAMPLES_INPUT), "Samples");
		assert_eq!(n.input_name(PANNING_INPUT), "Pan");
	}

	#[test]
	fn create_wires_inputs_and_flags() {
		let (core, behavior) = create();
		assert_eq!(behavior.type_id(), "org.olivevideoeditor.Olive.pan");
		assert_eq!(
			core.get_input(PANNING_INPUT).unwrap().default,
			NodeValue::Float(0.0)
		);
		assert_eq!(core.effect_input, SAMPLES_INPUT);
		assert_ne!(core.flags & crate::node::flags::AUDIO_EFFECT, 0);
	}

	#[test]
	fn value_mono_passes_through() {
		let (mut core, behavior) = create();
		core.set_standard_value(PANNING_INPUT, -1, NodeValue::Float(1.0));
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
		assert_eq!(out.sample_value(0, 0), 1.0, "non-stereo untouched");
	}

	#[test]
	fn value_static_pan_left_attenuates_right() {
		let (mut core, behavior) = create();
		core.set_standard_value(PANNING_INPUT, -1, NodeValue::Float(-1.0));
		let buf = planar(2, 1, &[1.0, 1.0]);
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
		assert_eq!(out.sample_value(0, 0), 1.0, "left stays");
		assert_eq!(out.sample_value(1, 0), 0.0, "right = 1 + (-1)");
	}

	#[test]
	fn value_static_pan_right_attenuates_left() {
		let (mut core, behavior) = create();
		core.set_standard_value(PANNING_INPUT, -1, NodeValue::Float(0.5));
		let buf = planar(2, 1, &[1.0, 1.0]);
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
		assert_eq!(out.sample_value(0, 0), 0.5, "left = 1 - 0.5");
		assert_eq!(out.sample_value(1, 0), 1.0, "right stays");
	}

	#[test]
	fn value_static_pan_center_untouched() {
		let (mut core, behavior) = create();
		core.set_standard_value(PANNING_INPUT, -1, NodeValue::Float(0.0));
		let buf = planar(2, 1, &[1.0, 1.0]);
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
		assert_eq!(out.sample_value(1, 0), 1.0);
	}

	#[test]
	fn process_samples_pan_right() {
		let (mut core, behavior) = create();
		core.set_standard_value(PANNING_INPUT, -1, NodeValue::Float(0.25));
		let buf = planar(2, 1, &[1.0, 1.0]);
		let inputs = std::collections::BTreeMap::from([
			(SAMPLES_INPUT.to_string(), NodeValue::Samples(buf)),
			(PANNING_INPUT.to_string(), NodeValue::Float(0.25)),
		]);
		let mut out = planar(2, 1, &[0.0, 0.0]);
		behavior.process_samples(
			&core,
			&inputs,
			TimeRange::new(Rational::new(0, 1), Rational::new(1, 1)),
			&mut out,
		);
		assert_eq!(out.sample_value(0, 0), 0.75);
		assert_eq!(out.sample_value(1, 0), 1.0);
	}

	#[test]
	fn duplicate_clones() {
		let (core, behavior) = create();
		let dup = behavior.duplicate(&core).unwrap();
		assert_eq!(dup.name(), "Pan");
	}
}
