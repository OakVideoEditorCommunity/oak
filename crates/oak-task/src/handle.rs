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

//! Refcounted-handle scaffolding. Same pattern as the oaknode/oakplugin
//! crates (`src/node/rust/src/handle.rs`); intentionally duplicated rather
//! than shared — each module DLL must run its own addref/release code (the
//! function pointers in a handle always point into the DLL that created the
//! object).
//!
//! M14 R5: only the parts the oakengine facade needs remain (owned
//! box/addref/release plus typed `get`/`get_mut` views — the facade boxes
//! its task payloads through [`make_owned`] and reads them back with
//! `get`/`get_mut`). The borrowed-handle and panic-guard helpers had no
//! in-crate callers and were removed.

use std::sync::atomic::AtomicU32;

/// ABI version stamped into every handle.
pub const OAKTASK_ABI_VERSION: u32 = 1;

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
/// here so the crate's `ffi.rs` signatures and handle scaffolding stay
/// source-compatible. `Send + Sync` come from the shared type.
pub use oak_core::handle::CHandle;

unsafe extern "C" fn owned_addref<T: 'static>(ctx: *mut std::ffi::c_void) {
	if !ctx.is_null() {
		// CPP-PARITY: src/task/c_api/taskhandle.h (task_addref)
		unsafe {
			(*(ctx as *const RefBox<T>))
				.refs
				.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
		}
	}
}

unsafe extern "C" fn owned_release<T: 'static>(ctx: *mut std::ffi::c_void) {
	if ctx.is_null() {
		return;
	}
	// CPP-PARITY: src/task/c_api/taskhandle.h (task_release)
	let b = ctx as *const RefBox<T>;
	let last = unsafe { (*b).refs.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) };
	if last == 1 {
		unsafe {
			drop(Box::from_raw(ctx as *mut RefBox<T>));
		}
	}
}

/// Owned handle with count 1; empty on allocation failure.
pub fn make_owned<T: Send + 'static>(value: T) -> CHandle {
	let b = Box::new(RefBox {
		refs: AtomicU32::new(1),
		value,
	});
	CHandle {
		ctx: Box::into_raw(b) as *mut std::ffi::c_void,
		addref: Some(owned_addref::<T>),
		release: Some(owned_release::<T>),
		abi_version: OAKTASK_ABI_VERSION,
	}
}

/// Typed view into a handle; `None` for empty handles.
///
/// # Safety
/// `T` must be the boxed type.
pub unsafe fn get<T: 'static>(h: &CHandle) -> Option<&T> {
	if h.ctx.is_null() {
		return None;
	}
	unsafe { Some(&(*(h.ctx as *const RefBox<T>)).value) }
}

/// Typed mutable view into a handle; `None` for empty handles.
///
/// # Safety
/// `T` must be the boxed type, and the handle must not be concurrently
/// shared mutably.
// The shared reference is the ABI input; exclusivity is the caller's
// `unsafe` contract above, so the lint's usual aliasing concern is
// discharged by the caller, not by `&mut CHandle` (which the C ABI does
// not pass).
#[allow(clippy::mut_from_ref)]
pub unsafe fn get_mut<T: 'static>(h: &CHandle) -> Option<&mut T> {
	if h.ctx.is_null() {
		return None;
	}
	unsafe { Some(&mut (*(h.ctx as *mut RefBox<T>)).value) }
}
