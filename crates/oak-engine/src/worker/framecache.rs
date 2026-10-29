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

//! LRU byte-budgeted cache of rendered F32 pipeline frames (M16 S2).
//!
//! The worker renders every batch ticket through the CPU eval path — an OFX
//! plugin frame can cost 100-500 ms of full graph re-evaluation with no
//! caching anywhere in the pipeline. Preview traffic is heavily repetitive:
//! the pause frame, scrubbing back over already-rendered time, and the
//! in-flight frames that re-render after a plugin is removed all re-request
//! pixels the worker has already produced. This module memoizes the F32
//! pipeline frame keyed by the **render-deterministic subset** of the ticket
//! spec, so repeated requests become a memcpy into the shm slot instead of a
//! re-render.
//!
//! Keying. The F32 pipeline bytes depend only on
//! (`time`, `width`, `height`, footage source, montage/effects, viewer graph
//! identity) — the fields [`spec_cache_key`] serializes. `ticket`, `slot`,
//! `format` and `channels` are deliberately excluded: they describe the
//! delivery, not the picture. In particular an F32-slot request and a
//! BGRA8-slot request for the same frame share one cache entry (the F32
//! pipeline renders at `force_format: F32` regardless of the slot format;
//! the end-of-pipe convert happens after the cache).
//!
//! Invalidations. A [`crate::worker::WorkerSession`] clears the whole cache
//! on every successful `load_graph` — the key set does not include the graph
//! contents, and a fresh snapshot (new project or new undo revision) can
//! change what any viewer identity renders. Media files are assumed
//! immutable for preview, matching the rest of the pipeline.
//!
//! Budget. The default budget is 64 MiB (env `OAK_WORKER_FRAME_CACHE_MB`),
//! capped at 64 entries, LRU-evicted. A single entry is never evicted for
//! size — 1080p F32 frames are 33 MB, 4K are 133 MB, so an oversized frame
//! still gets cached (it is the most likely re-request: the pause frame).

use std::collections::HashMap;

use serde::Serialize;

use crate::worker::ipc::BatchTicketSpec;

/// The default frame-cache budget in bytes (64 MiB).
const DEFAULT_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
/// The default maximum number of cached frames.
const DEFAULT_MAX_ENTRIES: usize = 64;
/// Env override for the budget, in MiB.
const BUDGET_ENV: &str = "OAK_WORKER_FRAME_CACHE_MB";

/// One cached frame: its F32 pipeline bytes and the LRU recency stamp.
struct Entry {
	bytes: Vec<u8>,
	stamp: u64,
}

/// The cache (see the module docs). Not `Clone`, owned by the
/// [`crate::worker::WorkerSession`], touched only on the worker's single
/// loop thread.
pub struct FrameCache {
	entries: HashMap<String, Entry>,
	budget_bytes: u64,
	max_entries: usize,
	/// Monotonic LRU clock; bumped on every insert and every hit.
	clock: u64,
}

impl FrameCache {
	/// An empty cache with the default budget and entry cap (env
	/// `OAK_WORKER_FRAME_CACHE_MB` overrides the budget in MiB; garbage
	/// values fall back to the default).
	pub fn new() -> Self {
		let budget = std::env::var(BUDGET_ENV)
			.ok()
			.and_then(|v| v.trim().parse::<f64>().ok())
			.filter(|mb| mb.is_finite() && *mb > 0.0)
			.map(|mb| (mb * 1024.0 * 1024.0).round() as u64)
			.unwrap_or(DEFAULT_BUDGET_BYTES);
		Self::with_limits(budget, DEFAULT_MAX_ENTRIES)
	}

	/// An empty cache with explicit limits (tests).
	pub fn with_limits(budget_bytes: u64, max_entries: usize) -> Self {
		Self {
			entries: HashMap::new(),
			budget_bytes,
			max_entries,
			clock: 0,
		}
	}

