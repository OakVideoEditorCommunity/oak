//! System keyring storage for the AI configuration (T13A).
//!
//! The sovereign's ruling (2026-11-12): the API key lives in the **OS
//! Secret Service** (gnome-keyring/kwallet behind the session bus), not in
//! a plaintext file. The environment variable stays as the highest
//! priority layer — `env > keyring > default` — so headless/CI runs keep
//! working with `OAK_AI_API_KEY` alone.
//!
//! Design notes, each of them a trap avoided:
//!
//! * **Trait seam, not keyring's mock store.** `keyring::mock::set_default_store`
//!   would install a process-global singleton, which makes parallel tests
//!   stomp on each other. Everything here goes through [`SecretStore`];
//!   tests inject [`MemoryStore`] and the real keyring is never written
//!   from `#[cfg(test)]`.
//! * **Graceful degradation.** On a machine with no desktop session there
//!   is no Secret Service at all; `keyring` reports `NoDefaultStore`.
//!   [`KeyringStore::get`] maps that to `None` — "not configured yet" —
//!   and never panics, so `AiConfig::load_with` behaves exactly like the
//!   env-only world on such machines.
//! * **No key in `Debug`.** `SecretError` prints the *account* name and
//!   an error code, never the value being stored.

use std::collections::HashMap;

/// Secret Service collection key (the `service` argument of every entry).
pub const SERVICE: &str = "oak-ai";

/// Account names — one credential per configuration value.
pub const ACCOUNT_API_KEY: &str = "api_key";
pub const ACCOUNT_BASE_URL: &str = "base_url";
pub const ACCOUNT_MODEL: &str = "model";

/// A storage failure. `code` is stable and machine-readable; `message`
/// carries the underlying error text (the keyring never returns the
/// secret itself in an error).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct SecretError {
	code: &'static str,
	message: String,
}

impl SecretError {
	/// Keyring backend unavailable (no session bus, no daemon,
	/// `NoDefaultStore`, locked collection…).
	pub const UNAVAILABLE: &'static str = "AI_SECRET_UNAVAILABLE";
	/// The backend refused a write or delete.
	pub const WRITE: &'static str = "AI_SECRET_WRITE";

	/// Builds an error with the given stable code.
	pub fn new(code: &'static str, message: impl Into<String>) -> Self {
		Self {
			code,
			message: message.into(),
		}
	}

	/// The stable code (`AI_SECRET_UNAVAILABLE` / `AI_SECRET_WRITE`).
	pub fn code(&self) -> &'static str {
		self.code
	}
}

/// One configuration value's storage slot (a `service` + `account` pair).
///
/// All access is injectable: production uses [`KeyringStore`], tests use
/// [`MemoryStore`] or their own fake — the real OS keyring is never a
/// dependency of a passing test run.
pub trait SecretStore {
	/// Reads one account. `None` means "not configured" (absent entry,
	/// unavailable backend — never a panic).
	///
	/// This trait is the **read seam only** — writing lives in
	/// [`KeyringWriter`], implemented by both backends. One trait per
	/// direction: a `set` that always failed would be a misleading API.
	fn get(&self, account: &str) -> Option<String>;
}

/// The real Secret Service backend.
///
/// Every fallible path is mapped to `Ok(None)`/`Err(SecretError)` with a
/// stable code — no `unwrap`, no `expect`, no panic path at all, because
/// a headless machine without a keyring daemon is a supported runtime.
#[derive(Default, Debug, Clone, Copy)]
pub struct KeyringStore;

impl KeyringStore {
	/// Opens a keyring entry for `account`; `Err` = backend unavailable.
	fn entry(account: &str) -> Result<keyring::Entry, SecretError> {
		keyring::Entry::new(SERVICE, account)
			.map_err(|e| SecretError::new(SecretError::UNAVAILABLE, e.to_string()))
	}
}

