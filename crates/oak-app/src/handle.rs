//! RAII wrapper around the engine's reference-counted C handle.
//!
//! Ownership convention: `Handle::new` takes ownership of one *existing*
//! reference (the one the engine handed out) without calling `add_ref`.
//! If the pointer was only borrowed from the engine, use
//! `Handle::from_borrowed` instead, which takes a new reference on it.

use oak_engine::{CHandle, add_ref, release};

type RawHandle = *mut CHandle;

pub struct Handle {
    handle: RawHandle,
}

// The engine's reference counting is atomic and both `add_ref`/`release`
// are null-safe, so this ownership token can move and be shared across
// threads freely. (This says nothing about thread-safety of the data
// behind the handle, which is the engine's concern.)
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Handle {
    /// Takes ownership of one existing reference on `handle`.
    pub fn new(handle: RawHandle) -> Handle {
        Handle { handle }
    }

    /// Wraps a borrowed pointer, taking a new reference on it.
    pub fn from_borrowed(handle: RawHandle) -> Handle {
        unsafe { add_ref(handle) };
        Handle { handle }
    }

    /// The raw pointer, for passing back into engine functions.
    /// The borrow ends with this `Handle`; do not release it manually.
    pub fn as_ptr(&self) -> RawHandle {
        self.handle
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { release(self.handle) };
    }
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        unsafe { add_ref(self.handle) };
        Handle { handle: self.handle }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oak_engine::chandle_new;
    use std::ffi::c_void;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Data whose destruction can be observed through the shared counter.
    struct DropFlag(Arc<AtomicUsize>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    unsafe extern "C" fn destroy(data: *mut c_void) {
        unsafe { drop(Box::from_raw(data.cast::<DropFlag>())) };
    }

    fn make_handle() -> (Handle, Arc<AtomicUsize>) {
        let flag = Arc::new(AtomicUsize::new(0));
        let data = Box::into_raw(Box::new(DropFlag(flag.clone()))).cast();
        let raw = unsafe { chandle_new(data, destroy) };
        (Handle::new(raw), flag)
    }

    #[test]
    fn new_takes_ownership_without_adding_a_ref() {
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
        let data = Box::into_raw(Box::new(DropFlag(flag.clone()))).cast();
        let raw = unsafe { chandle_new(data, destroy) };

        let handle = Handle::from_borrowed(raw);
        drop(handle);
        assert_eq!(flag.load(Ordering::SeqCst), 0, "the borrowed reference must remain");

        unsafe { release(raw) };
        assert_eq!(flag.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn as_ptr_returns_the_wrapped_pointer() {
        let (handle, _flag) = make_handle();
        assert_eq!(handle.as_ptr(), handle.handle);
    }

    #[test]
    fn null_handle_is_safe_to_clone_and_drop() {
        let handle = Handle::new(std::ptr::null_mut());
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
