//! Edit notification delivery, failure suppression and RAII cleanup tests.

use crate::edit::EditCommand;

#[test]
fn successful_commands_notify_once_and_drop_unsubscribes() {
	let _guard = crate::node::UNDO_SERIAL
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let session = crate::editor::EditorSession::new().expect("session");
	let (sender, receiver) = std::sync::mpsc::channel();
	let subscription = session
		.subscribe_edit(move |op, revision| {
			sender.send((op, revision)).expect("notification receiver");
		})
		.expect("subscription");
	assert!(subscription.is_active());
	session
		.execute(EditCommand::CreateFolder { name: "Observed".into() })
		.expect("observed command");
	let (op, revision) = receiver.try_recv().expect("one notification");
	assert_eq!(op, 8, "CreateFolder opcode");
	assert!(revision > 0);
	assert!(receiver.try_recv().is_err(), "exactly one callback per command");

	assert!(session
		.execute(EditCommand::RemoveNode {
			node: crate::node::NodeId::INVALID,
		})
		.is_err());
	assert!(receiver.try_recv().is_err(), "failed commands do not notify");
	drop(subscription);
	session
		.execute(EditCommand::CreateFolder { name: "After Drop".into() })
		.expect("command after unsubscribe");
	assert!(receiver.try_recv().is_err(), "Drop stops future callbacks");
}

#[test]
fn callback_panics_do_not_abort_the_edit() {
	let _guard = crate::node::UNDO_SERIAL
		.lock()
		.unwrap_or_else(|e| e.into_inner());
	let session = crate::editor::EditorSession::new().expect("session");
	let _subscription = session
		.subscribe_edit(|_, _| panic!("contained callback panic"))
		.expect("subscription");
	let folder = session
		.execute(EditCommand::CreateFolder { name: "Survives".into() })
		.expect("panic must not cross the ABI");
	assert!(folder.primary.is_some());
}
