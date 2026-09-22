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

//! Boundary tests for the `TaskManager` singleton: init/shutdown state
//! machine, task ownership (add/cancel/drain/delete), not-found cases,
//! pointer lookups (find/take/cancel by address) and worker joins.
//!
//! The manager is process-wide, so every test serializes on one static
//! mutex and starts from a shut-down singleton. The worker behaviors use
//! atomics and `yield_now` spins — no sleeps.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use oak_task::error::Error;
use oak_task::manager::TaskManager;
use oak_task::task::{Task, TaskBehavior};

/// Tests that touch the process-wide manager must not race each other.
static MANAGER_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
	MANAGER_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Shut down any leftover singleton, then create a fresh one.
fn init_fresh() {
	TaskManager::shutdown();
	assert!(TaskManager::instance().is_none());
	TaskManager::init().expect("manager init");
}

/// Bounded spin (no sleeps) until `flag` is set.
fn spin_until(flag: &AtomicBool) {
	for _ in 0..4_000_000 {
		if flag.load(Ordering::SeqCst) {
			return;
		}
		std::thread::yield_now();
	}
	panic!("flag was not set in time");
}

/// Marks that the behavior entered `run`, then returns success. The flag
/// also makes `Task::wait_finished` race-free (it no-ops on a task whose
/// worker has not reached `start` yet).
struct Signal {
	entered: Arc<AtomicBool>,
}

impl TaskBehavior for Signal {
	fn run(&mut self, _task: &mut Task) -> Result<(), oak_task::error::Error> {
		self.entered.store(true, Ordering::SeqCst);
		Ok(())
	}
}

/// Parks the worker until cancellation, recording both transitions.
struct Park {
	started: Arc<AtomicBool>,
	cancelled: Arc<AtomicBool>,
}

impl TaskBehavior for Park {
	fn run(&mut self, task: &mut Task) -> Result<(), oak_task::error::Error> {
		self.started.store(true, Ordering::SeqCst);
		while !task.is_cancelled() {
			std::thread::yield_now();
		}
		self.cancelled.store(true, Ordering::SeqCst);
		Err(oak_task::task::cancelled())
	}
}

/// Parks until an external release flag is set, then succeeds.
struct Gate {
	started: Arc<AtomicBool>,
	release: Arc<AtomicBool>,
}

impl TaskBehavior for Gate {
	fn run(&mut self, _task: &mut Task) -> Result<(), oak_task::error::Error> {
		self.started.store(true, Ordering::SeqCst);
		while !self.release.load(Ordering::SeqCst) {
			std::thread::yield_now();
		}
		Ok(())
	}
}

/// Build a `Park` behavior task plus its observation flags.
fn park_task(tag: &str) -> (Task, Arc<AtomicBool>, Arc<AtomicBool>) {
	let started = Arc::new(AtomicBool::new(false));
	let cancelled = Arc::new(AtomicBool::new(false));
	let mut task = Task::new(tag, None);
	task.set_behavior(Box::new(Park {
		started: started.clone(),
		cancelled: cancelled.clone(),
	}));
	(task, started, cancelled)
}

/// Add `task` and return its raw pointer after waiting for the worker to
/// start running.
fn add_and_wait(task: Task, started: &AtomicBool) -> *mut Task {
	TaskManager::with_manager_mut(|m| m.add_task(Box::new(task))).expect("manager present");
	spin_until(started);
	TaskManager::with_manager(|m| m.task_ptr_at(0))
		.expect("manager present")
		.expect("task 0")
}

fn task_ptr(index: usize) -> *mut Task {
	TaskManager::with_manager(|m| m.task_ptr_at(index))
		.expect("manager present")
		.unwrap_or_else(|e| panic!("task {index} missing: {e:?}"))
}

// ---- singleton state --------------------------------------------------

/// Given a fresh singleton, `init` rejects duplicates and the accessors see
/// the manager; `shutdown` is idempotent and clears the instance.
#[test]
fn init_is_a_singleton_and_shutdown_is_idempotent() {
	let _guard = serial();
	init_fresh();

	assert!(TaskManager::instance().is_some());
	assert!(matches!(TaskManager::init(), Err(Error::State)));

	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(0));
	assert_eq!(
		TaskManager::with_manager_mut(|m| {
			m.set_codec_submitter_registered(true);
			m.codec_submitter_registered()
		}),
		Some(true)
	);

	TaskManager::shutdown();
	assert!(TaskManager::instance().is_none());
	assert!(TaskManager::with_manager(|m| m.get_task_count()).is_none());
	assert!(TaskManager::with_manager_mut(|m| m.get_task_count()).is_none());

	TaskManager::shutdown(); // idempotent
	assert!(TaskManager::instance().is_none());
}