	/// The cached F32 bytes for `key`, or `None`. A hit refreshes the LRU
	/// recency.
	pub fn get(&mut self, key: &str) -> Option<&[u8]> {
		let clock = self.clock;
		let entry = self.entries.get_mut(key)?;
		self.clock = clock.wrapping_add(1);
		entry.stamp = self.clock;
		Some(&entry.bytes)
	}

	/// Insert `bytes` under `key`, LRU-evicting until within budget and the
	/// entry cap. The inserted entry is never evicted for size (a single
	/// oversized frame still gets cached); older entries yield to it.
	pub fn insert(&mut self, key: String, bytes: Vec<u8>) {
		if bytes.is_empty() {
			return;
		}
		self.clock = self.clock.wrapping_add(1);
		let stamp = self.clock;
		self.entries.insert(
			key,
			Entry {
				bytes,
				stamp,
			},
		);
		self.evict_until_within_limits();
	}

	/// Evict least-recently-used entries until the budget and entry cap
	/// hold, keeping at least the single most recent entry (which can be
	/// larger than the whole budget).
	fn evict_until_within_limits(&mut self) {
		loop {
			if self.entries.len() <= self.max_entries
				&& self.entries.values().map(|e| e.bytes.len() as u64).sum::<u64>()
					<= self.budget_bytes
			{
				return;
			}
			if self.entries.len() <= 1 {
				return;
			}
			// Drop the entry with the smallest stamp.
			let lru = self
				.entries
				.iter()
				.min_by_key(|(_, e)| e.stamp)
				.map(|(k, _)| k.clone());
			if let Some(lru) = lru {
				self.entries.remove(&lru);
			} else {
				return;
			}
		}
	}

	/// Drop every entry (the worker clears on each successful `load_graph`).
	pub fn clear(&mut self) {
		self.entries.clear();
	}

	/// Number of cached frames (test introspection).
	#[cfg(test)]
	pub fn len(&self) -> usize {
		self.entries.len()
	}

	/// Total cached bytes (test introspection).
	#[cfg(test)]
	pub fn bytes(&self) -> u64 {
		self.entries.values().map(|e| e.bytes.len() as u64).sum()
	}
}

impl Default for FrameCache {
	fn default() -> Self {
		Self::new()
	}
}

/// The render-deterministic ticket-spec subset (see the module docs): the
/// fields whose values determine the F32 pipeline frame, excluding the
/// delivery fields (`ticket`, `slot`, `format`, `channels`).
#[derive(Serialize)]
struct SpecKey {
	time_num: i64,
	time_den: i64,
	width: i32,
	height: i32,
	footage_file: String,
	footage_stream: i32,
	montage: Vec<crate::worker::ipc::WireMontageClip>,
	viewer_node: u64,
	project_key: String,
}

