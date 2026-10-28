//! End-to-end graph snapshot and connection-validation coverage.

use crate::edit::EditCommand;
use crate::editor::{GraphNodeRole, GraphSnapshot};
use crate::node::NodeId;

fn effect_type() -> String {
	crate::effect::factory_entries()
		.into_iter()
		.find(|entry| entry.flags & crate::effect::NODE_FLAG_VIDEO_EFFECT != 0)
		.expect("video effect")
		.type_id
}

fn find_node<'a>(snapshot: &'a GraphSnapshot, id: NodeId) -> &'a crate::editor::GraphNode {
	snapshot.nodes.iter().find(|node| node.id == id).expect("graph node")
}

#[test]
fn full_and_clip_graphs_are_consistent_owned_snapshots() {
	let _guard = crate::node::UNDO_SERIAL
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let session = crate::editor::EditorSession::new().expect("session");
	let sequence = session.create_sequence("Graph").expect("sequence");
	let media = crate::test_support::make_test_clip("graph_snapshot");
	let footage = session.project().import_footage(&media).expect("footage");
	let clip = session
		.execute(EditCommand::PlaceFootageClip {
			footage,
			kind: crate::editor::TrackKind::Video,
			track_index: 0,
			in_ts: 0,
			out_ts: 10,
			media_in_ts: 0,
		})
		.expect("place clip")
		.primary
		.expect("clip id");
	let effect = session
		.execute(EditCommand::EffectInsert {
			host: clip,
			index: usize::MAX,
			type_id: effect_type(),
		})
		.expect("insert effect")
		.primary
		.expect("effect id");

	let full = session.graph_snapshot(sequence, None).expect("full graph");
	assert!(find_node(&full, sequence).protected);
	assert_eq!(find_node(&full, clip).role, GraphNodeRole::Clip);
	assert_eq!(find_node(&full, footage).role, GraphNodeRole::Footage);
	assert_eq!(find_node(&full, effect).role, GraphNodeRole::Effect);
	let endpoints: Vec<_> = full
		.nodes
		.iter()
		.filter(|node| matches!(node.role, GraphNodeRole::InputEndpoint | GraphNodeRole::OutputEndpoint))
		.collect();
	assert_eq!(endpoints.len(), 2);
	assert!(endpoints.iter().all(|node| node.protected));
	assert!(find_node(&full, clip)
		.inputs
		.iter()
		.all(|port| port.id != "enabled_in" && port.media_type.is_some()));
	assert!(full.edges.iter().any(|edge| {
		edge.synthetic && edge.from == clip && edge.to == sequence && edge.input_id == "tex_in"
	}));
	assert!(full.edges.iter().any(|edge| edge.from == footage && edge.to == effect));
	assert!(full.edges.iter().any(|edge| edge.from == effect && edge.to == clip));

	let context = session.graph_snapshot(sequence, Some(clip)).expect("clip graph");
	assert_eq!(context.nodes.len(), 3, "footage, effect and clip only");
	assert!(context.nodes.iter().all(|node| {
		node.id != sequence && !matches!(node.role, GraphNodeRole::InputEndpoint | GraphNodeRole::OutputEndpoint)
	}));
	assert!(context.edges.iter().all(|edge| !edge.synthetic));

	session
		.execute(EditCommand::SetContextPosition {
			node: effect,
			context: sequence,
			x: 42.0,
			y: 24.0,
		})
		.expect("position effect");
	let moved = session.graph_snapshot(sequence, None).expect("updated graph");
	assert!(moved.revision > full.revision);
	assert_eq!(find_node(&moved, effect).position, Some((42.0, 24.0)));
	// The older handle remains an immutable point-in-time view.
	assert_eq!(find_node(&full, effect).position, None);
	let _ = std::fs::remove_file(media);
}

#[test]
fn connection_validation_is_live_and_has_no_side_effects() {
	let _guard = crate::node::UNDO_SERIAL
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let session = crate::editor::EditorSession::new().expect("session");
	let sequence = session.create_sequence("Connections").expect("sequence");
	let media = crate::test_support::make_test_clip("graph_connect");
	let footage = session.project().import_footage(&media).expect("footage");
	let clip = session
		.execute(EditCommand::PlaceFootageClip {
			footage,
			kind: crate::editor::TrackKind::Video,
			track_index: 0,
			in_ts: 0,
			out_ts: 10,
			media_in_ts: 0,
		})
		.expect("place")
		.primary
		.expect("clip");
	let effect = session
		.execute(EditCommand::EffectInsert {
			host: clip,
			index: usize::MAX,
			type_id: effect_type(),
		})
		.expect("effect")
		.primary
		.expect("effect id");
	let snapshot = session.graph_snapshot(sequence, None).expect("graph");
	let effect_input = find_node(&snapshot, effect).inputs[0].id.clone();
	assert!(!session.can_connect(footage, effect, &effect_input, -1));
	assert!(!session.can_connect(footage, footage, &effect_input, -1));
	assert!(!session.can_connect(footage, effect, "missing_input", -1));
	assert!(!session.can_connect(NodeId::INVALID, effect, &effect_input, -1));

	session
		.execute(EditCommand::Disconnect {
			to: effect,
			input_id: effect_input.clone(),
		})
		.expect("disconnect");
	assert!(session.can_connect(footage, effect, &effect_input, -1));
	session
		.execute(EditCommand::Connect {
			from: footage,
			to: effect,
			input_id: effect_input.clone(),
		})
		.expect("reconnect");
	assert!(!session.can_connect(footage, effect, &effect_input, -1));
	assert!(session.graph_snapshot(NodeId::INVALID, None).is_none());
	assert!(session.graph_snapshot(sequence, Some(NodeId::INVALID)).is_none());
	let _ = std::fs::remove_file(media);
}
