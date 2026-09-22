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

//! Boundary tests for the `Task` base lifecycle: `start` with/without a
//! behavior, cancellation before/after the run, error propagation,
//! progress/event emission (including the one-shot listener), `reset`,
//! `wait_finished`, shared cancel atoms and subscriber state.
//!
//! Everything is synchronous and deterministic (no sleeps, no fixtures).

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oak_core::cancelatom::CancelAtom;
use oak_task::error::{Error, Result};
use oak_task::task::{cancelled, system_time_ms, SubscriberState, Task, TaskBehavior, TaskEvent};

// ---- behaviors --------------------------------------------------------

/// Returns success without doing anything.
struct Noop;

impl TaskBehavior for Noop {
	fn run(&mut self, _task: &mut Task) -> Result<()> {
		Ok(())
	}
}

/// Records an error on the task and fails.
struct Fail;

impl TaskBehavior for Fail {
	fn run(&mut self, task: &mut Task) -> Result<()> {
		task.set_error("disk full");
		Err(Error::Failed("disk full".to_string()))
	}
}

/// Asserts cancellation was requested before the run and returns the
/// cancellation error.
struct CancelledBehavior;

impl TaskBehavior for CancelledBehavior {
	fn run(&mut self, task: &mut Task) -> Result<()> {
		assert!(
			task.is_cancelled(),
			"cancel must be visible to the behavior"
		);
		Err(cancelled())
	}
}

/// Cancels itself mid-run and succeeds.
struct SelfCancel;

impl TaskBehavior for SelfCancel {
	fn run(&mut self, task: &mut Task) -> Result<()> {
		assert!(!task.is_cancelled());
		task.cancel();
		assert!(task.is_cancelled());
		Ok(())
	}
}

/// Emits out-of-range progress values so the clamping is observable.
struct Progressor;

impl TaskBehavior for Progressor {
	fn run(&mut self, task: &mut Task) -> Result<()> {
		task.emit_progress(0.25);
		task.emit_progress(2.0);
		task.emit_progress(-1.0);
		Ok(())
	}
}

fn task_with(behavior: impl TaskBehavior + Send + 'static) -> Task {
	let mut task = Task::new("unit", None);
	task.set_behavior(Box::new(behavior));
	task
}

// ---- lifecycle --------------------------------------------------------

/// Given no behavior, `start` succeeds, marks the task finished/succeeded
/// and exposes an elapsed duration; the error and cancel state stay clean.
#[test]
fn start_without_a_behavior_succeeds() {
	let mut task = Task::new("unit", None);
	assert_eq!(task.title(), "unit");
	assert!(!task.is_finished());
	assert!(!task.succeeded());
	assert!(!task.is_cancelled());
	assert!(task.error().is_none());
	assert!(task.elapsed().is_none(), "not started yet");

	assert!(task.start().is_ok());
	assert!(task.is_finished());
	assert!(task.succeeded());
	assert!(task.elapsed().is_some());
	assert!(task.error().is_none());
}

/// Given a failing behavior, `start` returns the same error, the task is
/// finished but not succeeded, and the recorded error survives.
#[test]
fn failing_behavior_marks_failed_and_records_the_error() {
	let mut task = task_with(Fail);
	let result = task.start();
	assert!(result.is_err());
	assert!(task.is_finished());
	assert!(!task.succeeded());
	assert_eq!(task.error(), Some("disk full"));
}

/// Given a cancel before `start`, the behavior observes it and the task
/// fails with the cancellation error.
#[test]
fn cancel_before_start_is_visible_to_the_behavior() {
	let mut task = task_with(CancelledBehavior);
	task.cancel();
	assert!(task.is_cancelled());
	let result = task.start();
	assert!(matches!(result, Err(Error::Cancelled)));
	assert!(task.is_finished());
	assert!(!task.succeeded());
}

/// Given a cancel event hook, cancelling fires it exactly once per call
/// (and the atom stays cancelled).
#[test]
fn cancel_fires_the_event_hook_and_is_idempotent() {
	let fired = Arc::new(AtomicUsize::new(0));
	let hook_flag = fired.clone();
	let mut task = task_with(SelfCancel);
	task.set_cancel_event(Box::new(move || {
		hook_flag.fetch_add(1, Ordering::SeqCst);
	}));

	assert!(task.start().is_ok(), "the behavior itself succeeds");
	assert_eq!(fired.load(Ordering::SeqCst), 1, "self-cancel fires once");
	assert!(task.is_cancelled());

	// A later cancel keeps firing the hook (the C++ cancel() contract).
	task.cancel();
	assert_eq!(fired.load(Ordering::SeqCst), 2);
}

/// Given a listener, the events arrive as Started, the clamped progress
/// values, then Finished — and the listener is dropped after the run
/// (one-shot subscription).
#[test]
fn events_are_ordered_clamped_and_one_shot() {
	let events: Arc<Mutex<Vec<TaskEvent>>> = Arc::new(Mutex::new(Vec::new()));
	let sink = events.clone();
	let mut task = task_with(Progressor);
	task.set_event_listener(Box::new(move |event| {
		sink.lock().unwrap_or_else(|e| e.into_inner()).push(event);
	}));

	assert!(task.start().is_ok());
	assert_eq!(
		*events.lock().unwrap_or_else(|e| e.into_inner()),
		vec![
			TaskEvent::Started,
			TaskEvent::Progress(0.25),
			TaskEvent::Progress(1.0),
			TaskEvent::Progress(0.0),
			TaskEvent::Finished,
		]
	);

	// One-shot: after `start` the listener is gone.
	task.emit_progress(0.5);
	assert_eq!(
		events.lock().unwrap_or_else(|e| e.into_inner()).len(),
		5,
		"a finished task emits nothing to the dropped listener"
	);
}