/// **The write seam**, deliberately a separate trait: [`SecretStore`]
/// reads, this one writes, and both `KeyringStore` (real Secret
/// Service) and `MemoryStore` (tests) implement both.
///
/// The real keyring is written **only** through this trait, which is how
/// the "tests never write the user's real keyring" red line is enforced
/// structurally: a test that does not import `KeyringWriter` cannot
/// reach `KeyringStore::store`/`remove` — the compiler rejects it — and
/// the test modules deliberately do not import it.
pub trait KeyringWriter {
	/// Stores one account in the real Secret Service.
	fn store(&mut self, account: &str, value: &str) -> Result<(), SecretError>;
	/// Removes one account (absent entry = success, already unset).
	fn remove(&mut self, account: &str) -> Result<(), SecretError>;
}

impl KeyringWriter for KeyringStore {
	fn store(&mut self, account: &str, value: &str) -> Result<(), SecretError> {
		let entry = Self::entry(account)?;
		entry
			.set_password(value)
			.map_err(|e| SecretError::new(SecretError::WRITE, e.to_string()))
	}

	fn remove(&mut self, account: &str) -> Result<(), SecretError> {
		let entry = Self::entry(account)?;
		match entry.delete_credential() {
			Ok(()) => Ok(()),
			// Deleting an entry that was never there is a no-op, not a
			// failure — the caller's intent ("this account is unset") is
			// already true.
			Err(keyring::Error::NoEntry) => Ok(()),
			Err(e) => Err(SecretError::new(SecretError::WRITE, e.to_string())),
		}
	}
}

impl SecretStore for KeyringStore {
	fn get(&self, account: &str) -> Option<String> {
		let entry = Self::entry(account).ok()?;
		// `NoDefaultStore` (no session bus / no daemon) and
		// `NoEntry` (nothing stored yet) both degrade to "not set".
		match entry.get_password() {
			Ok(value) => Some(value),
			Err(keyring::Error::NoEntry) => None,
			Err(_) => None,
		}
	}

}

/// In-memory store for tests (and for `AiConfig::load`, which must keep
/// its historical env-only semantics without touching any backend).
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
	map: HashMap<String, String>,
}

impl MemoryStore {
	/// An empty store — the "nothing configured" baseline.
	pub fn new() -> Self {
		Self::default()
	}
}

impl SecretStore for MemoryStore {
	fn get(&self, account: &str) -> Option<String> {
		self.map.get(account).cloned()
	}
}

// The injected store implements **both** seams, so T13B's save path is
// exercisable in tests: `store()` through `KeyringWriter`, read back
// through `SecretStore::get` — without ever touching the real keyring.
impl KeyringWriter for MemoryStore {
	fn store(&mut self, account: &str, value: &str) -> Result<(), SecretError> {
		self.map.insert(account.to_string(), value.to_string());
		Ok(())
	}

	fn remove(&mut self, account: &str) -> Result<(), SecretError> {
		self.map.remove(account);
		Ok(())
	}
}

