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

//! # oakcodec — the media codec module (Rust)
//!
//! Reimplements the C++ oakcodec module behind its frozen C ABI
//! (`include/codec/*.h`). See README.md for the architectural mapping
//! (inheritance → traits, shared_ptr → `Arc`, etc.).
//!
//! ## Structure
//!
//! Single-lib unification: the module crates are called directly from
//! other module crates and the oakengine facade (`crates/oakengine`), so
//! no C-ABI export layer remains here (the `include/codec/*.h` contracts
//! are served by the facade). Shared state lives behind `Mutex`, decoder
//! instances behind `Arc<dyn Decoder>`.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

pub mod audioparams;
pub mod conformmanager;
pub mod decoder;
pub mod encoder;
pub mod encodingparams;
pub mod error;
pub mod exportcodec;
pub mod exportformat;
pub mod ffmpeg;
pub mod footagedescription;
pub mod frame;
pub mod framemanager;
pub mod gpuinterop;
pub mod hwdecode;
pub mod oiio;
pub mod oiioframebridge;
pub mod planarfiledevice;
pub mod proxymanager;
pub mod task;
pub mod testmedia;
pub mod timecodemetadata;

#[cfg(test)]
mod realmedia_tests;

/// Process-wide test lock: serializes every test that reads or mutates
/// crate-global state (the injected decoder/encoder registries). One lock
/// for the whole crate — tests race only with each other, never with
/// production code.
#[cfg(test)]
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard over [`TEST_LOCK`]: the lock is taken when the guard is
/// created ([`TestLock::acquire`]) and released when it is dropped —
/// including through panics, so a failing test can never deadlock the
/// tests that follow. Poison-tolerant: a panicking holder does not leave
/// the mutex poisoned for the next acquirer.
#[cfg(test)]
pub struct TestLock {
	/// The held lock guard; dropping it releases [`TEST_LOCK`] (never read,
	/// only dropped — the whole point of the RAII guard).
	#[allow(dead_code)]
	guard: MutexGuard<'static, ()>,
}

#[cfg(test)]
impl TestLock {
	/// Acquire exclusive access to the crate's shared test state, blocking
	/// until every earlier holder has released it.
	pub fn acquire() -> TestLock {
		TestLock {
			guard: TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner()),
		}
	}
}

#[cfg(test)]
impl Drop for TestLock {
	fn drop(&mut self) {
		// Dropping the held guard releases TEST_LOCK; the explicit Drop
		// documents the acquire-on-create / release-on-drop contract.
	}
}

/// Acquire the process-wide test lock (see [`TestLock::acquire`]).
#[cfg(test)]
pub(crate) fn lock_tests() -> TestLock {
	TestLock::acquire()
}

// Keep the oakffmpeg-link rlib referenced so its build script's native
// link flags (the static FFmpeg's transitive dependencies) reach the
// final link — rustc prunes the flags of an unreferenced rlib.
#[used]
static FORCE_FFMPEG_LINK: fn() = oak_ffmpeg_link::force_link;
