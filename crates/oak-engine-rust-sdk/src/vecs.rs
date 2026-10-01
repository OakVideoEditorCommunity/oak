//! Safe wrappers over the engine's `Vec<T>` C ABI — the reverse of
//! oak-engine's `export_vec!`.
//!
//! `define_vec!` stamps out one owned wrapper type per vector ABI, named
//! from the same prefix string the engine exports:
//!
//! ```ignore
//! define_vec!(i32, VecI32, "oak_vec_i32");
//! define_vec!(String, VecString, "oak_vec_string");
//! ```
//!
//! The wrapper owns one reference on the engine's Arc-backed vector
//! (released on drop). No `Clone`: the engine's mutation calls are not
//! synchronized, so aliasing handles would allow unsynchronized mutation
//! — re-query the engine or `to_vec` instead. Mutation goes through
//! `&mut self`.

// The audio module's extern block already links the engine cdylib; this
// empty block keeps vecs self-sufficient if it is ever used alone.
#[link(name = "oak_engine")]
unsafe extern "C" {}

/// Generates a safe wrapper for one of the engine's vector ABIs.
/// The `String` arm must stay first: `String` would also match `$elem:ty`.
#[macro_export]
macro_rules! define_vec {
	// Vec<String>: elements come back as borrowed &str views.
	(String, $name:ident, $prefix:literal) => {
		#[doc = concat!("Owned wrapper over the engine's `", $prefix, "_*` ABI (`Vec<String>`).")]
		pub struct $name {
			ptr: *mut std::ffi::c_void,
		}

		// Arc-backed handle; Vec<String> is Send+Sync.
		unsafe impl Send for $name {}
		unsafe impl Sync for $name {}

		const _: () = {
			unsafe extern "C" {
				#[link_name = concat!($prefix, "_init")]
				fn init_raw() -> *mut std::ffi::c_void;
				#[link_name = concat!($prefix, "_len")]
				fn len_raw(this: *const std::ffi::c_void) -> usize;
				#[link_name = concat!($prefix, "_get")]
				fn get_raw(this: *const std::ffi::c_void, index: usize, out_len: *mut usize) -> *const u8;
				#[link_name = concat!($prefix, "_push")]
				fn push_raw(this: *mut std::ffi::c_void, value: *const u8, length: usize) -> bool;
				#[link_name = concat!($prefix, "_clear")]
				fn clear_raw(this: *mut std::ffi::c_void);
				#[link_name = concat!($prefix, "_release")]
				fn release_raw(this: *const std::ffi::c_void);
			}

			impl $name {
				/// An empty vector (ref count 1).
				pub fn new() -> Self {
					Self { ptr: unsafe { init_raw() } }
				}

				/// Takes ownership of one reference on a raw engine vector.
				///
				/// # Safety
				/// `ptr` must be null or a live handle of this exact
				/// vector type from the engine.
				pub unsafe fn from_raw(ptr: *mut std::ffi::c_void) -> Self {
					Self { ptr }
				}

				pub fn len(&self) -> usize {
					unsafe { len_raw(self.ptr) }
				}

				pub fn is_empty(&self) -> bool {
					self.len() == 0
				}

				/// Borrows the string at `index`; `None` when out of
				/// bounds. The view lives as long as this wrapper.
				pub fn get(&self, index: usize) -> Option<&str> {
					let mut len = 0usize;
					let ptr = unsafe { get_raw(self.ptr, index, &mut len) };
					if ptr.is_null() {
						return None;
					}
					let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
					// Checked (not from_utf8_unchecked): validate at the
					// ABI boundary instead of trusting the other side.
					std::str::from_utf8(bytes).ok()
				}

				/// Appends one string. Only fails when this wrapper holds
				/// null (Rust `&str` is always valid UTF-8).
				pub fn push(&mut self, value: &str) -> bool {
					unsafe { push_raw(self.ptr, value.as_ptr(), value.len()) }
				}

				/// Drops all elements, keeping the vector alive.
				pub fn clear(&mut self) {
					unsafe { clear_raw(self.ptr) };
				}

				pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
					(0..self.len()).filter_map(move |i| self.get(i))
				}

				pub fn to_vec(&self) -> Vec<String> {
					self.iter().map(str::to_owned).collect()
				}
			}

			impl Default for $name {
				fn default() -> Self {
					Self::new()
				}
			}

			impl<'a> IntoIterator for &'a $name {
				type Item = &'a str;
				type IntoIter = Box<dyn Iterator<Item = &'a str> + 'a>;

				fn into_iter(self) -> Self::IntoIter {
					Box::new(self.iter())
				}
			}

			impl Drop for $name {
				fn drop(&mut self) {
					unsafe { release_raw(self.ptr) };
				}
			}
		};
	};

	// Vec<T> for Copy scalars: elements cross by value.
	($elem:ty, $name:ident, $prefix:literal) => {
		#[doc = concat!("Owned wrapper over the engine's `", $prefix, "_*` ABI (`Vec<", stringify!($elem), ">`).")]
		pub struct $name {
			ptr: *mut std::ffi::c_void,
		}

		// Arc-backed handle; Vec<T: Copy> is Send+Sync.
		unsafe impl Send for $name {}
		unsafe impl Sync for $name {}

		const _: () = {
			unsafe extern "C" {
				#[link_name = concat!($prefix, "_init")]
				fn init_raw() -> *mut std::ffi::c_void;
				#[link_name = concat!($prefix, "_len")]
				fn len_raw(this: *const std::ffi::c_void) -> usize;
				#[link_name = concat!($prefix, "_get")]
				fn get_raw(this: *const std::ffi::c_void, index: usize, out: *mut $elem) -> bool;
				#[link_name = concat!($prefix, "_push")]
				fn push_raw(this: *mut std::ffi::c_void, value: $elem);
				#[link_name = concat!($prefix, "_set")]
				fn set_raw(this: *mut std::ffi::c_void, index: usize, value: $elem) -> bool;
				#[link_name = concat!($prefix, "_clear")]
				fn clear_raw(this: *mut std::ffi::c_void);
				#[link_name = concat!($prefix, "_release")]
				fn release_raw(this: *const std::ffi::c_void);
			}

			impl $name {
				/// An empty vector (ref count 1).
				pub fn new() -> Self {
					Self { ptr: unsafe { init_raw() } }
				}

				/// Takes ownership of one reference on a raw engine vector.
				///
				/// # Safety
				/// `ptr` must be null or a live handle of this exact
				/// vector type from the engine.
				pub unsafe fn from_raw(ptr: *mut std::ffi::c_void) -> Self {
					Self { ptr }
				}

				pub fn len(&self) -> usize {
					unsafe { len_raw(self.ptr) }
				}

				pub fn is_empty(&self) -> bool {
					self.len() == 0
				}

				/// A copy of the element at `index`; `None` when out of
				/// bounds.
				pub fn get(&self, index: usize) -> Option<$elem> {
					let mut out = <$elem>::default();
					if unsafe { get_raw(self.ptr, index, &mut out) } {
						Some(out)
					} else {
						None
					}
				}

				pub fn push(&mut self, value: $elem) {
					unsafe { push_raw(self.ptr, value) };
				}

				/// Overwrites the element at `index`; false when out of
				/// bounds.
				pub fn set(&mut self, index: usize, value: $elem) -> bool {
					unsafe { set_raw(self.ptr, index, value) }
				}

				/// Drops all elements, keeping the vector alive.
				pub fn clear(&mut self) {
					unsafe { clear_raw(self.ptr) };
				}

				pub fn iter(&self) -> impl Iterator<Item = $elem> + '_ {
					(0..self.len()).map(move |i| self.get(i).expect("index < len"))
				}

				pub fn to_vec(&self) -> Vec<$elem> {
					self.iter().collect()
				}
			}

			impl Default for $name {
				fn default() -> Self {
					Self::new()
				}
			}

			impl<'a> IntoIterator for &'a $name {
				type Item = $elem;
				type IntoIter = Box<dyn Iterator<Item = $elem> + 'a>;

				fn into_iter(self) -> Self::IntoIter {
					Box::new(self.iter())
				}
			}

			impl Drop for $name {
				fn drop(&mut self) {
					unsafe { release_raw(self.ptr) };
				}
			}
		};
	};
}