/// `init` does not register the codec task submitter: the manager flag
/// starts `false` while the oakcodec registry stays untouched, and the
/// flag is pure bookkeeping — flipping it installs nothing.
#[test]
fn init_does_not_register_the_codec_submitter() {
	let _guard = serial();
	init_fresh();

	assert!(
		!oak_task::codecbridge::is_codec_task_submitter_registered(),
		"init() must not install the codec submitter"
	);
	assert_eq!(
		TaskManager::with_manager(|m| m.codec_submitter_registered()),
		Some(false),
		"a fresh manager has not registered anything"
	);

	TaskManager::with_manager_mut(|m| m.set_codec_submitter_registered(true)).unwrap();
	assert!(
		!oak_task::codecbridge::is_codec_task_submitter_registered(),
		"the bookkeeping flag never installs the oakcodec callback"
	);
	assert_eq!(
		TaskManager::with_manager(|m| m.codec_submitter_registered()),
		Some(true)
	);
	TaskManager::with_manager_mut(|m| m.set_codec_submitter_registered(false)).unwrap();
	assert_eq!(
		TaskManager::with_manager(|m| m.codec_submitter_registered()),
		Some(false)
	);
}

// ---- add / finish / drain --------------------------------------------

/// Given a task handed to the manager, the worker runs it; it can be found
/// by address, waited on, drained (with its join handle) and removed.
#[test]
fn add_task_runs_and_drain_finished_returns_the_pair() {
	let _guard = serial();
	init_fresh();

	let entered = Arc::new(AtomicBool::new(false));
	let mut task = Task::new("signal", None);
	task.set_behavior(Box::new(Signal {
		entered: entered.clone(),
	}));
	TaskManager::with_manager_mut(|m| m.add_task(Box::new(task))).expect("manager present");
	spin_until(&entered);
	let ptr = task_ptr(0);

	// Deterministic completion wait through the task itself.
	unsafe { (&*ptr).wait_finished() };
	assert!(unsafe { (&*ptr).is_finished() });
	assert!(unsafe { (&*ptr).succeeded() });

	// The list still owns the task until drained.
	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(1));
	assert_eq!(
		TaskManager::with_manager(|m| m.find_index(ptr)),
		Some(Some(0))
	);

	// An unrelated task is not in the list.
	let unrelated = Task::new("unrelated", None);
	let unrelated_ptr: *const Task = &unrelated;
	assert_eq!(
		TaskManager::with_manager(|m| m.find_index(unrelated_ptr)),
		Some(None)
	);

	let drained = TaskManager::with_manager_mut(|m| m.drain_finished()).expect("manager present");
	assert_eq!(drained.len(), 1, "the finished task and its thread");
	for (_task, handle) in drained {
		handle.join().expect("worker joined");
	}
	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(0));
	assert!(matches!(
		TaskManager::with_manager(|m| m.task_ptr_at(0)),
		Some(Err(Error::NotFound))
	));
}

/// Ending a still-running task through `cancel_task` reports NotFound for
/// out-of-range indices; `cancel_task_and_wait` stops and joins the worker.
#[test]
fn cancel_task_and_wait_stops_a_running_task() {
	let _guard = serial();
	init_fresh();

	let (task, started, cancelled) = park_task("park");
	let ptr = add_and_wait(task, &started);

	assert!(matches!(
		TaskManager::with_manager_mut(|m| m.cancel_task(7)),
		Some(Err(Error::NotFound))
	));
	assert!(matches!(
		TaskManager::with_manager(|m| m.task_ptr_at(7)),
		Some(Err(Error::NotFound))
	));

	TaskManager::with_manager_mut(|m| m.cancel_task_and_wait(0))
		.expect("manager present")
		.expect("cancel index 0");
	assert!(cancelled.load(Ordering::SeqCst));
	assert!(unsafe { (&*ptr).is_finished() });
	assert!(!unsafe { (&*ptr).succeeded() });

	// The handle was already joined by cancel_task_and_wait, so
	// delete_finished silently removes the finished task.
	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(1));
	TaskManager::with_manager_mut(|m| m.delete_finished()).expect("manager present");
	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(0));
}

/// `drain_finished` skips the still-running tasks and leaves them in the
/// list.
#[test]
fn drain_finished_keeps_running_tasks() {
	let _guard = serial();
	init_fresh();

	// A finished task at index 0...
	let entered = Arc::new(AtomicBool::new(false));
	let mut done = Task::new("done", None);
	done.set_behavior(Box::new(Signal {
		entered: entered.clone(),
	}));
	TaskManager::with_manager_mut(|m| m.add_task(Box::new(done))).unwrap();
	spin_until(&entered);
	unsafe { (&*task_ptr(0)).wait_finished() };

	// ...and a parked task at index 1.
	let (parked, started, cancelled) = park_task("running");
	TaskManager::with_manager_mut(|m| m.add_task(Box::new(parked))).unwrap();
	spin_until(&started);

	let drained = TaskManager::with_manager_mut(|m| m.drain_finished()).expect("manager present");
	assert_eq!(drained.len(), 1, "only the finished task is drained");
	for (_task, handle) in drained {
		handle.join().expect("worker joined");
	}
	assert_eq!(
		TaskManager::with_manager(|m| m.get_task_count()),
		Some(1),
		"the running task stays in the list"
	);

	TaskManager::shutdown();
	assert!(cancelled.load(Ordering::SeqCst));
}

