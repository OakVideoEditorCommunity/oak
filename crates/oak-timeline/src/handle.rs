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

//! Refcounted-handle scaffolding for the oakengine facade boundary.
//!
//! After the single-lib unification the crate's internal object
//! references are plain Rust types: the timeline commands hold
//! [`Arc<Mutex<…>>`] handles to the shared marker list / work area, and
//! [`CHandle`] appears only in the facade-facing entries that the
//! oakengine C ABI export layer (and the first-party frontends) call.
//!
//! Every [`make_owned`] value handle boxes an [`Arc<Mutex<T>>`] behind a
//! [`RefBox`] (the same pattern as the oaknode project handles): the
//! facade reads the shared object back through
//! `get::<Arc<Mutex<T>>>`, clones the `Arc`, and the command entries do
//! the same when they convert a handle into the crate's Rust-typed
//! command constructors. The box's addref/release functions only manage
//! the handle shell — the `Arc` keeps the actual value alive.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// ABI version stamped into every oaktimeline handle.
pub const OAKTIMELINE_ABI_VERSION: u32 = 1;

/// Heap box behind a handle's `ctx`.
pub struct RefBox<T: ?Sized> {
	/// Atomic reference count.
	pub refs: AtomicU32,
	/// Boxed value.
	pub value: T,
}

/// The shared ABI value-handle type (single-lib unification, see
/// `docs/zh/plans/riir/single-lib.md`): one canonical
/// `{ctx, addref, release, abi_version}` type in `oakcore-rs`, re-exported
/// here so the crate's handle scaffolding stays source-compatible.
pub use oak_core::handle::CHandle;

/// Generic `addref` implementation: increments the box's reference count.
///
/// # Safety
/// `ptr` must point to a `RefBox<T>`.
unsafe extern "C" fn addref_box<T: 'static>(ptr: *mut c_void) {
	if ptr.is_null() {
		return;
	}
	let rb = unsafe { &*(ptr as *const RefBox<T>) };
	rb.refs.fetch_add(1, Ordering::SeqCst);
}

/// Generic `release` implementation: decrements the reference count and
/// destroys the box at zero.
///
/// # Safety
/// `ptr` must point to a `RefBox<T>`.
unsafe extern "C" fn release_box<T: 'static>(ptr: *mut c_void) {
	if ptr.is_null() {
		return;
	}
	let rb = ptr as *mut RefBox<T>;
	let prev = unsafe { (*rb).refs.fetch_sub(1, Ordering::SeqCst) };
	if prev == 1 {
		// Last reference: reclaim the box.
		drop(unsafe { Box::from_raw(rb) });
	}
}

/// Owned handle with count 1; empty on allocation failure.
///
/// The boxed value is an [`Arc<Mutex<T>>`] (see the module docs): every
/// handle produced here references the same shared object that the
/// timeline commands hold directly.
pub fn make_owned<T: Send + 'static>(value: T) -> CHandle {
	make_owned_arc(Arc::new(Mutex::new(value)))
}

/// Owned handle over an already-shared `Arc<Mutex<T>>`; empty on
/// allocation failure. The `Arc` is moved into the box, so every handle
/// created from the same `Arc` shares one object.
pub fn make_owned_arc<T: Send + 'static>(arc: Arc<Mutex<T>>) -> CHandle {
	let boxed = Box::new(RefBox {
		refs: AtomicU32::new(1),
		value: arc,
	});
	let ptr = Box::into_raw(boxed) as *mut c_void;
	CHandle {
		ctx: ptr,
		addref: Some(addref_box::<Arc<Mutex<T>>>),
		release: Some(release_box::<Arc<Mutex<T>>>),
		abi_version: OAKTIMELINE_ABI_VERSION,
	}
}

/// Typed view into a handle; `None` for empty handles.
///
/// Value handles box an [`Arc<Mutex<T>>`]; read the shared object with
/// `get::<Arc<Mutex<T>>>(h)` and clone the `Arc` (or lock it in place).
///
/// # Safety
/// `T` must be the boxed type.
pub unsafe fn get<T: 'static>(h: &CHandle) -> Option<&T> {
	if h.ctx.is_null() {
		return None;
	}
	let rb = unsafe { &*(h.ctx as *const RefBox<T>) };
	Some(&rb.value)
}

/// Mutable typed view into a handle; `None` for empty handles. With the
/// `Arc`-boxed value handles this yields `&mut Arc<Mutex<T>>` — use
/// [`get::<Arc<Mutex<T>>>`](get) plus `Mutex::lock` to mutate the shared
/// value instead.
///
/// # Safety
/// `T` must be the boxed type, and the caller must guarantee exclusive
/// access to the boxed value for the duration of the borrow (no two
/// mutable views alive at once).
// The shared reference is the ABI input; exclusivity is the caller's
// `unsafe` contract above, so the lint's usual aliasing concern is
// discharged by the caller, not by `&mut CHandle` (which the C ABI does
// not pass).
#[allow(clippy::mut_from_ref)]
pub unsafe fn get_mut<T: 'static>(h: &CHandle) -> Option<&mut T> {
	if h.ctx.is_null() {
		return None;
	}
	let rb = unsafe { &mut *(h.ctx as *mut RefBox<T>) };
	Some(&mut rb.value)
}
