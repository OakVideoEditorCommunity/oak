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

//! The startup update check and the external oakvideoeditor.org links.
//!
//! On every launch (unless the user turned it off in the preferences, the
//! `CheckForUpdates` config key) the app GETs [`UPDATE_ENDPOINT`], parses
//! the `LatestRelease` JSON and — when the remote version is newer than
//! [`current_version`] — prompts with a dialog pointing at
//! [`DOWNLOAD_URL`]. Network and parse failures are silent (a missing
//! update server must never disturb startup); the fetch is blocking and
//! runs on the gpui background executor.
//!
//! The transport sits behind [`UpdateTransport`] so tests can script
//! responses without touching the network.

use std::time::Duration;

use serde::Deserialize;

/// The latest-release endpoint (see the API contract in the module docs
/// of the website; the response is the struct below).
pub const UPDATE_ENDPOINT: &str = "https://www.oakvideoeditor.org/api/v1/update/latest";
/// Where the update dialog sends the user to download the new build.
pub const DOWNLOAD_URL: &str = "https://www.oakvideoeditor.org/downloads";
/// The bug-report form opened from the Help menu.
pub const BUG_REPORT_URL: &str = "https://www.oakvideoeditor.org/bug-report";
/// Config key: run the startup update check (preferences toggle, default on).
pub const CONFIG_KEY_CHECK_UPDATES: &str = "CheckForUpdates";

/// One `GET /api/v1/update/latest` response.
///
/// ```json
/// {
///   "version": "v1.0.0",
///   "tag_name": "v1.0.0",
///   "notes": "更新说明（Markdown）",
///   "is_prerelease": false,
///   "published_at": "2026-09-20T08:00:00+00:00",
///   "download_url": "/api/v1/releases/<id>/download?asset_id=<asset_id>"
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LatestRelease {
	/// The release version (e.g. `v1.0.0`).
	pub version: String,
	/// The release tag (usually the same as `version`).
	#[serde(default)]
	pub tag_name: String,
	/// The release notes (Markdown; displayed as plain text).
	#[serde(default)]
	pub notes: String,
	/// Whether the release is flagged as a pre-release.
	#[serde(default)]
	pub is_prerelease: bool,
	/// The publication timestamp (ISO-8601).
	#[serde(default)]
	pub published_at: String,
	/// The download path relative to the API host.
	#[serde(default)]
	pub download_url: String,
}

/// The running app's version (`Cargo.toml` `version`, e.g. `0.5.0`).
pub fn current_version() -> &'static str {
	env!("CARGO_PKG_VERSION")
}

/// Parses one endpoint body. A missing `version` is rejected (an answer
/// without a version cannot drive the comparison).
pub fn parse_latest(body: &str) -> Result<LatestRelease, String> {
	let release: LatestRelease =
		serde_json::from_str(body).map_err(|e| format!("invalid update response: {e}"))?;
	if release.version.trim().is_empty() {
		return Err("the update response carries no version".to_string());
	}
	Ok(release)
}

/// Parses `v1.2.3` / `1.2.3-rc.1` / `1.2` into a comparable triple; `None`
/// when the string is not a dotted numeric version.
pub fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
	let trimmed = version.trim();
	let trimmed = trimmed.strip_prefix(['v', 'V']).unwrap_or(trimmed);
	// Pre-release/build metadata does not participate in the ordering the
	// prompt needs (`1.2.3-rc.1` compares as `1.2.3`).
	let core = trimmed.split(['-', '+']).next().unwrap_or(trimmed);
	let mut parts = core.split('.');
	let major = parts.next()?.trim().parse().ok()?;
	let minor = parts.next().unwrap_or("0").trim().parse().ok()?;
	let patch = parts.next().unwrap_or("0").trim().parse().ok()?;
	if parts.next().is_some() {
		return None;
	}
	Some((major, minor, patch))
}

/// Whether `remote` names a release newer than `current`.
///
/// Both sides accept the leading `v`; unparsable versions fall back to a
/// plain string inequality (a differently-named release is worth telling
/// the user about, per the "different version" contract).
pub fn is_newer(remote: &str, current: &str) -> bool {
	match (parse_version(remote), parse_version(current)) {
		(Some(remote), Some(current)) => remote > current,
		_ => {
			remote.trim().trim_start_matches(['v', 'V']) != current.trim().trim_start_matches(['v', 'V'])
		}
	}
}

/// Whether the preferences allow the startup check (default on).
pub fn check_enabled() -> bool {
	crate::oakui::real::config_get_bool(CONFIG_KEY_CHECK_UPDATES, true)
}

/// Persists the preferences toggle.
pub fn set_check_enabled(enabled: bool) {
	crate::oakui::real::config_set_bool(CONFIG_KEY_CHECK_UPDATES, enabled);
}

/// Whether a fetched release is worth prompting about (newer than the
/// running build).
pub fn should_notify(release: &LatestRelease) -> bool {
	is_newer(&release.version, current_version())
}

