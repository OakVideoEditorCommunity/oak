//! Reference-counted handles handed across the C ABI.
//!
//! Every exported type is its own opaque handle on the C side and an
//! `Arc<T>` on the Rust side. `export_handle!` stamps out a typed
//! `add_ref`/`release` pair per type (monomorphized), so C code cannot
//! add_ref one type and release it as another — the C header typedefs
//! each pointee as a distinct opaque struct, making it a type error.

use std::sync::Arc;

/// Hands ownership of one strong reference to the C side.
///
/// The returned pointer starts with a ref count of 1; every
/// `<type>_add_ref` must later be balanced by exactly one
/// `<type>_release`, plus one release for this initial reference.
pub fn into_ffi<T>(value: T) -> *const T {
	Arc::into_raw(Arc::new(value))
}

/// The Rust side of a handle type exported via `export_handle!`.
///
/// Lets a typed wrapper (e.g. oak-app's `Handle<T>`) reach the right
/// refcount pair through generics instead of hard-coded symbols.
/// Implementations must treat null pointers as no-ops.
pub trait ArcHandle: Sized {
	/// Adds one strong reference.
	///
	/// # Safety
	/// `ptr` must be null or a live handle of this type from the engine,
	/// and the caller must already hold a reference on it.
	unsafe fn add_ref(ptr: *const Self);
	/// Consumes one strong reference, dropping the value on the last one.
	///
	/// # Safety
	/// `ptr` must be null or a live handle of this type from the engine.
	unsafe fn release(ptr: *const Self);
}

/// Generates the typed C ABI refcount pair for one handle type and its
/// `ArcHandle` impl: `<add_ref>(handle: *const $ty)` and
/// `<release>(handle: *const $ty)`.
#[macro_export]
macro_rules! export_handle {
	($ty:ty, $add_ref:ident, $release:ident) => {
		#[unsafe(no_mangle)]
		pub unsafe extern "C" fn $add_ref(handle: *const $ty) {
			if handle.is_null() {
				return;
			}
			// Increment without manufacturing an Arc: the pointer stays
			// borrowed, ownership of the count is all that moves.
			unsafe { std::sync::Arc::increment_strong_count(handle) };
		}

		#[unsafe(no_mangle)]
		pub unsafe extern "C" fn $release(handle: *const $ty) {
			if handle.is_null() {
				return;
			}
			// Rebuild the Arc and drop it: decrements the count, runs
			// T's Drop and frees the allocation on the last reference.
			unsafe { drop(std::sync::Arc::from_raw(handle)) };
		}

		impl $crate::handle::ArcHandle for $ty {
			unsafe fn add_ref(ptr: *const Self) {
				unsafe { $add_ref(ptr) };
			}
			unsafe fn release(ptr: *const Self) {
				unsafe { $release(ptr) };
			}
		}
	};
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicUsize, Ordering};

	/// Counts how many times the wrapped value was dropped.
	pub struct DropCounter(pub Arc<AtomicUsize>);

	impl Drop for DropCounter {
		fn drop(&mut self) {
			self.0.fetch_add(1, Ordering::SeqCst);
		}
	}

	export_handle!(DropCounter, drop_counter_add_ref, drop_counter_release);

	/// Reads the strong count without consuming the reference.
	unsafe fn strong_count<T>(ptr: *const T) -> usize {
		let arc = unsafe { Arc::from_raw(ptr) };
		let count = Arc::strong_count(&arc);
		std::mem::forget(arc);
		count
	}

	fn make_handle() -> (*const DropCounter, Arc<AtomicUsize>) {
		let flag = Arc::new(AtomicUsize::new(0));
		(into_ffi(DropCounter(flag.clone())), flag)
	}

	#[test]
	fn new_handle_starts_with_one_ref() {
		let (handle, flag) = make_handle();
		unsafe {
			assert_eq!(strong_count(handle), 1);
			drop_counter_release(handle);
		}
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn release_destroys_value_when_last_ref_drops() {
		let (handle, flag) = make_handle();
		unsafe { drop_counter_release(handle) };
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn add_ref_defers_destruction_until_matching_release() {
		let (handle, flag) = make_handle();
		unsafe {
			drop_counter_add_ref(handle);
			assert_eq!(strong_count(handle), 2);

			drop_counter_release(handle);
			assert_eq!(flag.load(Ordering::SeqCst), 0, "value must survive while refs remain");
			assert_eq!(strong_count(handle), 1);

			drop_counter_release(handle);
		}
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn many_refs_require_matching_releases() {
		let (handle, flag) = make_handle();
		const EXTRA_REFS: usize = 10;
		unsafe {
			for _ in 0..EXTRA_REFS {
				drop_counter_add_ref(handle);
			}
			assert_eq!(strong_count(handle), 1 + EXTRA_REFS);
			for _ in 0..EXTRA_REFS {
				drop_counter_release(handle);
				assert_eq!(flag.load(Ordering::SeqCst), 0);
			}
			assert_eq!(strong_count(handle), 1);
			drop_counter_release(handle);
		}
		assert_eq!(flag.load(Ordering::SeqCst), 1, "value must be destroyed exactly once");
	}

	#[test]
	fn null_handle_add_ref_and_release_are_noops() {
		unsafe {
			drop_counter_add_ref(std::ptr::null());
			drop_counter_release(std::ptr::null());
		}
	}

	#[test]
	fn destroys_heap_owning_values() {
		let handle = into_ffi(String::from("heap allocated payload"));
		let flag = Arc::new(AtomicUsize::new(0));
		let _ = flag;
		// String has no observable Drop; this just must not leak or crash
		// (leaks are caught by sanitizers/miri, crashes fail the test).
		unsafe { drop(Arc::from_raw(handle)) };
	}

	#[test]
	fn trait_dispatch_matches_exported_functions() {
		let (handle, flag) = make_handle();
		unsafe {
			<DropCounter as ArcHandle>::add_ref(handle);
			assert_eq!(strong_count(handle), 2);
			<DropCounter as ArcHandle>::release(handle);
			<DropCounter as ArcHandle>::release(handle);
		}
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn concurrent_add_ref_release_destroys_exactly_once() {
		let (handle, flag) = make_handle();
		// Raw pointers are not Send; pass the address as usize across threads.
		let addr = handle as usize;
		const THREADS: usize = 8;
		const OPS_PER_THREAD: usize = 1_000;

		let threads: Vec<_> = (0..THREADS)
			.map(|_| {
				std::thread::spawn(move || {
					let handle = addr as *const DropCounter;
					for _ in 0..OPS_PER_THREAD {
						unsafe {
							drop_counter_add_ref(handle);
							drop_counter_release(handle);
						}
					}
				})
			})
			.collect();
		for t in threads {
			t.join().unwrap();
		}

		unsafe {
			assert_eq!(strong_count(handle), 1, "all transient refs must be balanced");
			assert_eq!(flag.load(Ordering::SeqCst), 0);
			drop_counter_release(handle);
		}
		assert_eq!(flag.load(Ordering::SeqCst), 1);
	}
}
