//! Effect-chain command and animated-parameter undo regression tests.

use crate::edit::EditCommand;
use crate::node::NodeValue;

use super::{fixture, ok};

#[test]
fn effect_commands_undo_redo_and_static_parameter_roundtrip() {
	let _guard = crate::node::UNDO_SERIAL
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let f = fixture();
	let project = f.session.project();
	let host = project.create_clip().expect("effect host");
	let effect_type = crate::effect::factory_entries()
		.into_iter()
		.find(|entry| entry.flags & crate::effect::NODE_FLAG_VIDEO_EFFECT != 0)
		.expect("video effect")
		.type_id;
	let first = ok(&f.session, EditCommand::EffectInsert {
		host: host,
		index: 0,
		type_id: effect_type.clone(),
	})
	.primary
	.expect("first effect");
	let second = ok(&f.session, EditCommand::EffectInsert {
		host: host,
		index: 0,
		type_id: effect_type,
	})
	.primary
	.expect("second effect");
	assert_eq!(project.effect_chain(host), vec![second, first]);

	ok(&f.session, EditCommand::EffectMove {
		host: host,
		effect: second,
		new_index: 1,
	});
	assert_eq!(project.effect_chain(host), vec![first, second]);
	assert!(crate::node::undo());
	assert_eq!(project.effect_chain(host), vec![second, first]);
	assert!(crate::node::redo());
	assert_eq!(project.effect_chain(host), vec![first, second]);

	ok(&f.session, EditCommand::SetNodeStandardValue {
		node: first,
		input_id: "enabled_in".into(),
		element: -1,
		value: NodeValue {
			kind: 3,
			num: 0,
			..NodeValue::default()
		},
	});
	assert!(!project.effect_is_enabled(first));
	assert!(crate::node::undo());
	assert!(project.effect_is_enabled(first));
	assert!(crate::node::redo());
	assert!(!project.effect_is_enabled(first));

	ok(&f.session, EditCommand::EffectSetEnabled {
		effect: first,
		enabled: true,
	});
	assert!(project.effect_is_enabled(first));
	ok(&f.session, EditCommand::EffectRemove {
		host: host,
		effect: second,
	});
	assert_eq!(project.effect_chain(host), vec![first]);
	assert!(crate::node::undo());
	assert_eq!(project.effect_chain(host), vec![first, second]);
	assert!(crate::node::redo());
	assert_eq!(project.effect_chain(host), vec![first]);
	let _ = std::fs::remove_file(f.media);
}
