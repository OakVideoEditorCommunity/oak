# TODO

## Goal
- Add an LRU prerender cache (2–3 seconds ahead) and proxy clip support, implemented as incremental, compile-safe steps within the current architecture.

## Where the Changes Live (Current Architecture)
- Playback/render scheduling: `app/render/renderprocessor.cpp`, `app/render/plugin/pluginrenderer.cpp`, `app/node/traverser.cpp`
- Plugin node inputs/defaults: `app/node/plugins/Plugin.cpp`
- Clip image/texture fetch: `app/pluginSupport/OliveClip.cpp`, `app/pluginSupport/OliveClip.h`
- Node graph and values: `app/node/node.h`, `app/node/node.cpp`, `app/node/value.h`
- Project/serialization: `app/node/project/serializer/*`

## LRU Cache Plan (Code Changes + Integration Points)

### Step 1 (compile-safe): Introduce cache data types (no behavior yet)
- Add a small cache module, e.g. `app/render/cache/framecache.h/.cpp`.
- Define:
  - `FrameCacheKey` (graph version/hash, time, params, proxy mode, render scale).
  - `FrameCacheEntry` (AVFrame or Texture + metadata + byte size + last-used).
  - `FrameCache` API: `get(key)`, `put(key, entry)`, `invalidateByVersion(version)`.
- Wire in a compile-only stub with no runtime usage.

### Step 2 (compile-safe): Define graph/version invalidation hook
- Add a lightweight “graph version” counter to `Node` or a render pipeline owner.
- Increment on param changes and graph edits.
- Expose a read-only version getter for the render pipeline.

### Step 3 (small behavior): Cache current frame only
- In `renderprocessor.cpp` playback path, check cache before rendering:
  - If hit, present cached frame.
  - If miss, render normally and `put` into cache.
- Keep budget small (few frames) to minimize risk.

### Step 4 (small behavior): Pre-render window scheduling
- Add a render queue for time range [now, now+N] (N = 2–3s).
- Limit worker count (e.g., 2–3 tasks) to avoid UI starvation.
- Prioritize current frame > near future.
- On seek, cancel or drop stale tasks.

### Step 5 (behavior): LRU eviction policy
- Enforce memory budget and frame count cap.
- Evict least-recently-used entries.

### Step 6 (behavior): GPU/CPU policy
- Cache CPU frames by default for safety.
- For GL outputs, upload from cached CPU frame when displayed.
- Optionally add GPU caching later behind a feature flag.

### Step 7 (observability)
- Add counters for hit rate, average render time, and drops.
- Log only in debug builds.

## Proxy Clip Plan (Code Changes + Integration Points)

### Step 1 (compile-safe): Data model + serialization
- Extend clip metadata with:
  - `proxy_path`, `proxy_width`, `proxy_height`, `proxy_codec`, `proxy_fps`.
- Add read/write in `app/node/project/serializer/*`.

### Step 2 (small behavior): Proxy selection policy
- Add project-level and clip-level proxy mode:
  - `Auto`, `ForceProxy`, `ForceOriginal`.
- Add a simple resolver in clip/media source code that picks proxy if enabled.

### Step 3 (behavior): Proxy generation pipeline
- Add a background task to build proxies (using existing render/export tasks).
- Store output path and metadata on success.

### Step 4 (behavior): UI wiring
- Add “Generate Proxy” action + proxy indicator.
- Add “Relink Proxy” dialog.

### Step 5 (validation)
- Compare proxy vs original for timing and sync.
- Ensure proxies are ignored for export unless explicitly enabled.

## Small-Step Implementation Plan (Each Step Builds)
1) Add cache module + types (no references).
2) Add graph version counter (increment on changes).
3) Wire cache lookup for current frame only.
4) Add prerender queue (2–3 seconds) with limited concurrency.
5) Add LRU eviction + memory budget.
6) Add cache invalidation on graph version change.
7) Add basic metrics/logging.
8) Add proxy metadata fields + serialization.
9) Add proxy selection policy (Auto/Force modes).
10) Add proxy generation task + UI entry points.

## Open Questions
- Default cache size per hardware tier.
- Where to store proxy files on disk.
- Whether to cache GPU textures or CPU frames only.
