//! Backend selection: `--backend mock` (default) / `--backend sdk`.
//!
//! The MCP layer never chooses a backend implicitly. `mock` is the
//! documented default because it needs no engine; `sdk` is only available
//! when the binary was built with `--features sdk`, and if it is not
//! available the server **fails to start** instead of silently running
//! the mock backend — a client that asked for the real editor must never
//! receive synthetic edits (plan section 5 iron rules, same stance as
//! `oak_ai_tools::ToolError::unsupported`).

use oak_ai_tools::backend::EngineBackend;
use oak_ai_tools::mock::MockBackend;

/// Which backend a session runs against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
	/// In-memory deterministic timeline (no engine, no FFmpeg).
	Mock,
	/// The real engine, in-process, through `oak-ai-tools` feature `sdk`.
	Sdk,
}

impl BackendKind {
	/// The CLI value accepted for this backend.
	pub fn label(self) -> &'static str {
		match self {
			Self::Mock => "mock",
			Self::Sdk => "sdk",
		}
	}

	/// Parses a `--backend` value.
	pub fn parse(value: &str) -> Result<Self, String> {
		match value {
			"mock" => Ok(Self::Mock),
			"sdk" => Ok(Self::Sdk),
			other => Err(format!(
				"unknown backend '{other}': expected 'mock' or 'sdk'"
			)),
		}
	}
}

/// Builds the backend, or explains why it cannot be built.
pub fn build(kind: BackendKind) -> Result<Box<dyn EngineBackend>, String> {
	match kind {
		BackendKind::Mock => Ok(Box::new(MockBackend::new())),
		BackendKind::Sdk => sdk_backend(),
	}
}

/// The `sdk` backend is only compiled in with `--features sdk`; the
/// engine dependency (`oak-engine-rust-sdk`, and through it the project
/// FFmpeg) is far too heavy to be a default build requirement.
#[cfg(not(feature = "sdk"))]
fn sdk_backend() -> Result<Box<dyn EngineBackend>, String> {
	Err("--backend sdk is not available: this binary was built without the 'sdk' \
	     feature. Rebuild with `cargo build --release --features sdk` (it links \
	     oak-engine-rust-sdk and the project FFmpeg/engine). Refusing to fall back \
	     to the mock backend; pass --backend mock to run against the in-memory mock."
		.to_string())
}

/// With the feature on, `--backend sdk` constructs the real engine
/// backend. `SdkBackend::new()` takes no arguments and only builds empty
/// state (the project is opened later through the `project.open` tool), so
/// the server starts and answers MCP requests before any engine handle is
/// touched.
///
/// Compile verification of this branch needs the engine (ocio) build; the
/// `cargo check --features sdk` record lives in README "已知限制".
#[cfg(feature = "sdk")]
fn sdk_backend() -> Result<Box<dyn EngineBackend>, String> {
	Ok(Box::new(oak_ai_tools::sdk::SdkBackend::new()))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_the_two_documented_values() {
		assert_eq!(BackendKind::parse("mock").unwrap(), BackendKind::Mock);
		assert_eq!(BackendKind::parse("sdk").unwrap(), BackendKind::Sdk);
		assert!(BackendKind::parse("engine").is_err());
		assert!(BackendKind::parse("").is_err());
	}

	#[test]
	fn mock_backend_builds_without_an_engine() {
		assert!(build(BackendKind::Mock).is_ok());
	}

	#[test]
	fn sdk_gating_follows_the_build_features() {
		#[cfg(not(feature = "sdk"))]
		{
			let message = build(BackendKind::Sdk)
				.err()
				.expect("sdk must be refused without the feature");
			assert!(message.contains("--features sdk"), "{message}");
			assert!(
				message.contains("Refusing to fall back"),
				"refusal must say so explicitly: {message}"
			);
		}
		#[cfg(feature = "sdk")]
		{
			// The feature is on: the backend must build, and the mock must
			// stay selectable in the same binary.
			let backend = build(BackendKind::Sdk).expect("sdk backend must be wired");
			assert_eq!(BackendKind::Sdk.label(), "sdk");
			drop(backend);
			assert!(build(BackendKind::Mock).is_ok());
		}
	}
}
