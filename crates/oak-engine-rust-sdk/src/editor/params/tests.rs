//! Parameter metadata, text values and animated editing integration tests.

use crate::edit::EditCommand;
use crate::editor::{KeyframeInterpolation, ParameterValueType};
use crate::node::NodeValue;
use crate::types::Rational;

fn float(value: f64) -> NodeValue {
	let mut raw = NodeValue {
		kind: 2,
		..NodeValue::default()
	};
	raw.f[0] = value;
	raw
}

#[test]
fn metadata_and_keyframe_commands_roundtrip_through_native_snapshots() {
	let _guard = crate::node::UNDO_SERIAL
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let session = crate::editor::EditorSession::new().expect("session");
	let project = session.project();
	let host = project.create_clip().expect("host");
	let effect = session
		.execute(EditCommand::EffectInsert {
			host,
			index: 0,
			type_id: "org.olivevideoeditor.Olive.colorcorrect".into(),
		})
		.expect("effect")
		.primary
		.expect("effect id");
	let initial = session.parameters(effect).expect("parameters");
	assert!(initial.get("enabled_in").is_none());
	let saturation = initial.get("saturation_in").expect("saturation");
	assert_eq!(saturation.value_type, ParameterValueType::Float);
	assert!(!saturation.label.is_empty());
	assert!(saturation.scalar_value().expect("value").pod.is_some());
	assert!(saturation.track(-1).is_none());

	for (time, value) in [(0, 0.0), (1, 2.0)] {
		session
			.execute(EditCommand::UpsertNodeKeyframe {
				node: effect,
				input_id: "saturation_in".into(),
				element: -1,
				time: Rational::new(time, 1),
				value: float(value),
				interpolation: KeyframeInterpolation::Linear,
				bezier_in: (0.0, 0.0),
				bezier_out: (0.0, 0.0),
			})
			.expect("linear key");
	}
	let midpoint = project
		.value_at_time(effect, "saturation_in", -1, Rational::new(1, 2))
		.expect("interpolated");
	assert_eq!(midpoint.f[0], 1.0);

	session
		.execute(EditCommand::UpsertNodeKeyframe {
			node: effect,
			input_id: "saturation_in".into(),
			element: -1,
			time: Rational::new(2, 1),
			value: float(4.0),
			interpolation: KeyframeInterpolation::Bezier,
			bezier_in: (-0.25, -1.0),
			bezier_out: (0.5, 1.0),
		})
		.expect("bezier key");
	let animated = session.parameters(effect).expect("animated parameters");
	assert!(animated.revision > initial.revision);
	let track = animated.get("saturation_in").expect("saturation").track(-1).expect("track");
	assert_eq!(track.len(), 3);
	assert_eq!(track[2].interpolation, KeyframeInterpolation::Bezier);
	assert_eq!(track[2].bezier_in, (-0.25, -1.0));
	assert_eq!(track[2].bezier_out, (0.5, 1.0));
	assert!(crate::node::undo());
	assert_eq!(session.parameters(effect).unwrap().get("saturation_in").unwrap().track(-1).unwrap().len(), 2);
	assert!(crate::node::redo());
	assert_eq!(session.parameters(effect).unwrap().get("saturation_in").unwrap().track(-1).unwrap().len(), 3);

	session
		.execute(EditCommand::SetNodeValueAtTime {
			node: effect,
			input_id: "saturation_in".into(),
			element: -1,
			time: Rational::new(1, 2),
			value: float(1.5),
		})
		.expect("value at time");
	let inserted = project
		.value_at_time(effect, "saturation_in", -1, Rational::new(1, 2))
		.expect("inserted value");
	assert_eq!(inserted.f[0], 1.5);
	session
		.execute(EditCommand::RemoveNodeKeyframe {
			node: effect,
			input_id: "saturation_in".into(),
			element: -1,
			time: Rational::new(1, 2),
		})
		.expect("remove key");
	assert!(crate::node::undo());
	let restored = project
		.value_at_time(effect, "saturation_in", -1, Rational::new(1, 2))
		.expect("restored value");
	assert_eq!(restored.f[0], 1.5);
	assert!(crate::node::redo());
	assert!(session.parameters(crate::node::NodeId::INVALID).is_none());
}

#[test]
fn string_parameter_values_and_types_remain_owned() {
	let session = crate::editor::EditorSession::new().expect("session");
	let text = session
		.execute(EditCommand::CreateTextFootage)
		.expect("text node")
		.primary
		.expect("text id");
	let snapshot = session.parameters(text).expect("text parameters");
	let font = snapshot.get("font_family_in").expect("font family");
	assert_eq!(font.value_type, ParameterValueType::StrCombo);
	assert!(font.scalar_value().expect("font value").text.is_some());
	let plain = snapshot.get("plain_text_in").expect("plain text");
	assert_eq!(plain.value_type, ParameterValueType::Text);
	assert!(plain.scalar_value().expect("plain value").text.is_some());
	session
		.execute(EditCommand::SetNodeTextValue {
			node: text,
			input_id: "plain_text_in".into(),
			element: -1,
			value: "hello".into(),
		})
		.expect("set text");
	assert_eq!(
		session
			.parameters(text)
			.unwrap()
			.get("plain_text_in")
			.unwrap()
			.scalar_value()
			.unwrap()
			.text
			.as_deref(),
		Some("hello")
	);
	assert!(crate::node::undo());
	assert_ne!(
		session
			.parameters(text)
			.unwrap()
			.get("plain_text_in")
			.unwrap()
			.scalar_value()
			.unwrap()
			.text
			.as_deref(),
		Some("hello")
	);
	assert!(crate::node::redo());
	assert_eq!(
		session
			.parameters(text)
			.unwrap()
			.get("plain_text_in")
			.unwrap()
			.scalar_value()
			.unwrap()
			.text
			.as_deref(),
		Some("hello")
	);
}