/// The transport seam: one blocking GET. Implementations must be cheap to
/// clone-free share (`Send + Sync`); the checked-in implementation is
/// [`HttpTransport`].
pub trait UpdateTransport: Send + Sync {
	/// GETs `url` and returns the body as text.
	fn get(&self, url: &str) -> Result<String, String>;
}

/// The real HTTPS transport (ureq + rustls, bounded at 5 s); the call is
/// blocking, so callers run it on the background executor.
pub struct HttpTransport;

impl UpdateTransport for HttpTransport {
	fn get(&self, url: &str) -> Result<String, String> {
		let config = ureq::Agent::config_builder()
			.timeout_global(Some(Duration::from_secs(5)))
			.build();
		let agent: ureq::Agent = config.into();
		let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
		response
			.body_mut()
			.read_to_string()
			.map_err(|e| e.to_string())
	}
}

/// Fetches and parses the latest release through `transport`.
pub fn fetch_latest(transport: &dyn UpdateTransport) -> Result<LatestRelease, String> {
	let body = transport.get(UPDATE_ENDPOINT)?;
	parse_latest(&body)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The documented response shape parses field by field.
	#[test]
	fn parse_latest_reads_the_documented_shape() {
		let body = r#"{
			"version": "v1.0.0",
			"tag_name": "v1.0.0",
			"notes": "更新说明（Markdown）",
			"is_prerelease": false,
			"published_at": "2026-09-20T08:00:00+00:00",
			"download_url": "/api/v1/releases/7/download?asset_id=12"
		}"#;
		let release = parse_latest(body).expect("valid response");
		assert_eq!(release.version, "v1.0.0");
		assert_eq!(release.tag_name, "v1.0.0");
		assert_eq!(release.notes, "更新说明（Markdown）");
		assert!(!release.is_prerelease);
		assert_eq!(release.published_at, "2026-09-20T08:00:00+00:00");
		assert_eq!(release.download_url, "/api/v1/releases/7/download?asset_id=12");
	}

	/// Optional fields default; a response without a version is rejected.
	#[test]
	fn parse_latest_rejects_missing_versions() {
		let minimal = parse_latest(r#"{"version":"v2.1.0"}"#).expect("minimal response");
		assert!(minimal.notes.is_empty());
		assert!(!minimal.is_prerelease);

		assert!(parse_latest("{}").is_err(), "no version");
		assert!(parse_latest(r#"{"version":"  "}"#).is_err(), "blank version");
		assert!(parse_latest("not json").is_err());
	}

	/// The version parser strips the tag prefix and ignores pre-release
	/// metadata; the comparison orders numerically.
	#[test]
	fn version_comparison_orders_numerically() {
		assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
		assert_eq!(parse_version("1.2.3-rc.1"), Some((1, 2, 3)));
		assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
		assert_eq!(parse_version("v10.0.1"), Some((10, 0, 1)));
		assert_eq!(parse_version("nightly"), None);

		assert!(is_newer("v1.0.0", "0.5.0"));
		assert!(is_newer("v0.5.1", "0.5.0"));
		assert!(!is_newer("v0.5.0", "0.5.0"));
		assert!(!is_newer("v0.4.9", "0.5.0"));
		assert!(is_newer("2026.1", "0.5.0"), "unparsable remote falls back to inequality");
		assert!(!is_newer("v0.5.0", "0.5.0-rc.1"), "pre-release current compares by core");
	}

	/// The check is on by default and follows the preferences toggle.
	#[test]
	fn check_enabled_follows_the_config() {
		let _guard = crate::oakui::graphops::test_lock();
		// Restore the key on drop (the config store is process-global).
		struct Restore(&'static str, String);
		impl Drop for Restore {
			fn drop(&mut self) {
				crate::oakui::real::config_set_string(self.0, &self.1);
			}
		}
		let _save = Restore(
			CONFIG_KEY_CHECK_UPDATES,
			crate::oakui::real::config_get_string(CONFIG_KEY_CHECK_UPDATES),
		);
		crate::oakui::real::config_set_string(CONFIG_KEY_CHECK_UPDATES, "");
		assert!(check_enabled(), "default-on");
		set_check_enabled(false);
		assert!(!check_enabled());
		set_check_enabled(true);
		assert!(check_enabled());
	}

	/// A scripted transport drives `fetch_latest` end to end.
	#[test]
	fn fetch_latest_reads_through_the_transport() {
		struct Scripted;
		impl UpdateTransport for Scripted {
			fn get(&self, url: &str) -> Result<String, String> {
				assert_eq!(url, UPDATE_ENDPOINT);
				Ok(r#"{"version":"v9.9.9"}"#.to_string())
			}
		}
		let release = fetch_latest(&Scripted).expect("scripted response");
		assert_eq!(release.version, "v9.9.9");
		assert!(should_notify(&release));

		struct Failing;
		impl UpdateTransport for Failing {
			fn get(&self, _url: &str) -> Result<String, String> {
				Err("offline".to_string())
			}
		}
		assert!(fetch_latest(&Failing).is_err());
	}
}
