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

//! The `TaskManager` singleton, mirroring `src/task/src/taskmanager.h`.
//!
//! Holds the set of live [`crate::task::Task`] instances and starts them
//! on worker threads. Codec task-submitter registration is a **separate**
//! concern owned by [`crate::codecbridge`]: the embedding application
//! calls `register_codec_task_submitter()` explicitly (the manager never
//! installs the callback itself — see [`TaskManager::init`]).
//!
//! CPP-PARITY: src/task/src/taskmanager.h
//!
//! ## Threading model
//!
//! [`TaskManager::add_task`] spawns a worker thread that runs the task's
//! [`Task::start`]. The worker thread only touches the task (through a raw
//! pointer) and never the manager, so the manager's own mutex is never held
//! across a join in the C ABI paths (joins happen lock-free via
//! [`TaskManager::drain_finished`] / [`TaskManager::take_thread_by_ptr`]).

use std::sync::Mutex;

use crate::error::{Error, Result};
use crate::task::Task;

/// Process-wide singleton manager. C++ is a lazy singleton; the Rust side
/// keeps a `Mutex<Option<...>>` so init/shutdown is explicit and races are
/// rejected with `OAKTASK_E_STATE` (matching `oaktask_manager_init`).
pub struct TaskManager {
	/// Live tasks, most-recently-added last (mirrors the C++ `task_list_`).
	tasks: Vec<Box<dyn TaskBox>>,
	/// Worker threads, parallel to `tasks` (moved out before joining).
	threads: Vec<Option<std::thread::JoinHandle<()>>>,
	/// Whether the embedding application registered the codec task
	/// submitter (`crate::codecbridge::register_codec_task_submitter`).
	/// Pure bookkeeping on the manager: [`TaskManager::init`] never
	/// registers anything itself, so a fresh singleton reads `false`.
	codec_submitter_registered: bool,
}

/// Boxable shim so tasks of heterogeneous concrete types can be stored in
/// the manager's list.
pub trait TaskBox: Send {
	/// Downcast/erase operations the manager needs.
	fn task(&self) -> &Task;
}

impl TaskBox for Task {
	fn task(&self) -> &Task {
		self
	}
}

/// The process-wide instance slot. A raw pointer (never freed while the
/// slot is set) so [`TaskManager::instance`] can hand out a `&'static`.
static INSTANCE: Mutex<Option<ManagerPtr>> = Mutex::new(None);

/// `Send`/`Sync` wrapper for the manager pointer (the manager's state is
/// guarded by the `INSTANCE` mutex; the pointer itself is plain).
struct ManagerPtr(*mut TaskManager);

// Safety: all access to the pointee goes through the `INSTANCE` mutex.
unsafe impl Send for ManagerPtr {}
unsafe impl Sync for ManagerPtr {}

