//! Typed RAII wrappers for the engine's reference-counted handles.
//!
//! The boundary between oak-app and oak-engine is a pure C ABI: only
//! `extern "C"` symbols and opaque pointers cross it, never Rust types.
//! `define_handle!` stamps out one wrapper struct per handle type, wired
//! to that type's `<prefix>_add_ref` / `<prefix>_release` pair exported
//! by the engine (see `oak_engine::export_handle!`):
//!
//! ```ignore
//! define_handle!(Timeline, "oak_timeline");
//! ```
//!
//! Ownership convention: `from_owned` takes ownership of one *existing*
//! reference (the one the engine handed out); `from_borrowed` first takes
//! a new reference on a pointer that is only borrowed.

// Symbols resolve dynamically at runtime (see oak-engine-rust-sdk);
// this crate deliberately has no link-time dependency on the engine.

// Unused until the first real engine type is exported; keep it compiled
// and warn-free in the meantime.
#[allow(unused_macros)]
macro_rules! define_handle {
	($name:ident, $prefix:literal) => {
		/// Reference-counted engine handle (opaque, C ABI).
		///
		/// `Clone`/`Drop` map to the engine's refcount pair. `Send`/`Sync`
		/// hold because the engine's counting is atomic and the last
		/// release may drop the payload on whatever thread performs it —
		/// so only wrap thread-safe engine objects with this macro.
		#[repr(transparent)]
		pub struct $name {
			ptr: *const std::ffi::c_void,
		}

		unsafe impl Send for $name {}
		unsafe impl Sync for $name {}

		impl $name {
			/// Takes ownership of one existing reference on `ptr`.
			///
			/// # Safety
			/// `ptr` must be null or a live handle of this exact type
			/// from the engine.
			pub unsafe fn from_owned(ptr: *const std::ffi::c_void) -> Self {
				Self { ptr }
			}

			/// Wraps a borrowed pointer, taking a new reference on it.
			///
			/// # Safety
			/// `ptr` must be null or a live handle of this exact type
			/// from the engine.
			pub unsafe fn from_borrowed(ptr: *const std::ffi::c_void) -> Self {
				unsafe { Self::add_ref(ptr) };
				Self { ptr }
			}

			/// The raw pointer, for passing back into engine functions.
			/// The borrow ends with this handle; do not release it manually.
			pub fn as_ptr(&self) -> *const std::ffi::c_void {
				self.ptr
			}

			unsafe fn add_ref(ptr: *const std::ffi::c_void) {
				extern "C" {
					#[link_name = concat!($prefix, "_add_ref")]
					fn raw_add_ref(handle: *const std::ffi::c_void);
				}
				// The engine side treats null as a no-op.
				unsafe { raw_add_ref(ptr) };
			}

			unsafe fn release(ptr: *const std::ffi::c_void) {
				extern "C" {
					#[link_name = concat!($prefix, "_release")]
					fn raw_release(handle: *const std::ffi::c_void);
				}
				unsafe { raw_release(ptr) };
			}
		}

		impl Clone for $name {
			fn clone(&self) -> Self {
				unsafe { Self::add_ref(self.ptr) };
				Self { ptr: self.ptr }
			}
		}

		impl Drop for $name {
			fn drop(&mut self) {
				unsafe { Self::release(self.ptr) };
			}
		}
	};
}

#[cfg(test)]
mod tests {
	use std::ffi::c_void;
	use std::sync::Arc;
	use std::sync::atomic::{AtomicUsize, Ordering};

	// The engine side of the ABI for a test payload, mirroring what
	// `oak_engine::export_handle!` generates. Defined locally so the
	// wrapper is exercised through the C ABI boundary, not through
	// engine internals.
	struct DropFlag(Arc<AtomicUsize>);

	impl Drop for DropFlag {
		fn drop(&mut self) {
			self.0.fetch_add(1, Ordering::SeqCst);
		}
	}

	#[no_mangle]
	pub extern "C" fn oak_testflag_add_ref(handle: *const c_void) {
		if handle.is_null() {
			return;
		}
		unsafe { Arc::increment_strong_count(handle.cast::<DropFlag>()) };
	}

	#[no_mangle]
	pub extern "C" fn oak_testflag_release(handle: *const c_void) {
		if handle.is_null() {
			return;
		}
		unsafe { drop(Arc::from_raw(handle.cast::<DropFlag>())) };
	}

	define_handle!(TestFlag, "oak_testflag");

	fn make_handle() -> (TestFlag, Arc<AtomicUsize>) {
		let flag = Arc::new(AtomicUsize::new(0));
		let raw = Arc::into_raw(Arc::new(DropFlag(flag.clone()))).cast::<c_void>();
		(unsafe { TestFlag::from_owned(raw) }, flag)
	}

	#[test]
	fn from_owned_takes_ownership_without_adding_a_ref() {
		let (handle, flag) = make_handle();
		drop(handle);
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn clone_keeps_data_alive_until_last_drop() {
		let (handle, flag) = make_handle();
		let clone = handle.clone();
		drop(handle);
		assert_eq!(flag.load(Ordering::SeqCst), 0);
		drop(clone);
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn many_clones_drop_order_does_not_matter() {
		let (handle, flag) = make_handle();
		let clones: Vec<_> = (0..10).map(|_| handle.clone()).collect();
		for clone in clones {
			drop(clone);
			assert_eq!(flag.load(Ordering::SeqCst), 0);
		}
		drop(handle);
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn from_borrowed_takes_a_new_reference() {
		let flag = Arc::new(AtomicUsize::new(0));
		let raw = Arc::into_raw(Arc::new(DropFlag(flag.clone()))).cast::<c_void>();

		let handle = unsafe { TestFlag::from_borrowed(raw) };
		drop(handle);
		assert_eq!(flag.load(Ordering::SeqCst), 0, "the borrowed reference must remain");

		unsafe { oak_testflag_release(raw) };
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn as_ptr_returns_the_wrapped_pointer() {
		let (handle, _flag) = make_handle();
		assert_eq!(handle.as_ptr(), handle.ptr);
	}

	#[test]
	fn null_handle_is_safe_to_clone_and_drop() {
		let handle = unsafe { TestFlag::from_owned(std::ptr::null()) };
		let clone = handle.clone();
		drop(handle);
		drop(clone);
	}

	#[test]
	fn handle_can_move_to_another_thread() {
		let (handle, flag) = make_handle();
		std::thread::spawn(move || drop(handle)).join().unwrap();
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn concurrent_clone_and_drop_destroys_exactly_once() {
		let (handle, flag) = make_handle();
		std::thread::scope(|s| {
			for _ in 0..8 {
				s.spawn(|| {
					for _ in 0..1_000 {
						drop(handle.clone());
					}
				});
			}
		});
		assert_eq!(flag.load(Ordering::SeqCst), 0);
		drop(handle);
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}
}
