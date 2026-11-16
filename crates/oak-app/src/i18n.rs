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

//! Localization: UI strings live in YAML language packs discovered at
//! runtime (one `<lang>.yaml` per language — the file stem IS the
//! language code, e.g. `en-US.yaml`, `zh-CN.yaml`). Nothing is compiled in: adding a language means
//! dropping one more `.yaml` into any searched directory — no rebuild,
//! no code change. Each pack carries its own display name in the
//! `language.name` key (the endonym, e.g. "Français"), which the
//! language pickers show.
//!
//! # Lookup order
//!
//! 1. The user pack directory (`~/.oak/i18n/`);
//! 2. The bundled packs (next to the executable, the app bundle's
//!    `Resources/i18n/`, or the system install's `<exe>/../share/oak/i18n/`);
//! 3. The dev checkout's `assets/i18n/` (running from the repo).
//!
//! Later sources override earlier ones per key. `tr` falls back from
//! the active language to `en-US` and then to the key itself, so a
//! missing key can never panic — it degrades to a visible key string.
//!
//! # The language setting
//!
//! The active language code is a process-global string (behind an
//! `RwLock` — any thread can read it). At startup [`init`] loads the
//! persisted value from the config key `Language`; unknown codes fall
//! back to `en-US`. [`set_language_code`] flips the global and persists.

use crate::oakui;
use crate::widgets;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path;
use std::sync::{OnceLock, RwLock};

/// The fallback language code: the reference pack every other pack is
/// key-compared against, the config default, and `tr`'s last resort
/// before the raw key.
pub const FALLBACK_CODE: &str = "en-US";

/// One language's key → value table (values are leaked once at load —
/// the tables live for the whole process).
type Table = HashMap<String, &'static str>;

/// All discovered packs: language code → table.
static TABLES: OnceLock<RwLock<HashMap<String, Table>>> = OnceLock::new();

/// The active language code. Empty means "not yet initialized" and reads
/// as [`FALLBACK_CODE`].
static CURRENT_CODE: RwLock<String> = RwLock::new(String::new());

/// The active language code (a discovered pack's code, or `en-US`).
pub fn language_code() -> String {
	let code = CURRENT_CODE.read().unwrap_or_else(|e| e.into_inner());
	if code.is_empty() {
		FALLBACK_CODE.to_string()
	} else {
		code.clone()
	}
}

/// Switches the active language to any discovered pack's code and
/// persists the choice. Unknown codes fall back to en-US.
pub fn set_language_code(code: &str) {
	let exists = tables()
		.read()
		.unwrap_or_else(|e| e.into_inner())
		.contains_key(code);
	let code = if exists { code } else { FALLBACK_CODE };
	*CURRENT_CODE.write().unwrap_or_else(|e| e.into_inner()) = code.to_string();
	persist_language(code);
	sync_widgets();
}

/// A pack's own display name — its `language.name` value (the endonym,
/// e.g. "Français") — which the language pickers show, so a newly
/// dropped-in pack needs no code change. Falls back to the code.
pub fn pack_native_name(code: &str) -> String {
	tables()
		.read()
		.unwrap_or_else(|e| e.into_inner())
		.get(code)
		.and_then(|t| t.get("language.name"))
		.map(|s| s.to_string())
		.unwrap_or_else(|| code.to_string())
}

/// The language codes with a discovered pack: en-US first, zh-CN
/// second, the rest alphabetically by code.
pub fn available_languages() -> Vec<String> {
	let tables = tables().read().unwrap_or_else(|e| e.into_inner());
	let mut codes: Vec<String> = tables.keys().cloned().collect();
	codes.sort();
	let mut out = Vec::with_capacity(codes.len());
	for preferred in [FALLBACK_CODE, "zh-CN"] {
		if let Some(pos) = codes.iter().position(|c| c == preferred) {
			out.push(codes.remove(pos));
		}
	}
	out.extend(codes);
	out
}