impl TaskManager {
	/// The process-wide instance; `None` until [`TaskManager::init`].
	///
	/// CPP-PARITY: src/task/src/taskmanager.h (instance)
	pub fn instance() -> Option<&'static TaskManager> {
		let guard = INSTANCE.lock().unwrap();
		guard.as_ref().map(|p| unsafe { &*p.0 })
	}

	/// Create the singleton. Returns `Err(Error::State)` if already
	/// initialized (mirrors `oaktask_manager_init`).
	///
	/// This **does not** register the codec task submitter: registration is
	/// `crate::codecbridge::register_codec_task_submitter`, an explicit
	/// call by the embedding application. Two reasons:
	///
	/// - the C++ parity (`TaskManager::create_instance`) creates the
	///   manager only; codecbridge wires the callback separately, and
	/// - the codec bridge's interim contract is *synchronous* submission
	///   (`submit_codec_task` runs the task inline), so registration needs
	///   no manager state.
	///
	/// Consequently a fresh manager reports
	/// [`TaskManager::codec_submitter_registered`] as `false`; the
	/// application flips it via [`TaskManager::set_codec_submitter_registered`]
	/// when it performs the registration itself.
	pub fn init() -> Result<()> {
		let mut guard = INSTANCE.lock().unwrap();
		if guard.is_some() {
			return Err(Error::State);
		}
		let mgr = Box::into_raw(Box::new(TaskManager {
			tasks: Vec::new(),
			threads: Vec::new(),
			codec_submitter_registered: false,
		}));
		*guard = Some(ManagerPtr(mgr));
		Ok(())
	}

	/// Destroy the singleton. Idempotent. Does not unregister the codec
	/// task submitter (the registration is owned by the embedding
	/// application, not by the manager — see [`TaskManager::init`]).
	pub fn shutdown() {
		let ptr = {
			let mut guard = INSTANCE.lock().unwrap();
			guard.take()
		};
		if let Some(p) = ptr {
			// The Drop impl cancels and joins every worker thread.
			unsafe {
				drop(Box::from_raw(p.0));
			}
		}
	}

	/// Run `f` with the singleton (when initialized), without taking
	/// ownership. Used by the C ABI layer.
	pub fn with_manager<R>(f: impl FnOnce(&TaskManager) -> R) -> Option<R> {
		let guard = INSTANCE.lock().unwrap();
		guard.as_ref().map(|p| f(unsafe { &*p.0 }))
	}

	/// Run `f` with the singleton mutably (when initialized). Used by the C
	/// ABI layer. The caller must not call [`TaskManager::with_manager`]
	/// from a worker callback while this is held (same rule as the C++
	/// `mutex_`).
	pub fn with_manager_mut<R>(f: impl FnOnce(&mut TaskManager) -> R) -> Option<R> {
		let mut guard = INSTANCE.lock().unwrap();
		guard.as_mut().map(|p| f(unsafe { &mut *p.0 }))
	}

	/// Add a task, taking ownership (the manager now owns it). A worker
	/// thread is spawned that runs [`Task::start`] on the task.
	///
	/// The worker runs through a raw pointer while the box lives in
	/// `self.tasks`; the shared reference produced by `task()` ends before
	/// the pointer is used, so the cast is safe in this exclusively-owned
	/// setting.
	#[allow(invalid_reference_casting)]
	pub fn add_task(&mut self, task: Box<dyn TaskBox>) {
		let ptr = task.task() as *const Task as usize as *mut Task;
		self.tasks.push(task);
		// The worker only touches the task through the raw pointer; it never
		// reaches the manager, so the box stays alive in `self.tasks`.
		let ptr_usize = ptr as usize;
		let handle = std::thread::spawn(move || {
			unsafe {
				// The manager tracks success/failure through the task's own
				// finished state; the Result is intentionally ignored.
				let _ = (&mut *(ptr_usize as *mut Task)).start();
			}
		});
		self.threads.push(Some(handle));
	}

	/// Cancel the task at `index`.
	#[allow(invalid_reference_casting)]
	pub fn cancel_task(&mut self, index: usize) -> Result<()> {
		let task = self.tasks.get(index).ok_or(Error::NotFound)?;
		let ptr = task.task() as *const Task as usize as *mut Task;
		unsafe {
			(&mut *ptr).cancel();
		}
		Ok(())
	}

	/// Cancel the task at `index` and block until it finishes.
	pub fn cancel_task_and_wait(&mut self, index: usize) -> Result<()> {
		self.cancel_task(index)?;
		let handle = self.threads.get_mut(index).and_then(|h| h.take());
		if let Some(h) = handle {
			let _ = h.join();
		}
		Ok(())
	}

	/// Remove finished tasks from the list.
	pub fn delete_finished(&mut self) {
		for (_task, handle) in self.drain_finished() {
			let _ = handle.join();
		}
	}

	/// Number of live tasks.
	pub fn get_task_count(&self) -> usize {
		self.tasks.len()
	}

	/// Raw pointer to the task at `index` (stable while the manager owns
	/// it). Used by `oaktask_manager_at` to build a borrowed task handle.
	pub fn task_ptr_at(&self, index: usize) -> Result<*mut Task> {
		let task = self.tasks.get(index).ok_or(Error::NotFound)?;
		Ok(task.task() as *const Task as *mut Task)
	}

	/// Index of the task with the given address.
	pub fn find_index(&self, ptr: *const Task) -> Option<usize> {
		self.tasks.iter().position(|t| std::ptr::eq(t.task(), ptr))
	}

	/// Cancel the task with the given address (no-op when absent).
	pub fn cancel_task_by_ptr(&mut self, ptr: *const Task) {
		if let Some(index) = self.find_index(ptr) {
			let _ = self.cancel_task(index);
		}
	}

	/// Move the worker thread of the task with the given address out of the
	/// list so the caller can join it without holding the manager lock.
	pub fn take_thread_by_ptr(&mut self, ptr: *const Task) -> Option<std::thread::JoinHandle<()>> {
		let index = self.find_index(ptr)?;
		self.threads.get_mut(index).and_then(|h| h.take())
	}

	/// Remove every finished task (and its worker thread) from the list,
	/// returning the task boxes and thread handles so the caller can join
	/// them lock-free (the boxes must stay alive until the threads exit).
	pub fn drain_finished(&mut self) -> Vec<(Box<dyn TaskBox>, std::thread::JoinHandle<()>)> {
		let mut out = Vec::new();
		let mut i = 0;
		while i < self.tasks.len() {
			if self.tasks[i].task().is_finished() {
				let task = self.tasks.remove(i);
				let handle = self.threads.remove(i);
				if let Some(handle) = handle {
					out.push((task, handle));
				}
			} else {
				i += 1;
			}
		}
		out
	}

	/// Whether this manager's bookkeeping flag says the embedding
	/// application registered the codec submitter. Never set by
	/// [`TaskManager::init`] (which registers nothing); see
	/// [`TaskManager::set_codec_submitter_registered`].
	pub fn codec_submitter_registered(&self) -> bool {
		self.codec_submitter_registered
	}

	/// Record whether the embedding application registered the codec
	/// submitter. This is pure bookkeeping: it neither installs nor
	/// removes the oakcodec callback (that is
	/// `crate::codecbridge::register_codec_task_submitter` /
	/// `unregister_codec_task_submitter`).
	pub fn set_codec_submitter_registered(&mut self, registered: bool) {
		self.codec_submitter_registered = registered;
	}
}

impl Drop for TaskManager {
	fn drop(&mut self) {
		// Cancel every task, then join every worker (mirrors the C++
		// destructor: cancel all first, then join all).
		#[allow(invalid_reference_casting)]
		for task in &self.tasks {
			let ptr = task.task() as *const Task as usize as *mut Task;
			unsafe {
				(&mut *ptr).cancel();
			}
		}
		for handle in self.threads.iter_mut() {
			if let Some(h) = handle.take() {
				let _ = h.join();
			}
		}
	}
}
