//! C ABI wrappers for `Vec<T>` of Rust/std built-in element types.
//!
//! `export_vec!` stamps out one self-contained C API per vector type,
//! named from a prefix string (`#[export_name = concat!(...)]` does the
//! symbol concatenation, `const _: () = { ... }` scopes the generated
//! Rust item names so invocations cannot collide):
//!
//! ```ignore
//! export_vec!(i32, OakVecI32, "oak_vec_i32");
//! export_vec!(String, OakVecString, "oak_vec_string");
//! ```
//!
//! Generated functions (all null-safe, no panics cross the boundary):
//!
//! | symbol | signature |
//! |---|---|
//! | `<prefix>_init` | `() -> *mut Vec<T>` — empty vector, ref count 1 |
//! | `<prefix>_add_ref` / `_release` | the standard Arc handle pair |
//! | `<prefix>_len` | `(*const Vec<T>) -> usize` |
//! | `<prefix>_push` | scalar: `(*mut, T)`; string: `(*mut, *const u8, usize) -> bool` (UTF-8 checked) |
//! | `<prefix>_get` | scalar: `(*const, usize, *mut T) -> bool`; string: `(*const, usize, *mut usize) -> *const u8` |
//! | `<prefix>_set` | scalar only: `(*mut, usize, T) -> bool` |
//! | `<prefix>_clear` | `(*mut Vec<T>)` |
//!
//! String getters return a borrowed, non-NUL-terminated pointer; the
//! length comes back through the out-parameter. The pointer stays valid
//! until the vector is cleared or released.

/// Converts an owned `Vec<T>` into its C ABI wrapper handle.
///
/// The generated aliases are plain `Vec<T>`, so inherent methods are
/// impossible — this trait (implemented per alias by `export_vec!`) is
/// the conversion entry point. The result starts with ref count 1 and
/// must be balanced with `<prefix>_release`.
///
/// ```
/// # use oak_engine::vecs::IntoFfiVec;
/// let handle: *mut Vec<i32> = vec![1, 2, 3].into_ffi_vec();
/// # unsafe { drop(std::sync::Arc::from_raw(handle)) };
/// ```
pub trait IntoFfiVec: Sized {
	fn into_ffi_vec(self) -> *mut Self {
		crate::handle::into_ffi(self) as *mut Self
	}
}