/// `delete_finished` also joins the handles `drain_finished` would return.
#[test]
fn delete_finished_removes_completed_tasks() {
	let _guard = serial();
	init_fresh();

	for (index, tag) in ["a", "b"].into_iter().enumerate() {
		let entered = Arc::new(AtomicBool::new(false));
		let mut task = Task::new(tag, None);
		task.set_behavior(Box::new(Signal {
			entered: entered.clone(),
		}));
		TaskManager::with_manager_mut(|m| m.add_task(Box::new(task))).unwrap();
		spin_until(&entered);
		unsafe { (&*task_ptr(index)).wait_finished() };
	}
	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(2));

	TaskManager::with_manager_mut(|m| m.delete_finished()).unwrap();
	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(0));
}

// ---- pointer lookups --------------------------------------------------

/// `cancel_task_by_ptr` cancels a known task and no-ops for an absent one;
/// `take_thread_by_ptr` hands the handle out exactly once.
#[test]
fn cancel_and_take_by_pointer_handle_absent_targets() {
	let _guard = serial();
	init_fresh();

	let (task, started, cancelled) = park_task("byptr");
	let ptr = add_and_wait(task, &started);

	// Known pointer: the worker thread moves out (once).
	let handle = TaskManager::with_manager_mut(|m| m.take_thread_by_ptr(ptr))
		.expect("manager present")
		.expect("worker thread");
	assert!(TaskManager::with_manager_mut(|m| m.take_thread_by_ptr(ptr))
		.expect("manager present")
		.is_none());

	// Cancel through the pointer, then join lock-free.
	TaskManager::with_manager_mut(|m| m.cancel_task_by_ptr(ptr)).expect("manager present");
	handle.join().expect("worker joined");
	assert!(cancelled.load(Ordering::SeqCst));
	assert!(unsafe { (&*ptr).is_cancelled() });

	// Absent pointer: both operations are no-ops.
	let absent = Task::new("absent", None);
	let absent_ptr: *const Task = &absent;
	TaskManager::with_manager_mut(|m| m.cancel_task_by_ptr(absent_ptr)).expect("manager present");
	assert!(
		TaskManager::with_manager_mut(|m| m.take_thread_by_ptr(absent_ptr))
			.expect("manager present")
			.is_none()
	);

	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(1));
	TaskManager::with_manager_mut(|m| m.delete_finished()).unwrap();
	assert_eq!(TaskManager::with_manager(|m| m.get_task_count()), Some(0));
}

/// `wait_finished` blocks on a live task until its behavior returns.
#[test]
fn wait_finished_blocks_until_the_behavior_returns() {
	let _guard = serial();
	init_fresh();

	let started = Arc::new(AtomicBool::new(false));
	let release = Arc::new(AtomicBool::new(false));
	let mut task = Task::new("gate", None);
	task.set_behavior(Box::new(Gate {
		started: started.clone(),
		release: release.clone(),
	}));
	TaskManager::with_manager_mut(|m| m.add_task(Box::new(task))).unwrap();
	spin_until(&started);
	let ptr = task_ptr(0);

	// The waiter records whether the task was finished when
	// `wait_finished` returned; it must not return while the gate is shut.
	let entered = Arc::new(AtomicBool::new(false));
	let waiter_entered = entered.clone();
	let ptr_addr = ptr as usize;
	let waiter = std::thread::spawn(move || {
		let task = unsafe { &*(ptr_addr as *const Task) };
		waiter_entered.store(true, Ordering::SeqCst);
		task.wait_finished();
		task.is_finished()
	});
	spin_until(&entered);
	assert!(!waiter.is_finished(), "the gate is still shut");

	release.store(true, Ordering::SeqCst);
	assert!(
		waiter.join().expect("waiter thread"),
		"wait_finished must not return before the task finished"
	);
	assert!(unsafe { (&*ptr).succeeded() });
	TaskManager::with_manager_mut(|m| m.delete_finished()).unwrap();
}

/// Dropping the manager (via `shutdown`) cancels every running task before
/// joining its worker.
#[test]
fn shutdown_cancels_running_tasks_before_joining() {
	let _guard = serial();
	init_fresh();

	let (task, started, cancelled) = park_task("drop");
	TaskManager::with_manager_mut(|m| m.add_task(Box::new(task))).unwrap();
	spin_until(&started);

	TaskManager::shutdown();
	assert!(
		cancelled.load(Ordering::SeqCst),
		"the manager destructor cancels its tasks"
	);
	assert!(TaskManager::instance().is_none());
}
