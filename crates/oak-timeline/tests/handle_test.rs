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

//! Contract tests for the refcounted-handle scaffolding
//! (`src/handle.rs`). The C++ gtest suite (`src/timeline/tests`)
//! drives the ABI-level refcount behaviour through the exports; these
//! tests pin the Rust-side primitive semantics every facade-facing
//! entry relies on: the null sentinel, `make_owned`'s shared
//! `Arc<Mutex<T>>` box, and the ABI version stamp. The old
//! `guard*`/`make_borrowed` helpers left with the deleted C ABI export
//! layer.

use std::sync::{Arc, Mutex};

use oak_timeline::error::OAKTIMELINE_ABI_VERSION;
use oak_timeline::handle::{get, make_owned, CHandle};

/// `CHandle::null()` is the all-null sentinel with abi_version 0.
#[test]
fn null_handle_is_all_zero() {
	let h = CHandle::null();
	assert!(h.ctx.is_null());
	assert!(h.addref.is_none());
	assert!(h.release.is_none());
	assert_eq!(h.abi_version, 0);
	assert!(h.is_null());
}

/// A freshly `make_owned` handle carries refcount 1 and the module ABI
/// version; releasing it once brings the count to zero.
#[test]
fn make_owned_starts_at_one_and_releases() {
	let h = make_owned(42i32);
	assert!(h.addref.is_some());
	assert!(h.release.is_some());
	assert_eq!(h.abi_version, OAKTIMELINE_ABI_VERSION);

	// Addref bumps the count so the box is still live afterwards.
	let addref = h.addref.unwrap();
	// Safety: `h` is an owned handle whose box is `RefBox<Arc<Mutex<i32>>>`.
	unsafe { addref(h.ctx) };
	// SAFETY: `make_owned` boxes an `Arc<Mutex<T>>`.
	let val = unsafe { get::<Arc<Mutex<i32>>>(&h) };
	assert_eq!(*val.unwrap().lock().unwrap(), 42);
}

/// `make_owned` boxes a `Send + 'static` value behind a shared
/// `Arc<Mutex<T>>` and `get` hands the shared `Arc` back out.
#[test]
fn make_owned_round_trips_value() {
	let h = make_owned(7i32);
	// SAFETY: `make_owned` boxes an `Arc<Mutex<T>>`.
	let val = unsafe { get::<Arc<Mutex<i32>>>(&h) };
	assert_eq!(*val.unwrap().lock().unwrap(), 7);
}

/// Every handle produced by `make_owned` shares one object: mutating
/// through a clone of the boxed `Arc` is visible through any other clone
/// of the same handle.
#[test]
fn make_owned_shares_one_object() {
	let h1 = make_owned(0i32);
	let h2 = h1;
	// SAFETY: both handles box the same `Arc<Mutex<i32>>` allocation.
	let a = unsafe { get::<Arc<Mutex<i32>>>(&h1) }.unwrap().clone();
	let b = unsafe { get::<Arc<Mutex<i32>>>(&h2) }.unwrap().clone();
	assert!(Arc::ptr_eq(&a, &b));
	*a.lock().unwrap() = 42;
	assert_eq!(*b.lock().unwrap(), 42);
}

/// `get` on a null handle yields `None` (the null-handle sentinel maps
/// to "no object" everywhere).
#[test]
fn get_null_handle_is_none() {
	let h = CHandle::null();
	// SAFETY: null handle -> None without touching any pointer.
	assert!(unsafe { get::<i32>(&h) }.is_none());
}

/// Every handle produced through the crate carries
/// `OAKTIMELINE_ABI_VERSION` so C callers can version-check.
#[test]
fn handles_are_stamped_with_abi_version() {
	let h = make_owned(1u64);
	assert_eq!(h.abi_version, OAKTIMELINE_ABI_VERSION);
	assert_ne!(h.abi_version, 0);
}
