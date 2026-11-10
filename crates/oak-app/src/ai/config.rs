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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
	/// `OAK_AI_API_KEY` is unset or blank.
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

impl fmt::Display for ConfigError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::MissingApiKey => write!(
				f,
				"AI backend is not configured: set {} to use the agent",
				ENV_API_KEY
			),
		}
	}
}

impl std::error::Error for ConfigError {}

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
	pub fn load(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
		let api_key = get(ENV_API_KEY)
			.map(|v| v.trim().to_string())
			.filter(|v| !v.is_empty())
			.ok_or(ConfigError::MissingApiKey)?;
		let base_url = get(ENV_BASE_URL)
			.map(|v| v.trim().to_string())
			.filter(|v| !v.is_empty())
			.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
		let model = get(ENV_MODEL)
			.map(|v| v.trim().to_string())
			.filter(|v| !v.is_empty())
			.unwrap_or_else(|| DEFAULT_MODEL.to_string());
		Ok(Self {
			api_key,
			base_url,
			model,
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
