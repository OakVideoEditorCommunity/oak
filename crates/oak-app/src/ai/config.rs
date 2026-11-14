//! Environment-driven AI configuration (T8.1).
//!
//! Three variables, one rule: the API key only ever lives in this struct's
//! memory. It is never written to disk, never logged, never formatted into
//! a `Debug`/`Display`, and never leaves the process except as the
//! `Authorization` header of the one HTTP request that needs it.

use std::fmt;

/// Default endpoint — the xiaomi mimo openai-completions surface that was
/// verified end-to-end from this machine (see `ai/agent/` for the pi-side
/// precedent).
pub const DEFAULT_BASE_URL: &str = "https://token-plan-cn.xiaomimimo.com/v1";

/// Default model on that endpoint.
pub const DEFAULT_MODEL: &str = "mimo-v2.6-flash";

/// The three environment variables this module understands.
pub const ENV_API_KEY: &str = "OAK_AI_API_KEY";
pub const ENV_BASE_URL: &str = "OAK_AI_BASE_URL";
pub const ENV_MODEL: &str = "OAK_AI_MODEL";

/// Missing/empty configuration. Structured on purpose: a missing key is a
/// normal "not set up yet" state, never a panic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
	/// `OAK_AI_API_KEY` is unset or blank.
	#[error("AI backend is not configured: set OAK_AI_API_KEY to use the agent")]
	MissingApiKey,
}

impl ConfigError {
	/// Stable machine-readable code (UI/telemetry may key off this).
	pub fn code(&self) -> &'static str {
		match self {
			Self::MissingApiKey => "AI_NOT_CONFIGURED",
		}
	}
}

/// Loaded configuration.
///
/// `Debug` is hand-written so an accidental `{:?}` in a log line cannot
/// leak the key.
pub struct AiConfig {
	/// Bearer token; see the redaction note in the module docs.
	pub api_key: String,
	pub base_url: String,
	pub model: String,
}

impl fmt::Debug for AiConfig {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("AiConfig")
			.field("api_key", &"[REDACTED]")
			.field("base_url", &self.base_url)
			.field("model", &self.model)
			.finish()
	}
}

impl AiConfig {
	/// Reads the three environment variables.
	pub fn from_env() -> Result<Self, ConfigError> {
		Self::load(|name| std::env::var(name).ok())
	}

	/// Same as [`from_env`](Self::from_env) but with an injectable
	/// getter — tests exercise the missing-key path without touching the
	/// process environment (which is shared across parallel tests).
	///
	/// Historical env-only semantics, unchanged: no keyring layer (an
	/// empty [`SecretStore`](crate::ai::secrets::SecretStore) is passed
	/// down), so every existing caller and test behaves exactly as before.
	pub fn load(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
		Self::load_with(get, &crate::ai::secrets::MemoryStore::new())
	}

	/// Layered read: **env > keyring > default**.
	///
	/// `env_get` wins for every value it provides (headless runs keep
	/// working with `OAK_AI_API_KEY` alone); the keyring store fills the
	/// gaps; `base_url`/`model` fall back to their defaults; a missing
	/// `api_key` in both layers is [`ConfigError::MissingApiKey`].
	/// A degraded store (no desktop/keyring) simply reads as "unset" —
	/// this function never panics on its account.
	pub fn load_with(
		env_get: impl Fn(&str) -> Option<String>,
		store: &dyn crate::ai::secrets::SecretStore,
	) -> Result<Self, ConfigError> {
		let pick = |env_name: &str, account: &str| -> Option<String> {
			let from_env = env_get(env_name)
				.map(|v| v.trim().to_string())
				.filter(|v| !v.is_empty());
			from_env.or_else(|| {
				store
					.get(account)
					.map(|v| v.trim().to_string())
					.filter(|v| !v.is_empty())
			})
		};
		let api_key = pick(ENV_API_KEY, crate::ai::secrets::ACCOUNT_API_KEY)
			.ok_or(ConfigError::MissingApiKey)?;
		let base_url = pick(ENV_BASE_URL, crate::ai::secrets::ACCOUNT_BASE_URL)
			.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
		let model = pick(ENV_MODEL, crate::ai::secrets::ACCOUNT_MODEL)
			.unwrap_or_else(|| DEFAULT_MODEL.to_string());
		Ok(Self {
			api_key,
			base_url,
			model,
		})
	}