/// Generates the C ABI for one `Vec<T>` (see the module docs).
/// The `String` arm must stay first: `String` would also match `$elem:ty`.
#[macro_export]
macro_rules! export_vec {
	// Vec<String>: elements cross the boundary as (ptr, len) UTF-8 pairs.
	(String, $alias:ident, $prefix:literal) => {
		#[doc = concat!("C ABI wrapper over `Vec<String>` (`", $prefix, "_*`).")]
		pub type $alias = Vec<String>;

		$crate::export_vec!(@common $alias, $prefix);

		const _: () = {
			#[doc = concat!("Pushes one UTF-8 string (`", $prefix, "_push`). False on null / invalid UTF-8.")]
			#[unsafe(export_name = concat!($prefix, "_push"))]
			pub extern "C" fn push(this: *mut $alias, value: *const u8, length: usize) -> bool {
				if this.is_null() {
					return false;
				}
				// null + 0 means the empty string; null + N is an error.
				// from_raw_parts(null, 0) would be UB, so special-case it.
				let bytes = if value.is_null() {
					if length > 0 {
						return false;
					}
					&[]
				} else {
					unsafe { std::slice::from_raw_parts(value, length) }
				};
				match std::str::from_utf8(bytes) {
					Ok(s) => {
						unsafe { &mut *this }.push(s.to_owned());
						true
					}
					Err(_) => false,
				}
			}

			#[doc = concat!("Borrows the string at `index` (`", $prefix, "_get`). Null when out of bounds.")]
			#[unsafe(export_name = concat!($prefix, "_get"))]
			pub extern "C" fn get(
				this: *const $alias,
				index: usize,
				out_len: *mut usize,
			) -> *const u8 {
				if this.is_null() {
					return std::ptr::null();
				}
				match unsafe { &*this }.get(index) {
					Some(s) => {
						if !out_len.is_null() {
							unsafe { *out_len = s.len() };
						}
						s.as_ptr()
					}
					None => std::ptr::null(),
				}
			}
		};
	};

	// Vec<T> for Copy scalars: elements cross the boundary by value.
	($elem:ty, $alias:ident, $prefix:literal) => {
		#[doc = concat!("C ABI wrapper over `Vec<", stringify!($elem), ">` (`", $prefix, "_*`).")]
		pub type $alias = Vec<$elem>;

		$crate::export_vec!(@common $alias, $prefix);

		const _: () = {
			#[doc = concat!("Appends one element (`", $prefix, "_push`).")]
			#[unsafe(export_name = concat!($prefix, "_push"))]
			pub extern "C" fn push(this: *mut $alias, value: $elem) {
				if this.is_null() {
					return;
				}
				unsafe { &mut *this }.push(value);
			}

			#[doc = concat!("Copies the element at `index` into `out` (`", $prefix, "_get`). False when out of bounds.")]
			#[unsafe(export_name = concat!($prefix, "_get"))]
			pub extern "C" fn get(this: *const $alias, index: usize, out: *mut $elem) -> bool {
				if this.is_null() || out.is_null() {
					return false;
				}
				match unsafe { &*this }.get(index) {
					Some(v) => {
						unsafe { *out = *v };
						true
					}
					None => false,
				}
			}

			#[doc = concat!("Overwrites the element at `index` (`", $prefix, "_set`). False when out of bounds.")]
			#[unsafe(export_name = concat!($prefix, "_set"))]
			pub extern "C" fn set(this: *mut $alias, index: usize, value: $elem) -> bool {
				if this.is_null() {
					return false;
				}
				match unsafe { &mut *this }.get_mut(index) {
					Some(slot) => {
						*slot = value;
						true
					}
					None => false,
				}
			}
		};
	};

	// Shared by both arms: the Arc handle pair + init/len/clear.
	(@common $alias:ident, $prefix:literal) => {
		const _: () = {
			#[doc = concat!("Adds one strong reference (`", $prefix, "_add_ref`). Null is a no-op.")]
			#[unsafe(export_name = concat!($prefix, "_add_ref"))]
			pub unsafe extern "C" fn add_ref(this: *const $alias) {
				if this.is_null() {
					return;
				}
				unsafe { std::sync::Arc::increment_strong_count(this) };
			}

			#[doc = concat!("Consumes one strong reference (`", $prefix, "_release`). Null is a no-op.")]
			#[unsafe(export_name = concat!($prefix, "_release"))]
			pub unsafe extern "C" fn release(this: *const $alias) {
				if this.is_null() {
					return;
				}
				unsafe { drop(std::sync::Arc::from_raw(this)) };
			}

			#[doc = concat!("Creates an empty vector with ref count 1 (`", $prefix, "_init`).")]
			#[unsafe(export_name = concat!($prefix, "_init"))]
			pub extern "C" fn init() -> *mut $alias {
				$crate::handle::into_ffi(<$alias>::new()) as *mut $alias
			}

			#[doc = concat!("Element count (`", $prefix, "_len`). Null reads as 0.")]
			#[unsafe(export_name = concat!($prefix, "_len"))]
			pub extern "C" fn len(this: *const $alias) -> usize {
				if this.is_null() {
					return 0;
				}
				unsafe { &*this }.len()
			}

			#[doc = concat!("Drops all elements, keeping the vector alive (`", $prefix, "_clear`).")]
			#[unsafe(export_name = concat!($prefix, "_clear"))]
			pub extern "C" fn clear(this: *mut $alias) {
				if this.is_null() {
					return;
				}
				unsafe { &mut *this }.clear();
			}

			impl $crate::handle::ArcHandle for $alias {
				unsafe fn add_ref(ptr: *const Self) {
					unsafe { add_ref(ptr) };
				}
				unsafe fn release(ptr: *const Self) {
					unsafe { release(ptr) };
				}
			}

			impl $crate::vecs::IntoFfiVec for $alias {}
		};
	};
}

// The built-in scalar vectors. Element types are plain-old-data, so
// getters copy out and setters copy in — no borrowed pointers cross.
export_vec!(u8, OakVecU8, "oak_vec_u8");
export_vec!(u16, OakVecU16, "oak_vec_u16");
export_vec!(u32, OakVecU32, "oak_vec_u32");
export_vec!(u64, OakVecU64, "oak_vec_u64");
export_vec!(usize, OakVecUsize, "oak_vec_usize");
export_vec!(i8, OakVecI8, "oak_vec_i8");
export_vec!(i16, OakVecI16, "oak_vec_i16");
export_vec!(i32, OakVecI32, "oak_vec_i32");
export_vec!(i64, OakVecI64, "oak_vec_i64");
export_vec!(isize, OakVecIsize, "oak_vec_isize");
export_vec!(f32, OakVecF32, "oak_vec_f32");
export_vec!(f64, OakVecF64, "oak_vec_f64");
export_vec!(bool, OakVecBool, "oak_vec_bool");

// Strings need (ptr, len) pairs and UTF-8 validation at the boundary.
export_vec!(String, OakVecString, "oak_vec_string");