/// Given a failed run, `reset` clears every lifecycle flag and error so the
/// task can run again.
#[test]
fn reset_clears_state_and_allows_a_second_run() {
	let mut task = task_with(Fail);
	assert!(task.start().is_err());
	assert!(task.error().is_some());

	task.reset();
	assert!(!task.is_finished());
	assert!(!task.succeeded());
	assert!(task.error().is_none());
	assert!(task.elapsed().is_none());

	task.set_behavior(Box::new(Noop));
	assert!(task.start().is_ok());
	assert!(task.succeeded());
}

/// `wait_finished` returns immediately for a task that was never started or
/// is already finished.
#[test]
fn wait_finished_returns_for_unstarted_and_finished_tasks() {
	let never = Task::new("never", None);
	never.wait_finished();
	assert!(!never.is_finished());

	let mut task = task_with(Noop);
	assert!(task.start().is_ok());
	task.wait_finished();
	assert!(task.is_finished());
}

/// Given a subscriber state, `start` publishes the wall-clock start and
/// 1.0/0.0 for success/failure.
#[test]
fn subscriber_state_tracks_start_time_and_result() {
	let state = Arc::new(SubscriberState::default());
	let mut task = task_with(Noop);
	task.set_subscriber(state.clone());
	let before = system_time_ms();
	assert!(task.start().is_ok());
	assert!(
		state.start_ms.load(Ordering::SeqCst) >= before,
		"start timestamp is published"
	);
	assert_eq!(state.finished_value.load(Ordering::SeqCst), 1);

	let state = Arc::new(SubscriberState::default());
	let mut failed = task_with(Fail);
	failed.set_subscriber(state.clone());
	assert!(failed.start().is_err());
	assert_eq!(state.finished_value.load(Ordering::SeqCst), 0);
}

/// A shared cancel atom links an outer task and its inner base: cancelling
/// either side is visible from the other.
#[test]
fn shared_cancel_atom_links_inner_and_outer_tasks() {
	let atom = Arc::new(CancelAtom::new());
	let mut outer = Task::new("outer", Some(atom.clone()));
	assert!(Arc::ptr_eq(&outer.get_cancel_atom(), &atom));
	outer.set_cancel_atom(atom.clone());

	let mut inner = Task::new("inner", Some(atom));
	inner.cancel();
	assert!(outer.is_cancelled());
}

/// Titles and error messages round-trip through their setters.
#[test]
fn title_and_error_are_settable() {
	let mut task = Task::new("old", None);
	task.set_title("new");
	assert_eq!(task.title(), "new");
	assert!(task.error().is_none());
	task.set_error("boom");
	assert_eq!(task.error(), Some("boom"));
}

/// The module helpers: `cancelled()` is the cancellation error and
/// `system_time_ms()` reports a plausible epoch timestamp.
#[test]
fn helpers_report_cancellation_and_wall_clock() {
	assert!(matches!(cancelled(), Error::Cancelled));
	assert!(system_time_ms() > 0);
}

/// `emit_progress` without a listener must not panic or resurrect state;
/// a listener attached afterwards observes the clamped value (`2.0` is
/// reported as `1.0`).
#[test]
fn progress_without_a_listener_is_a_no_op() {
	let mut task = task_with(Progressor);
	task.emit_progress(0.5);
	assert!(!task.is_finished(), "emitting progress does not finish");
	assert!(task.start().is_ok());
	assert!(task.is_finished());

	// The one-shot listener is gone after the run; a fresh one still
	// receives the clamped value.
	let events: Arc<Mutex<Vec<TaskEvent>>> = Arc::new(Mutex::new(Vec::new()));
	let sink = events.clone();
	task.set_event_listener(Box::new(move |event| {
		sink.lock().unwrap_or_else(|e| e.into_inner()).push(event);
	}));
	task.emit_progress(2.0);
	assert_eq!(
		*events.lock().unwrap_or_else(|e| e.into_inner()),
		vec![TaskEvent::Progress(1.0)],
		"the final progress value is clamped"
	);
}

/// A subscriber is not required for the lifecycle; setting one after a
/// reset still receives the next run's values.
#[test]
fn subscriber_is_optional_and_reusable_after_reset() {
	let mut task = task_with(Noop);
	assert!(task.start().is_ok());
	task.reset();
	let state = Arc::new(SubscriberState {
		start_ms: AtomicI64::new(0),
		finished_value: AtomicI64::new(-1),
	});
	task.set_subscriber(state.clone());
	assert!(task.start().is_ok());
	assert!(state.start_ms.load(Ordering::SeqCst) > 0);
	assert_eq!(state.finished_value.load(Ordering::SeqCst), 1);
}
