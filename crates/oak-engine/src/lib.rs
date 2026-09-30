use std::sync::atomic::{AtomicI64, Ordering};

pub mod audio;

pub fn add(left: u64, right: u64) -> u64 {
	left + right
}

/// A reference counted handle
pub struct CHandle{
	ref_count: AtomicI64,
	data: *mut std::ffi::c_void,
	release: unsafe extern "C" fn(*mut std::ffi::c_void),
}
pub type Handle = *mut CHandle;

impl CHandle{
	pub fn new<T>(value: T) -> Handle {
		unsafe extern "C" fn destroy<T>(data: *mut std::ffi::c_void) {
			unsafe { drop(Box::from_raw(data.cast::<T>())); }
		}
		let handle = Box::new(CHandle {
			ref_count: AtomicI64::new(1),
			data: Box::into_raw(Box::new(value)).cast(),
			release: destroy::<T>,
		});
		Box::into_raw(handle)
	}
}

/// Creates a handle over a raw data pointer owned by the caller (e.g. C code).
/// `destroy` is called exactly once, when the last reference is released, and
/// must dispose of `data`. The returned handle starts with a ref count of 1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chandle_new(
	data: *mut std::ffi::c_void,
	destroy: unsafe extern "C" fn(*mut std::ffi::c_void),
) -> Handle {
	Box::into_raw(Box::new(CHandle {
		ref_count: AtomicI64::new(1),
		data,
		release: destroy,
	}))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn add_ref(handle: Handle){
	if handle.is_null() { return; }
	unsafe{(*handle).ref_count.fetch_add(1, Ordering::Relaxed);}
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn release(handle: Handle){
	if handle.is_null() {
		return;
	}
	unsafe {
		if (*handle).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
			((*handle).release)((*handle).data);
			drop(Box::from_raw(handle));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicUsize, Ordering as TestOrdering};

	/// Counts how many times the wrapped value was dropped.
	struct DropCounter<'a>(&'a AtomicUsize);

	impl Drop for DropCounter<'_> {
		fn drop(&mut self) {
			self.0.fetch_add(1, TestOrdering::SeqCst);
		}
	}

	unsafe fn ref_count(handle: Handle) -> i64 {
		unsafe { (*handle).ref_count.load(Ordering::SeqCst) }
	}

	#[test]
	fn it_works() {
		let result = add(2, 2);
		assert_eq!(result, 4);
	}

	#[test]
	fn new_handle_starts_with_one_ref() {
		let drops = AtomicUsize::new(0);
		let handle = CHandle::new(DropCounter(&drops));
		unsafe {
			assert_eq!(ref_count(handle), 1);
			release(handle);
		}
		assert_eq!(drops.load(TestOrdering::SeqCst), 1);
	}

	#[test]
	fn release_destroys_data_and_handle_when_last_ref_drops() {
		let drops = AtomicUsize::new(0);
		let handle = CHandle::new(DropCounter(&drops));
		unsafe { release(handle) };
		assert_eq!(drops.load(TestOrdering::SeqCst), 1);
	}

	#[test]
	fn add_ref_defers_destruction_until_matching_release() {
		let drops = AtomicUsize::new(0);
		let handle = CHandle::new(DropCounter(&drops));
		unsafe {
			add_ref(handle);
			assert_eq!(ref_count(handle), 2);

			release(handle);
			assert_eq!(drops.load(TestOrdering::SeqCst), 0, "data must survive while refs remain");
			assert_eq!(ref_count(handle), 1);

			release(handle);
		}
		assert_eq!(drops.load(TestOrdering::SeqCst), 1);
	}

	#[test]
	fn many_refs_require_matching_releases() {
		let drops = AtomicUsize::new(0);
		let handle = CHandle::new(DropCounter(&drops));
		const EXTRA_REFS: i64 = 10;
		unsafe {
			for _ in 0..EXTRA_REFS {
				add_ref(handle);
			}
			assert_eq!(ref_count(handle), 1 + EXTRA_REFS);
			for _ in 0..EXTRA_REFS {
				release(handle);
				assert_eq!(drops.load(TestOrdering::SeqCst), 0);
			}
			assert_eq!(ref_count(handle), 1);
			release(handle);
		}
		assert_eq!(drops.load(TestOrdering::SeqCst), 1, "data must be destroyed exactly once");
	}

	#[test]
	fn null_handle_add_ref_and_release_are_noops() {
		unsafe {
			add_ref(std::ptr::null_mut());
			release(std::ptr::null_mut());
		}
	}

	#[test]
	fn destroys_heap_owning_values() {
		let handle = CHandle::new(String::from("heap allocated payload"));
		unsafe { release(handle) };
	}

	#[test]
	fn ffi_constructor_lifecycle() {
		static DESTROYS: AtomicUsize = AtomicUsize::new(0);

		unsafe extern "C" fn destroy(data: *mut std::ffi::c_void) {
			DESTROYS.fetch_add(1, TestOrdering::SeqCst);
			unsafe { drop(Box::from_raw(data.cast::<u64>())); }
		}

		let data = Box::into_raw(Box::new(42u64)).cast();
		let handle = unsafe { chandle_new(data, destroy) };
		unsafe {
			assert_eq!(ref_count(handle), 1);
			add_ref(handle);
			release(handle);
			assert_eq!(DESTROYS.load(TestOrdering::SeqCst), 0);
			release(handle);
		}
		assert_eq!(DESTROYS.load(TestOrdering::SeqCst), 1);
	}

	#[test]
	fn concurrent_add_ref_release_destroys_exactly_once() {
		let drops = AtomicUsize::new(0);
		let handle = CHandle::new(DropCounter(&drops));
		// Raw pointers are not Send; pass the address as usize across threads.
		let addr = handle as usize;
		const THREADS: usize = 8;
		const OPS_PER_THREAD: usize = 1_000;

		let threads: Vec<_> = (0..THREADS)
			.map(|_| {
				std::thread::spawn(move || {
					let handle = addr as Handle;
					for _ in 0..OPS_PER_THREAD {
						unsafe {
							add_ref(handle);
							release(handle);
						}
					}
				})
			})
			.collect();
		for t in threads {
			t.join().unwrap();
		}

		unsafe {
			assert_eq!(ref_count(handle), 1, "all transient refs must be balanced");
			assert_eq!(drops.load(TestOrdering::SeqCst), 0);
			release(handle);
		}
		assert_eq!(drops.load(TestOrdering::SeqCst), 1);
	}
}