	/// Production entry point: env first, then the system keyring
	/// (`service = "oak-ai"`). On a machine with no Secret Service the
	/// keyring layer reads as unset; the env-only contract still holds.
	pub fn from_env_and_keyring() -> Result<Self, ConfigError> {
		Self::load_with(|n| std::env::var(n).ok(), &crate::ai::secrets::KeyringStore)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	// The write methods used below come from `KeyringWriter` (the store
	// seam for injected `MemoryStore`s), not from `SecretStore` (reads).
	use crate::ai::secrets::KeyringWriter as _;

	/// A missing key is a structured error, not a panic — and the error
	/// itself says how to fix it.
	#[test]
	fn missing_key_is_a_structured_error() {
		let err = AiConfig::load(|_| None).expect_err("no key must not load");
		assert_eq!(err, ConfigError::MissingApiKey);
		assert_eq!(err.code(), "AI_NOT_CONFIGURED");
		assert!(err.to_string().contains(ENV_API_KEY), "{err}");
	}

	/// Blank keys count as missing; defaults fill in for url/model.
	#[test]
	fn blank_key_is_missing_and_defaults_apply() {
		let err = AiConfig::load(|name| {
			(name == ENV_API_KEY).then(|| "   ".to_string())
		})
		.expect_err("whitespace key must not load");
		assert_eq!(err, ConfigError::MissingApiKey);

		let cfg = AiConfig::load(|name| match name {
			ENV_API_KEY => Some("k".into()),
			_ => None,
		})
		.expect("key present");
		assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
		assert_eq!(cfg.model, DEFAULT_MODEL);
	}

	/// Priority: env > store. An env value wins even when the store
	/// holds a different one.
	#[test]
	fn env_wins_over_the_store() {
		let mut store = crate::ai::secrets::MemoryStore::new();
		store
			.store(crate::ai::secrets::ACCOUNT_API_KEY, "from-keyring")
			.unwrap();
		let cfg = AiConfig::load_with(
			|name| (name == ENV_API_KEY).then(|| "from-env".to_string()),
			&store,
		)
		.expect("env key present");
		assert_eq!(cfg.api_key, "from-env", "env outranks the store");
	}

	/// No env → the store fills in; blank env also falls through to it.
	#[test]
	fn store_fills_in_when_env_is_absent_or_blank() {
		let mut store = crate::ai::secrets::MemoryStore::new();
		store
			.store(crate::ai::secrets::ACCOUNT_API_KEY, "k-store")
			.unwrap();
		store
			.store(crate::ai::secrets::ACCOUNT_BASE_URL, "https://u")
			.unwrap();

		let cfg = AiConfig::load_with(|_| None, &store).expect("store key");
		assert_eq!(cfg.api_key, "k-store");
		assert_eq!(cfg.base_url, "https://u");
		assert_eq!(cfg.model, DEFAULT_MODEL, "unset store account → default");

		// Blank env value must not shadow the store.
		let cfg = AiConfig::load_with(
			|name| (name == ENV_API_KEY).then(|| "   ".to_string()),
			&store,
		)
		.expect("blank env falls through");
		assert_eq!(cfg.api_key, "k-store");
	}

	/// Neither layer has the key → structured `MissingApiKey`, no panic.
	#[test]
	fn missing_in_env_and_store_is_missing_api_key() {
		let store = crate::ai::secrets::MemoryStore::new();
		let err = AiConfig::load_with(|_| None, &store).expect_err("no key");
		assert_eq!(err, ConfigError::MissingApiKey);
		assert_eq!(err.code(), "AI_NOT_CONFIGURED");
	}

	/// A degraded store (the real keyring on a headless box) must read
	/// as "not configured" without any panic — env still works.
	#[test]
	fn a_degraded_store_never_panics_and_env_still_works() {
		// Read seam only (`SecretStore::get`): a headless box's keyring
		// reads as "not set"; there is no write method to fail here —
		// writes are `KeyringWriter`'s business, not this layer's.
		struct Unavailable;
		impl crate::ai::secrets::SecretStore for Unavailable {
			fn get(&self, _account: &str) -> Option<String> {
				None
			}
		}

		// Store dead, env empty → the normal "not set up yet" error.
		let err = AiConfig::load_with(|_| None, &Unavailable).expect_err("unset");
		assert_eq!(err, ConfigError::MissingApiKey, "degrades to unconfigured");

		// Store dead, env set → config loads (env outranks everything).
		let cfg = AiConfig::load_with(|n| (n == ENV_API_KEY).then(|| "k".into()), &Unavailable)
			.expect("env alone suffices");
		assert_eq!(cfg.api_key, "k");
	}

	/// The legacy `load` path is exactly `load_with` over an empty store:
	/// its historical env-only behaviour is preserved bit for bit.
	#[test]
	fn load_keeps_env_only_semantics() {
		let empty = crate::ai::secrets::MemoryStore::new();
		let via_load = AiConfig::load(|n| (n == ENV_API_KEY).then(|| "k".to_string()))
			.expect("legacy path");
		let via_with =
			AiConfig::load_with(|n| (n == ENV_API_KEY).then(|| "k".to_string()), &empty)
				.expect("layered path");
		assert_eq!(via_load.api_key, via_with.api_key);
		assert_eq!(via_load.base_url, via_with.base_url);
		assert_eq!(via_load.model, via_with.model);

		let err = AiConfig::load(|_| None).expect_err("no key");
		assert_eq!(err, ConfigError::MissingApiKey);
	}

	/// The key must not survive a `Debug` round-trip.
	#[test]
	fn debug_redacts_the_key() {
		let cfg = AiConfig::load(|name| {
			(name == ENV_API_KEY).then(|| "super-secret-key".to_string())
		})
		.unwrap();
		let printed = format!("{cfg:?}");
		assert!(!printed.contains("super-secret-key"), "leaked: {printed}");
		assert!(printed.contains("[REDACTED]"), "{printed}");
	}
}