/// Loads the persisted language from the config. Called once at startup.
/// Never fails: a missing key keeps the default (en-US).
pub fn init() {
	let code = oakui::real::config_get_string("Language");
	if code.is_empty() {
		sync_widgets();
		return;
	}
	set_language_code(code.trim());
}

/// Persists the active language `code` to the config `Language` key.
///
/// Called by [`set_language_code`] after the switch, so the choice survives a
/// restart (`init` reads the key back). `code` must already be a valid,
/// discovered code — the fallback is resolved by the caller, not here. The write
/// is best-effort: a config-store error is discarded, so a failed persist leaves
/// the running session switched and only the next start reverts.
fn persist_language(code: &str) {
	oakui::real::config_set_string("Language", code);
}

/// Translates `key` in the active language. Never panics: unknown keys
/// fall back to en-US and then to the key itself.
pub fn tr(key: &'static str) -> &'static str {
	let tables = tables().read().unwrap_or_else(|e| e.into_inner());
	let current = language_code();
	if let Some(value) = tables.get(&current).and_then(|t| t.get(key)) {
		return value;
	}
	if let Some(value) = tables.get(FALLBACK_CODE).and_then(|t| t.get(key)) {
		return value;
	}
	key
}

/// Translates a key built at runtime — the built-in effect and parameter
/// names, whose keys are `node.<type-id suffix>.name` and
/// `node.<type-id suffix>.input.<input id>` (see
/// [`oakui::effectchain::addable_effects`](crate::oakui::effectchain::addable_effects)).
///
/// Same lookup as [`tr`] — the active language, then `en-US` — with the
/// node layer's own English name as the last resort: `tr` takes
/// `&'static str` keys and returns the key itself on a miss, which for a
/// formatted key would put a raw `node.…` identifier in the UI.
pub fn tr_or(key: &str, fallback: &str) -> String {
	let tables = tables().read().unwrap_or_else(|e| e.into_inner());
	let current = language_code();
	if let Some(value) = tables.get(&current).and_then(|t| t.get(key)) {
		return value.to_string();
	}
	if let Some(value) = tables.get(FALLBACK_CODE).and_then(|t| t.get(key)) {
		return value.to_string();
	}
	fallback.to_string()
}

/// The keys the gpui widget crates localize through
/// `crate::widgets::i18n` (viewer transport labels, effect-stack empty state).
pub const WIDGET_KEYS: &[&str] = &[
	"viewer.safe_frames",
	"viewer.zoom",
	"viewer.no_frame_source",
	"viewer.in_point",
	"viewer.step_back",
	"viewer.play",
	"viewer.pause",
	"viewer.step_forward",
	"viewer.out_point",
	"viewer.clear_range",
	"effect_stack.empty",
	"effect_stack.add",
	"explorer.tree",
	"explorer.icons",
];

/// Installs the active language's widget strings into the
/// `crate::widgets::i18n` string-table hook.
pub fn sync_widgets() {
	let mut table = widgets::i18n::StringTable::new();
	for key in WIDGET_KEYS {
		table.insert((*key).to_string(), tr(key).to_string());
	}
	widgets::i18n::set_table(table);
}

// ---------------------------------------------------------------------------
// Pack loading
// ---------------------------------------------------------------------------

/// The loaded packs (lazily parsed on first use).
fn tables() -> &'static RwLock<HashMap<String, Table>> {
	TABLES.get_or_init(|| RwLock::new(load_packs()))
}