/// The frame-cache key for `spec`: the serialized render-deterministic
/// subset. Serialization cannot fail for these plain fields; a degenerate
/// failure yields the empty key (a cache miss, never a wrong hit).
pub fn spec_cache_key(spec: &BatchTicketSpec) -> String {
	let key = SpecKey {
		time_num: spec.time_num,
		time_den: spec.time_den,
		width: spec.width,
		height: spec.height,
		footage_file: spec.footage_file.clone(),
		footage_stream: spec.footage_stream,
		montage: spec.montage.clone(),
		viewer_node: spec.viewer_node,
		project_key: spec.project_key.clone(),
	};
	serde_json::to_string(&key).unwrap_or_default()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A default spec; callers tweak the fields they want to test.
	fn spec() -> BatchTicketSpec {
		BatchTicketSpec {
			ticket: 7,
			slot: 2,
			time_num: 100,
			time_den: 25,
			width: 1920,
			height: 1080,
			format: crate::worker::ipc::SLOT_FORMAT_BGRA8,
			channels: 4,
			..Default::default()
		}
	}

	#[test]
	fn key_ignores_delivery_fields() {
		let base = spec();
		// Different ticket/slot/format/channels — same picture.
		let other = BatchTicketSpec {
			ticket: 99,
			slot: 17,
			format: 0,
			channels: 2,
			..base.clone()
		};
		assert_eq!(spec_cache_key(&base), spec_cache_key(&other));
	}

	#[test]
	fn key_captures_render_fields() {
		let base = spec();
		let time_shifted = BatchTicketSpec {
			time_num: base.time_num + 1,
			..base.clone()
		};
		assert_ne!(spec_cache_key(&base), spec_cache_key(&time_shifted));
		let resized = BatchTicketSpec {
			width: 1280,
			height: 720,
			..base.clone()
		};
		assert_ne!(spec_cache_key(&base), spec_cache_key(&resized));
	}

	#[test]
	fn hit_returns_same_bytes_and_refreshes_lru() {
		let mut cache = FrameCache::with_limits(1024 * 1024, 64);
		let key = "k1".to_string();
		assert!(cache.get(&key).is_none());
		cache.insert(key.clone(), vec![1u8; 64]);
		assert_eq!(cache.get(&key), Some(vec![1u8; 64].as_slice()));
		// A hit must not drop the entry.
		assert_eq!(cache.len(), 1);
	}

	#[test]
	fn evicts_by_byte_budget_keeping_oversized_single() {
		// Budget 100 bytes: two 64-byte frames cannot both fit, but a single
		// 200-byte frame must stay cached.
		let mut cache = FrameCache::with_limits(100, 64);
		cache.insert("a".to_string(), vec![1u8; 64]);
		assert_eq!(cache.bytes(), 64);
		cache.insert("b".to_string(), vec![2u8; 64]);
		// Over budget: the older entry (a) is evicted, b stays.
		assert_eq!(cache.len(), 1);
		assert!(cache.get("a").is_none());
		assert_eq!(cache.get("b"), Some(vec![2u8; 64].as_slice()));

		// A single entry larger than the whole budget is still cached.
		let mut cache = FrameCache::with_limits(100, 64);
		cache.insert("big".to_string(), vec![3u8; 200]);
		assert_eq!(cache.len(), 1);
		assert_eq!(cache.bytes(), 200);
		assert_eq!(cache.get("big"), Some(vec![3u8; 200].as_slice()));
	}

	#[test]
	fn evicts_by_entry_cap() {
		let mut cache = FrameCache::with_limits(1 << 20, 2);
		cache.insert("a".to_string(), vec![1u8; 8]);
		cache.insert("b".to_string(), vec![2u8; 8]);
		cache.insert("c".to_string(), vec![3u8; 8]);
		assert_eq!(cache.len(), 2);
		assert!(cache.get("a").is_none());
		assert!(cache.get("b").is_some());
		assert!(cache.get("c").is_some());
	}

	#[test]
	fn lru_eviction_prefers_untouched_entries() {
		let mut cache = FrameCache::with_limits(1 << 20, 3);
		cache.insert("a".to_string(), vec![1u8; 8]);
		cache.insert("b".to_string(), vec![2u8; 8]);
		// Touch "a" so it is the most recent; then "b" is the LRU.
		let _ = cache.get("a");
		cache.insert("c".to_string(), vec![3u8; 8]);
		cache.insert("d".to_string(), vec![4u8; 8]);
		assert_eq!(cache.len(), 3);
		assert!(cache.get("b").is_none(), "untouched b is evicted first");
		assert!(cache.get("a").is_some());
		assert!(cache.get("c").is_some());
		assert!(cache.get("d").is_some());
	}

	#[test]
	fn clear_drops_everything() {
		let mut cache = FrameCache::with_limits(1 << 20, 64);
		cache.insert("a".to_string(), vec![1u8; 8]);
		cache.insert("b".to_string(), vec![2u8; 8]);
		cache.clear();
		assert_eq!(cache.len(), 0);
		assert_eq!(cache.bytes(), 0);
		assert!(cache.get("a").is_none());
		assert!(cache.get("b").is_none());
	}
}
