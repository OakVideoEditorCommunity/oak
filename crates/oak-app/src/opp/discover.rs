//! Plugin discovery and start-up (T7.2 P1.2).
//!
//! Scan `plugin.toml` manifests under the search directories, hand each to
//! the P1 `PluginHost` (spawn + handshake + session), and gate the whole
//! thing behind the `OAK_OPP_PLUGINS` environment switch — the GUI toggle
//! arrives with P2 (task note: "插件扫描目录 + `OAK_OPP_PLUGINS` env 开关").
//!
//! Search order (first hit wins per plugin id is *not* applied: every
//! directory is scanned, deduplicated by manifest path):
//!
//! 1. every directory listed in `OAK_OPP_PLUGINS` (path-list syntax:
//!    `:`-separated on Unix, like `PATH`; the literal value `1` means
//!    "use the default directory only"),
//! 2. the default `$OAK_HOME/plugins` (`~/.oak/plugins` when `OAK_HOME`
//!    is unset).
//!
//! A plugin is a **directory** containing `plugin.toml`; its executable is
//! resolved relative to that directory (`Manifest::resolve_command`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use oak_extensions::lifecycle::{LifecycleConfig, PluginHost};
use oak_extensions::manifest::Manifest;

/// The manifest filename every plugin directory must carry.
pub const MANIFEST_FILE: &str = "plugin.toml";

/// `OAK_OPP_PLUGINS` values that mean "enabled, default directory".
const ON: [&str; 3] = ["1", "true", "yes"];

/// One discovered plugin: its manifest plus the directory it lives in.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
	pub dir: PathBuf,
	pub manifest: Manifest,
}

/// `$OAK_HOME/plugins`, or `~/.oak/plugins` (`OAK_HOME` keeps other Oak
/// state in one place, matching the engine's cache-root convention).
pub fn default_plugin_dir() -> PathBuf {
	match std::env::var_os("OAK_HOME") {
		Some(home) if !home.is_empty() => PathBuf::from(home).join("plugins"),
		_ => dirs_home().join(".oak").join("plugins"),
	}
}