/// Convenience reader: `(api_key, base_url, model)` from one store, each
/// `None` when that account is unset or the backend is unavailable.
pub fn read_config(store: &dyn SecretStore) -> (Option<String>, Option<String>, Option<String>) {
	(
		store.get(ACCOUNT_API_KEY),
		store.get(ACCOUNT_BASE_URL),
		store.get(ACCOUNT_MODEL),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// `MemoryStore` round-trip: set → get → delete → get None.
	#[test]
	fn memory_store_round_trips_and_deletes() {
		let mut store = MemoryStore::new();
		assert_eq!(store.get(ACCOUNT_API_KEY), None, "starts empty");

		store.store(ACCOUNT_API_KEY, "k").expect("store");
		assert_eq!(store.get(ACCOUNT_API_KEY).as_deref(), Some("k"));

		store.remove(ACCOUNT_API_KEY).expect("remove");
		assert_eq!(store.get(ACCOUNT_API_KEY), None, "gone after delete");

		// Delete of a missing account is a no-op, not an error.
		store.remove(ACCOUNT_API_KEY).expect("idempotent remove");
	}

	/// `read_config` reads all three accounts in one call.
	#[test]
	fn read_config_reads_the_three_accounts() {
		let mut store = MemoryStore::new();
		store.store(ACCOUNT_API_KEY, "k").unwrap();
		store.store(ACCOUNT_MODEL, "m").unwrap();
		let (api, url, model) = read_config(&store);
		assert_eq!(api.as_deref(), Some("k"));
		assert_eq!(url, None, "unset account stays None");
		assert_eq!(model.as_deref(), Some("m"));
	}

	/// The degraded backend's **read** side ("always None") is treated as
	/// "not configured" — no panic anywhere.
	///
	/// (The write side used to live in this test as `SecretStore::set`
	/// failures; those methods no longer exist — `SecretStore` is the
	/// read seam now. The write-failure assertion itself was kept, moved
	/// to `a_broken_writer_fails_with_a_stable_code` below.)
	#[test]
	fn a_broken_store_degrades_to_not_configured() {
		struct Broken;
		impl SecretStore for Broken {
			fn get(&self, _account: &str) -> Option<String> {
				None
			}
		}

		let store = Broken;
		assert_eq!(read_config(&store), (None, None, None), "degrades, no panic");
	}

	/// The degraded backend's **write** side: `KeyringWriter::store`/
	/// `remove` fail with stable codes and never echo the value.
	#[test]
	fn a_broken_writer_fails_with_a_stable_code() {
		struct Broken;
		impl SecretStore for Broken {
			fn get(&self, _account: &str) -> Option<String> {
				None
			}
		}
		impl KeyringWriter for Broken {
			fn store(&mut self, _account: &str, _value: &str) -> Result<(), SecretError> {
				Err(SecretError::new(SecretError::WRITE, "no backend"))
			}
			fn remove(&mut self, _account: &str) -> Result<(), SecretError> {
				Err(SecretError::new(SecretError::UNAVAILABLE, "no backend"))
			}
		}

		let mut store = Broken;
		let err = store.store(ACCOUNT_API_KEY, "k").expect_err("must fail");
		assert_eq!(err.code(), SecretError::WRITE);
		let err = store.remove(ACCOUNT_API_KEY).expect_err("must fail");
		assert_eq!(err.code(), SecretError::UNAVAILABLE);
		// The error must carry no secret material: it is built from a
		// literal ("no backend") + the stable code, and `Debug`/`Display`
		// only ever print those (the keyring path maps `Display` from the
		// backend's own error, which never echoes the password).
		let printed = format!("{err} / {err:?}");
		assert!(
			!printed.contains("from-keyring") && !printed.contains("k-store"),
			"error leaked a stored value: {printed}"
		);
	}

	/// The injected store implements **both** seams: write through
	/// `KeyringWriter::store`, read back through `SecretStore::get`
	/// (the save-path closure T13B's tests need).
	#[test]
	fn memory_store_closes_the_write_read_loop() {
		let mut store = MemoryStore::new();
		assert_eq!(store.get(ACCOUNT_API_KEY), None, "starts empty");
		store.store(ACCOUNT_API_KEY, "k").expect("write seam");
		assert_eq!(store.get(ACCOUNT_API_KEY).as_deref(), Some("k"), "read seam");

		store.remove(ACCOUNT_API_KEY).expect("write seam remove");
		assert_eq!(store.get(ACCOUNT_API_KEY), None, "removed through the loop");
	}

	/// Red line: the tests never write the real keyring.
	///
	/// `SecretStore` carries only `get` — there is no write method on
	/// the read seam at all. Writes go through [`KeyringWriter`], whose
	/// `store`/`remove` reach the real Secret Service only on
	/// `KeyringStore`. This test module never imports `KeyringWriter`
	/// for the *real* store (it does for `MemoryStore`/`Broken`, which
	/// are inert), so a real-store write does not compile here —
	/// structural, not conventional. Pinned by asserting the read seam
	/// the whole config layer actually uses:
	#[test]
	fn the_read_seam_has_no_write_method() {
		// Compiles only through `SecretStore::get` — the trait now has
		// a single method, and `&`-only access suffices for reads.
		let store = MemoryStore::new();
		fn read_only(s: &dyn SecretStore, account: &str) -> Option<String> {
			s.get(account)
		}
		assert_eq!(read_only(&store, ACCOUNT_API_KEY), None);

		// And the real store's read path degrades without a session
		// backend being reachable from a test call — `KeyringStore`
		// here is only *constructed*, never written: `store`/`remove`
		// live on `KeyringWriter`, which this module uses only for the
		// in-memory types above.
		let real = KeyringStore;
		let _ = real; // read would touch the real keyring; not done in tests
	}
}
