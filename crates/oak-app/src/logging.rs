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

//! A minimal stderr backend for the `log` facade.
//!
//! The gpui / wgpu / oak stack all emit through `log::*`, but nothing in the
//! binary installed a logger, so every validation error, adapter warning and
//! diagnostic was silently dropped (this is what made the viewer-black-screen
//! fault invisible for so long). [`init`] installs a small logger that writes
//! to stderr; `RUST_LOG` selects the verbosity:
//!
//! * unset — `warn` and above;
//! * a level name (`error` / `warn` / `info` / `debug` / `trace`) — that
//!   level and above for every target;
//! * comma-separated `target=level` pairs (`wgpu=debug,oak_render=info`) —
//!   per-target overrides on top of the global level. A bare level among the
//!   pairs sets the global level.

use std::sync::Once;

use log::{Level, LevelFilter, Metadata, Record};

/// One parsed `RUST_LOG` directive set.
struct Filter {
	/// The global (default) level.
	global: LevelFilter,
	/// Per-target overrides (`target` prefix → level).
	targets: Vec<(String, LevelFilter)>,
}

impl Filter {
	/// Parse the `RUST_LOG` value (`None` / empty → warn-only).
	///
	/// `spec` is the raw environment value: comma-separated directives, each either
	/// `target=level` or a bare level. A bare level sets the global filter, a
	/// `target=level` pair becomes a prefix override, and a directive whose level is
	/// unknown is ignored. Unset, empty or all-blank specs leave the global filter at
	/// `warn`, so the logger stays quiet by default; `init` then raises the facade's
	/// max level to the highest configured one.
	fn parse(spec: Option<&str>) -> Self {
		let mut global = LevelFilter::Warn;
		let mut targets = Vec::new();
		if let Some(spec) = spec.filter(|s| !s.trim().is_empty()) {
			for part in spec.split(',') {
				let part = part.trim();
				if part.is_empty() {
					continue;
				}
				match part.split_once('=') {
					Some((target, level)) => {
						if let Some(level) = parse_level(level.trim()) {
							targets.push((target.trim().to_string(), level));
						}
					}
					None => {
						// A bare level sets the global filter.
						if let Some(level) = parse_level(part) {
							global = level;
						}
					}
				}
			}
		}
		Self { global, targets }
	}

	/// The effective level for `target` (longest matching prefix wins).
	///
	/// A target override applies when `target.starts_with(prefix)`; among several
	/// matching prefixes the longest wins (so `wgpu_hal=trace` beats `wgpu=debug`),
	/// and `global` is the fallback when none match. `target` is the `log` target
	/// (normally a module path); the test is a plain prefix test, not a module
	/// boundary test.
	fn level_for(&self, target: &str) -> LevelFilter {
		let mut best: Option<(usize, LevelFilter)> = None;
		for (prefix, level) in &self.targets {
			if target.starts_with(prefix.as_str()) {
				let len = prefix.len();
				if best.map(|(b, _)| len >= b).unwrap_or(true) {
					best = Some((len, *level));
				}
			}
		}
		best.map(|(_, level)| level).unwrap_or(self.global)
	}
}

/// Map a level name to a [`LevelFilter`].
///
/// The match is ASCII case-insensitive (`OFF` works) and does not trim — callers
/// trim. `off`, `error`, `warn` / `warning`, `info`, `debug` and `trace` map to the
/// corresponding filters; any other text returns `None`, which `Filter::parse`
/// treats as "ignore this directive".
fn parse_level(text: &str) -> Option<LevelFilter> {
	match text.to_ascii_lowercase().as_str() {
		"off" => Some(LevelFilter::Off),
		"error" => Some(LevelFilter::Error),
		"warn" | "warning" => Some(LevelFilter::Warn),
		"info" => Some(LevelFilter::Info),
		"debug" => Some(LevelFilter::Debug),
		"trace" => Some(LevelFilter::Trace),
		_ => None,
	}
}

/// The logger installed by [`init`].
struct StderrLogger {
	filter: Filter,
}

impl log::Log for StderrLogger {
	fn enabled(&self, metadata: &Metadata) -> bool {
		metadata.level() <= self.filter.level_for(metadata.target())
	}

	fn log(&self, record: &Record) {
		if !self.enabled(record.metadata()) {
			return;
		}
		let level = match record.level() {
			Level::Error => "ERROR",
			Level::Warn => "WARN",
			Level::Info => "INFO",
			Level::Debug => "DEBUG",
			Level::Trace => "TRACE",
		};
		eprintln!(
			"[{level} {}] {}",
			record.target(),
			record.args()
		);
	}

	fn flush(&self) {}
}

static INIT: Once = Once::new();

/// Install the stderr logger once (idempotent). Honors `RUST_LOG`.
pub fn init() {
	INIT.call_once(|| {
		let filter = Filter::parse(std::env::var("RUST_LOG").ok().as_deref());
		let max = filter
			.targets
			.iter()
			.map(|(_, level)| *level)
			.max()
			.unwrap_or(LevelFilter::Off)
			.max(filter.global);
		let logger = Box::leak(Box::new(StderrLogger { filter }));
		if log::set_logger(logger).is_ok() {
			log::set_max_level(max);
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn default_is_warn() {
		let f = Filter::parse(None);
		assert_eq!(f.level_for("anything"), LevelFilter::Warn);
		assert_eq!(f.level_for(""), LevelFilter::Warn);
	}

	#[test]
	fn bare_level_sets_global() {
		let f = Filter::parse(Some("info"));
		assert_eq!(f.level_for("wgpu_hal"), LevelFilter::Info);
		assert_eq!(f.level_for("oak_render"), LevelFilter::Info);
	}

	#[test]
	fn target_overrides_with_longest_prefix() {
		let f = Filter::parse(Some("warn,wgpu=debug,wgpu_hal=trace"));
		assert_eq!(f.level_for("wgpu"), LevelFilter::Debug);
		assert_eq!(f.level_for("wgpu_core"), LevelFilter::Debug);
		assert_eq!(f.level_for("wgpu_hal::vulkan"), LevelFilter::Trace);
		assert_eq!(f.level_for("oak_render"), LevelFilter::Warn);
	}

	#[test]
	fn parse_level_aliases() {
		assert_eq!(parse_level("warning"), Some(LevelFilter::Warn));
		assert_eq!(parse_level("OFF"), Some(LevelFilter::Off));
		assert_eq!(parse_level("bogus"), None);
	}

	#[test]
	fn empty_spec_falls_back_to_warn() {
		let f = Filter::parse(Some("   "));
		assert_eq!(f.level_for("x"), LevelFilter::Warn);
	}
}