/// The user's home directory without pulling in the `dirs` crate: fall back
/// to `/tmp` rather than panicking when neither variable is set.
fn dirs_home() -> PathBuf {
	std::env::var_os("HOME")
		.filter(|h| !h.is_empty())
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// The directories to scan, honouring `OAK_OPP_PLUGINS`; empty when the
/// switch is off (P1.2 default: plugins do not start with the GUI).
pub fn search_dirs() -> Vec<PathBuf> {
	let raw = match std::env::var("OAK_OPP_PLUGINS") {
		Ok(v) => v,
		Err(_) => return Vec::new(),
	};
	let trimmed = raw.trim();
	if trimmed.is_empty() {
		return Vec::new();
	}
	if ON.contains(&trimmed) {
		return vec![default_plugin_dir()];
	}
trimmed
		.split(':')
		.map(str::trim)
		.filter(|s| !s.is_empty())
		.map(PathBuf::from)
		.collect()
}

/// Scan `dir` for plugin directories (each a child holding `plugin.toml`;
/// `dir` itself is accepted too when it *is* a plugin directory).
/// Unreadable entries and invalid manifests are skipped — one bad plugin
/// must not take the host down — and the caller can inspect them through
/// [`discover_with_errors`].
pub fn discover(dir: &Path) -> Vec<DiscoveredPlugin> {
	discover_with_errors(dir).0
}

/// [`discover`] plus the manifests that failed to load/validate, so the
/// caller can log them instead of silently dropping a broken plugin.
pub fn discover_with_errors(dir: &Path) -> (Vec<DiscoveredPlugin>, Vec<(PathBuf, String)>) {
	let mut found = Vec::new();
	let mut errors = Vec::new();
	let push = |path: PathBuf, found: &mut Vec<_>, errors: &mut Vec<_>| {
		match Manifest::load(&path) {
			Ok(manifest) => match manifest.validate() {
				Ok(()) => found.push(DiscoveredPlugin {
					dir: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
					manifest,
				}),
				Err(e) => errors.push((path, format!("invalid manifest: {e}"))),
			},
			Err(e) => errors.push((path, format!("load failed: {e}"))),
		}
	};

	if !dir.exists() {
		return (found, errors);
	}
	// The directory itself may be a plugin.
	let own = dir.join(MANIFEST_FILE);
	if own.is_file() {
		push(own, &mut found, &mut errors);
	}
	// Otherwise each child directory that carries a manifest is one plugin.
	let Ok(entries) = std::fs::read_dir(dir) else {
		return (found, errors);
	};
	let mut children: Vec<PathBuf> = entries
		.flatten()
		.map(|e| e.path())
		.filter(|p| p.is_dir())
		.collect();
	children.sort();
	for child in children {
		let manifest = child.join(MANIFEST_FILE);
		if manifest.is_file() {
			push(manifest, &mut found, &mut errors);
		}
	}
	(found, errors)
}

/// Every plugin under every search directory (deduplicated by manifest
/// path so an explicit list overlapping the default does not double-start).
pub fn discover_all() -> (Vec<DiscoveredPlugin>, Vec<(PathBuf, String)>) {
	let mut all = Vec::new();
	let mut errors = Vec::new();
	let mut seen = BTreeSet::new();
	for dir in search_dirs() {
		let (found, errs) = discover_with_errors(&dir);
		errors.extend(errs);
		for plugin in found {
			let key = plugin.dir.join(MANIFEST_FILE);
			if seen.insert(key) {
				all.push(plugin);
			}
		}
	}
	(all, errors)
}

/// Start every discovered plugin when `OAK_OPP_PLUGINS` is on; returns the
/// live hosts (empty when the switch is off or nothing was found).
///
/// A plugin that fails to spawn is skipped and recorded in the returned
/// pair's `failed` list — the host keeps running (notes: one bad plugin
/// must not take Oak down).
pub fn start_enabled() -> (Vec<PluginHost>, Vec<(String, String)>) {
	let (plugins, errors) = discover_all();
	let mut failed: Vec<(String, String)> = errors
		.into_iter()
		.map(|(path, e)| (path.display().to_string(), e))
		.collect();
	let mut hosts = Vec::new();
	for plugin in plugins {
		let id = plugin.manifest.id.clone();
		match PluginHost::start(plugin.manifest, plugin.dir, LifecycleConfig::default()) {
			Ok(host) => hosts.push(host),
			Err(e) => failed.push((id, e.to_string())),
		}
	}
	(hosts, failed)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The smallest manifest `validate` accepts (id/name/version + api 1 +
	/// a non-empty argv).
	const GOOD: &str = r#"id = "com.example.demo"
name = "Demo"
version = "0.1.0"
api = 1

[process]
command = ["./plugin"]
"#;

	/// Wire-layer coverage: a valid manifest is accepted, an unsupported
	/// `api` is reported rather than silently dropped, and a directory
	/// without `plugin.toml` is skipped without an error.
	#[test]
	fn discover_accepts_valid_and_reports_invalid_manifests() {
		let dir = std::env::temp_dir().join(format!("opp-discover-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		for child in ["good", "bad", "empty"] {
			std::fs::create_dir_all(dir.join(child)).expect("temp plugin dir");
		}
		std::fs::write(dir.join("good/plugin.toml"), GOOD).unwrap();
		// `api = 99` fails `validate` (host speaks OPP/1 only).
		std::fs::write(
			dir.join("bad/plugin.toml"),
			GOOD.replace("api = 1", "api = 99"),
		)
		.unwrap();

		let (found, errors) = discover_with_errors(&dir);
		assert_eq!(found.len(), 1, "only the valid plugin is accepted: {found:?}");
		assert_eq!(found[0].manifest.id, "com.example.demo");
		assert_eq!(found[0].dir, dir.join("good"));
		assert_eq!(errors.len(), 1, "the invalid manifest is reported: {errors:?}");
		assert!(
			errors[0].0.ends_with("bad/plugin.toml"),
			"wrong manifest reported: {:?}",
			errors[0].0
		);

		// A directory that *is* a plugin directory is discovered on its own.
		let (own, own_errors) = discover_with_errors(&dir.join("good"));
		assert_eq!(own.len(), 1);
		assert!(own_errors.is_empty(), "{own_errors:?}");

		let _ = std::fs::remove_dir_all(&dir);
	}
}
