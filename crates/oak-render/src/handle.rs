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

//! Refcounted-handle scaffolding for the oakengine facade entry points.
//!
//! M14 R5: after the single-lib unification, no object reference passes
//! as a `CHandle` inside oakrender anymore — the crate's internal calls
//! use Rust types directly. The remaining surface is only what the
//! facade's stubs.rs calls at the boundary: [`make_owned`] (box a value
//! into an owned handle), [`get`]/[`get_mut`] (typed views back out).
//!
//! Mirrors `src/render/c_api/internalhandles.h`: every public oakrender
//! handle is `{ctx, addref, release, abi_version}`; `ctx` points at a
//! [`RefBox<T>`] on this crate's heap.

use std::any::Any;
use std::sync::atomic::{AtomicU32, Ordering};

/// ABI version stamped into every handle.
pub const OAKRENDER_ABI_VERSION: u32 = 1;

/// Heap box behind a handle's `ctx`.
struct RefBox<T: ?Sized> {
	/// Atomic reference count.
	refs: AtomicU32,
	/// Boxed value.
	value: T,
}

/// The shared ABI value-handle type (single-lib unification, see
/// `docs/zh/plans/riir/single-lib.md`): one canonical
/// `{ctx, addref, release, abi_version}` type in `oakcore-rs`, re-exported
/// here so the facade's handle scaffolding stays source-compatible.
/// `Send + Sync` come from the shared type.
pub use oak_core::handle::CHandle;

/// addref implementation: atomic +1 on the box's refcount.
unsafe extern "C" fn refbox_addref<T: Any + Send>(ctx: *mut std::ffi::c_void) {
	unsafe {
		let rb = ctx as *const RefBox<T>;
		// The caller guarantees the handle stays valid for the call.
		(*rb).refs.fetch_add(1, Ordering::Relaxed);
	}
}

/// release implementation: atomic -1; at zero, reclaim the box and destroy
/// the contained object.
unsafe extern "C" fn refbox_release_owned<T: Any + Send>(ctx: *mut std::ffi::c_void) {
	unsafe {
		let rb = ctx as *mut RefBox<T>;
		// AcqRel: the thread that drops the last reference must observe all
		// prior writes (including internal state the destructor needs).
		if (*rb).refs.fetch_sub(1, Ordering::AcqRel) == 1 {
			drop(Box::from_raw(rb));
		}
	}
}

/// Owned handle with count 1; empty on allocation failure.
pub fn make_owned<T: Any + Send>(value: T) -> CHandle {
	let rb = Box::into_raw(Box::new(RefBox {
		refs: AtomicU32::new(1),
		value,
	}));
	CHandle {
		ctx: rb as *mut std::ffi::c_void,
		addref: Some(refbox_addref::<T>),
		release: Some(refbox_release_owned::<T>),
		abi_version: OAKRENDER_ABI_VERSION,
	}
}

/// Typed view into a handle; `None` for empty handles.
///
/// # Safety
/// `T` must be the boxed type.
pub unsafe fn get<T: Any>(h: &CHandle) -> Option<&T> {
	if h.is_null() {
		return None;
	}
	unsafe { Some(&(*(h.ctx as *const RefBox<T>)).value) }
}

/// Typed mutable view into a handle; `None` for empty handles.
///
/// # Safety
/// `T` must be the boxed type, and the caller must guarantee the handle
/// is not concurrently borrowed (the C ABI contract: a handle passed by
/// value is exclusively owned for the duration of the call).
// The shared reference is the ABI input; exclusivity is the caller's
// `unsafe` contract above, so the lint's usual aliasing concern is
// discharged by the caller, not by `&mut CHandle` (which the C ABI does
// not pass).
#[allow(clippy::mut_from_ref)]
pub unsafe fn get_mut<T: Any>(h: &CHandle) -> Option<&mut T> {
	if h.is_null() {
		return None;
	}
	unsafe { Some(&mut (*(h.ctx as *mut RefBox<T>)).value) }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Debug, PartialEq)]
	struct Obj(u32);

	#[test]
	fn owned_handle_refcount_and_destruction() {
		let h = make_owned(Obj(7));
		assert!(!h.is_null());
		assert_eq!(h.abi_version, OAKRENDER_ABI_VERSION);
		// addref/release through the stored function pointers.
		unsafe { h.addref.unwrap()(h.ctx) };
		unsafe { h.release.unwrap()(h.ctx) };
		unsafe { h.release.unwrap()(h.ctx) };
	}

	#[test]
	fn get_returns_boxed_value() {
		let h = make_owned(Obj(42));
		let v = unsafe { get::<Obj>(&h) };
		assert_eq!(v, Some(&Obj(42)));
		assert!(unsafe { get::<Obj>(&CHandle::null()) }.is_none());
		unsafe { h.release.unwrap()(h.ctx) };
	}

	#[test]
	fn get_mut_mutates_boxed_value() {
		let h = make_owned(Obj(3));
		unsafe {
			let v = get_mut::<Obj>(&h).unwrap();
			v.0 = 9;
			assert_eq!(get::<Obj>(&h).unwrap().0, 9);
		}
		unsafe { h.release.unwrap()(h.ctx) };
	}
}