/// The directories searched for packs, lowest precedence first; each
/// directory's packs override the previous source per key.
fn pack_dirs() -> Vec<path::PathBuf> {
	let mut dirs = Vec::new();
	// Dev checkout: the repo-root assets/i18n (the app crate now lives in
	// crates/oak-app, so CARGO_MANIFEST_DIR is no longer the repo root),
	// plus a CWD-relative fallback for running the binary from the root.
	dirs.push(path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/i18n"));
	dirs.push(path::PathBuf::from("assets/i18n"));
	if let Ok(exe) = env::current_exe() {
		if let Some(dir) = exe.parent() {
			// Portable layout: <exe>/i18n.
			dirs.push(dir.join("i18n"));
			if let Some(contents) = dir.parent() {
				// macOS bundle: <exe>/../Resources/i18n.
				dirs.push(contents.join("Resources/i18n"));
				// cargo-packager deb/AppImage: <exe>/../lib/oak-editor/i18n.
				dirs.push(contents.join("lib/oak-editor/i18n"));
				// System install (deb/rpm/pkg): <exe>/../share/oak/i18n.
				dirs.push(contents.join("share/oak/i18n"));
			}
		}
	}
	// User language packs (add or override languages without a rebuild).
	if let Some(home) = env::var_os("HOME") {
		dirs.push(path::Path::new(&home).join(".oak/i18n"));
	}
	dirs
}

/// Parses one YAML language pack into a key-value table.
///
/// `code` names the pack (used only in diagnostics) and `source` is the YAML
/// text. Keys are UI string ids (`language.name`, `menu.file.open`, ...) and
/// values the translated strings; each value is leaked to `&'static str` so
/// [`tr`] can return it without allocating.
///
/// Returns an empty table on any YAML error — malformed document or duplicate
/// keys — after logging the reason to stderr, so one bad pack degrades to raw
/// keys instead of failing startup.
fn parse_pack(code: &str, source: &str) -> Table {
	let mut table = Table::new();
	let parsed: HashMap<String, String> = match serde_yaml::from_str(source) {
		Ok(map) => map,
		Err(err) => {
			eprintln!("[i18n] failed to parse the {code} pack: {err}");
			return table;
		}
	};
	for (key, value) in parsed {
		let leaked: &'static str = Box::leak(value.into_boxed_str());
		table.insert(key, leaked);
	}
	table
}

/// Loads every pack file found in [`pack_dirs`] (later sources override
/// earlier ones per key; a file named `<code>.yaml` registers or extends
/// language `<code>`).
fn load_packs() -> HashMap<String, Table> {
	let mut packs: HashMap<String, Table> = HashMap::new();
	for dir in pack_dirs() {
		let Ok(read_dir) = fs::read_dir(&dir) else {
			continue;
		};
		let mut files: Vec<_> = read_dir
			.filter_map(|entry| entry.ok())
			.map(|entry| entry.path())
			.filter(|path| {
				path.extension().and_then(|e| e.to_str()) == Some("yaml")
					|| path.extension().and_then(|e| e.to_str()) == Some("yml")
			})
			.collect();
		files.sort();
		for path in files {
			let Some(code) = path.file_stem().and_then(|s| s.to_str()) else {
				continue;
			};
			let Ok(source) = fs::read_to_string(&path) else {
				continue;
			};
			let table = parse_pack(code, &source);
			packs.entry(code.to_string()).or_default().extend(table);
		}
	}
	if packs.is_empty() {
		eprintln!(
			"[i18n] no language packs found in {:?}; UI strings degrade to raw keys",
			pack_dirs()
		);
	}
	packs
}

/// Reloads every pack (tests).
#[cfg(test)]
fn reload_for_test() {
	let mut tables = tables().write().unwrap_or_else(|e| e.into_inner());
	*tables = load_packs();
}

/// Serializes tests that mutate the language global / HOME.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
	lang_test_lock().lock().unwrap_or_else(|e| e.into_inner())
}

/// Compatibility alias used by the app/actions test modules. Both entry
/// points MUST share the one static mutex: with two separate mutexes the
/// i18n tests and the app/actions tests do not exclude each other, and
/// the language global races (observed as a Windows-only CI failure
/// where `tr` returned the English value mid-test).
#[cfg(test)]
pub(crate) fn lang_test_lock() -> &'static std::sync::Mutex<()> {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	&LOCK
}

#[cfg(test)]
mod tests;
