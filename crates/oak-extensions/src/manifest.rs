//! `plugin.toml` manifest: parse, validate, resolve argv/env.
//!
//! Field table follows the frozen design (`external-plugin-system.md`
//! §2.2 的 manifest 段) as recorded by task T7.1; the frozen spec lives at
//! `~/.oak-reviews/opp-spec.md` (persistent — the `/tmp` copy may be wiped),
//! so defaults are pinned here:
//! `restart.max = 5`, `restart.backoff_ms = 1000`, `env_passthrough =
//! ["PATH","HOME"]`, `api = 1`.

use crate::protocol::{PanelSpec, API_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::path::Path;

/// The frozen v1 capability set (protocol §7.1).
pub const KNOWN_CAPABILITIES: &[&str] = &[
	"project.read",
	"project.edit",
	"media.read",
	"media.import",
	"timeline.read",
	"timeline.edit",
	"node.read",
	"node.edit",
	"render.frame",
	"playback",
	"export",
	"ui.panel",
	"ui.pixel",
];

/// Default environment whitelist: never inherit the full environment (§2.1 —
/// 防泄密钥).
pub const DEFAULT_ENV_PASSTHROUGH: &[&str] = &["PATH", "HOME"];
/// Aligned with `procpool.rs` `MAX_RESTARTS: u32 = 5`.
pub const DEFAULT_MAX_RESTARTS: u32 = 5;
pub const DEFAULT_BACKOFF_MS: u64 = 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
	/// Reverse-domain id, globally unique; also the panel-id prefix.
	pub id: String,
	/// User-visible name (plugin manager / history labels).
	pub name: String,
	/// semver, display only.
	pub version: String,
	/// Protocol main version; v1 = `1`.
	pub api: u32,
	pub process: ProcessSpec,
	/// Must be a subset of [`KNOWN_CAPABILITIES`]; shown at install time.
	#[serde(default)]
	pub capabilities: Vec<String>,
	/// Must equal `session.hello.panels` — the host rejects a mismatch.
	#[serde(default)]
	pub panels: Vec<PanelSpec>,
	/// Initial subscriptions (`session.hello.subscribe`).
	#[serde(default)]
	pub subscribe: Vec<String>,
	#[serde(default)]
	pub restart: RestartSpec,
	/// Overrides of the protocol §12 limits (host may trim them).
	#[serde(default)]
	pub limits: LimitsSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessSpec {
	/// argv array; `{plugin_dir}` is substituted by the host.
	pub command: Vec<String>,
	#[serde(default = "default_env_passthrough")]
	pub env_passthrough: Vec<String>,
}

fn default_env_passthrough() -> Vec<String> {
	DEFAULT_ENV_PASSTHROUGH.iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestartSpec {
	/// Crash restarts before giving up (`0` = never restart).
	#[serde(default = "default_max_restarts")]
	pub max: u32,
	/// Exponential backoff base: 1s → 2s → 4s → 8s → 16s.
	#[serde(default = "default_backoff_ms")]
	pub backoff_ms: u64,
}

fn default_max_restarts() -> u32 {
	DEFAULT_MAX_RESTARTS
}
fn default_backoff_ms() -> u64 {
	DEFAULT_BACKOFF_MS
}

impl Default for RestartSpec {
	fn default() -> Self {
		Self {
			max: DEFAULT_MAX_RESTARTS,
			backoff_ms: DEFAULT_BACKOFF_MS,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LimitsSpec {
	#[serde(default)]
	pub max_message_bytes: Option<usize>,
	#[serde(default)]
	pub request_timeout_s: Option<u64>,
	#[serde(default)]
	pub slots: Option<u32>,
	#[serde(default)]
	pub slot_bytes: Option<u64>,
}

#[derive(Debug)]
pub enum ManifestError {
	Parse(String),
	Invalid(String),
}

impl fmt::Display for ManifestError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			ManifestError::Parse(e) => write!(f, "plugin.toml parse error: {e}"),
			ManifestError::Invalid(e) => write!(f, "plugin.toml invalid: {e}"),
		}
	}
}

impl std::error::Error for ManifestError {}

impl Manifest {
	pub fn from_toml(text: &str) -> Result<Manifest, ManifestError> {
		let m: Manifest = toml::from_str(text).map_err(|e| ManifestError::Parse(e.to_string()))?;
		m.validate()?;
		Ok(m)
	}

	pub fn load(path: impl AsRef<Path>) -> Result<Manifest, ManifestError> {
		let text = std::fs::read_to_string(path.as_ref())
			.map_err(|e| ManifestError::Parse(format!("{}: {e}", path.as_ref().display())))?;
		Self::from_toml(&text)
	}

	pub fn validate(&self) -> Result<(), ManifestError> {
		let bad = |m: &str| Err(ManifestError::Invalid(m.to_string()));
		if self.id.trim().is_empty()
			|| self.id.chars().any(|c| c.is_whitespace() || c == '/' || c == '\\')
		{
			return bad("id must be a non-empty reverse-domain token");
		}
		if self.name.trim().is_empty() {
			return bad("name must not be empty");
		}
		if self.version.trim().is_empty() {
			return bad("version must not be empty");
		}
		if self.api != API_VERSION {
			return bad(&format!(
				"unsupported api {} (host speaks {API_VERSION})",
				self.api
			));
		}
		if self.process.command.is_empty() || self.process.command[0].trim().is_empty() {
			return bad("process.command must be a non-empty argv");
		}
		for cap in &self.capabilities {
			if !KNOWN_CAPABILITIES.contains(&cap.as_str()) {
				return bad(&format!("unknown capability {cap:?}"));
			}
		}
		let mut seen = HashSet::new();
		for p in &self.panels {
			if p.id.trim().is_empty() {
				return bad("panels[].id must not be empty");
			}
			if !seen.insert(p.id.as_str()) {
				return bad(&format!("duplicate panel id {:?}", p.id));
			}
		}
		if self.restart.backoff_ms == 0 {
			return bad("restart.backoff_ms must be >= 1");
		}
		Ok(())
	}

	/// argv with `{plugin_dir}` substituted (design §2.2 `process.command`).
	pub fn resolve_command(&self, plugin_dir: &Path) -> Vec<String> {
		let dir = plugin_dir.to_string_lossy();
		self.process
			.command
			.iter()
			.map(|a| a.replace("{plugin_dir}", &dir))
			.collect()
	}

	/// Child environment: only whitelisted names copied from `base`.
	pub fn resolve_env(&self, base: &[(String, String)]) -> Vec<(String, String)> {
		self.process
			.env_passthrough
			.iter()
			.filter_map(|want| {
				base.iter()
					.find(|(k, _)| k == want)
					.map(|(k, v)| (k.clone(), v.clone()))
			})
			.collect()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Body after the fixed header (`id/name/version/api`); tables such as
	/// `[process]` are supplied by the caller.
	fn from_body(body: &str) -> Result<Manifest, ManifestError> {
		Manifest::from_toml(&format!(
			"id = \"com.example.p\"\nname = \"Example\"\nversion = \"0.1.0\"\napi = 1\n{body}"
		))
	}

	#[test]
	fn defaults_match_the_frozen_table() {
		let m = from_body("capabilities = []\n[process]\ncommand = [\"x\"]\n").unwrap();
		assert_eq!(m.restart.max, DEFAULT_MAX_RESTARTS, "restart.max defaults to 5");
		assert_eq!(m.restart.backoff_ms, DEFAULT_BACKOFF_MS, "backoff defaults to 1000 ms");
		assert_eq!(m.process.env_passthrough, vec!["PATH", "HOME"]);
		assert!(m.panels.is_empty() && m.subscribe.is_empty());
	}

	#[test]
	fn rejects_invalid_manifests() {
		let raw = |text: &str| Manifest::from_toml(text).unwrap_err();
		let body = |b: &str| from_body(b).unwrap_err();
		// api is required
		assert!(matches!(
			raw("id = \"a.b\"\nname = \"n\"\nversion = \"1\"\n[process]\ncommand = [\"x\"]\n"),
			ManifestError::Parse(_)
		));
		// unsupported api
		let e = Manifest::from_toml(
			"id = \"a.b\"\nname = \"n\"\nversion = \"1\"\napi = 2\n[process]\ncommand = [\"x\"]\n",
		)
		.unwrap_err();
		assert!(matches!(e, ManifestError::Invalid(_)), "{e}");
		// empty command
		let e = body("[process]\ncommand = []\n");
		assert!(matches!(e, ManifestError::Invalid(_)), "{e}");
		// unknown capability
		let e = body("capabilities = [\"ai.world-domination\"]\n[process]\ncommand = [\"x\"]\n");
		assert!(matches!(e, ManifestError::Invalid(_)), "{e}");
		// duplicate panel id
		let e = body(
			"[[panels]]\nid = \"chat\"\ntitle = \"A\"\n[[panels]]\nid = \"chat\"\ntitle = \"B\"\n[process]\ncommand = [\"x\"]\n",
		);
		assert!(matches!(e, ManifestError::Invalid(_)), "{e}");
		// backoff must be >= 1
		let e = body("[restart]\nbackoff_ms = 0\n[process]\ncommand = [\"x\"]\n");
		assert!(matches!(e, ManifestError::Invalid(_)), "{e}");
	}

	#[test]
	fn resolve_command_substitutes_plugin_dir() {
		let m = from_body(
			"[process]\ncommand = [\"/usr/bin/python3\", \"{plugin_dir}/plugin.py\"]\n",
		)
		.unwrap();
		let argv = m.resolve_command(Path::new("/opt/plugins/p1"));
		assert_eq!(
			argv,
			vec!["/usr/bin/python3", "/opt/plugins/p1/plugin.py"]
		);
	}

	#[test]
	fn resolve_env_is_a_whitelist() {
		let m = from_body(
			"[process]\ncommand = [\"x\"]\nenv_passthrough = [\"PATH\", \"OAK_SECRET\"]\n",
		)
		.unwrap();
		let base = vec![
			("PATH".to_string(), "/usr/bin".to_string()),
			("OAK_SECRET".to_string(), "s3cr3t".to_string()),
			("UNRELATED".to_string(), "nope".to_string()),
		];
		let env = m.resolve_env(&base);
		assert_eq!(env.len(), 2, "only whitelisted names pass");
		assert!(env.iter().any(|(k, _)| k == "PATH"));
		assert!(env.iter().any(|(k, _)| k == "OAK_SECRET"));
		assert!(!env.iter().any(|(k, _)| k == "UNRELATED"));
	}
}