// Mirrors oak-engine's instantiated vector ABIs.
define_vec!(u8, VecU8, "oak_vec_u8");
define_vec!(u16, VecU16, "oak_vec_u16");
define_vec!(u32, VecU32, "oak_vec_u32");
define_vec!(u64, VecU64, "oak_vec_u64");
define_vec!(usize, VecUsize, "oak_vec_usize");
define_vec!(i8, VecI8, "oak_vec_i8");
define_vec!(i16, VecI16, "oak_vec_i16");
define_vec!(i32, VecI32, "oak_vec_i32");
define_vec!(i64, VecI64, "oak_vec_i64");
define_vec!(isize, VecIsize, "oak_vec_isize");
define_vec!(f32, VecF32, "oak_vec_f32");
define_vec!(f64, VecF64, "oak_vec_f64");
define_vec!(bool, VecBool, "oak_vec_bool");
define_vec!(String, VecString, "oak_vec_string");

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn scalar_vec_roundtrip() {
		let mut v = VecI32::new();
		assert!(v.is_empty());
		v.push(10);
		v.push(20);
		v.push(30);
		assert_eq!(v.len(), 3);
		assert_eq!(v.get(1), Some(20));
		assert!(v.set(1, 99));
		assert!(!v.set(3, 0), "out-of-bounds set must fail");
		assert_eq!(v.get(3), None, "out-of-bounds get must be None");
		assert_eq!(v.to_vec(), vec![10, 99, 30]);
		assert_eq!(v.iter().sum::<i32>(), 139);
		v.clear();
		assert!(v.is_empty());
	}

	#[test]
	fn string_vec_roundtrip() {
		let mut v = VecString::new();
		assert!(v.is_empty());
		assert!(v.push("hello"));
		assert!(v.push("世界"));
		assert!(v.push(""));
		assert_eq!(v.len(), 3);
		assert_eq!(v.get(1), Some("世界"));
		assert_eq!(v.get(2), Some(""));
		assert_eq!(v.get(3), None);
		assert_eq!(v.to_vec(), vec!["hello", "世界", ""]);
		assert_eq!((&v).into_iter().count(), 3);
		v.clear();
		assert!(v.is_empty());
	}

	#[test]
	fn wrappers_move_across_threads() {
		let mut v = VecF64::new();
		v.push(2.5);
		let len = std::thread::spawn(move || (v.len(), v.get(0))).join().unwrap();
		assert_eq!(len, (1, Some(2.5)));
	}
}