#[cfg(test)]
mod tests {
	// Resolve the generated functions through their C symbol names, the
	// same way a C consumer (or oak-app's define_handle!) would: over
	// opaque void pointers, not Rust types.
	unsafe extern "C" {
		#[link_name = "oak_vec_i32_init"]
		fn i32_init() -> *mut std::ffi::c_void;
		#[link_name = "oak_vec_i32_len"]
		fn i32_len(this: *const std::ffi::c_void) -> usize;
		#[link_name = "oak_vec_i32_push"]
		fn i32_push(this: *mut std::ffi::c_void, value: i32);
		#[link_name = "oak_vec_i32_get"]
		fn i32_get(this: *const std::ffi::c_void, index: usize, out: *mut i32) -> bool;
		#[link_name = "oak_vec_i32_set"]
		fn i32_set(this: *mut std::ffi::c_void, index: usize, value: i32) -> bool;
		#[link_name = "oak_vec_i32_clear"]
		fn i32_clear(this: *mut std::ffi::c_void);
		#[link_name = "oak_vec_i32_add_ref"]
		unsafe fn i32_add_ref(this: *const std::ffi::c_void);
		#[link_name = "oak_vec_i32_release"]
		unsafe fn i32_release(this: *const std::ffi::c_void);

		#[link_name = "oak_vec_string_init"]
		fn string_init() -> *mut std::ffi::c_void;
		#[link_name = "oak_vec_string_len"]
		fn string_len(this: *const std::ffi::c_void) -> usize;
		#[link_name = "oak_vec_string_push"]
		fn string_push(this: *mut std::ffi::c_void, value: *const u8, length: usize) -> bool;
		#[link_name = "oak_vec_string_get"]
		fn string_get(this: *const std::ffi::c_void, index: usize, out_len: *mut usize) -> *const u8;
		#[link_name = "oak_vec_string_release"]
		unsafe fn string_release(this: *const std::ffi::c_void);
	}

	#[test]
	fn scalar_vec_roundtrip_via_c_symbols() {
		unsafe {
			let v = i32_init();
			assert!(!v.is_null());
			assert_eq!(i32_len(v), 0);

			i32_push(v, 10);
			i32_push(v, 20);
			i32_push(v, 30);
			assert_eq!(i32_len(v), 3);

			let mut out = 0i32;
			assert!(i32_get(v, 1, &mut out));
			assert_eq!(out, 20);

			assert!(i32_set(v, 1, 99));
			assert!(i32_get(v, 1, &mut out));
			assert_eq!(out, 99);

			// Out-of-bounds reads/writes fail instead of panicking.
			assert!(!i32_get(v, 3, &mut out));
			assert!(!i32_set(v, 3, 0));

			i32_clear(v);
			assert_eq!(i32_len(v), 0);

			i32_release(v);
		}
	}

	#[test]
	fn scalar_vec_null_is_safe() {
		unsafe {
			assert_eq!(i32_len(std::ptr::null()), 0);
			i32_push(std::ptr::null_mut(), 1);
			assert!(!i32_get(std::ptr::null(), 0, &mut 0i32));
			assert!(!i32_set(std::ptr::null_mut(), 0, 1));
			i32_clear(std::ptr::null_mut());
			i32_add_ref(std::ptr::null());
			i32_release(std::ptr::null());
		}
	}

	#[test]
	fn refcount_keeps_vec_alive() {
		unsafe {
			let v = i32_init();
			i32_add_ref(v);
			i32_push(v, 7);
			i32_release(v);
			// One reference remains: still fully usable.
			let mut out = 0i32;
			assert!(i32_get(v, 0, &mut out));
			assert_eq!(out, 7);
			i32_release(v);
		}
	}

	#[test]
	fn vec_converts_into_wrapper_via_trait() {
		use crate::vecs::IntoFfiVec;
		unsafe {
			let v = vec![4, 8, 15].into_ffi_vec() as *mut std::ffi::c_void;
			assert_eq!(i32_len(v), 3);
			let mut out = 0i32;
			assert!(i32_get(v, 2, &mut out));
			assert_eq!(out, 15);
			i32_release(v);
		}
	}

	#[test]
	fn string_vec_roundtrip_via_c_symbols() {
		unsafe {
			let v = string_init();
			assert_eq!(string_len(v), 0);

			assert!(string_push(v, "hello".as_ptr(), 5));
			assert!(string_push(v, "世界".as_ptr(), "世界".len()));
			// Empty string from a null pointer is valid.
			assert!(string_push(v, std::ptr::null(), 0));
			assert_eq!(string_len(v), 3);

			let mut len = usize::MAX;
			let p = string_get(v, 1, &mut len);
			assert!(!p.is_null());
			assert_eq!(len, "世界".len());
			assert_eq!(std::slice::from_raw_parts(p, len), "世界".as_bytes());

			// Out-of-bounds: null pointer, out_len untouched.
			len = 12345;
			assert!(string_get(v, 3, &mut len).is_null());

			string_release(v);
		}
	}

	#[test]
	fn string_push_rejects_invalid_utf8_and_null_data() {
		unsafe {
			let v = string_init();
			let bad = [0xffu8, 0xfe];
			assert!(!string_push(v, bad.as_ptr(), bad.len()));
			assert!(!string_push(v, std::ptr::null(), 5));
			assert_eq!(string_len(v), 0);
			string_release(v);

			// Null vector: push fails, get yields null, len reads 0.
			assert_eq!(string_len(std::ptr::null()), 0);
			assert!(!string_push(std::ptr::null_mut(), "x".as_ptr(), 1));
			assert!(string_get(std::ptr::null(), 0, &mut 0usize).is_null());
			string_release(std::ptr::null());
		}
	}
}
