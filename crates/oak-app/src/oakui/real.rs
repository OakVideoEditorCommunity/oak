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

//! The real engine: [`RealEngine`] is a pure `quercus-engine-rs`
//! consumer behind the same [`EngineGateway`](super::engine::EngineGateway) /
//! [`AppEngine`](super::engine::AppEngine) seam the mock implements. The app
//! links no other Oak crate.
//!
//! The engine owns the domain state: a live SDK `EditorSession` holds the
//! project, the current sequence, the marker list / work area and the
//! write-through binding; `graphops` composes SDK `EditCommand`s and owned
//! DTO queries over that session. The remaining app modules are UI
//! adapters: [`super::effectchain`] maps effect-chain DTOs,
//! [`super::renderops`] maps SDK video/audio/export tasks, and
//! [`super::nodegraph`] / [`super::projectbrowser`] present SDK snapshots.
//!
//! # What is real here
//!
//! * **Project** — open/save/close through `EditorSession` (`.ove`; `.otio` /
//!   `.fcpxml` through SDK interchange tasks).
//! * **Sequence** — name / format / length / tracks / clips come from SDK
//!   `Project` queries and `ProjectSnapshot`.
//! * **Edits** — timeline edits are SDK semantic commands, each one undoable
//!   entry on the engine's undo stack; undo/redo walk that stack.
//! * **Export** — the SDK export task, driven on a background thread, with
//!   progress events and cancellation from the SDK task handle.
//! * **Config** — preferences round-trip through the SDK config store; audio
//!   devices are selected through the SDK audio facade.
//! * **Storage** — the session binds opened projects through the SDK
//!   project library; the manager window uses `ProjectLibrary`.
//!
//! # Threading note
//!
//! Preview frames, audio and exports run on SDK engine workers. The app polls
//! task state without waiting on its UI thread and converts owned SDK pixels
//! for the viewer and scopes. Background requests retain owned SDK project
//! handles, so a project drop only makes their completion stale through the
//! generation check.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::widgets::effect_stack::{
	EffectCardKind, EffectData, EffectId, EffectStackDataSource, EffectStackEvent,
};
use crate::widgets::node_graph::{NodeGraphDataSource, NodeGraphEvent};
use crate::widgets::timeline::{
	ClipData, ClipId, Frame, FrameRange, FrameRate, Marker, TimelineDataSource, TimelineEvent,
	TrackData, TrackHeaderEvent, TrackKind, TrimEdge,
};
use gpui_kit::{prelude::*, px, App, Context, Entity, Hsla, Pixels, RenderImage, SharedString};
use crate::widgets::audio_meter::AudioMeterDataSource;
use crate::widgets::project_explorer::{ProjectDataSource, ProjectEntry};
use crate::widgets::viewer::PlaybackClock;

use quercus_engine_rs::editor::TrackKind as DomainTrackKind;
use quercus_engine_rs::node::{NodeId, NodeKind};
use quercus_engine_rs::edit::EditCommand;
use quercus_engine_rs::editor::EditorSession;
use quercus_engine_rs::state::Config;
use quercus_engine_rs::types::{Rational, TimeRange};

use super::engine::{
	AppEngine, EngineGateway, ExportSession, LibraryProject, Monitor, MulticamState, Project,
	ScopeData, Sequence, SequenceParameters, VideoFormat, WizardFootage, WizardSyncOffset,
};
use super::frames::{f32_rgba_to_bgra_image, synthetic_frame_samples};
use super::graphops::{self, ProjectRef};
use super::settings::proxy_params_from_config;
use super::transport::TransportState;

/// Converts the SDK proxy DTO into the proxy dialog's app-owned parameters.
fn proxy_ui_from_sdk(params: &quercus_engine_rs::codec::ProxyParams) -> super::engine::ProxyParamsUi {
	super::engine::ProxyParamsUi {
		width: params.width,
		height: params.height,
		divider: params.divider,
		crf: params.crf,
		preset: params.preset().to_string(),
		include_audio: params.include_audio != 0,
	}
}
/// Creates a migration session over an existing live project Arc.
thread_local! {
	/// The live session of the engine on this thread. Edits executed by the
	/// app's graphops helpers go through this session so its current
	/// sequence, clipboard, markers and work area are the user-visible ones.
	static LIVE_SESSION: std::cell::RefCell<Option<EditorSession>> =
		const { std::cell::RefCell::new(None) };
}

/// Publishes (or clears) the engine's live session for this thread.
pub(crate) fn set_live_session(session: Option<EditorSession>) {
	LIVE_SESSION.with(|slot| *slot.borrow_mut() = session);
}

/// Returns the live session when one is published, otherwise a temporary
/// session over the same project (test/CLI-style use) with the project's
/// first sequence selected.
pub(crate) fn editor_session_for_project(project: &ProjectRef) -> Result<EditorSession, String> {
	if let Some(session) = LIVE_SESSION.with(|slot| slot.borrow().clone()) {
		return Ok(session);
	}
	let session = EditorSession::from_project(project.clone()).map_err(|error| error.to_string())?;
	if let Some(seq) = graphops::sequence_ids(project).first() {
		let _ = session.set_current_sequence(*seq);
	}
	Ok(session)
}
/// Converts the SDK's codec-neutral proxy state into the app facade's UI state.
fn proxy_media_state(state: quercus_engine_rs::codec::ProxyState) -> super::engine::ProxyMediaState {
	match state {
		quercus_engine_rs::codec::ProxyState::Missing => super::engine::ProxyMediaState::Missing,
		quercus_engine_rs::codec::ProxyState::Generating => {
			super::engine::ProxyMediaState::Generating
		}
		quercus_engine_rs::codec::ProxyState::Ready => super::engine::ProxyMediaState::Ready,
		quercus_engine_rs::codec::ProxyState::Failed => super::engine::ProxyMediaState::Failed,
	}
}
/// Converts the SDK multicam DTO into the app facade's stable-identity state.
fn multicam_state_from_sdk(
	state: quercus_engine_rs::editor::multicam::MulticamState,
) -> super::engine::MulticamState {
	super::engine::MulticamState {
		sequence_id: state.sequence.raw(),
		node_id: state.multicam.raw(),
		clip_id: state.clip.raw(),
		source_count: state.source_count,
		current_source: state.current_source,
	}
}

fn rational_to_sdk(value: Rational) -> quercus_engine_rs::types::Rational {
	quercus_engine_rs::types::Rational::new(value.num, value.den)
}

fn rational_from_sdk(value: quercus_engine_rs::types::Rational) -> Rational {
	Rational::new(value.num, value.den)
}

/// The project name of a blank project before it is saved.
const UNTITLED: &str = "Untitled Project";

// ---------------------------------------------------------------------------
// Full-resolution frame cache (M12 P5a)
// ---------------------------------------------------------------------------
//
// Each monitor displays the small proxy frame immediately and lets a
// background thread fill the same frame at the sequence's native size;
// when the fill lands it replaces the proxy in the display path. The cache
// below is the UI-thread-owned state of that schedule: the two cached
// frames plus the one in-flight background job per monitor.

/// The proxy frame cached for one monitor: the image plus the scope samples
/// analyzed in the same render pass (a paused viewer never regenerates
/// either).
struct ProxyEntry {
	/// The playhead frame that produced the frame.
	frame: i64,
	/// The viewer image.
	image: Arc<RenderImage>,
	/// The scope samples of the same render.
	scope: ScopeData,
}

/// A full-resolution frame the background worker filled in. No scope data:
/// the scopes keep reading the proxy pass (same content, lower resolution),
/// so a full-res fill costs no extra analysis and the scopes tab behaves
/// exactly as before.
struct FullResEntry {
	/// The playhead frame the frame was rendered for.
	frame: i64,
	/// The viewer image at the sequence's native size.
	image: Arc<RenderImage>,
}

/// One monitor's display cache: the proxy frame plus the full-resolution
/// fill, and the identity of the in-flight background job.
#[derive(Default)]
struct MonitorFrameCache {
	/// The last proxy render (immediate display path).
	proxy: Option<ProxyEntry>,
	/// The last full-resolution fill (replaces the proxy when its frame
	/// matches the playhead).
	full: Option<FullResEntry>,
	/// The in-flight background job's `(frame, generation)`, or None. Only
	/// one job runs per monitor; the drain re-schedules when the playhead
	/// moves while a job is in flight.
	pending: Option<(i64, u64)>,
}

impl MonitorFrameCache {
	/// The image to display for `frame`: the full-resolution fill when the
	/// worker has landed it, else the proxy frame, else None.
	fn image_for(&self, frame: i64) -> Option<&Arc<RenderImage>> {
		if let Some(full) = &self.full {
			if full.frame == frame {
				return Some(&full.image);
			}
		}
		if let Some(proxy) = &self.proxy {
			if proxy.frame == frame {
				return Some(&proxy.image);
			}
		}
		None
	}

	/// The scope samples matching [`MonitorFrameCache::image_for`] (always
	/// the proxy pass; see [`FullResEntry`]).
	fn scope_for(&self, frame: i64) -> Option<&ScopeData> {
		let proxy = self.proxy.as_ref()?;
		(proxy.frame == frame).then_some(&proxy.scope)
	}

	/// Whether a background full-resolution render should be started for
	/// `frame`: the playhead is not moving (`playing`), the frame is not
	/// already cached full-res, and no job is in flight for this monitor.
	fn needs_full_res(&self, frame: i64, playing: bool) -> bool {
		// Proxy stays primary while playing (a full-res render would fight
		// the moving playhead); a cached fill or an in-flight job mean no
		// new job (the drain re-schedules once the job lands).
		!playing && self.pending.is_none() && !self.full.as_ref().is_some_and(|f| f.frame == frame)
	}

	/// Installs a completed full-res frame. Returns false — and keeps the
	/// cache untouched — when the completion is stale (the pending job it
	/// belongs to no longer matches, i.e. an edit, a selection change or a
	/// project drop happened while it was in flight).
	fn install_full_res(&mut self, frame: i64, generation: u64, image: Arc<RenderImage>) -> bool {
		if self.pending != Some((frame, generation)) {
			return false;
		}
		self.pending = None;
		self.full = Some(FullResEntry { frame, image });
		true
	}
}

/// What a background full-res job renders: the program monitor's sequence
/// or the source monitor's selected footage node. The request also carries
/// the project's `Arc`, which keeps the graph alive regardless of what the
/// UI thread does with the engine's own reference.
enum FullResTarget {
	/// The program monitor's sequence.
	Sequence(NodeId),
	/// The source monitor's selected footage node.
	Footage(NodeId),
}

// ---------------------------------------------------------------------------
// Playback pre-render window (M15 S2)
// ---------------------------------------------------------------------------
//
// During playback the engine feeds the forward window (config
// `PlaybackPreRenderFrames`) to Playback-priority SDK tasks. Completed owned
// frames wait in this cache; `cpu_frame` converts them for display and scopes.
// A miss falls back to the last displayed proxy, while a cold cache gets a
// temporary pattern. Stale requests are cancelled and stale frames dropped.

/// The default forward pre-render window (frames). Kept small: each
/// frame is slow on some hardware (multi-clip montage decodes through
/// FFmpeg), and a large window queues far ahead of the playhead — the
/// workers never catch up, the painted frame lags seconds behind the
/// crown and playback looks frozen. A shorter window bounds the backlog
/// to what the pool can render in a fraction of a second, so the
/// displayed frame tracks the playhead instead of chasing it.
pub const DEFAULT_PREVIEW_WINDOW_FORWARD: i64 = 12;
/// Config key for the forward pre-render window size (frames).
pub const CONFIG_KEY_PREVIEW_WINDOW: &str = "PlaybackPreRenderFrames";

/// One monitor's playback pre-render state. SDK tasks are polled on the UI
/// thread, but all rendering and waiting stays on engine workers.
#[derive(Default)]
struct PreviewWindow {
	/// The node identity the window renders (sequence / footage).
	sequence: u64,
	/// The render-params generation the window was built for (bumped on
	/// invalidation; a mismatch cancels the old requests and rebuilds).
	generation: u64,
	/// Frames already submitted to the scheduler (pending or in flight).
	submitted: BTreeSet<i64>,
	/// Pollable SDK renders. Terminal tasks are drained without waiting.
	requests: BTreeMap<i64, quercus_engine_rs::task::Task>,
	/// Cancelled renders retained until terminal, so dropping them never joins
	/// a running worker on the UI thread.
	retired: Vec<quercus_engine_rs::task::Task>,
	/// Owned rendered frames, keyed by frame number. A frame is consumed by
	/// `cpu_frame` or dropped when it falls out of the window.
	slots: BTreeMap<i64, PreviewSlot>,
}

/// A finished, backend-independent frame in the playback pre-render window.
type PreviewSlot = quercus_engine_rs::preview_video::VideoFrame;

// ---------------------------------------------------------------------------
// Playback audio prefetch (M15 S3; M16 S3: rendered on the audio thread)
// ---------------------------------------------------------------------------
//
// Real-time audio is pulled by the UI tick. Through the worker pool it had
// one IPC round trip and could wait behind a busy render worker, so the
// chunks are rendered AHEAD (tickets complete on the dispatcher's poll) and
// buffered here. M16 S3 renders each chunk on the dedicated audio-render
// thread (see oakui::audio_thread) — never on the UI tick — and delivers it
// into the channel asynchronously, so the prefetch window serves as the
// scheduling cushion: the pull never blocks on a worker. `AUDIO_PREFETCH_CHUNKS`
// ahead ≈ 64 ms of audio at 60 fps — enough to cover the delivery latency
// while keeping at most a handful of audio chunks in flight.

/// How many audio chunks are kept rendered ahead of the playhead (M15
/// S3; M16 S3: rendered on the audio thread, so the window is the
/// scheduling cushion). Each chunk is one sequence frame of audio;
/// 6 frames ahead ≈ 100 ms at 60 fps and ≈ 240 ms at 25 fps — the
/// output-device queue holds the whole window (chunks are pushed as soon
/// as rendered), so this is also how long a UI stall can last before the
/// device underruns.
const AUDIO_PREFETCH_CHUNKS: i64 = 6;

/// The playhead frame whose audio is playing at audio-clock time `secs`:
/// `start_frame + (secs - start_secs)` seconds at `rate`. The wall clock
/// anchors play() at the click instant, but the first chunk only reaches
/// the device ~50-150 ms later, so the anchor is taken at the first push —
/// `start_secs` is the output clock at that moment, NOT zero (on a replay
/// the device has already padded underruns onto `output_frames_consumed`).
/// The multiply is split into `num as f64 / den as f64` first so large
/// frame counts cannot overflow i64.
fn audio_target_frame(start_frame: i64, start_secs: f64, secs: f64, rate: FrameRate) -> i64 {
	start_frame + ((secs - start_secs) * rate.num as f64 / rate.den as f64).round() as i64
}

/// The playback-audio prefetch buffer: rendered chunks ordered by start
/// timestamp, plus the submission cursor. UI-thread-only (guarded by the
/// engine's `audio_prefetch` mutex so the engine stays `Sync`).
struct AudioPrefetch {
	/// Sequence frame of the first buffered chunk (or `next_submit` when
	/// the buffer is empty).
	front_ts: i64,
	/// Sequence frame of the next chunk to submit.
	next_submit: i64,
	/// Chunk length (sequence frames per tick) the buffer is built with.
	chunk: i64,
	/// Sequence frame of the chunk most recently served to the output
	/// device (None until the first pop). A playhead stalled between
	/// sequence frames (a 60 Hz tick at a 25 fps rate) presents the same
	/// frame again; [`AudioPrefetch::needs_reset`] distinguishes that from
	/// a genuine seek so the frame is not re-pushed.
	last_consumed: Option<i64>,
	/// Rendered chunks in start-ts order.
	buffered: VecDeque<(i64, super::renderops::RenderedAudio)>,
}

impl AudioPrefetch {
	/// An empty, uninitialized prefetch.
	fn new() -> Self {
		Self {
			front_ts: i64::MIN,
			next_submit: i64::MIN,
			chunk: 1,
			last_consumed: None,
			buffered: VecDeque::new(),
		}
	}

	/// True when the playhead jumped beyond what serving can follow — a
	/// genuine seek or restart. Serving runs AHEAD of the playhead by up
	/// to `AUDIO_PREFETCH_CHUNKS` chunks (rendered chunks are pushed to the
	/// output device immediately so its queue rides out UI stalls), so the
	/// served cursor normally leads the playhead; only a backward jump past
	/// that lead (loop wrap, scrub) or a forward jump past the submitted
	/// window means a real seek. The audio-master correction's small
	/// re-anchors (a frame or two) stay inside the tolerance.
	fn needs_reset(&self, frame: i64) -> bool {
		if self.front_ts == i64::MIN {
			return true;
		}
		let served = self.last_consumed.unwrap_or(self.front_ts - self.chunk);
		frame < served - (AUDIO_PREFETCH_CHUNKS + 2) || frame >= self.next_submit
	}

	/// Reset for a (re)start at `frame`: drop every buffered chunk and
	/// restart the submission cursor there (a seek or a new project).
	fn reset(&mut self, frame: i64, chunk: i64) {
		self.front_ts = frame;
		self.next_submit = frame;
		self.chunk = chunk.max(1);
		self.last_consumed = None;
		self.buffered.clear();
	}

	/// Insert a rendered chunk; stale arrivals (outside the submitted
	/// window — a seek raced the render) are dropped.
	fn insert(&mut self, ts: i64, data: super::renderops::RenderedAudio) {
		if ts < self.front_ts || ts >= self.next_submit {
			return;
		}
		if self.buffered.iter().any(|(t, _)| *t == ts) {
			return;
		}
		let pos = self
			.buffered
			.iter()
			.position(|(t, _)| *t > ts)
			.unwrap_or(self.buffered.len());
		self.buffered.insert(pos, (ts, data));
	}

	/// Serve every rendered chunk in start-ts order, returning the ones to
	/// push to the output device. Chunks whose sequence time is already
	/// behind the playhead arrived too late — they are DROPPED rather than
	/// played late, so an underrun/production gap costs content but never
	/// accumulates A/V desync. On-time and future chunks are pushed
	/// immediately (not one-per-tick): the output device's queue then
	/// holds up to the whole prefetch window, so a stalled UI tick can no
	/// longer starve the device into crackling.
	fn drain_ready(&mut self, playhead: i64) -> Vec<super::renderops::RenderedAudio> {
		let mut out = Vec::new();
		while let Some((ts, _)) = self.buffered.front() {
			let ts = *ts;
			let (_, data) = self.buffered.pop_front().unwrap();
			if ts >= playhead {
				audio_dbg(&format!(
					"push chunk {ts} (lead {} vs playhead {playhead})",
					ts - playhead
				));
				out.push(data);
			} else {
				audio_dbg(&format!(
					"chunk {ts} arrived late (playhead {playhead}) -> dropped"
				));
			}
			self.front_ts = ts + self.chunk;
			self.last_consumed = Some(ts);
		}
		out
	}
}

/// OAK_DEBUG_AUDIO-gated playback diagnostics: the events below (prefetch
/// resets, late-chunk drops, underrun heals) each produce an audible
/// discontinuity, so their timestamps correlate with pops/crackles.
pub(crate) fn audio_dbg_enabled() -> bool {
	static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
	*ENABLED.get_or_init(|| std::env::var_os("OAK_DEBUG_AUDIO").is_some())
}

pub(crate) fn audio_dbg(msg: &str) {
	if !audio_dbg_enabled() {
		return;
	}
	static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
	let ms = T0
		.get_or_init(std::time::Instant::now)
		.elapsed()
		.as_millis();
	eprintln!("[audio {ms:>8}ms] {msg}");
}

/// One background full-resolution render request (built on the UI thread
/// at schedule time; the worker thread owns it from there).
struct FullResRequest {
	/// The monitor the frame belongs to.
	monitor: Monitor,
	/// The playhead frame to render.
	frame: i64,
	/// The engine's full-res generation when the job was scheduled (stale
	/// completions are discarded by the drain).
	generation: u64,
	/// The project the target lives in (keeps the graph alive).
	project: ProjectRef,
	/// The sequence or footage node to render.
	target: FullResTarget,
	/// Output width (the sequence's native size).
	width: i32,
	/// Output height.
	height: i32,
	/// The sequence's timebase (frame duration = `tb.0 / tb.1` seconds).
	tb: (i64, i64),
}

/// A completed full-res frame, delivered through the completion channel.
struct FullResEvent {
	/// The monitor the frame belongs to.
	monitor: Monitor,
	/// The playhead frame that was rendered.
	frame: i64,
	/// The job's generation (see [`FullResRequest`]).
	generation: u64,
	/// The rendered viewer image.
	image: Arc<RenderImage>,
}

// ---------------------------------------------------------------------------
// Material-bin thumbnails
// ---------------------------------------------------------------------------
//
// The icon view shows a PNG of each footage's first frame. The PNGs are
// generated lazily on background threads (the render is a ticket wait and
// must not block the UI thread) into a shared directory keyed by the media
// filename; completions are drained on the app tick like full-res frames.

/// The thumbnail render size (twice the icon view's 72x48 slot).
const THUMBNAIL_WIDTH: i32 = 144;
/// The thumbnail render height (see [`THUMBNAIL_WIDTH`]).
const THUMBNAIL_HEIGHT: i32 = 96;

/// A completed thumbnail, delivered through the completion channel.
struct ThumbEvent {
	/// The footage node's stable identity (the bin entry's id).
	identity: u64,
	/// The job's generation (stale jobs are discarded by the drain).
	generation: u64,
	/// The PNG file on disk.
	path: PathBuf,
}

/// The thumbnail cache: finished PNG paths keyed by footage identity, plus
/// the identities with a generation job in flight (never re-scheduled).
#[derive(Default)]
struct ThumbnailState {
	/// Finished thumbnails.
	done: HashMap<u64, PathBuf>,
	/// Identities with a job in flight (or already attempted).
	pending: HashSet<u64>,
}

/// The project settings key of the OCIO config override (the 项目属性
/// color tab).
pub const PROJECT_SETTING_OCIO_CONFIG: &str = "ocioconfig";

/// The shared directory holding generated footage thumbnails.
fn thumbnail_dir() -> PathBuf {
	std::env::temp_dir().join("oak-thumbnails")
}

/// The PNG path of a footage's thumbnail: an FNV-1a hash of the media
/// filename, so the same file always hits the same cached PNG.
/// `cache_dir` is the project's cache-location override (the 项目属性
/// disk-cache setting); `None` uses the shared default directory.
fn thumbnail_path(cache_dir: Option<&str>, filename: &str) -> PathBuf {
	let mut h: u64 = 0xcbf29ce484222325;
	for b in filename.as_bytes() {
		h ^= u64::from(*b);
		h = h.wrapping_mul(0x100000001b3);
	}
	let dir = cache_dir
		.map(|d| PathBuf::from(d).join("thumbnails"))
		.unwrap_or_else(thumbnail_dir);
	dir.join(format!("{h:016x}.png"))
}

// ---------------------------------------------------------------------------
// Multicam angle frames (M15 S2)
// ---------------------------------------------------------------------------
//
// The Multicam panel draws one cell per angle (= the source sequence's
// track `i` at the playhead). The engine renders those frames on background
// threads — the same ticket path as the viewers, one single-track montage
// per angle — and caches them keyed by (multicam node, source) with an LRU
// cap, so a paused panel never re-renders a cell and playback refreshes
// cells round-robin (the panel throttles its requests; the engine only ever
// has one in-flight render per source).

/// The grid cell render size: the sequence's aspect scaled to a 320px long
/// edge (the panel grid cells are roughly this size; keeping the tickets
/// small bounds the 9-angle burst cost).
const MULTICAM_ANGLE_LONG_EDGE: u32 = 320;

/// A completed multicam angle frame, delivered through the completion
/// channel (drained on the app tick, like full-res frames). `None` image =
/// the render failed; the drain still clears the in-flight marker so the
/// cell can be retried on the next invalidation.
struct MulticamAngleEvent {
	/// The multicam node identity the frame belongs to.
	node_id: u64,
	/// The source index rendered.
	source: i32,
	/// The playhead frame the frame was rendered for.
	playhead: i64,
	/// The rendered display image (`None` when the render failed).
	image: Option<Arc<RenderImage>>,
}

/// One background multicam angle render request (UI-thread-built; the
/// worker thread owns it from there).
struct MulticamAngleRequest {
	/// The multicam node identity (the cache key's node half).
	node_id: u64,
	/// The source index.
	source: i32,
	/// The playhead frame to render.
	playhead: i64,
	/// The project (keeps the graph alive while the worker renders).
	project: ProjectRef,
	/// The source sequence node.
	seq: NodeId,
	/// The track whose clip makes up this angle.
	track: NodeId,
	/// Output width.
	width: i32,
	/// Output height.
	height: i32,
	/// The sequence's timebase.
	tb: (i64, i64),
}

/// The multicam angle-frame cache: rendered frames keyed by
/// `(multicam node, source)` with the playhead they were rendered for,
/// LRU-capped, plus the in-flight sources per node.
#[derive(Default)]
struct MulticamFrameCache {
	/// `(node_id, source) -> (rendered playhead, image)`, insertion-ordered
	/// (the LRU eviction drops the head).
	frames: Vec<((u64, i32), (i64, Arc<RenderImage>))>,
	/// `(node_id, source)` renders currently in flight (never re-scheduled).
	pending: HashSet<(u64, i32)>,
}

impl MulticamFrameCache {
	/// The cached image for `(node, source)` rendered at exactly `playhead`
	/// (a playhead change makes the frame stale).
	fn lookup(&self, node: u64, source: i32, playhead: i64) -> Option<Arc<RenderImage>> {
		self.frames
			.iter()
			.find(|(k, v)| k == &(node, source) && v.0 == playhead)
			.map(|(_, (_, img))| img.clone())
	}

	/// The most recent image for `(node, source)` regardless of playhead
	/// (the panel's stale-OK fallback during playback).
	fn last(&self, node: u64, source: i32) -> Option<Arc<RenderImage>> {
		self.frames
			.iter()
			.rev()
			.find(|(k, _)| k == &(node, source))
			.map(|(_, (_, img))| img.clone())
	}

	/// Store a freshly rendered frame, evicting the LRU head past the cap.
	fn insert(&mut self, node: u64, source: i32, playhead: i64, image: Arc<RenderImage>) {
		self.frames.retain(|(k, _)| k != &(node, source));
		self.frames.push(((node, source), (playhead, image)));
		const CAP: usize = 24;
		if self.frames.len() > CAP {
			let excess = self.frames.len() - CAP;
			self.frames.drain(0..excess);
		}
	}
}

// ---------------------------------------------------------------------------
// Frame conversion
// ---------------------------------------------------------------------------

/// Builds an owned viewer image from an SDK frame for a long-lived cache.
fn rendered_to_owned_image(rendered: &super::renderops::RenderedFrame) -> Option<Arc<RenderImage>> {
	let (image, _scope, samples) = rendered.to_display()?;
	if let Some(samples) = samples {
		super::gpu::register_display_frame(
			image.id.0,
			rendered.width().max(0) as u32,
			rendered.height().max(0) as u32,
			&samples,
		);
	}
	Some(Arc::new(image))
}

// ---------------------------------------------------------------------------
// Transport clock
// ---------------------------------------------------------------------------

/// The real engine's transport clock: the playhead plus the wall-clock
/// anchor while playing. Mirrors the mock's clock; the engine additionally
/// writes the program playhead back to the sequence.
pub struct RealClock {
	/// The transport state (play/pause, playhead, loop range).
	pub transport: TransportState,
	/// The clock's frame rate.
	pub rate: FrameRate,
	/// Wall-clock anchor `(started_at, anchored_frame)` while playing.
	started: Option<(Instant, Frame)>,
}

impl RealClock {
	/// A stopped clock at frame zero running at `rate`.
	pub fn new(rate: FrameRate) -> Self {
		Self {
			transport: TransportState::new(),
			rate,
			started: None,
		}
	}

	/// Starts playback from the current playhead.
	pub fn play(&mut self) {
		self.transport.play();
		self.started = Some((Instant::now(), self.transport.frame()));
	}

	/// Pauses playback, keeping the playhead.
	pub fn pause(&mut self) {
		self.transport.pause();
		self.started = None;
	}

	/// Re-anchors a PLAYING clock at `frame`: subsequent wall-clock ticks
	/// advance from here. A bare `transport.seek` while playing is
	/// overwritten by the next [`tick`](Self::tick) (it recomputes the
	/// playhead from the `started` anchor), so external clock followers
	/// (the audio-master correction) must re-anchor instead.
	pub fn reanchor_while_playing(&mut self, frame: Frame, length: Frame) {
		if self.started.is_none() {
			return;
		}
		self.transport.seek(frame, length);
		self.started = Some((Instant::now(), self.transport.frame()));
	}

	/// Advances the playhead from the wall clock while playing, looping at
	/// `length` — or pausing on the last frame when `stop_on_last` is set.
	/// No-op when stopped. The advance is clamped per tick: a
	/// long stall (the first render after pressing play, a disk stall)
	/// must not teleport the playhead past the pre-render window — the
	/// dropped time is re-anchored away instead (NLEs drop frames during
	/// stalls; they never jump the playhead over rendered content).
	pub fn tick(&mut self, length: Frame, stop_on_last: bool) {
		let Some((started, anchored)) = self.started else {
			return;
		};
		let elapsed = started.elapsed();
		let mut frame = anchored
			+ Frame(
				(elapsed.as_secs_f64() * self.rate.num as f64 / self.rate.den as f64).round()
					as i64,
			);
		// At 60 Hz ticks this allows up to 120 fps of advance — normal
		// playback rates are unaffected; only stall teleporting is clamped.
		const MAX_ADVANCE_PER_TICK: i64 = 2;
		let current = self.transport.frame();
		if frame.0 > current.0 + MAX_ADVANCE_PER_TICK {
			frame = Frame(current.0 + MAX_ADVANCE_PER_TICK);
			self.started = Some((Instant::now(), frame));
		}
		if length.0 > 0 && frame.0 >= length.0 {
			if stop_on_last {
				// Stop on the last frame and pause playback (the C++ viewer's
				// Stop on Last option): the playhead never wraps.
				self.transport.pause();
				self.started = None;
				frame = Frame(length.0 - 1);
				self.transport.seek(frame, length);
				return;
			}
			frame = Frame(frame.0 % length.0);
		}
		self.transport.seek(frame, length);
	}
}

impl PlaybackClock for RealClock {
	fn current_frame(&self) -> Frame {
		self.transport.frame()
	}

	fn is_playing(&self) -> bool {
		self.transport.is_playing()
	}

	fn frame_rate(&self) -> FrameRate {
		self.rate
	}
}

// ---------------------------------------------------------------------------
// Timeline model
// ---------------------------------------------------------------------------

/// A clip on the real timeline: the widget snapshot plus the graph
/// addressing (the block's `NodeId`; the widget-facing id is its stable
/// identity).
#[derive(Debug, Clone)]
pub struct RealClip {
	id: ClipId,
	range: FrameRange,
	media_in: Frame,
	label: SharedString,
	color: Hsla,
	/// The block node in the project graph.
	block: NodeId,
	/// The clip's texture chain contains a multicam node (the timeline
	/// overlay + the panel's detection read the same truth).
	multicam: bool,
	/// Duration in frames of the transition attached to the clip's head
	/// (the seam shared with the previous clip), if any.
	in_transition: Option<Frame>,
	/// Duration in frames of the transition attached to the clip's tail
	/// (the seam shared with the next clip), if any.
	out_transition: Option<Frame>,
}

impl ClipData for RealClip {
	fn id(&self) -> ClipId {
		self.id
	}

	fn range(&self) -> FrameRange {
		self.range
	}

	fn media_in(&self) -> Frame {
		self.media_in
	}

	fn label(&self) -> SharedString {
		self.label.clone()
	}

	fn color(&self) -> Option<Hsla> {
		Some(self.color)
	}

	fn is_multicam(&self) -> bool {
		self.multicam
	}

	fn in_transition(&self) -> Option<Frame> {
		self.in_transition
	}

	fn out_transition(&self) -> Option<Frame> {
		self.out_transition
	}
}

/// A running proxy transcode the engine polls on the tick loop. Completion
/// applies the footage's proxy fields and invalidates the rendered frames.
struct ProxyRun {
	/// Project-explorer/node identity of the footage being transcoded.
	footage: u64,
	/// The session that registered the task. It must live until completion so the
	/// engine can validate the footage/task pair and clear its active-task entry.
	session: EditorSession,
	/// Pollable engine task. Dropping it cancels and joins an unfinished worker.
	task: quercus_engine_rs::task::Task,
}

/// One card of the real effect stack: the chain node's identity, its
/// factory display name, its enabled flag, and the app-owned expansion
/// state ([`RealEngine::expanded_effects`]). No source/output cards: the
/// host clip node is the implicit output, the chain's unlinked upstream
/// input is the implicit source (the effect stack shows only the editable
/// middle).
#[derive(Debug, Clone)]
struct RealEffect {
	/// The node's stable identity (also the card's `EffectId`).
	id: EffectId,
	/// The factory display name of the node's type.
	title: SharedString,
	/// The node's `enabled_in` flag.
	enabled: bool,
	/// The app-owned expansion state (not undoable).
	expanded: bool,
	/// Optional secondary line (the "OpenFX" tag for plugin nodes).
	subtitle: Option<SharedString>,
	/// The persistent-message badge count (plugin nodes only; `None`
	/// otherwise).
	badge: Option<usize>,
}

impl EffectData for RealEffect {
	fn id(&self) -> EffectId {
		self.id
	}

	fn kind(&self) -> EffectCardKind {
		EffectCardKind::Effect
	}

	fn title(&self) -> SharedString {
		self.title.clone()
	}

	fn subtitle(&self) -> Option<SharedString> {
		self.subtitle.clone()
	}

	fn is_enabled(&self) -> bool {
		self.enabled
	}

	fn is_expanded(&self) -> bool {
		self.expanded
	}

	fn badge_count(&self) -> Option<usize> {
		self.badge
	}
}

/// A track on the real timeline (snapshot handed to the timeline widget).
#[derive(Debug, Clone)]
pub struct RealTrack {
	kind: TrackKind,
	name: SharedString,
	height: Pixels,
	locked: bool,
	muted: bool,
	solo: bool,
	visible: bool,
	clips: Vec<RealClip>,
	/// The track node in the project graph.
	track: NodeId,
	/// The track's per-type index (the facade's clip coordinates; edit
	/// commands address nodes directly now, but the drop policy and the
	/// widget's cross-track moves still speak display/per-type indices).
	track_index: usize,
}

impl TrackData for RealTrack {
	type Clip = RealClip;

	fn kind(&self) -> TrackKind {
		self.kind
	}

	fn name(&self) -> SharedString {
		self.name.clone()
	}

	fn is_locked(&self) -> bool {
		self.locked
	}

	fn is_muted(&self) -> bool {
		self.muted
	}

	fn is_solo(&self) -> bool {
		self.solo
	}

	fn is_visible(&self) -> bool {
		self.visible
	}

	fn height(&self) -> Pixels {
		self.height
	}

	fn clips(&self) -> &[Self::Clip] {
		&self.clips
	}
}

/// The color of an adjustment layer's timeline band
/// ([`RealClip::color`]): a fixed slate that sits outside the per-clip
/// palette, so a band that carries no media is recognizable at a glance.
const ADJUSTMENT_LAYER_COLOR: Hsla = Hsla {
	h: 0.72,
	s: 0.3,
	l: 0.58,
	a: 1.0,
};

/// The default label of an adjustment layer (the block itself stores no
/// label; the snapshot synthesizes one like it does for unnamed clips).
const ADJUSTMENT_LAYER_LABEL: &str = "调整图层";

/// A deterministic clip color from a stable per-clip index (the module
/// graph exposes no clip color).
fn clip_color(index: u64) -> Hsla {
	let hues = [0.55f32, 0.6, 0.08, 0.3, 0.78, 0.45, 0.9, 0.15];
	Hsla {
		h: hues[(index as usize) % hues.len()],
		s: 0.55,
		l: 0.45,
		a: 1.0,
	}
}

/// A marker color for a marker color index (a small palette around the
/// amber accent, so adjacent markers stay distinguishable).
fn marker_color(index: i32) -> Hsla {
	let hues = [0.10f32, 0.0, 0.55, 0.30, 0.78];
	let h = hues[(index.max(0) as usize) % hues.len()];
	Hsla {
		h,
		s: 0.75,
		l: 0.55,
		a: 1.0,
	}
}

/// The widget [`TrackKind`] of an SDK domain track kind.
fn track_kind_of(kind: DomainTrackKind) -> TrackKind {
	match kind {
		DomainTrackKind::Video => TrackKind::Video,
		DomainTrackKind::Audio => TrackKind::Audio,
		DomainTrackKind::Subtitle => TrackKind::Subtitle,
	}
}

/// The SDK domain track kind of a widget [`TrackKind`].
fn track_type_of(kind: TrackKind) -> DomainTrackKind {
	match kind {
		TrackKind::Video => DomainTrackKind::Video,
		TrackKind::Audio => DomainTrackKind::Audio,
		TrackKind::Subtitle => DomainTrackKind::Subtitle,
	}
}

/// A node in the real node graph (M12 P2: built from the current
/// sequence's graph by [`crate::oakui::nodegraph`]).
pub use crate::oakui::nodegraph::{RealEdge, RealNode, RealPort};

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// The real engine: the domain project/sequence plus the snapshot models
/// the widgets read.
pub struct RealEngine {
	/// The domain project (None before any project is open).
	project: Option<ProjectRef>,
	/// The live editor session: the project, current sequence, marker and
	/// work-area state and the write-through binding all live here.
	session: Option<EditorSession>,
	/// The current sequence node.
	sequence: Option<NodeId>,
	/// The gateway's cached project info.
	project_info: Project,
	/// The gateway's cached sequence info.
	sequence_info: Option<Sequence>,
	/// The source monitor's clock.
	pub source_clock: Entity<RealClock>,
	/// The program monitor's clock.
	pub program_clock: Entity<RealClock>,
	/// The timeline snapshot (rebuilt on open/edit).
	tracks: Vec<RealTrack>,
	/// Timeline waveform cache (M12 P4), created lazily at the current
	/// frame rate.
	waveforms: Mutex<Option<Arc<crate::oakui::waveform::WaveformCache>>>,
	/// The selected material-bin entry (a node identity).
	selected_item: Option<u64>,
	/// The single selected timeline clip — the effect stack's target
	/// (`None` for an empty or multi-clip selection, or before any
	/// selection event). Also drives the node graph's scope: while a single
	/// clip is selected, the editor shows that clip's context chain only.
	selected_clip: Option<ClipId>,
	/// The whole timeline selection (the effect stack only ever sees
	/// [`Self::selected_clip`]; the multi-clip set is what a group drag
	/// moves together).
	selected_clips: Vec<ClipId>,
	/// The node-graph selection mirror (`None` = no single node selected):
	/// set when the user clicks a node in the node editor (or an effect
	/// card in the inspector) so the inspector and the node graph share one
	/// selection. A timeline clip selection also writes its block node here.
	selected_graph_node: Option<u64>,
	/// The engine clipboard for Cut/Copy/Paste (graphops::ClipboardClip
	/// entries in timeline order).
	clipboard: Vec<graphops::ClipboardClip>,
	/// Node identities whose effect cards are expanded (view state; kept
	/// here because `EffectData` is a pure read and expansion is not
	/// undoable).
	expanded_effects: BTreeSet<u64>,
	/// Whether the program monitor is playing (mirrors the clock; kept here
	/// because the audio-meter data source has no `App` to read the clock).
	program_playing: bool,
	/// Phase counter driving the (silent) audio levels.
	meter_phase: u32,
	/// Cache of the CPU frames handed to the viewers, keyed by monitor.
	/// Each monitor holds its proxy frame (rendered by an SDK worker, with
	/// the scope samples analyzed from the same owned pixels) plus the
	/// full-resolution fill the background worker lands when the playhead
	/// rests (see [`MonitorFrameCache`]). The synthetic pattern is only a
	/// cold-cache placeholder while the first request is in flight.
	cpu_frame_cache: Mutex<HashMap<Monitor, MonitorFrameCache>>,
	/// The display-color transform generation the frame cache was built
	/// against (a change drops every cached image — they were produced
	/// with the stale transform).
	display_color_gen: std::cell::Cell<u64>,
	/// Bumped whenever the rendered content can change underneath an
	/// in-flight background full-res job (an edit, a selection change or a
	/// project drop); completions tagged with a stale generation are
	/// discarded by the drain.
	full_res_generation: u64,
	/// M15 S2 playback pre-render state, keyed by monitor (see
	/// [`PreviewWindow`]). An `Arc` so the ticket completions (which fire
	/// from the dispatcher's poll on the UI thread) can reach the cache;
	/// the mutex keeps the engine `Sync`.
	preview_windows: Arc<Mutex<HashMap<Monitor, PreviewWindow>>>,
	/// Bumped whenever the preview content can change (edit / proxy
	/// toggle / selection / project drop); the pre-render window rebuilds
	/// against the new generation.
	preview_generation: u64,
	/// The channel background full-res jobs report finished frames through;
	/// drained on the app tick. The mutex keeps the engine `Sync` (the
	/// channel is only ever touched on the UI thread).
	full_res_rx: Mutex<mpsc::Receiver<FullResEvent>>,
	/// The sending half of `full_res_rx` (cloned into every job).
	full_res_tx: Mutex<mpsc::Sender<FullResEvent>>,
	/// Material-bin thumbnail cache (finished PNGs + in-flight jobs).
	thumbnails: Mutex<ThumbnailState>,
	/// Generation counter invalidating in-flight thumbnail jobs on project
	/// drop (same semantics as `full_res_generation`).
	thumb_generation: u64,
	/// The channel background thumbnail jobs report finished PNGs through;
	/// drained on the app tick. The mutex keeps the engine `Sync`.
	thumb_rx: Mutex<mpsc::Receiver<ThumbEvent>>,
	/// The sending half of `thumb_rx` (cloned into every job).
	thumb_tx: Mutex<mpsc::Sender<ThumbEvent>>,
	proxy_runs: Vec<ProxyRun>,
	/// 待启动的代理生成队列（entry id）：自动代理不并发轰炸系统——
	/// 同时只跑 [`PROXY_MAX_CONCURRENT`] 个转码，完成后从队列递补。
	proxy_queue: Vec<u64>,
	/// Last waveform-cache version seen by the tick (background waveform
	/// extractions bump it on insert → repaint the timeline).
	waveform_version_seen: u64,
	/// The multicam angle-frame cache (rendered grid cells keyed by
	/// (multicam node, source), LRU-capped). An `Arc` so the background
	/// angle workers' completions can reach it; the mutex keeps the engine
	/// `Sync`.
	multicam_frames: Arc<Mutex<MulticamFrameCache>>,
	/// The channel background multicam angle workers report finished frames
	/// through; drained on the app tick.
	multicam_rx: Mutex<mpsc::Receiver<MulticamAngleEvent>>,
	/// The sending half of `multicam_rx` (cloned into every worker).
	multicam_tx: Mutex<mpsc::Sender<MulticamAngleEvent>>,
	/// M15 S3: async playback-audio prefetch — the process dispatcher
	/// renders audio chunks ahead (completions arrive on the UI tick's
	/// poll); the channel delivers `(start_ts, samples)` and the prefetch
	/// state reorders them for the real-time pull.
	audio_rx: Mutex<mpsc::Receiver<(i64, super::renderops::RenderedAudio)>>,
	/// The sending half of `audio_rx` (cloned into every audio ticket).
	audio_tx: Mutex<mpsc::Sender<(i64, super::renderops::RenderedAudio)>>,
	/// The audio prefetch buffer (see [`AudioPrefetch`]).
	audio_prefetch: Mutex<AudioPrefetch>,
	/// Audio-mastered playback anchor `(playhead_frame, output_clock_secs)`
	/// recorded at the first chunk push of a play run (M12 P1a). While set,
	/// the tick loop re-anchors the program playhead onto the frame the
	/// output device is actually consuming (`manager.seconds()`), fixing
	/// the fixed startup offset and letting playback catch up after
	/// underruns. Cleared on pause / seek / stop; `None` falls back to the
	/// wall clock.
	audio_playback: Option<(i64, f64)>,
	/// Cooldown for the underrun self-heal resync in `pull_audio_tick`
	/// (without it a resync retriggers every tick while the fresh chunks
	/// are still rendering).
	last_audio_resync: Option<std::time::Instant>,
	/// Debounced graph-snapshot upload: the last undo revision the workers
	/// were told about, and the time the latest push was requested. Rapid
	/// edits (multicam switching) coalesce into ONE serialization + worker
	/// load instead of blocking the UI tick on every edit (the "switch
	/// several times and everything grinds to a halt" report).
	pending_snapshot: Option<u64>,
	last_snapshot_push: Option<std::time::Instant>,
}

impl RealEngine {
	/// Render one tick's worth of audio at the program playhead and queue
	/// it for playback (each chunk is mixed on an SDK engine worker — see
	/// [`renderops::submit_audio_chunk`](super::renderops::submit_audio_chunk)
	/// — and delivered asynchronously into the channel, drained below, so
	/// the render never runs on the UI tick). The chunks are submitted AHEAD
	/// of the playhead and buffered by [`AudioPrefetch`], so the real-time
	/// pull never blocks on a render worker or an IPC round trip. Failures
	/// degrade to silence; when the audio thread is down the channel stays
	/// empty and playback continues video-only.
	fn pull_audio_tick(&mut self, cx: &mut Context<Self>) {
		let (Some(_), Some(seq)) = (self.project.clone(), self.sequence) else {
			return;
		};
		let Some(tb) = self.time_base() else {
			return;
		};
		let Some(session) = self.editor_session() else {
			return;
		};
		let frame = self.clock_frame(Monitor::Program, cx).0;
		if frame < 0 {
			return;
		}
		// One sequence frame of audio per chunk, keyed by the playhead
		// frame. A per-tick wall-clock heuristic (~1/60 s) truncates to zero
		// frames for sub-60 fps sequences, so render exactly one frame per
		// playhead frame instead — the output device consumes one frame's
		// audio per frame advance regardless of the rate.
		let chunk: i64 = 1;

		let mut st = self
			.audio_prefetch
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		// Reset on seek / restart (see needs_reset): serving runs ahead of
		// the playhead, so only genuine jumps re-arm the buffer.
		if st.needs_reset(frame) {
			audio_dbg(&format!(
				"prefetch reset at frame {frame} (seek/restart; next_submit was {})",
				st.next_submit
			));
			st.reset(frame, chunk);
		}
		// Submit chunks to cover [next_submit, frame + PREFETCH ahead). In
		// steady state the window moves by one chunk per tick, so exactly
		// one new ticket is posted; the rest are already buffered.
		let tx = self
			.audio_tx
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.clone();
		let target = frame + chunk * AUDIO_PREFETCH_CHUNKS;
		let mut submitted = 0i32;
		while st.next_submit < target {
			let ts = st.next_submit;
			if super::renderops::submit_audio_chunk(&session, seq, ts, chunk, tb, tx.clone())
				.is_err()
			{
				break;
			}
			st.next_submit += chunk;
			submitted += 1;
		}
		// Drain completed chunks. With the M16 S3 audio thread the submit
		// above only queues; the render lands here asynchronously, so the
		// drain is the sole delivery path.
		let rx = self.audio_rx.lock().unwrap_or_else(|e| e.into_inner());
		while let Ok((ts, data)) = rx.try_recv() {
			st.insert(ts, data);
		}
		drop(rx);
		// Underrun self-heal: zero-padding advanced the output clock past
		// the queued content, so everything audible from here on would
		// play late (the classic "audio drifts behind video"). Resync by
		// trimming just the stale lead from the device queue — the rest
		// stays audible and the master-clock anchor stays valid (the
		// clock already advanced past the zero-fill). The old full
		// clear + prefetch reset + re-anchor skipped ~190 ms of unheard
		// content and re-rendered already-played audio whenever the
		// playhead lagged (audible as a pop plus a repeated phrase), and
		// the reset that followed always dropped its first re-rendered
		// chunk as "late" — a ~1.1 s periodic skip/repeat cycle.
		if self.audio_playback.is_some() {
			let cooldown_ok = self
				.last_audio_resync
				.map(|t| t.elapsed() > std::time::Duration::from_secs(1))
				.unwrap_or(true);
			let underrun = quercus_engine_rs::audio::take_output_underrun_frames().unwrap_or(0);
			if underrun > 0 {
				let q = quercus_engine_rs::audio::output_queued_frames().unwrap_or(-1);
				audio_dbg(&format!("underrun +{underrun} frames (device queue {q})"));
			}
			if cooldown_ok && underrun > 480 {
				let dropped = quercus_engine_rs::audio::drop_output_frames(underrun).unwrap_or(0);
				audio_dbg(&format!(
					"underrun heal: trimmed {dropped} stale frames (underrun {underrun}) at frame {frame}"
				));
				self.last_audio_resync = Some(std::time::Instant::now());
			}
		}
		// Push every rendered chunk to the output device in ts order (the
		// device queue is the cushion that rides out UI stalls); chunks
		// whose time already passed are dropped inside drain_ready.
		let mut pushed = 0i32;
		for buf in st.drain_ready(frame) {
			if buf.sample_rate <= 0 || buf.channel_count <= 0 || buf.data.is_empty() {
				continue;
			}
			// Packed F32; layout: 1ch → mono mask, else stereo.
			let layout: u64 = if buf.channel_count == 1 { 0x4 } else { 0x3 };
			let bytes: Vec<u8> = buf.data.iter().flat_map(|v| v.to_ne_bytes()).collect();
			let _ = quercus_engine_rs::audio::push_to_output(
				quercus_engine_rs::audio::AudioParams::new(
					buf.sample_rate,
					layout,
					quercus_engine_rs::audio::SampleFormat::F32,
				),
				&bytes,
			);
			pushed += 1;
			// M12 P1a: anchor the audio master clock at the first push of a
			// play run. Recording the CURRENT output clock (not 0) keeps the
			// anchor honest on a replay — the device has been padding
			// underruns onto `output_frames_consumed` since reset, so the
			// correction starts from the frame actually audible right now
			// instead of teleporting the playhead back to the push position.
			if self.audio_playback.is_none() {
				if let Some(secs) = quercus_engine_rs::audio::seconds() {
					if secs >= 0.0 {
						self.audio_playback = Some((frame, secs));
						// Zeros padded before this first push (stream
						// startup, or the whole paused interval) are
						// not a playback underrun — reset the counter
						// so the self-heal doesn't fire spuriously.
						let _ = quercus_engine_rs::audio::take_output_underrun_frames();
					}
				}
			}
		}
		if audio_dbg_enabled() {
			let secs = quercus_engine_rs::audio::seconds().unwrap_or(-1.0);
			let q = quercus_engine_rs::audio::output_queued_frames().unwrap_or(-1);
			audio_dbg(&format!(
				"tick f={frame} secs={secs:.3} buf={} sub={submitted} push={pushed} q={q}",
				st.buffered.len()
			));
		}
	}

	/// Stop program playback: clear the play flag and silence the audio
	/// output (drop the queued samples and re-anchor the output clock so
	/// a later play() starts clean). Shared by [`AppEngine::pause`] and
	/// the Stop-on-Last auto-stop — a clock that stopped itself leaves
	/// `program_playing` set, and without this the last frame's audio
	/// re-pushes forever.
	fn stop_program_playback(&mut self, cx: &mut Context<Self>) {
		self.program_playing = false;
		self.audio_playback = None;
		let _ = quercus_engine_rs::audio::stop_output();
		let _ = quercus_engine_rs::audio::reset_output_clock();
		// Reset the prefetch at the resting playhead so a later play()
		// re-renders it: `last_consumed` would otherwise suppress the
		// re-push of the very frame still on screen.
		let frame = self.program_clock.read(cx).transport.frame().0;
		let mut st = self
			.audio_prefetch
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		st.reset(frame, 1);
	}

	/// Builds an engine with no project open.
	pub fn new(cx: &mut Context<Self>) -> Self {
		// Text nodes reach their layout/rasterizer through the process-global
		// hooks of the engine's text backend (it links no font or
		// shaping crate); fill them in before anything can run a node.
		super::textengine::install();
		let rate = VideoFormat::hd_1080p25().rate;
		let (full_res_tx, full_res_rx) = mpsc::channel::<FullResEvent>();
		let (thumb_tx, thumb_rx) = mpsc::channel::<ThumbEvent>();
		let (multicam_tx, multicam_rx) = mpsc::channel::<MulticamAngleEvent>();
		let (audio_tx, audio_rx) = mpsc::channel::<(i64, super::renderops::RenderedAudio)>();
		Self {
			project: None,
			session: None,
			sequence: None,
			project_info: Project {
				name: UNTITLED.into(),
				path: PathBuf::new(),
			},
			sequence_info: None,
			source_clock: cx.new(|_cx| RealClock::new(rate)),
			program_clock: cx.new(|_cx| RealClock::new(rate)),
			tracks: Vec::new(),
			waveforms: Mutex::new(None),
			selected_item: None,
			selected_clip: None,
			selected_clips: Vec::new(),
			selected_graph_node: None,
			clipboard: Vec::new(),
			expanded_effects: BTreeSet::new(),
			program_playing: false,
			meter_phase: 0,
			cpu_frame_cache: Mutex::new(HashMap::new()),
			display_color_gen: std::cell::Cell::new(0),
			full_res_generation: 0,
			preview_windows: Arc::new(Mutex::new(HashMap::new())),
			preview_generation: 0,
			full_res_rx: Mutex::new(full_res_rx),
			full_res_tx: Mutex::new(full_res_tx),
			thumbnails: Mutex::new(ThumbnailState::default()),
			thumb_generation: 0,
			thumb_rx: Mutex::new(thumb_rx),
			thumb_tx: Mutex::new(thumb_tx),
			proxy_runs: Vec::new(),
			proxy_queue: Vec::new(),
			waveform_version_seen: 0,
			multicam_frames: Arc::new(Mutex::new(MulticamFrameCache::default())),
			multicam_rx: Mutex::new(multicam_rx),
			multicam_tx: Mutex::new(multicam_tx),
			audio_rx: Mutex::new(audio_rx),
			audio_tx: Mutex::new(audio_tx),
			audio_prefetch: Mutex::new(AudioPrefetch::new()),
			audio_playback: None,
			last_audio_resync: None,
			pending_snapshot: None,
			last_snapshot_push: None,
		}
	}

	/// Resolves the clock entity for a monitor.
	fn clock(&self, monitor: Monitor) -> &Entity<RealClock> {
		match monitor {
			Monitor::Source => &self.source_clock,
			Monitor::Program => &self.program_clock,
		}
	}

	/// The open project reference, if any.
	fn project_ref(&self) -> Option<&ProjectRef> {
		self.project.as_ref()
	}

	/// Creates a migration session over the exact same project Arc. The storage
	/// handle owns one reference and is consumed by the session constructor, so
	/// graph/effect SDK calls never reload or fork the current document.
	fn editor_session(&self) -> Option<EditorSession> {
		self.session.clone()
	}

	/// The sequence's timebase `(rate_den, rate_num)`, if a sequence with
	/// a valid frame rate is open.
	fn time_base(&self) -> Option<(i64, i64)> {
		let project = self.project_ref()?;
		graphops::sequence_time_base(project, self.sequence?)
	}

	/// The selected footage's duration in frames at the current rate
	/// (0 when nothing is selected or the footage was not probed).
	fn source_length(&self) -> Frame {
		let (Some(project), Some(identity)) = (self.project_ref(), self.selected_item) else {
			return Frame(0);
		};
		let Some(id) = graphops::id_of(identity) else {
			return Frame(0);
		};
		let Some(seconds) = graphops::footage_duration_seconds(project, id) else {
			return Frame(0);
		};
		let rate = self.frame_rate();
		let fps = rate.num as f64 / rate.den.max(1) as f64;
		Frame((seconds * fps).round().max(1.0) as i64)
	}

	/// Current sequence length (0 without a sequence).
	fn sequence_length(&self) -> Frame {
		self.sequence_info
			.as_ref()
			.map(|s| s.length)
			.unwrap_or(Frame(0))
	}

	/// Mirrors the program playhead into the sequence (best effort).
	fn mirror_program_playhead(&self, cx: &App) {
		let (Some(project), Some(seq)) = (self.project_ref(), self.sequence) else {
			return;
		};
		let Some(tb) = self.time_base() else {
			return;
		};
		let frame = self.program_clock.read(cx).transport.frame().0;
		graphops::sequence_set_playhead(project, seq, graphops::ts_to_rational(frame, tb));
	}

	/// The proxy resolution the viewer renders at: the sequence's aspect
	/// scaled to a small long edge. SDK workers render both this proxy and
	/// the divider-scaled resting fill off the UI thread.
	fn proxy_render_size(&self) -> Option<(i32, i32)> {
		let info = self.sequence_info.as_ref()?;
		let (w, h) = (info.format.width.max(1), info.format.height.max(1));
		// The playback resolution divider (the C++ viewer `Playback
		// Resolution ▸` menu): render the preview at 480/divider long edge
		// so slower machines (or debug builds) can still play in real time.
		let divider = self.playback_divider().max(1) as u32;
		let max_long_edge = 480 / divider;
		let scale = max_long_edge as f64 / w.max(h) as f64;
		let width = ((w as f64 * scale).round() as u32).max(2);
		let height = ((h as f64 * scale).round() as u32).max(2);
		Some((width as i32, height as i32))
	}

	/// The selected entry's footage node (M12 P3: entry ids are the
	/// nodes' stable identities), or `None` when the selection is a folder
	/// or absent.
	fn selected_footage_node(&self) -> Option<NodeId> {
		let project = self.project_ref()?;
		let id = graphops::id_of(self.selected_item?)?;
		(project.node_kind(id) == Some(NodeKind::Footage)).then_some(id)
	}

	/// Builds the background full-res job for `monitor` at `frame` (the
	/// program monitor's sequence or the source monitor's selected footage
	/// node) at the sequence's native size scaled by the playback
	/// resolution divider (a paused Half/Quarter/Eighth preview must not
	/// be overwritten by a native-size background render). Returns None
	/// when there is nothing to render (no sequence open, no footage
	/// selected).
	fn build_full_res_request(&self, monitor: Monitor, frame: i64) -> Option<FullResRequest> {
		let info = self.sequence_info.as_ref()?;
		let target = match monitor {
			Monitor::Program => FullResTarget::Sequence(self.sequence?),
			Monitor::Source => FullResTarget::Footage(self.selected_footage_node()?),
		};
		let (width, height) = super::renderops::full_res_render_size(
			info.format.width.max(1),
			info.format.height.max(1),
			self.playback_divider().max(1) as u32,
		);
		Some(FullResRequest {
			monitor,
			frame,
			generation: self.full_res_generation,
			project: self.project_ref()?.clone(),
			target,
			width,
			height,
			tb: self.time_base()?,
		})
	}

	/// Runs one background full-resolution render (the worker thread of
	/// the full-res path) and reports the finished frame through `tx`. The
	/// request owns a project `Arc`, so the render stays valid even when
	/// the engine's project is dropped mid-flight (the drain discards the
	/// stale completion).
	fn full_res_worker(request: FullResRequest, tx: mpsc::Sender<FullResEvent>) {
		let FullResRequest {
			monitor,
			frame,
			generation,
			project,
			target,
			width,
			height,
			tb,
		} = request;
		let mut event = None;
		if super::renderops::ensure_render_manager() {
			let rendered = match target {
				FullResTarget::Sequence(seq) => super::renderops::render_sequence_frame(
					&project,
					seq,
					frame,
					tb,
					width,
					height,
					Some(quercus_engine_rs::types::PixelFormat::F32),
				),
				FullResTarget::Footage(node) => super::renderops::render_footage_frame(
					&project,
					node,
					frame,
					tb,
					width,
					height,
					Some(quercus_engine_rs::types::PixelFormat::F32),
				),
			};
			if let Ok(rendered) = rendered {
				// The SDK owns and releases backend resources; the app only
				// converts the owned pixels for its long-lived display cache.
				let image = rendered_to_owned_image(&rendered);
				if let Some(image) = image {
					event = Some(FullResEvent {
						monitor,
						frame,
						generation,
						image,
					});
				}
			}
		}
		if let Some(event) = event {
			let _ = tx.send(event);
		}
	}

	/// Schedules a background full-resolution render for `monitor`'s current
	/// playhead when the policy says so: the playhead is resting, the frame
	/// is not already cached full-res, and no job is in flight for this
	/// monitor (M12 P5a). The job runs on its own thread, so the UI thread
	/// never blocks.
	fn schedule_full_res(&mut self, monitor: Monitor, cx: &mut Context<Self>) {
		let frame = self.clock_frame(monitor, cx).0;
		if frame < 0 {
			return;
		}
		let clock = self.clock(monitor).clone();
		let playing = clock.read(cx).transport.is_playing();
		let schedule = {
			let cache = self.cpu_frame_cache.lock().unwrap();
			cache
				.get(&monitor)
				.is_none_or(|entry| entry.needs_full_res(frame, playing))
		};
		if !schedule {
			return;
		}
		let Some(request) = self.build_full_res_request(monitor, frame) else {
			return;
		};
		self.cpu_frame_cache
			.lock()
			.unwrap()
			.entry(monitor)
			.or_default()
			.pending = Some((frame, request.generation));
		let tx = self.full_res_tx.lock().unwrap().clone();
		std::thread::spawn(move || Self::full_res_worker(request, tx));
	}

	/// Installs completed full-res frames into the cache, discarding stale
	/// completions (a job that outlived an edit, a selection change or a
	/// project drop — its generation no longer matches the pending job).
	fn drain_full_res(&mut self) {
		let mut cache = self.cpu_frame_cache.lock().unwrap();
		let rx = self.full_res_rx.lock().unwrap();
		while let Ok(event) = rx.try_recv() {
			cache.entry(event.monitor).or_default().install_full_res(
				event.frame,
				event.generation,
				event.image,
			);
		}
	}

	// -----------------------------------------------------------------------
	// Multi-camera (the Multicam panel grid + the timeline Multi-Cam menu)
	// -----------------------------------------------------------------------

	/// The program playhead as a sequence-frame timestamp (the angle render
	/// time; 0 without a sequence). The sequence's stored playhead is
	/// mirrored from the program clock on every seek/tick.
	fn program_playhead_ts(&self) -> i64 {
		let Some(project) = self.project_ref() else {
			return 0;
		};
		let Some(seq) = self.sequence else { return 0 };
		let Some(tb) = self.time_base() else { return 0 };
		let time = graphops::sequence_playhead(project, seq);
		graphops::rational_to_ts(time, tb)
	}

	/// The multicam state the panel displays (the C++ viewer's
	/// `detect_multicam_node`): the selected clip's multicam, falling back
	/// to the clip under the program playhead on the video tracks. The
	/// detection runs on demand, so the panel always reads a fresh answer;
	/// the node-graph-selection level of the C++ is not ported (the Rust
	/// node editor has no multicam selection).
	fn multicam_state_internal(&self) -> Option<MulticamState> {
		let session = self.editor_session()?;
		let sequence = quercus_engine_rs::node::NodeId::from_raw(self.sequence?.raw());
		if let Some(clip) = self.selected_clip_node() {
			if let Some(state) = session.multicam_state(quercus_engine_rs::node::NodeId::from_raw(clip.raw())) {
				return Some(multicam_state_from_sdk(state));
			}
		}
		let time = session.project().sequence_playhead(sequence);
		let clip = session.multicam_clip_at(sequence, time)?;
		session.multicam_state(clip).map(multicam_state_from_sdk)
	}

	/// Renders one multicam angle on a background thread (the same ticket
	/// path as the viewers, one single-track montage per angle) and reports
	/// the finished frame through `tx`.
	fn multicam_angle_worker(request: MulticamAngleRequest, tx: mpsc::Sender<MulticamAngleEvent>) {
		let MulticamAngleRequest {
			node_id,
			source,
			playhead,
			project,
			seq,
			track,
			width,
			height,
			tb,
		} = request;
		let mut image = None;
		if super::renderops::ensure_render_manager() {
			if let Ok(rendered) = super::renderops::render_multicam_angle_frame(
				&project, seq, track, playhead, tb, width, height,
			) {
				image = rendered_to_owned_image(&rendered);
			}
		}
		// Always report (also on failure) so the in-flight marker clears.
		let _ = tx.send(MulticamAngleEvent {
			node_id,
			source,
			playhead,
			image,
		});
	}

	/// The engine's [`AppEngine::multicam_angle_frame`]: returns the cached
	/// angle frame for the current playhead when present, otherwise
	/// schedules a background render (deduplicated per source) and returns
	/// `None`. The panel shows its last image until the frame lands.
	fn multicam_angle_frame_internal(&mut self, source: i32) -> Option<Arc<RenderImage>> {
		let Some(state) = self.multicam_state_internal() else {
			return None;
		};
		if source < 0 || source >= state.source_count {
			return None;
		}
		let Some(project) = self.project.clone() else {
			return None;
		};
		let Some(seq) = self.sequence else {
			return None;
		};
		let Some(tb) = self.time_base() else {
			return None;
		};
		let playhead = self.program_playhead_ts();
		// Exact-playhead cache hit.
		if let Some(img) =
			self.multicam_frames
				.lock()
				.unwrap()
				.lookup(state.node_id, source, playhead)
		{
			return Some(img);
		}
		let Some(session) = self.editor_session() else {
			return None;
		};
		let Some(track) = session
			.multicam_source_track(
				quercus_engine_rs::node::NodeId::from_raw(state.node_id),
				source,
			)
			.and_then(|track| graphops::id_of(track.raw()))
		else {
			return None;
		};
		// The panel may outlive a stale node (a multicam removed under it):
		// treat a node mismatch as a fresh cache.
		let mut cache = self.multicam_frames.lock().unwrap();
		if cache.pending.contains(&(state.node_id, source)) {
			return cache.last(state.node_id, source);
		}
		let (width, height) = {
			let info = self.sequence_info.as_ref()?;
			let (w, h) = (info.format.width.max(1), info.format.height.max(1));
			let scale = MULTICAM_ANGLE_LONG_EDGE as f64 / w.max(h) as f64;
			(
				((w as f64 * scale).round() as u32).max(2) as i32,
				((h as f64 * scale).round() as u32).max(2) as i32,
			)
		};
		cache.pending.insert((state.node_id, source));
		let request = MulticamAngleRequest {
			node_id: state.node_id,
			source,
			playhead,
			project,
			seq,
			track,
			width,
			height,
			tb,
		};
		let tx = self.multicam_tx.lock().unwrap().clone();
		std::thread::spawn(move || Self::multicam_angle_worker(request, tx));
		cache.last(state.node_id, source)
	}

	/// Installs completed multicam angle frames into the cache and repaints
	/// (the panel re-reads the fresh cell images on the next render).
	fn drain_multicam_frames(&mut self, cx: &mut Context<Self>) {
		let rx = self.multicam_rx.lock().unwrap();
		let mut any = false;
		while let Ok(event) = rx.try_recv() {
			any = true;
			let mut cache = self.multicam_frames.lock().unwrap();
			cache.pending.remove(&(event.node_id, event.source));
			if let Some(image) = event.image {
				cache.insert(event.node_id, event.source, event.playhead, image);
			}
		}
		if any {
			cx.notify();
		}
	}

	/// Clears the multicam angle cache (project drop / edit invalidation).
	fn clear_multicam_frames(&mut self) {
		self.multicam_frames.lock().unwrap().frames.clear();
		self.multicam_frames.lock().unwrap().pending.clear();
	}

	// -----------------------------------------------------------------------
	// M15 S2: playback pre-render window
	// -----------------------------------------------------------------------

	/// Feeds the playback pre-render window through Playback-priority SDK
	/// tasks. Paused/cold monitors request only the current proxy frame.
	fn update_preview_window(&mut self, monitor: Monitor, cx: &mut Context<Self>) {
		// Playback fills the forward window; a paused/cold viewer requests only
		// its current proxy frame. Both paths poll SDK tasks without waiting.
		let playing = self.clock(monitor).read(cx).transport.is_playing();
		let Some(project) = self.project.clone() else {
			return;
		};
		let Some(tb) = self.time_base() else { return };
		let Some((width, height)) = self.proxy_render_size() else {
			return;
		};
		let Some(node) = (match monitor {
			Monitor::Program => self.sequence,
			Monitor::Source => self.selected_footage_node(),
		}) else {
			return;
		};
		let node_id = node.raw();
		let playhead = self.clock_frame(monitor, cx).0;
		if playhead < 0 {
			return;
		}
		let length = match monitor {
			Monitor::Program => self.sequence_length().0,
			Monitor::Source => self.source_length().0,
		};
		let forward = config_get_int(CONFIG_KEY_PREVIEW_WINDOW, DEFAULT_PREVIEW_WINDOW_FORWARD)
			.clamp(8, 1200);
		let end = if playing {
			length.max(playhead).min(playhead + forward)
		} else {
			playhead + 1
		};

		// Reset / rebuild when the node changed or an invalidation bumped the
		// generation (the old pending/claimed requests are cancelled).
		let mut windows = self
			.preview_windows
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let window = windows.entry(monitor).or_default();
		let mut stale_slots: Vec<PreviewSlot> = Vec::new();
		if window.sequence != node_id || window.generation != self.preview_generation {
			for (_, task) in std::mem::take(&mut window.requests) {
				task.cancel();
				window.retired.push(task);
			}
			stale_slots.extend(std::mem::take(&mut window.slots).into_values());
			window.submitted.clear();
			window.sequence = node_id;
			window.generation = self.preview_generation;
		}
		// Release slots that fell behind the playhead (frames passed without
		// being displayed); the just-displayed frame was already consumed by
		// `cpu_frame`. Prune the submitted set the same way.
		let keep_from = (playhead - 2).max(0);
		let stale: Vec<i64> = window
			.slots
			.keys()
			.copied()
			.filter(|f| *f < keep_from)
			.collect();
		for f in stale {
			if let Some(slot) = window.slots.remove(&f) {
				stale_slots.push(slot);
			}
		}
		window
			.submitted
			.retain(|f| *f >= keep_from || (*f >= playhead && *f < end));
		// Cancel in-flight/pending frames the playhead has already passed:
		// when they complete they can never be displayed, but they still
		// occupy worker time. Dropping them keeps the workers on frames
		// around the playhead, so a stall-deferred window converges back
		// instead of grinding through ancient history forever (the
		// "picture frozen while the playhead moves" regression).
		let stale_pending: Vec<i64> = window
			.submitted
			.iter()
			.copied()
			.filter(|f| *f < keep_from && !window.slots.contains_key(f))
			.collect();
		for f in stale_pending {
			if let Some(task) = window.requests.remove(&f) {
				task.cancel();
				window.retired.push(task);
			}
			window.submitted.remove(&f);
		}
		// Reap only terminal tasks. Dropping a running SDK Task joins its worker,
		// so cancelled requests stay in `retired` until a later tick.
		window
			.retired
			.retain(|task| task.state() == quercus_engine_rs::task::TaskState::Running);
		let finished: Vec<i64> = window
			.requests
			.iter()
			.filter(|(_, task)| task.state() != quercus_engine_rs::task::TaskState::Running)
			.map(|(frame, _)| *frame)
			.collect();
		for frame in finished {
			let Some(task) = window.requests.remove(&frame) else {
				continue;
			};
			if task.state() == quercus_engine_rs::task::TaskState::Succeeded {
				if let Some(frame_data) = task.take_video() {
					window.slots.insert(frame, frame_data);
				} else {
					window.submitted.remove(&frame);
				}
			} else {
				window.submitted.remove(&frame);
			}
		}
		let new_frames: Vec<i64> = (playhead.max(0)..end)
			.filter(|f| !window.submitted.contains(f))
			.collect();
		drop(windows);

		drop(stale_slots);

		for frame in new_frames {
			let Ok(task) = super::renderops::submit_playback_video_frame(
				&project,
				node,
				frame,
				tb,
				width,
				height,
				Some(quercus_engine_rs::types::PixelFormat::F32),
				playhead,
				self.preview_generation,
			) else {
				continue;
			};
			let mut windows = self
				.preview_windows
				.lock()
				.unwrap_or_else(|e| e.into_inner());
			let window = windows.entry(monitor).or_default();
			window.requests.insert(frame, task);
			window.submitted.insert(frame);
		}
	}

	/// Converts a cached SDK frame into the viewer image and matching scopes.
	fn preview_slot_frame(
		&self,
		monitor: Monitor,
		frame: Frame,
	) -> Option<(Arc<RenderImage>, ScopeData)> {
		let node = match monitor {
			Monitor::Program => self.sequence?,
			Monitor::Source => self.selected_footage_node()?,
		};
		let node_id = node.raw();
		let mut windows = self
			.preview_windows
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let window = windows.get_mut(&monitor)?;
		if window.sequence != node_id || window.generation != self.preview_generation {
			return None;
		}
		let slot = window.slots.remove(&frame.0)?;
		let (width, height) = {
			let info = slot.info();
			(info.width.max(0) as u32, info.height.max(0) as u32)
		};
		let rendered = super::renderops::RenderedFrame::Sdk(slot);
		let (image, scope, samples) = rendered.to_display()?;
		if let Some(samples) = samples {
			super::gpu::register_display_frame(image.id.0, width, height, &samples);
		}
		Some((Arc::new(image), scope))
	}

	/// Cancels every monitor's pre-render window and drops its owned frames.
	fn cancel_preview_windows(&mut self) {
		let mut windows = self
			.preview_windows
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		for window in windows.values_mut() {
			for (_, task) in std::mem::take(&mut window.requests) {
				task.cancel();
				window.retired.push(task);
			}
			window.submitted.clear();
			window.slots.clear();
		}
	}

	/// Cancels one monitor's pre-render window (source selection change:
	/// the old footage's window is stale even though the program window is
	/// untouched).
	fn cancel_preview_window(&mut self, monitor: Monitor) {
		let mut windows = self
			.preview_windows
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		let Some(window) = windows.get_mut(&monitor) else {
			return;
		};
		for (_, task) in std::mem::take(&mut window.requests) {
			task.cancel();
			window.retired.push(task);
		}
		window.submitted.clear();
		window.slots.clear();
	}

	/// Invalidates every cached/rendered preview frame: the CPU cache is
	/// cleared, the full-res and pre-render-window generations bumped, and
	/// the pre-render windows cancelled (pending/claimed requests and held
	/// slots released). Shared by edits, undo-stack changes, project drops
	/// and preview-media invalidation.
	fn invalidate_rendered_frames(&mut self) {
		self.cpu_frame_cache.lock().unwrap().clear();
		self.full_res_generation = self.full_res_generation.wrapping_add(1);
		self.preview_generation = self.preview_generation.wrapping_add(1);
		self.cancel_preview_windows();
		self.clear_multicam_frames();
	}

	/// The bin entry's proxy badge (角标): the footage's proxy lifecycle
	/// mapped onto the widget's badge enum; folders and footage without
	/// proxy state get none.
	fn proxy_badge_of(&self, id: u64) -> Option<crate::widgets::project_explorer::ProxyBadge> {
		use crate::widgets::project_explorer::ProxyBadge;
		match self.proxy_state(id)? {
			super::engine::ProxyMediaState::Ready => Some(ProxyBadge::Ready),
			super::engine::ProxyMediaState::Generating => Some(ProxyBadge::Generating),
			super::engine::ProxyMediaState::Failed => Some(ProxyBadge::Failed),
			super::engine::ProxyMediaState::Missing => None,
		}
	}

	/// Attaches cached thumbnails to the bin entries, spawning a background
	/// generation job for every footage that has none yet. Entries without a
	/// renderable frame keep the widget's placeholder.
	fn attach_thumbnails(&self, entries: Vec<ProjectEntry>) -> Vec<ProjectEntry> {
		let Some(project) = self.editor_session().map(|session| session.project()) else {
			return entries;
		};
		entries
			.into_iter()
			.map(|entry| {
				if entry.is_dir {
					return entry;
				}
				// The proxy badge rides every bin row (with or without a
				// thumbnail), so a Ready proxy is visible at a glance.
				let entry = match self.proxy_badge_of(entry.id) {
					Some(badge) => entry.with_proxy_badge(badge),
					None => entry,
				};
				let mut thumbs = self.thumbnails.lock().unwrap();
				if let Some(path) = thumbs.done.get(&entry.id) {
					return entry.with_thumbnail(path.to_string_lossy().into_owned());
				}
				if thumbs.pending.insert(entry.id) {
					let tx = self.thumb_tx.lock().unwrap().clone();
					let project = project.clone();
					let identity = entry.id;
					let generation = self.thumb_generation;
					std::thread::spawn(move || {
						Self::thumbnail_worker(project, identity, generation, tx)
					});
				}
				entry
			})
			.collect()
	}

	/// Renders one footage's first frame to a PNG on a background thread
	/// and reports the file through `tx`. The request owns a project `Arc`,
	/// so the render stays valid when the engine drops the project mid-flight
	/// (the drain discards the stale completion).
	fn thumbnail_worker(
		project: quercus_engine_rs::node::Project,
		identity: u64,
		generation: u64,
		tx: mpsc::Sender<ThumbEvent>,
	) {
		let Some(path) = Self::render_thumbnail(&project, identity) else {
			return;
		};
		let _ = tx.send(ThumbEvent {
			identity,
			generation,
			path,
		});
	}

	/// Renders `identity`'s footage first frame into the thumbnail
	/// directory and returns the PNG path (`None` when the entry is not a
	/// renderable footage or the render fails). The directory follows the
	/// project's disk-cache location when it overrides the default (the
	/// 项目属性 disk-cache setting's live consumer).
	fn render_thumbnail(
		project: &quercus_engine_rs::node::Project,
		identity: u64,
	) -> Option<PathBuf> {
		let node = quercus_engine_rs::node::NodeId::from_raw(identity);
		if project.node_kind(node) != Some(quercus_engine_rs::node::NodeKind::Footage) {
			return None;
		}
		let filename = project.footage_filename(node).filter(|filename| !filename.is_empty())?;
		let (cache_mode, cache_path) = project.cache_location();
		let cache_dir = (cache_mode != 0).then_some(cache_path);
		let path = thumbnail_path(cache_dir.as_deref(), &filename);
		if path.exists() {
			return Some(path);
		}
		// The SDK returns owned F32 RGBA pixels, so the PNG has no shared-memory
		// slot or GPU texture lifetime dependency.
		let task = quercus_engine_rs::task::render_thumbnail(
			project,
			node,
			quercus_engine_rs::types::Rational::new(0, 1),
			(THUMBNAIL_WIDTH as u32, THUMBNAIL_HEIGHT as u32),
		)?;
		if !task.wait() {
			return None;
		}
		let thumbnail = task.take_thumbnail()?;
		let samples = thumbnail.pixels();
		let bytes: Vec<u8> = samples
			.iter()
			.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
			.collect();
		let (width, height) = (THUMBNAIL_WIDTH as u32, THUMBNAIL_HEIGHT as u32);
		let image = image::RgbaImage::from_raw(width, height, bytes)?;
		std::fs::create_dir_all(path.parent()?).ok()?;
		// Write aside then rename so readers never see a partial file. The
		// temp name ends in `.part` (not `.png`), so the encoder is given
		// explicitly — `Image::save` infers the format from the extension
		// and would reject it.
		let tmp = path.with_extension("part");
		{
			let file = std::fs::File::create(&tmp).ok()?;
			image
				.write_to(&mut std::io::BufWriter::new(file), image::ImageFormat::Png)
				.ok()?;
		}
		std::fs::rename(&tmp, &path).ok()?;
		Some(path)
	}

	/// Installs completed thumbnails, discarding stale completions from a
	/// dropped project's generation.
	fn drain_thumbnails(&mut self) {
		let rx = self.thumb_rx.lock().unwrap();
		let mut thumbs = self.thumbnails.lock().unwrap();
		while let Ok(event) = rx.try_recv() {
			if event.generation != self.thumb_generation {
				continue;
			}
			thumbs.pending.remove(&event.identity);
			thumbs.done.insert(event.identity, event.path);
		}
	}

	/// Invalidates every monitor's rendered frames (the preview media
	/// changed — proxy toggled, generated or deleted). No timeline rebuild
	/// is needed — the montage resolution reads the proxy switch on every
	/// pull.
	fn invalidate_preview_frames(&mut self, cx: &mut Context<Self>) {
		self.invalidate_rendered_frames();
		cx.notify();
	}

	/// The disk cache directory the proxy files live in (the config
	/// override, else the platform default).
	fn proxy_cache_path() -> String {
		let configured = config_get_string(CONFIG_KEY_DISK_CACHE_PATH);
		if configured.trim().is_empty() {
			quercus_engine_rs::utils::default_disk_cache_path().unwrap_or_default()
		} else {
			configured
		}
	}

	/// The footage node behind a project-explorer entry id, when it is a
	/// footage node of the open project.
	fn footage_of(&self, id: u64) -> Option<NodeId> {
		let project = self.project.as_ref()?;
		let node = graphops::id_of(id)?;
		(project.node_kind(node) == Some(NodeKind::Footage)).then_some(node)
	}

	/// Polls in-flight proxy transcodes from the tick loop.
	///
	/// Terminal tasks are finalized through the same editor session that started
	/// them, so stale workers cannot overwrite newer metadata. Only completion
	/// invalidates rendered media; progress alone merely repaints the status bar.
	fn drain_proxy_runs(&mut self, cx: &mut Context<Self>) {
		if self.proxy_runs.is_empty() {
			return;
		}
		let finished: Vec<(u64, bool)> = self
			.proxy_runs
			.iter()
			.filter_map(|run| match run.task.state() {
				quercus_engine_rs::task::TaskState::Running => None,
				quercus_engine_rs::task::TaskState::Succeeded => Some((run.footage, true)),
				quercus_engine_rs::task::TaskState::Failed
				| quercus_engine_rs::task::TaskState::Cancelled => Some((run.footage, false)),
			})
			.collect();
		for run in &self.proxy_runs {
			if let Some((_, succeeded)) = finished.iter().find(|(id, _)| *id == run.footage) {
				let id = quercus_engine_rs::node::NodeId::from_raw(run.footage);
				let _ = run.session.complete_proxy(id, &run.task, *succeeded);
			}
		}
		self.proxy_runs
			.retain(|run| !finished.iter().any(|(footage, _)| *footage == run.footage));
		if !finished.is_empty() {
			self.invalidate_preview_frames(cx);
			self.pump_proxy_queue(cx);
		} else {
			cx.notify();
		}
	}

	/// Maximum concurrent proxy workers (configurable in Proxy Settings).
	/// The SDK value preserves the engine's per-worker CPU/thread budget.
	fn proxy_max_concurrent() -> usize {
		quercus_engine_rs::task::proxy_max_concurrent()
	}

	/// 按并发上限从队列递补启动代理生成（队列去重由入队侧保证）。
	fn pump_proxy_queue(&mut self, cx: &mut Context<Self>) {
		while self.proxy_runs.len() < Self::proxy_max_concurrent() && !self.proxy_queue.is_empty() {
			let id = self.proxy_queue.remove(0);
			if let Err(err) = self.proxy_generate(id, cx) {
				println!("[real engine] queued proxy generation skipped {id}: {err}");
			}
		}
	}

	/// Auto-start proxy generation in the background for every footage
	/// that needs one (设计：全局 UseProxyMedia 开启时素材代理在后台
	/// 自动生成，不要求手动点 Generate；触发点：导入素材、打开工程）。
	///
	/// 逐素材规则：
	/// - 已有 Ready 代理 / 已在生成 / 上次失败（state 3，避免失败
	///   循环）→ 跳过；
	/// - 显式关过代理的素材（记录了代理路径但未启用）→ 跳过
	///   （尊重每个素材的 opt-out）；
	/// - 其余（从未触碰过代理的，或已启用但未就绪的）→ 启动生成，
	///   生成路径会同时把 proxy_enabled 置位，完成后预览即走代理。
	fn autostart_proxies(&mut self, cx: &mut Context<Self>) {
		if !super::renderops::use_proxy_media() {
			return;
		}
		let Some(session) = self.editor_session() else {
			return;
		};
		let candidates: Vec<u64> = session
			.snapshot()
			.footages
			.into_iter()
			.filter(|footage| {
				let Some(status) = session.proxy_status(footage.id) else {
					return false;
				};
				status.can_generate
					&& status.state != quercus_engine_rs::codec::ProxyState::Ready
					&& status.state != quercus_engine_rs::codec::ProxyState::Failed
					&& (!status.has_proxy || status.enabled)
					&& self.proxy_runs.iter().all(|run| run.footage != footage.id.raw())
			})
			.map(|footage| footage.id.raw())
			.collect();
		// 入队 + 按并发上限递补（不一次全部启动：每个转码本身就是
		// 多线程 ffmpeg，N 个并发会把 CPU 打满）。
		for id in candidates {
			if !self.proxy_queue.contains(&id) {
				self.proxy_queue.push(id);
			}
		}
		self.pump_proxy_queue(cx);
	}

	/// Synchronize the selected clips by their footage's source start
	/// timecode (the C++ `synchronize_selected_clips_by_source_time`):
	/// the clip whose source head (start time + media in) is earliest is
	/// the reference, the earliest selected in point is the anchor, and
	/// every clip is re-placed so its source head lines up with the
	/// reference's at the anchor (one multi-undo).
	fn sync_clips_by_source_time_internal(&mut self, clips: &[ClipId]) {
		use quercus_engine_rs::audio_analysis::{SourceClip, place_by_source_time};

		let Some(project) = self.project.clone() else {
			return;
		};

		struct SourceTarget {
			node: NodeId,
			track: NodeId,
			list: NodeId,
			track_index: i32,
			source: SourceClip,
			source_head: Rational,
			block_in: Rational,
		}

		let mut targets: Vec<SourceTarget> = Vec::new();
		for clip in clips {
			let Some(node) = graphops::id_of(clip.0) else {
				continue;
			};
			let Some((block_in, _, media_in)) = graphops::clip_range(&project, node) else {
				continue;
			};
			let Some(track) = graphops::clip_track(&project, node) else {
				continue;
			};
			let Some(list) = project.track_list(track) else {
				continue;
			};
			let Some(footage) = graphops::find_input_footage(&project, node) else {
				continue;
			};
			let Some(source_start_time) = project.footage_source_start_time(footage) else {
				continue;
			};
			targets.push(SourceTarget {
				node,
				track,
				list,
				track_index: project.track_index(track),
				source_head: source_start_time + media_in,
				block_in,
				source: SourceClip {
					source_start_time: rational_to_sdk(source_start_time),
					media_in: rational_to_sdk(media_in),
					has_source_start_time: true,
				},
			});
		}
		if targets.len() < 2 {
			return;
		}

		let mut reference = &targets[0];
		let mut anchor_in = targets[0].block_in;
		for target in &targets[1..] {
			if target.source_head < reference.source_head {
				reference = target;
			}
			if target.block_in < anchor_in {
				anchor_in = target.block_in;
			}
		}

		// (node, track, list, track index, placement) for every valid
		// placement (the reference itself lands on the anchor).
		let mut placements: Vec<(NodeId, NodeId, NodeId, i32, Rational)> = Vec::new();
		for target in &targets {
			let placement = place_by_source_time(
				&reference.source,
				&target.source,
				rational_to_sdk(anchor_in),
			);
			if placement.valid {
				let timeline_in = rational_from_sdk(placement.timeline_in);
				placements.push((
					target.node,
					target.track,
					target.list,
					target.track_index,
					timeline_in,
				));
			}
		}
		if placements.len() < 2 {
			return;
		}

		let Some(tb) = self.time_base() else {
			return;
		};
		let session = self.editor_session();
		let Some(session) = session else {
			return;
		};
		let _ = quercus_engine_rs::node::undo_group_begin("Synchronize Clips by Source Time");
		for (node, _track, _list, _track_index, timeline_in) in &placements {
			let _ = session.execute(EditCommand::MoveClip {
				clip: *node,
				new_in_ts: graphops::rational_to_ts(*timeline_in, tb),
			});
		}
		let _ = quercus_engine_rs::node::undo_group_end();
	}

	/// Synchronize the selected clips by waveform correlation (the C++
	/// `synchronize_selected_clips_by_waveform_internal`): the leftmost
	/// clip is the reference, every other clip is shifted by the
	/// estimated envelope offset; with `allow_speed`, an inconclusive
	/// offset triggers a rate search whose winner also rescales the clip
	/// speed (one multi-undo).
	fn sync_clips_by_waveform_internal(&mut self, clips: &[ClipId], allow_speed: bool) {
		use quercus_engine_rs::audio_analysis::place_by_waveform_offset;
		use quercus_engine_rs::audio_analysis::{
			estimate_envelope_offset, estimate_stretch_offset,
		};

		let Some(cache) = self.waveform_cache() else {
			return;
		};
		let Some(project) = self.project.clone() else {
			return;
		};

		struct WaveTarget {
			node: NodeId,
			track: NodeId,
			list: NodeId,
			track_index: i32,
			block_in: Rational,
			speed: f64,
			media_in_s: f64,
			media_len_s: f64,
			waveform: Arc<crate::oakui::waveform::ClipWaveform>,
		}

		let mut targets: Vec<WaveTarget> = Vec::new();
		for clip in clips {
			let Some(node) = graphops::id_of(clip.0) else {
				continue;
			};
			let Some((block_in, block_out, media_in)) = graphops::clip_range(&project, node) else {
				continue;
			};
			let Some(track) = graphops::clip_track(&project, node) else {
				continue;
			};
			let Some(list) = project.track_list(track) else {
				continue;
			};
			let Some(waveform) = cache.get(clip.0) else {
				continue;
			};
			// The C++ media range is media_in + timeline length
			// (speed/reverse ignored there); the Rust cache covers the
			// whole file from sample 0.
			let media_len_s = (block_out - block_in).to_f64();
			if !super::waveformsync::waveform_sync_eligible(&waveform, media_len_s) {
				continue;
			}
			targets.push(WaveTarget {
				node,
				track,
				list,
				track_index: project.track_index(track),
				block_in,
				speed: project.block_speed(node),
				media_in_s: media_in.to_f64(),
				media_len_s,
				waveform,
			});
		}
		if targets.len() < 2 {
			return;
		}

		let mut ref_index = 0usize;
		for (i, target) in targets.iter().enumerate() {
			if target.block_in < targets[ref_index].block_in {
				ref_index = i;
			}
		}
		let sample_rate = targets[ref_index].waveform.sample_rate;
		if sample_rate <= 0 {
			return;
		}
		let window_samples = (sample_rate / 20).max(1) as usize;
		let max_offset_windows = (i64::from(sample_rate) * 600) / window_samples as i64;

		let reference = &targets[ref_index];
		let (ref_envelope, ref_valid) = super::waveformsync::extract_cache_envelope(
			&reference.waveform,
			reference.media_in_s,
			reference.media_len_s,
			window_samples,
		);

		// (node, track, list, track index, placement, speed, old speed).
		let mut placements: Vec<(NodeId, NodeId, NodeId, i32, Rational, f64, f64)> =
			vec![(
				reference.node,
				reference.track,
				reference.list,
				reference.track_index,
				reference.block_in,
				1.0,
				reference.speed,
			)];

		for (i, target) in targets.iter().enumerate() {
			if i == ref_index {
				continue;
			}
			let (cand_envelope, cand_valid) = super::waveformsync::extract_cache_envelope(
				&target.waveform,
				target.media_in_s,
				target.media_len_s,
				window_samples,
			);
			let mut offset = estimate_envelope_offset(
				&ref_envelope,
				&cand_envelope,
				&ref_valid,
				&cand_valid,
				window_samples,
				max_offset_windows,
			);
			let mut speed = 1.0;
			if allow_speed && (!offset.valid || offset.confidence < 0.6) {
				// Inconclusive: the clips may run at different speeds —
				// search a rate range with a tighter offset radius.
				let radius =
					max_offset_windows.min((i64::from(sample_rate) * 30) / window_samples as i64);
				let stretch = estimate_stretch_offset(
					&ref_envelope,
					&cand_envelope,
					&ref_valid,
					&cand_valid,
					window_samples,
					radius,
					0.75,
					1.34,
					0.005,
				);
				if stretch.valid
					&& stretch.confidence > (if offset.valid { offset.confidence } else { 0.0 })
				{
					speed = stretch.rate;
					offset.valid = true;
					offset.confidence = stretch.confidence;
					offset.offset_samples = stretch.offset_samples;
				}
			}
			if !offset.valid {
				continue;
			}
			let placement = place_by_waveform_offset(
				rational_to_sdk(reference.block_in),
				offset.offset_samples,
				sample_rate,
			);
			let timeline_in = rational_from_sdk(placement.timeline_in);
			if placement.valid && timeline_in >= Rational::new(0, 1) {
				placements.push((
					target.node,
					target.track,
					target.list,
					target.track_index,
					timeline_in,
					speed,
					target.speed,
				));
			}
		}
		if placements.len() < 2 {
			return;
		}

		let Some(tb) = self.time_base() else {
			return;
		};
		let Some(session) = self.editor_session() else {
			return;
		};
		let _ = quercus_engine_rs::node::undo_group_begin("Synchronize Clips by Waveform");
		for (node, _track, _list, _track_index, timeline_in, speed, old_speed) in &placements {
			if (*speed - 1.0).abs() > f64::EPSILON {
				// The C++ multiplies the clip's current speed by the
				// estimated rate (`clip_speed * placement.speed`).
				let _ = session.execute(EditCommand::SetClipsSpeed {
					clips: vec![*node],
					speed: *old_speed * speed,
				});
			}
			let _ = session.execute(EditCommand::MoveClip {
				clip: *node,
				new_in_ts: graphops::rational_to_ts(*timeline_in, tb),
			});
		}
		let _ = quercus_engine_rs::node::undo_group_end();
	}

	/// M16 S1 graph mode: pushes the current project state to the worker
	/// pool (snapshot serialized once per undo-stack revision; the worker
	/// renders node-graph tickets from it).
	fn push_graph_snapshot(&mut self) {
		// Debounced: record the wanted revision, flush on the next tick
		// after the debounce window (or immediately when no edit raced
		// recently). Rapid edits coalesce instead of serializing +
		// uploading the whole project snapshot on every keystroke.
		let revision = quercus_engine_rs::node::undo_index().max(0) as u64;
		self.pending_snapshot = Some(revision);
	}

	/// Flushes the pending snapshot upload when the debounce window passed
	/// (called from the engine tick; the window lets a burst of edits
	/// coalesce into one serialization + one worker load).
	fn flush_snapshot_if_due(&mut self, cx: &mut Context<Self>) {
		let Some(revision) = self.pending_snapshot.take() else {
			return;
		};
		let due = match self.last_snapshot_push {
			Some(last) => last.elapsed().as_millis() >= 150,
			None => true,
		};
		if !due {
			return;
		}
		let Some(project) = self.project.clone() else {
			return;
		};
		if quercus_engine_rs::render::manager_set_graph_snapshot(&project, revision) {
			{
				// Every frame rendered since the edit went through workers
				// holding the STALE snapshot (the debounce window — the
				// proxy that just landed in the cache, a full-res fill,
				// the pre-render window). Drop them so the next paint
				// re-renders from the fresh snapshot; otherwise a paused
				// viewer keeps the pre-edit picture until the playhead
				// moves (the "edits only show during playback" report).
				self.invalidate_rendered_frames();
			}
		}
		self.last_snapshot_push = Some(std::time::Instant::now());
		cx.notify();
	}

	/// Adopts a newly created/loaded project, dropping any previous one,
	/// and rebuilds every snapshot. The undo stack is cleared (a project
	/// switch starts a fresh history, mirroring the facade's
	/// project_new/load) and the project is bound to the write-through
	/// library. Projects without a sequence get a blank default.
	fn adopt_project(&mut self, project: ProjectRef, cx: &mut Context<Self>) {
		self.drop_project();
		quercus_engine_rs::node::undo_clear();

		// Cached display info.
		let (name, path, first_sequence) = {
			(
				project.name(),
				PathBuf::from(project.filename()),
				graphops::sequence_ids(&project).first().copied(),
			)
		};
		self.project_info = Project {
			name: if name.is_empty() || name == "(untitled)" {
				UNTITLED.into()
			} else {
				name
			},
			path,
		};
		let live = EditorSession::from_project(project.clone())
			.expect("adopt the project into an editor session");
		let _ = live.bind_project_to_library();
		set_live_session(Some(live.clone()));
		self.project = Some(project.clone());
		self.session = Some(live);
		// The project's color pipeline properties (working colorspace +
		// delivery target) drive the app-side decode/output transforms;
		// the render workers pick them up at graph-load time.
		{
			let working = project
				.setting("workingcolorspace")
				.unwrap_or_else(|| "acescg".to_string());
			let gamut = project
				.setting("outputgamut")
				.unwrap_or_else(|| "srgb".to_string());
			let transfer = project
				.setting("outputtransfer")
				.unwrap_or_else(|| "srgb".to_string());
			let _ = quercus_engine_rs::state::color_set_pipeline_settings(
				&working, &gamut, &transfer,
			);
		}
		// The inline (test) render backend's node-graph source: the live
		// project, so preview frames evaluate through the traverser just
		// like the worker pool's process snapshot does.
		let _ = quercus_engine_rs::render::manager_set_inline_project(&project);

		// Footage loaded from a file may lack stream metadata (C++ projects
		// have no `<streams>` segment; older Rust saves predate the probe
		// recording them): reprobe so durations, drop track kinds and the
		// source monitor's playback length come back (the facade's load
		// probe cascade).
		graphops::reprobe_unprobed_footage(&project);

		// Sequences saved before they mounted under the root folder load
		// free-floating; reattach them so the project explorer lists them.
		graphops::ensure_sequences_mounted(&project);

		// The sequence: the project's first, or a blank default.
		let seq =
			first_sequence.unwrap_or_else(|| graphops::create_sequence(&project, "Sequence 1"));
		self.activate_sequence(seq);
		self.refresh_sequence_info();
		self.rebuild_timeline();
		self.push_graph_snapshot();

		// The project's stored OCIO override (if any) drives the display
		// color pipeline from here on.
		Self::apply_project_color_config(Some(&project));
		// The project's colorspace settings may differ from the previous
		// project's: re-declare the display policy on every open window.
		self.reapply_display_policy_to_windows(cx);

		// 全局代理开关开启时，工程里未就绪素材的代理在后台自动生成
		// （打开长素材工程不等待：生成走任务线程）。
		self.autostart_proxies(cx);

		cx.notify();
	}

	/// Drops the project and every auxiliary handle. The undo stack is
	/// cleared FIRST: its commands reference the marker/workarea
	/// auxiliaries, so they must be gone before those handles release.
	fn drop_project(&mut self) {
		quercus_engine_rs::node::undo_clear();
		quercus_engine_rs::render::manager_clear_graph_snapshot();
		set_live_session(None);
		if let Some(session) = self.session.take() {
			session.close();
		}
		self.project = None;
		self.sequence = None;
		// The audio prefetch belongs to the dropped project's sequence time:
		// invalidate it so the next playback restarts the submission cursor.
		self.audio_prefetch
			.lock()
			.unwrap_or_else(|e| e.into_inner())
			.reset(0, 1);
		// The sequence an in-flight full-res job may still be rendering is
		// gone (the job holds its own project `Arc`, so it stays valid, but
		// its frame belongs to the dropped project): mark it stale.
		self.invalidate_rendered_frames();
		// Same for in-flight thumbnail jobs; the cache is per-project too
		// (identities are only unique within one graph).
		self.thumb_generation = self.thumb_generation.wrapping_add(1);
		*self.thumbnails.lock().unwrap() = ThumbnailState::default();
		self.tracks.clear();

		self.sequence_info = None;
		self.project_info = Project {
			name: UNTITLED.into(),
			path: PathBuf::new(),
		};
		// The dropped project's OCIO override leaves with it: the display
		// color pipeline returns to the app default config.
		Self::apply_project_color_config(None);
	}

	/// Applies a project's stored OCIO override (`Some(project)`) or
	/// restores the app default config (`None`, or a project without an
	/// override). A config that fails to load falls back to the app default
	/// (the file may have moved since the project was saved).
	fn apply_project_color_config(project: Option<&ProjectRef>) {
		let stored = project.and_then(|p| p.setting(PROJECT_SETTING_OCIO_CONFIG));
		let applied =
			quercus_engine_rs::state::color_set_up_default_config(stored.as_deref());
		if let Err(e) = applied {
			println!("[real engine] project OCIO config apply failed: {e}");
			let _ = quercus_engine_rs::state::color_set_up_default_config(None);
		}
		super::displaycolor::invalidate();
	}

	/// Makes `seq` the current sequence for both the app widgets and the
	/// live editor session, so commands that omit an explicit sequence act
	/// on the one the user sees.
	fn activate_sequence(&mut self, seq: NodeId) {
		self.sequence = Some(seq);
		if let Some(session) = &self.session {
			let _ = session.set_current_sequence(seq);
		}
	}

	/// Refreshes the cached `Sequence` (name / format / length) from the
	/// graph.
	fn refresh_sequence_info(&mut self) {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			self.sequence_info = None;
			return;
		};
		let name = graphops::node_label(&project, seq);
		let rate = graphops::sequence_video_params(&project, seq)
			.map(|(_, _, r)| FrameRate::new(r.num.max(1) as u32, r.den.max(1) as u32))
			.unwrap_or(VideoFormat::hd_1080p25().rate);
		let (width, height) = graphops::sequence_video_params(&project, seq)
			.map(|(w, h, _)| (w.max(1) as u32, h.max(1) as u32))
			.unwrap_or((1920, 1080));
		let seconds = graphops::sequence_length(&project, seq);
		let seconds = seconds.num as f64 / seconds.den.max(1) as f64;
		let length = Frame((seconds * rate.num as f64 / rate.den as f64).round() as i64);
		self.sequence_info = Some(Sequence {
			name: if name.is_empty() {
				"Sequence 1".into()
			} else {
				name
			},
			format: VideoFormat {
				width,
				height,
				rate,
			},
			length,
		});
		// The worker pool's size follows the sequence resolution: the GPU
		// vram budget per worker grows with the pixel count (an NVDEC 4K
		// decoder needs ~4× the 1080p surface memory), so a 1080p pool of
		// 20+ workers exhausts a consumer GPU the moment the sequence
		// switches to 4K and every decoder open starts OOMing. Resize the
		// pool to what fits now; the size change on the OTHER sequence's
		// update re-expands it.
		let fps = (rate.num as f64 / rate.den.max(1) as f64).round().max(1.0) as u32;
		quercus_engine_rs::render::manager_set_workspace_size(
			width as i32,
			height as i32,
			fps,
		);
		// Stage 6b: keep the OFX normalised-coordinate default conversion in
		// sync with the current sequence's extent.
		crate::oakui::ofx::update_project_extent(width as f64, height as f64);
	}

	/// Stage 6b: refreshes the OFX timeline-suite fallback time snapshot
	/// from the program monitor's playhead (seconds) and the sequence
	/// length (the range bounds). Called on seek and on every tick.
	fn update_ofx_viewer_time(&self, cx: &App) {
		let fps = self.frame_rate().as_f64();
		if fps <= 0.0 {
			return;
		}
		let frame = self.clock_frame(Monitor::Program, cx).0;
		let time = frame as f64 / fps;
		let length = self.sequence_length().0 as f64 / fps;
		crate::oakui::ofx::update_viewer_time(time, 0.0, length);
	}

	/// Rebuilds the timeline snapshot from the graph.
	fn rebuild_timeline(&mut self) {
		self.tracks.clear();
		let (Some(project), Some(seq)) = (self.project_ref(), self.sequence) else {
			return;
		};
		let tb = self.time_base();
		let mut out: Vec<RealTrack> = Vec::new();
		{
			// Per-type track lists, each displayed topmost-first. NLE
			// growth direction: video/subtitle tracks grow upward (the
			// list is displayed reversed, so an appended track lands on
			// top), audio tracks grow downward (list order = display
			// order, so an appended track lands at the bottom).
			for kind in [DomainTrackKind::Video, DomainTrackKind::Audio, DomainTrackKind::Subtitle] {
				let tracks = graphops::track_ids(&project, seq, kind);
				let ordered: Box<dyn Iterator<Item = (usize, &NodeId)>> = match kind {
					DomainTrackKind::Audio => Box::new(tracks.iter().enumerate()),
					_ => Box::new(tracks.iter().enumerate().rev()),
				};
				for (track_index, &track_id) in ordered {
					out.push(Self::snapshot_track(&project, track_id, kind, track_index, tb));
				}
			}
		}
		self.tracks = out;
		// M12 P4: refresh the audio waveforms for the visible audio clips.
		if let Some(cache) = self.waveform_cache() {
			for track in &self.tracks {
				if track.kind != TrackKind::Audio {
					continue;
				}
				for clip in &track.clips {
					self.refresh_clip_waveform(cache.clone(), clip);
				}
			}
		}
	}

	/// The shared refresh after any undo-stack change (undo / redo /
	/// history jump): the sequence info, the timeline snapshot and the
	/// frame caches all follow the reverted or re-applied state.
	fn apply_stack_change(&mut self, cx: &mut Context<Self>) {
		self.refresh_sequence_info();
		self.rebuild_timeline();
		self.invalidate_rendered_frames();
		self.push_graph_snapshot();
		cx.notify();
	}

	/// Snapshots one track (with its clips) from the SDK project queries.
	fn snapshot_track(
		project: &ProjectRef,
		track: NodeId,
		kind: DomainTrackKind,
		track_index: usize,
		tb: Option<(i64, i64)>,
	) -> RealTrack {
		let name = match kind {
			DomainTrackKind::Video => format!("V{}", track_index + 1),
			DomainTrackKind::Audio => format!("A{}", track_index + 1),
			DomainTrackKind::Subtitle => format!("S{}", track_index + 1),
		};
		let height = px(project.track_height_pixels(track) as f32);
		// The transition wedges the track's clips carry (left = head
		// transition, right = tail transition), keyed by clip block.
		let widths = graphops::clip_transition_widths(project, track);
		let clips = graphops::clip_ids(project, track)
			.iter()
			.enumerate()
			.filter_map(|(clip_index, &block)| {
				let (in_r, out_r, media_r) = graphops::clip_range(project, block)?;
				let to_ts = |r: Rational| {
					tb.map(|tb| graphops::rational_to_ts(r, tb)).unwrap_or(0)
				};
				let wedge = |w: Option<Rational>| w.map(|r| Frame(to_ts(r)));
				let (head, tail) = widths.get(&block).copied().unwrap_or((None, None));
				let is_adjustment = project.node_kind(block) == Some(NodeKind::Adjustment);
				// The clip's color is locked in at creation time (its
				// `override_color`); clips without one (older projects)
				// fall back to the track-relative palette so their color
				// still varies. Adjustment layers wear their own fixed
				// color instead of a palette entry: their band carries no
				// media and must read as an effect layer, not a clip.
				let override_color = project.node_color(block);
				let color = if is_adjustment {
					ADJUSTMENT_LAYER_COLOR
				} else if override_color >= 0 {
					clip_color(override_color as u64)
				} else {
					clip_color(clip_index as u64)
				};
				let node_label = project.node_label(block);
				let label = if is_adjustment && node_label.is_empty() {
					ADJUSTMENT_LAYER_LABEL.to_string()
				} else if node_label.is_empty() {
					format!("Clip {}", clip_index + 1)
				} else {
					node_label
				};
				let multicam = project.block_multicam_source(block).is_some();
				Some(RealClip {
					id: ClipId(block.raw()),
					range: FrameRange::new(Frame(to_ts(in_r)), Frame(to_ts(out_r))),
					media_in: Frame(to_ts(media_r)),
					label: label.into(),
					color,
					block,
					multicam,
					in_transition: wedge(head),
					out_transition: wedge(tail),
				})
			})
			.collect();
		// The muted flag doubles as the video/subtitle visibility toggle
		// (Olive parity: the eye button flips `muted`).
		let muted = project.track_muted(track);
		let locked = project.track_locked(track);
		RealTrack {
			kind: track_kind_of(kind),
			name: name.into(),
			height,
			locked,
			muted,
			solo: false,
			visible: !muted,
			clips,
			track,
			track_index,
		}
	}

	/// The waveform cache (created lazily at the current frame rate).
	fn waveform_cache(&self) -> Option<Arc<crate::oakui::waveform::WaveformCache>> {
		let mut slot = self.waveforms.lock().unwrap_or_else(|e| e.into_inner());
		if slot.is_none() {
			let fps = self
				.sequence_info
				.as_ref()
				.map(|s| s.format.rate.num as f32 / s.format.rate.den.max(1) as f32)
				.unwrap_or(25.0);
			*slot = Some(crate::oakui::waveform::WaveformCache::new(fps));
		}
		slot.clone()
	}

	/// Extract (or reuse) the waveform of one audio clip.
	fn refresh_clip_waveform(
		&self,
		cache: Arc<crate::oakui::waveform::WaveformCache>,
		clip: &RealClip,
	) {
		let Some(project) = self.project_ref() else {
			return;
		};
		let filename = graphops::clip_media_filename(project, clip.block);
		let Some(filename) = filename else {
			return;
		};
		let duration_frames = (clip.range.end.0 - clip.range.start.0).max(1);
		// 后台提取：全音频解码按素材时长走（40 分钟 HEVC 要秒级），
		// 在 UI 线程上同步跑会把打开工程/重建时间轴卡死；落盘后由
		// 缓存 version 触发重绘（见 drain_proxy_runs 处的检查）。
		let clip_id = clip.id.0;
		std::thread::spawn(move || cache.refresh(clip_id, &filename, duration_frames));
	}

	/// Looks up the snapshot clip's block node by `ClipId` (the id IS the
	/// block's stable identity).
	fn clip_block(&self, id: ClipId) -> Option<NodeId> {
		let block = graphops::id_of(id.0)?;
		let project = self.project_ref()?;
		matches!(
			project.node_kind(block),
			Some(NodeKind::Clip | NodeKind::Gap | NodeKind::Transition | NodeKind::Adjustment)
		)
		.then_some(())?;
		// The clip must be on the CURRENT timeline (a stale id of a
		// removed clip must not resolve).
		self.tracks
			.iter()
			.any(|t| t.clips.iter().any(|c| c.id == id))
			.then_some(block)
	}

	/// Whether the track hosting clip `block` is locked (locked tracks
	/// reject every clip edit: trim, move, split, delete).
	fn clip_track_locked(&self, block: NodeId) -> bool {
		let Some(project) = self.project_ref() else {
			return false;
		};
		graphops::clip_track(project, block)
			.map(|track| project.track_locked(track))
			.unwrap_or(false)
	}

	/// Resolves the node-graph selection to the inspector's view: which
	/// clip's stack to show, and which effect card (if any) to highlight.
	/// A single selected node that names a clip block on the current
	/// timeline selects that clip's stack; one that names an effect of some
	/// clip's chain selects that clip's stack and returns the matching
	/// card. Without a node-graph selection the timeline-selected clip
	/// (the existing behavior) is the target.
	fn inspector_selection(&self) -> (Option<ClipId>, Option<EffectId>) {
		let Some(project) = self.project_ref() else {
			return (self.selected_clip, None);
		};
		let Some(ident) = self.selected_graph_node else {
			return (self.selected_clip, None);
		};
		let Some(node) = graphops::id_of(ident) else {
			return (self.selected_clip, None);
		};
		// A clip block node on the current timeline: its stack is the target.
		if matches!(
			project.node_kind(node),
			Some(NodeKind::Clip | NodeKind::Gap | NodeKind::Transition | NodeKind::Adjustment)
		)
			&& self
				.tracks
				.iter()
				.any(|t| t.clips.iter().any(|c| c.id.0 == ident))
		{
			return (Some(ClipId(ident)), None);
		}
		// An effect node of some clip's chain: that clip's stack, with the
		// matching card highlighted.
		for track in &self.tracks {
			for clip in &track.clips {
				let Some(block) = graphops::id_of(clip.id.0) else {
					continue;
				};
				if !matches!(
					project.node_kind(block),
					Some(
						NodeKind::Clip
							| NodeKind::Gap
							| NodeKind::Transition
							| NodeKind::Adjustment
					)
				) {
					continue;
				}
				if let Some(effect) = super::effectchain::chain(project, block)
					.iter()
					.find(|n| n.raw() == ident)
				{
					return (Some(clip.id()), Some(EffectId(effect.raw())));
				}
			}
		}
		(self.selected_clip, None)
	}

	/// The selected clip's block node, or `None` when no single clip is
	/// selected. The inspector's stack target follows the node-graph
	/// selection (see [`inspector_selection`](Self::inspector_selection)).
	fn selected_clip_node(&self) -> Option<NodeId> {
		self.clip_block(self.inspector_selection().0?)
	}

	/// The display label of the selected clip (its timeline snapshot
	/// label), if any.
	fn selected_clip_label(&self) -> Option<SharedString> {
		let clip_id = self.inspector_selection().0?;
		for track in &self.tracks {
			if let Some(clip) = track.clips.iter().find(|c| c.id() == clip_id) {
				return Some(clip.label());
			}
		}
		None
	}

	/// The card list of the selected clip's effect chain. Each card wraps
	/// one chain node (index 0 = closest to the source); a clip that
	/// cannot host effects yields an empty list (its `target_label` is
	/// `None`, so the stack shows the empty state).
	fn selected_effect_cards(&self) -> Vec<Arc<dyn EffectData>> {
		let Some(session) = self.editor_session() else {
			return Vec::new();
		};
		let Some(host) = self.selected_clip_node() else {
			return Vec::new();
		};
		let chain = session
			.project()
			.effect_chain(quercus_engine_rs::node::NodeId::from_raw(host.raw()));
		let project = self.project_ref().expect("session requires a project");
		let mut out: Vec<Arc<dyn EffectData>> = Vec::new();
		for node in chain {
			let identity = node.raw();
			let node = graphops::id_of(identity).expect("SDK and app share node identities");
			let type_id = graphops::node_type_id(project, node);
			// `name_of` covers both static (built-in) and dynamic (OpenFX
			// plugin) factory entries.
			let title = quercus_engine_rs::effect::factory_name(&type_id)
				.filter(|n| !n.is_empty())
				.unwrap_or(type_id);
			let plugin_handle = super::effectchain::plugin_instance_handle(project, node);
			// The OpenFX plugin badge: the persistent-message count (the
			// simplified 徽标/计数 of stage 6b). Built-in effects show none.
			let badge = plugin_handle.and_then(|handle| {
				quercus_engine_rs::ofx::persistent_message_count(handle).filter(|count| *count > 0)
			});
			let subtitle = plugin_handle.map(|_| {
				// A muted secondary line identifying the plugin effect as an
				// OpenFX entry.
				crate::i18n::tr("inspector.badge.openfx").to_string()
			});
			out.push(Arc::new(RealEffect {
				id: EffectId(identity),
				title: title.into(),
				subtitle: subtitle.map(Into::into),
				enabled: super::effectchain::is_enabled(project, node),
				expanded: self.expanded_effects.contains(&identity),
				badge,
			}) as Arc<dyn EffectData>);
		}
		out
	}

	/// Applies an edit command's result, then refreshes the snapshots and
	/// repaints.
	fn apply_edit(&mut self, result: Result<(), String>, what: &str, cx: &mut Context<Self>) {
		if let Err(error) = result {
			println!("[real engine] {what} failed: {error}");
		}
		self.refresh_sequence_info();
		self.rebuild_timeline();
		// The sequence content changed: cached rendered frames are stale,
		// and so are any in-flight full-res renders and pre-render windows
		// (M12 P5a / M15 S2).
		self.invalidate_rendered_frames();
		self.push_graph_snapshot();
		cx.notify();
	}
}

// ---------------------------------------------------------------------------
// EngineGateway
// ---------------------------------------------------------------------------

impl EngineGateway for RealEngine {
	fn project(&self) -> Option<&Project> {
		self.project.as_ref().map(|_| &self.project_info)
	}

	fn current_sequence(&self) -> Option<&Sequence> {
		self.sequence_info.as_ref()
	}

	fn open_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
		if let Err(err) = self.open_project_path(path, cx) {
			println!("[real engine] open failed: {err}");
		}
	}

	fn request_frame(&mut self, monitor: Monitor, frame: Frame, cx: &mut Context<Self>) {
		let length = self.sequence_length();
		let clock = self.clock(monitor).clone();
		clock.update(cx, |clock, cx| {
			clock.transport.seek(frame, length);
			if clock.transport.is_playing() {
				clock.play();
			}
			cx.notify();
		});
		if monitor == Monitor::Program {
			// A seek must not keep playing audio queued for the old
			// playhead: clear the output buffer and reset the prefetch so
			// the new position renders from scratch (without the reset the
			// dedup guard could suppress the re-push of the seek target).
			// The audio master anchor is dropped too — the next play run
			// re-anchors at its own first push.
			self.audio_playback = None;
			let _ = quercus_engine_rs::audio::clear_buffered_output();
			let _ = quercus_engine_rs::audio::reset_output_clock();
			let mut st = self
				.audio_prefetch
				.lock()
				.unwrap_or_else(|e| e.into_inner());
			st.reset(frame.0, 1);
		}
		self.mirror_program_playhead(cx);
		self.update_ofx_viewer_time(cx);
		cx.notify();
	}

	fn play(&mut self, monitor: Monitor, cx: &mut Context<Self>) {
		if monitor == Monitor::Program {
			self.program_playing = true;
			// A new play run re-anchors the audio master clock at its first
			// chunk push; a stale anchor (previous run, since cleared by
			// pause/stop/seek) must not leak into the new one.
			self.audio_playback = None;
		}
		let clock = self.clock(monitor).clone();
		clock.update(cx, |clock, cx| {
			clock.play();
			cx.notify();
		});
		self.mirror_program_playhead(cx);
		cx.notify();
	}

	fn pause(&mut self, monitor: Monitor, cx: &mut Context<Self>) {
		let clock = self.clock(monitor).clone();
		clock.update(cx, |clock, cx| {
			clock.pause();
			cx.notify();
		});
		if monitor == Monitor::Program {
			self.stop_program_playback(cx);
		}
		cx.notify();
	}

	fn step(&mut self, monitor: Monitor, delta: i64, cx: &mut Context<Self>) {
		let length = self.sequence_length();
		let clock = self.clock(monitor).clone();
		clock.update(cx, |clock, cx| {
			clock.transport.step(delta, length);
			if clock.transport.is_playing() {
				clock.play();
			}
			cx.notify();
		});
		self.mirror_program_playhead(cx);
		cx.notify();
	}

	fn tick(&mut self, cx: &mut Context<Self>) {
		// Keep the playback clocks on the sequence's actual rate. The
		// clocks are created at engine init with the fallback rate
		// (hd_1080p25), before any sequence is loaded; a sequence whose
		// video params say otherwise (e.g. 29.97 from the config default)
		// would otherwise play on a stale clock — the audio chunks are
		// sized by the SEQUENCE time base while the playhead advances at
		// the CLOCK rate, and a mismatch chronically underfeeds the
		// output device (every few words a slice of silence).
		let seq_rate = self.frame_rate();
		for clock in [&self.source_clock, &self.program_clock] {
			let stale = {
				let r = &clock.read(cx).rate;
				r.num != seq_rate.num || r.den != seq_rate.den
			};
			if stale {
				clock.update(cx, |c, _cx| c.rate = seq_rate);
			}
		}
		// Each clock loops at its own monitor's length: the program at the
		// sequence length, the source at the selected footage's duration
		// (the sequence length is wrong for footage playback — an empty
		// project's length 0 used to freeze the source playhead at 0).
		let length = self.sequence_length();
		let source_length = self.source_length();
		let stop_on_last = self.stop_on_last();
		for (clock, len) in [
			(&self.source_clock, source_length),
			(&self.program_clock, length),
		] {
			let clock = clock.clone();
			clock.update(cx, |clock, cx| {
				clock.tick(len, stop_on_last);
				cx.notify();
			});
		}
		// M12 P1a: while the program plays with an audio anchor, re-anchor
		// the playhead onto the frame the output device is actually
		// consuming. The wall clock anchored play() at the click instant,
		// but the first chunk reaches the device 50-150 ms later (tick
		// interval + inline mix + cpal open) — a fixed startup offset every
		// play run — and underruns pad the output with silence while still
		// advancing `output_frames_consumed`, so playback also drifts.
		// `manager.seconds()` is the truth: the playhead is set to the
		// audible frame (loop playback wraps with the sequence). The strict
		// `secs > start_secs` guard is the fail-safe: if the device never
		// opens, seconds() stays 0/-1 and playback falls back to the wall
		// clock untouched.
		if self.program_playing && self.program_clock.read(cx).is_playing() {
			if let Some((start_frame, start_secs)) = self.audio_playback {
				if let Some(secs) = quercus_engine_rs::audio::seconds() {
					if secs > start_secs {
						let rate = self.program_clock.read(cx).frame_rate();
						let mut target = audio_target_frame(start_frame, start_secs, secs, rate);
						if length.0 > 0 && target >= length.0 {
							target %= length.0;
						}
						let cur = self.clock_frame(Monitor::Program, cx).0;
						if (target - cur).abs() > 2 {
							audio_dbg(&format!(
								"playhead reanchor jump {cur} -> {target} (secs {secs:.3}, anchor f{start_frame}@{start_secs:.3})"
							));
						}
						let clock = self.program_clock.clone();
						clock.update(cx, |clock, cx| {
							// Re-anchor (not transport.seek): the next
							// wall-clock tick recomputes from this anchor,
							// so the playhead keeps following the output
							// device's consumed-sample clock instead of
							// snapping back to the click-instant anchor.
							clock.reanchor_while_playing(Frame(target), length);
							cx.notify();
						});
					} else if secs < 0.0 {
						// No running output stream (device closed): drop the
						// anchor and stay on the wall clock.
						self.audio_playback = None;
					}
				} else {
					self.audio_playback = None;
				}
			}
		}
		// A Stop-on-Last clock pauses itself at the last frame; mirror
		// that into the engine's play state like pause() (the transport is
		// already stopped, so the output must go silent too — otherwise
		// the last frame's audio re-pushes and the UI re-renders forever).
		if self.program_playing && !self.program_clock.read(cx).is_playing() {
			self.stop_program_playback(cx);
		}
		self.mirror_program_playhead(cx);
		self.update_ofx_viewer_time(cx);
		self.meter_phase = self.meter_phase.wrapping_add(1);
		// M15 S2: pump the process dispatcher — ticket completions (the
		// pre-render window, full-res fills, synchronous renders) are
		// delivered from its poll loop, which must run on the UI tick.
		// Then feed the playback pre-render windows before draining the
		// completion channels.
		quercus_engine_rs::render::manager_poll();
		self.update_preview_window(Monitor::Source, cx);
		self.update_preview_window(Monitor::Program, cx);
		// M12 P1: while the program plays, pull the audio for the
		// current playhead window and queue it for the output device.
		if self.program_playing {
			self.pull_audio_tick(cx);
		}
		// M12 P5a: install finished full-resolution frames and schedule
		// the next fills for the resting playheads (the schedule skips
		// playing monitors, so playback keeps the proxy path).
		self.drain_full_res();
		// 后台波形提取落盘（version 递增）→ 重绘时间轴显示波形。
		if let Some(cache) = self.waveform_cache() {
			let v = cache.version();
			if v != self.waveform_version_seen {
				self.waveform_version_seen = v;
				cx.notify();
			}
		}
		self.drain_thumbnails();
		self.drain_proxy_runs(cx);
		self.drain_multicam_frames(cx);
		self.schedule_full_res(Monitor::Source, cx);
		self.schedule_full_res(Monitor::Program, cx);
		// Debounced snapshot upload: burst edits (multicam switching)
		// coalesce into one worker load instead of per-edit serialization.
		self.flush_snapshot_if_due(cx);
		cx.notify();
	}
}

// ---------------------------------------------------------------------------
// Data-source traits
// ---------------------------------------------------------------------------

impl TimelineDataSource for RealEngine {
	type Track = RealTrack;

	fn frame_rate(&self) -> FrameRate {
		self.sequence_info
			.as_ref()
			.map(|s| s.format.rate)
			.unwrap_or(VideoFormat::hd_1080p25().rate)
	}

	fn sequence_length(&self) -> Frame {
		self.sequence_length()
	}

	fn track_count(&self) -> usize {
		self.tracks.len()
	}

	fn track(&self, index: usize) -> Option<Self::Track> {
		self.tracks.get(index).cloned()
	}

	fn markers(&self) -> Vec<Marker> {
		let Some(session) = self.editor_session() else {
			return Vec::new();
		};
		let Some(tb) = self.time_base() else {
			return Vec::new();
		};
		graphops::markers(&session)
			.into_iter()
			.map(|(time, name, color)| Marker {
				frame: Frame(graphops::rational_to_ts(time, tb)),
				label: name.into(),
				color: Some(marker_color(color)),
			})
			.collect()
	}
}

impl EffectStackDataSource for RealEngine {
	fn effects(&self) -> Vec<Arc<dyn EffectData>> {
		self.selected_effect_cards()
	}

	fn target_label(&self) -> Option<SharedString> {
		let label = self.selected_clip_label()?;
		let project = self.project_ref()?;
		let host = self.selected_clip_node()?;
		// A clip that cannot host effects keeps the empty state (no label,
		// no cards).
		super::effectchain::effect_input_of(project, host)?;
		Some(label)
	}

	fn selected_effect(&self) -> Option<EffectId> {
		self.inspector_selection().1
	}
}

impl NodeGraphDataSource for RealEngine {
	type Node = RealNode;
	type Edge = RealEdge;

	fn nodes(&self) -> Vec<Self::Node> {
		let (Some(project), Some(seq)) = (self.project_ref(), self.sequence) else {
			return Vec::new();
		};
		match self.selected_clip {
			// While one clip is selected the editor shows that clip's
			// context chain (footage → effects → clip) instead of the whole
			// sequence graph.
			Some(clip) => crate::oakui::nodegraph::build_graph_for_clip(project, seq, clip.0).0,
			None => crate::oakui::nodegraph::build_graph(project, seq).0,
		}
	}

	fn edges(&self) -> Vec<Self::Edge> {
		let (Some(project), Some(seq)) = (self.project_ref(), self.sequence) else {
			return Vec::new();
		};
		match self.selected_clip {
			Some(clip) => crate::oakui::nodegraph::build_graph_for_clip(project, seq, clip.0).1,
			None => crate::oakui::nodegraph::build_graph(project, seq).1,
		}
	}

	fn can_connect(&self, from: crate::widgets::node_graph::PortId, to: crate::widgets::node_graph::PortId) -> bool {
		let Some(project) = self.project_ref() else {
			return false;
		};
		crate::oakui::nodegraph::can_connect(project, from, to)
	}
}

impl ProjectDataSource for RealEngine {
	fn roots(&self) -> Vec<ProjectEntry> {
		let Some(project) = self.project_ref() else {
			return Vec::new();
		};
		self.attach_thumbnails(crate::oakui::projectbrowser::roots(project))
	}

	fn children(&self, parent_id: u64) -> Vec<ProjectEntry> {
		let Some(project) = self.project_ref() else {
			return Vec::new();
		};
		self.attach_thumbnails(crate::oakui::projectbrowser::children(project, parent_id))
	}
}

impl AudioMeterDataSource for RealEngine {
	fn levels(&self) -> Vec<f32> {
		// Per-channel linear peaks of the engine's buffered audio output
		// (clamped to the meter's 0..1 range). Silent when nothing has been
		// pushed to the output or without an AudioManager instance.
		let mut peaks = [0.0f32; 8];
		let Some(n) = quercus_engine_rs::audio::output_levels(&mut peaks) else {
			return vec![0.0, 0.0];
		};
		if n <= 0 {
			return vec![0.0, 0.0];
		}
		peaks[..n as usize]
			.iter()
			.map(|p| p.clamp(0.0, 1.0))
			.collect()
	}
}

// ---------------------------------------------------------------------------
// AppEngine
// ---------------------------------------------------------------------------

impl Drop for RealEngine {
	/// Cancel outstanding SDK previews before their project owner disappears.
	fn drop(&mut self) {
		self.cancel_preview_windows();
		set_live_session(None);
	}
}

impl AppEngine for RealEngine {
	type Clock = RealClock;

	fn create(cx: &mut Context<Self>) -> Self {
		Self::new(cx)
	}

	fn source_clock(&self) -> &Entity<Self::Clock> {
		&self.source_clock
	}

	fn program_clock(&self) -> &Entity<Self::Clock> {
		&self.program_clock
	}

	fn clock_frame(&self, monitor: Monitor, cx: &App) -> Frame {
		self.clock(monitor).read(cx).transport.frame()
	}

	fn cpu_frame(&self, monitor: Monitor, cx: &App) -> Arc<RenderImage> {
		let frame = self.clock_frame(monitor, cx);
		let mut cache = self.cpu_frame_cache.lock().unwrap();
		// A display-color transform change (mode / ICC / content space)
		// invalidates every cached image: they were produced with the old
		// transform.
		let generation = super::displaycolor::generation();
		if generation != self.display_color_gen.get() {
			self.display_color_gen.set(generation);
			cache.clear();
		}
		// Land the current proxy first, even when its full-res fill won the
		// race: scopes are tied to this proxy pass and must not starve behind
		// the full-resolution cache hit.
		if let Some((image, scope)) = self.preview_slot_frame(monitor, frame) {
			cache.entry(monitor).or_default().proxy = Some(ProxyEntry {
				frame: frame.0,
				image: image.clone(),
				scope,
			});
			return image;
		}
		// The full-resolution fill replaces the proxy when its frame matches
		// the playhead; otherwise the proxy frame is displayed.
		if let Some(image) = cache.entry(monitor).or_default().image_for(frame.0) {
			return image.clone();
		}
		// A cache miss never blocks the UI thread. Show the last displayed
		// frame while the SDK request and resting full-res fill are in flight.
		if let Some(image) = cache
			.get(&monitor)
			.and_then(|e| e.proxy.as_ref().map(|p| p.image.clone()))
		{
			return image;
		}
		// A completely cold cache returns its temporary pattern immediately.
		let (width, height, samples) = synthetic_frame_samples(frame);
		let image = f32_rgba_to_bgra_image(width, height, &samples);
		super::gpu::register_display_frame(image.id.0, width, height, &samples);
		Arc::new(image)
	}

	fn scope_data(&self, monitor: Monitor, cx: &App) -> ScopeData {
		// Ensure the cache holds the current playhead frame (the analysis
		// runs inside that render pass, so this never re-walks a frame).
		let _ = self.cpu_frame(monitor, cx);
		let frame = self.clock_frame(monitor, cx);
		let cache = self.cpu_frame_cache.lock().unwrap();
		cache
			.get(&monitor)
			.and_then(|entry| entry.scope_for(frame.0))
			.cloned()
			.unwrap_or_default()
	}

	fn add_track(&mut self, kind: TrackKind, cx: &mut Context<Self>) {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			return;
		};
		let result = graphops::add_track(&project, seq, track_type_of(kind)).map(|_| ());
		self.apply_edit(result, "add track", cx);
	}

	fn remove_track(&mut self, index: usize, cx: &mut Context<Self>) {
		let Some(track) = self.tracks.get(index) else {
			return;
		};
		let track_id = track.track;
		let Some(project) = self.project.clone() else {
			return;
		};
		self.apply_edit(
			graphops::remove_track(&project, track_id),
			"remove track",
			cx,
		);
	}

	fn add_adjustment_layer(
		&mut self,
		index: usize,
		frame: Frame,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(project) = self.project.clone() else {
			return Err("add adjustment layer: no project is open".to_string());
		};
		let Some(seq) = self.sequence else {
			return Err("add adjustment layer: no sequence is open".to_string());
		};
		let Some(tb) = self.time_base() else {
			return Err("add adjustment layer: the sequence has no frame rate".to_string());
		};
		let Some(track) = self.tracks.get(index).map(|t| t.track) else {
			return Err(format!("add adjustment layer: no track at index {index}"));
		};
		// The default span: five seconds from the clicked frame (the
		// snapshot's non-snapping approximation of the C++ default).
		let fps = tb.1 as f64 / tb.0.max(1) as f64;
		let length = (5.0 * fps).round().max(1.0) as i64;
		let in_ts = frame.0.max(0);
		let result = graphops::create_adjustment_layer(&project, seq, track, in_ts, in_ts + length);
		self.apply_edit(result.clone().map(|_| ()), "add adjustment layer", cx);
		result.map(|_| ())
	}

	fn set_track_height(&mut self, height: Pixels, cx: &mut Context<Self>) {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			return;
		};
		let internal = quercus_engine_rs::node::internal_height_from_pixels(f32::from(height) as i32);
		let ids: Vec<NodeId> = [
			DomainTrackKind::Video,
			DomainTrackKind::Audio,
			DomainTrackKind::Subtitle,
		]
		.into_iter()
		.flat_map(|kind| graphops::track_ids(&project, seq, kind))
		.collect();
		for track in ids {
			graphops::set_track_height(&project, track, internal);
		}
		self.rebuild_timeline();
		cx.notify();
	}

	fn select_item(&mut self, id: u64, cx: &mut Context<Self>) {
		let changed = self.selected_item != Some(id);
		self.selected_item = Some(id);
		if changed {
			// The source monitor renders the selected footage node: a new
			// selection must drop the stale cached frame (the cache key only
			// tracks the playhead frame), and any in-flight full-res job for
			// the old selection is stale. The source pre-render window (M15
			// S2) rebuilds against the new footage.
			self.cpu_frame_cache
				.lock()
				.unwrap()
				.remove(&Monitor::Source);
			self.full_res_generation = self.full_res_generation.wrapping_add(1);
			self.cancel_preview_window(Monitor::Source);
		}
		cx.notify();
	}

	fn set_selected_clips(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		// The effect stack targets exactly one clip: an empty or
		// multi-clip selection keeps the empty state (see
		// `EffectStackDataSource::target_label`). The node graph follows:
		// while one clip is selected it shows (and highlights) that clip's
		// context chain, so its block node becomes the graph selection.
		self.selected_clip = (clips.len() == 1).then(|| clips[0]);
		self.selected_clips = clips;
		self.selected_graph_node = self.selected_clip.map(|clip| clip.0);
		cx.notify();
	}

	fn selected_graph_node(&self) -> Option<u64> {
		self.selected_graph_node
	}

	/// The open project graph's virtual endpoint pair, if any: the node
	/// editor must not let the user delete, copy or rename them.
	fn protected_graph_nodes(&self) -> Vec<crate::widgets::node_graph::NodeId> {
		let Some(project) = self.project.clone() else {
			return Vec::new();
		};
		match project.graph_endpoints() {
			Some((input, output)) => vec![
				crate::widgets::node_graph::NodeId(input.raw()),
				crate::widgets::node_graph::NodeId(output.raw()),
			],
			None => Vec::new(),
		}
	}

	fn addable_effects(&self) -> Vec<crate::oakui::engine::EffectEntry> {
		super::effectchain::addable_effects()
	}

	fn add_effect(
		&mut self,
		index: usize,
		type_id: &str,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(session) = self.editor_session() else {
			return Err("no project open".into());
		};
		let Some(host) = self.selected_clip_node() else {
			return Err("no selected clip".into());
		};
		let result = session
			.execute(EditCommand::EffectInsert {
				host: quercus_engine_rs::node::NodeId::from_raw(host.raw()),
				index,
				type_id: type_id.to_string(),
			})
			.map(|_| ());
		self.apply_edit(result.clone(), "add effect", cx);
		result
	}

	/// Append the effect to `clip`'s chain end (the timeline's
	/// drop-effect-on-a-clip gesture), one undoable entry.
	fn add_effect_to_clip(
		&mut self,
		clip: ClipId,
		type_id: &str,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(session) = self.editor_session() else {
			return Err("no project open".into());
		};
		let Some(host) = graphops::id_of(clip.0) else {
			return Err("clip not found".into());
		};
		if self.clip_track_locked(host) {
			return Err("the clip's track is locked".into());
		}
		let result = session
			.execute(EditCommand::EffectInsert {
				host: quercus_engine_rs::node::NodeId::from_raw(host.raw()),
				index: usize::MAX,
				type_id: type_id.to_string(),
			})
			.map(|_| ());
		self.apply_edit(result.clone(), "add effect", cx);
		result
	}

	fn effect_params(&self, effect: EffectId) -> Option<Vec<crate::oakui::engine::EffectParam>> {
		let session = self.editor_session()?;
		let node = graphops::id_of(effect.0)?;
		let node = quercus_engine_rs::node::NodeId::from_raw(node.raw());
		let snapshot = session.parameters(node)?;
		let type_id = session.project().node_type_id(node).unwrap_or_default();
		Some(super::effectchain::params_from_sdk(&snapshot, &type_id))
	}

	fn set_effect_param(
		&mut self,
		effect: EffectId,
		input_id: &str,
		value: super::engine::EffectValue,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(session) = self.editor_session() else {
			return Err("no project open".into());
		};
		let Some(node) = graphops::id_of(effect.0) else {
			return Err("effect node not found".into());
		};
		let node = quercus_engine_rs::node::NodeId::from_raw(node.raw());
		let result = super::effectchain::set_sdk_input_value(&session, node, input_id, value);
		self.apply_edit(result.clone(), "set parameter", cx);
		result
	}

	fn effect_push_button(
		&mut self,
		effect: EffectId,
		input_id: &str,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(session) = self.editor_session() else {
			return Err("no project open".into());
		};
		let Some(node) = graphops::id_of(effect.0) else {
			return Err("effect node not found".into());
		};
		session
			.execute(EditCommand::PushNodeButton {
				node: quercus_engine_rs::node::NodeId::from_raw(node.raw()),
				input_id: input_id.to_string(),
			})
			.map_err(|error| format!("push button \"{input_id}\" failed: {error}"))?;
		// A button press can change the plugin's other parameters; refresh
		// the snapshots and repaint.
		cx.notify();
		Ok(())
	}

	fn ofx_interact_target(&self, _cx: &App) -> Option<u64> {
		let Some(session) = self.editor_session() else {
			return None;
		};
		let Some(host) = self.selected_clip_node() else {
			return None;
		};
		let chain = session
			.project()
			.effect_chain(quercus_engine_rs::node::NodeId::from_raw(host.raw()));
		let project = self.project_ref()?;
		for node in chain {
			let node = graphops::id_of(node.raw())?;
			// The inspector's current selection: the first *expanded*
			// OFX plugin card (its parameter UI is on screen, so its
			// custom interact drives the viewer overlay).
			if !self.expanded_effects.contains(&node.raw()) {
				continue;
			}
			return super::effectchain::plugin_instance_handle(project, node);
		}
		None
	}

	fn apply_effect_event(&mut self, event: &EffectStackEvent, cx: &mut Context<Self>) {
		match event {
			EffectStackEvent::EnableToggled { effect, enabled } => {
				let (Some(session), Some(_host)) =
					(self.editor_session(), self.selected_clip_node())
				else {
					cx.notify();
					return;
				};
				let Some(node) = graphops::id_of(effect.0) else {
					cx.notify();
					return;
				};
				let result = session.execute(EditCommand::EffectSetEnabled {
					effect: quercus_engine_rs::node::NodeId::from_raw(node.raw()),
					enabled: *enabled,
				})
				.map(|_| ());
				self.apply_edit(result, "toggle effect", cx);
			}
			EffectStackEvent::ExpansionToggled { effect, expanded } => {
				// View state only (not undoable); kept here so the card
				// list re-reads it after the notify.
				if *expanded {
					self.expanded_effects.insert(effect.0);
				} else {
					self.expanded_effects.remove(&effect.0);
				}
				cx.notify();
			}
			EffectStackEvent::RemoveRequested(id) => {
				let (Some(session), Some(host)) = (self.editor_session(), self.selected_clip_node())
				else {
					cx.notify();
					return;
				};
				let Some(node) = graphops::id_of(id.0) else {
					cx.notify();
					return;
				};
				let result = session.execute(EditCommand::EffectRemove {
					host: quercus_engine_rs::node::NodeId::from_raw(host.raw()),
					effect: quercus_engine_rs::node::NodeId::from_raw(node.raw()),
				})
				.map(|_| ());
				self.apply_edit(result, "remove effect", cx);
			}
			EffectStackEvent::ReorderRequested { effect, new_index } => {
				let (Some(session), Some(host)) = (self.editor_session(), self.selected_clip_node())
				else {
					cx.notify();
					return;
				};
				let Some(node) = graphops::id_of(effect.0) else {
					cx.notify();
					return;
				};
				let result = session.execute(EditCommand::EffectMove {
					host: quercus_engine_rs::node::NodeId::from_raw(host.raw()),
					effect: quercus_engine_rs::node::NodeId::from_raw(node.raw()),
					new_index: *new_index,
				})
				.map(|_| ());
				self.apply_edit(result, "reorder effect", cx);
			}
			EffectStackEvent::AddRequested { index } => {
				// The effect choice is a panel-owned menu (see
				// `InspectorPanel`); the undoable insert runs through
				// `AppEngine::add_effect` once the user picks a type.
				// `index` is acknowledged here for parity with the
				// request semantics (the panel passes it back).
				let _ = index;
				cx.notify();
			}
			EffectStackEvent::AddTypeRequested { index, type_id } => {
				// A drag-and-drop add from the effect library: the type is
				// already chosen (normally handled by the inspector panel;
				// applied here too so direct event drives work).
				let (index, type_id) = (*index, type_id.clone());
				let result = self.add_effect(index, &type_id, cx);
				if let Err(err) = result {
					println!("[real engine] drop-add effect failed: {err}");
					cx.notify();
				}
			}
			EffectStackEvent::CardSelected { effect } => {
				// The inspector card click selects the effect's node in the
				// node editor (the bidirectional node↔inspector link). The
				// node editor panel observes the engine and pushes the
				// highlight into the graph widget.
				self.selected_graph_node = Some(effect.0);
				cx.notify();
			}
			// The app owns the context menu; parameter changes have no
			// metadata to refresh yet.
			EffectStackEvent::ContextMenuRequested { .. }
			| EffectStackEvent::ParameterChanged { .. } => {
				cx.notify();
			}
		}
	}

	fn node_library(&self) -> Vec<super::engine::NodeLibraryEntry> {
		super::nodegraph::node_library()
	}

	fn apply_node_graph_event(&mut self, event: &NodeGraphEvent, cx: &mut Context<Self>) {
		match event {
			// Preview events never persist: the widget draws dragged nodes at
			// their model position plus the preview delta, and the position
			// is written back (as one undoable step) only when the drag ends
			// in `NodeMoveRequested`.
			NodeGraphEvent::NodeMovePreview { .. }
			| NodeGraphEvent::ViewChanged { .. }
			| NodeGraphEvent::BackgroundClicked { .. }
			// The node editor panel answers the right-click itself (it owns
			// the popup); the engine has nothing to apply.
			| NodeGraphEvent::NodeContextMenuRequested { .. } => {}
			NodeGraphEvent::SelectionChanged { nodes } => {
				// The node-graph selection is the inspector's shared
				// selection: a single selected node is mirrored into
				// `selected_graph_node`, and when it names an effect of the
				// targeted clip's chain its card is expanded too (so the
				// inspector shows that effect's params).
				let prev = self.selected_graph_node;
				self.selected_graph_node = (nodes.len() == 1).then(|| {
					nodes
						.iter()
						.next()
						.expect("a one-element set always yields an item")
					.0
				});
				if prev != self.selected_graph_node {
					if let Some(effect) = self.selected_effect() {
						self.expanded_effects.insert(effect.0);
					}
				}
			}
			_ => {
				if let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) {
					// The node editor shows the selected clip's chain while
					// one is selected; the move base must come from that
					// view's fallback grid, not the whole-sequence one.
					let result = crate::oakui::nodegraph::apply_edit_in_view(
						&project,
						seq,
						self.selected_clip.map(|clip| clip.0),
						event,
					);
					if let Err(e) = result {
						println!("[real engine] node-graph request rejected: {e}");
					}
				}
			}
		}
		cx.notify();
	}

	fn apply_timeline_event(&mut self, event: &TimelineEvent, cx: &mut Context<Self>) {
		match event {
			TimelineEvent::PlayheadChanged(frame) => {
				let current = self.clock_frame(Monitor::Program, cx);
				if *frame != current {
					self.request_frame(Monitor::Program, *frame, cx);
				}
			}
			TimelineEvent::ClipTrimRequested {
				clip,
				edge,
				new_frame,
			} => {
				let Some(block) = self.clip_block(*clip) else {
					return;
				};
				if self.clip_track_locked(block) {
					return;
				}
				let Some(project) = self.project.clone() else {
					return;
				};
				// Re-read the clip's current range, then compute the new
				// in/out pair for the trim.
				let Some(tb) = self.time_base() else {
					return;
				};
				let (in_ts, out_ts) = {
					let Some((in_r, out_r, _)) = graphops::clip_range(&project, block) else {
						return;
					};
					(
						graphops::rational_to_ts(in_r, tb),
						graphops::rational_to_ts(out_r, tb),
					)
				};
				let (new_in, new_out) = match edge {
					TrimEdge::Start => (new_frame.0, out_ts),
					TrimEdge::End => (in_ts, new_frame.0),
				};
				let result = graphops::trim_clip(&project, block, new_in, new_out);
				self.apply_edit(result, "trim clip", cx);
			}
			TimelineEvent::ClipSplitRequested { clip, time } => {
				self.split_clip(*clip, *time, cx);
			}
			TimelineEvent::ClipRippleTrimRequested {
				clip,
				edge,
				new_frame,
			} => {
				let Some(block) = self.clip_block(*clip) else {
					return;
				};
				if self.clip_track_locked(block) {
					return;
				}
				let Some(project) = self.project.clone() else {
					return;
				};
				let result = graphops::ripple_trim_clip(
					&project,
					block,
					matches!(edge, TrimEdge::Start),
					new_frame.0,
				);
				self.apply_edit(result, "ripple trim clip", cx);
			}
			TimelineEvent::ClipRollRequested {
				clip_a,
				clip_b,
				new_frame,
			} => {
				let (Some(block_a), Some(project)) =
					(self.clip_block(*clip_a), self.project.clone())
				else {
					return;
				};
				if self.clip_track_locked(block_a) {
					return;
				}
				let Some(block_b) = self.clip_block(*clip_b) else {
					return;
				};
				// The roll pair lives on one track; locate that track's node.
				let Some(track) = self
					.tracks
					.iter()
					.find(|t| t.clips.iter().any(|c| c.block == block_a))
					.map(|t| t.track)
				else {
					return;
				};
				let result = graphops::roll_edit(&project, track, block_a, block_b, new_frame.0);
				self.apply_edit(result, "roll edit", cx);
			}
			TimelineEvent::ClipSlideRequested { clip, new_start } => {
				let Some(block) = self.clip_block(*clip) else {
					return;
				};
				if self.clip_track_locked(block) {
					return;
				}
				let Some(project) = self.project.clone() else {
					return;
				};
				let result = graphops::slide_clip(&project, block, new_start.0);
				self.apply_edit(result, "slide clip", cx);
			}
			TimelineEvent::ClipSlipRequested { clip, new_media_in } => {
				let Some(block) = self.clip_block(*clip) else {
					return;
				};
				if self.clip_track_locked(block) {
					return;
				}
				let Some(project) = self.project.clone() else {
					return;
				};
				let result = graphops::slip_clip(&project, block, new_media_in.0);
				self.apply_edit(result, "slip clip", cx);
			}
			TimelineEvent::ClipMoveRequested {
				clip,
				new_track,
				new_start,
			} => {
				// The dragged clip moves to `new_track`/`new_start`; the rest
				// of the group follows in lockstep, each staying on its own
				// track and shifting by the same frame offset. The group is
				// the dragged clip's transitive link group (the A/V pair
				// dropped from one file) UNION — when the dragged clip is
				// part of the multi-selection — every other selected clip
				// and THEIR link groups, so a marquee selection moves
				// together with relative positions intact. All of it lands
				// as ONE undoable entry.
				let Some(block) = self.clip_block(*clip) else {
					return;
				};
				if self.clip_track_locked(block) {
					return;
				}
				let Some(project) = self.project.clone() else {
					return;
				};
				// Seed the group: the dragged clip, plus every selected clip
				// when the drag started inside the multi-selection (a plain
				// drag of an unselected clip moves just its link group).
				let mut seeds: Vec<NodeId> = vec![block];
				if self.selected_clips.contains(clip) {
					for id in self.selected_clips.clone() {
						if let Some(selected) = self.clip_block(id) {
							seeds.push(selected);
						}
					}
				}
				// Transitive closure over the graph links (A-B-C chains move
				// as one), restricted to clips; clips on locked tracks are
				// left in place.
				let mut seen: BTreeSet<NodeId> = seeds.iter().copied().collect();
				let mut stack = seeds;
				let mut linked: Vec<NodeId> = Vec::new();
				{
					while let Some(node) = stack.pop() {
						if node != block {
							linked.push(node);
						}
						for other in project.block_links(node) {
							if project.node_kind(other) == Some(NodeKind::Clip)
								&& seen.insert(other)
							{
								stack.push(other);
							}
						}
					}
				}
				let linked: Vec<NodeId> = linked
					.into_iter()
					.filter(|&other| !self.clip_track_locked(other))
					.collect();
				let current_track = self
					.tracks
					.iter()
					.position(|t| t.clips.iter().any(|c| c.block == block));
				// Cross-track moves go through the gap + re-home + place
				// composition; same-track moves use the plain move command.
				// A clip may only move between tracks of the same kind: a
				// video → audio (or audio → video) drop is silently ignored
				// (no error, no effect).
				let dest_track = match (current_track, self.tracks.get(*new_track)) {
					(Some(current), _) if current == *new_track => None,
					(Some(current), Some(dest)) if self.tracks[current].kind == dest.kind => {
						Some(dest.track)
					}
					(Some(_), Some(_)) => {
						return;
					}
					_ => {
						self.apply_edit(
							Err("move clip: destination track out of range".to_string()),
							"move clip",
							cx,
						);
						return;
					}
				};
				let result = graphops::move_clip_with_links(
					&project,
					block,
					dest_track,
					new_start.0,
					&linked,
				);
				self.apply_edit(result, "move clip", cx);
			}
			TimelineEvent::TrackHeightChanged { track, height } => {
				if let (Some(t), Some(project)) = (self.tracks.get(*track), self.project.clone()) {
					let internal = quercus_engine_rs::node::internal_height_from_pixels(
						f32::from(*height) as i32,
					);
					graphops::set_track_height(&project, t.track, internal);
					self.rebuild_timeline();
				}
				cx.notify();
			}
			TimelineEvent::TransitionChanged {
				clip,
				edge,
				new_length,
			} => {
				let Some(block) = self.clip_block(*clip) else {
					return;
				};
				if self.clip_track_locked(block) {
					return;
				}
				let Some(project) = self.project.clone() else {
					return;
				};
				let Some(tb) = self.time_base() else {
					return;
				};
				// The widget works in the sequence's frame unit, the
				// command layer in seconds.
				let frame = graphops::ts_to_rational(1, tb);
				let requested = graphops::ts_to_rational(new_length.0.max(1), tb);
				let result = graphops::set_transition_length(
					&project,
					block,
					matches!(edge, TrimEdge::Start),
					frame,
					requested,
				);
				self.apply_edit(result.map(|_| ()), "transition length", cx);
			}
			// Selection / zoom / track-selected / context-menu:
			// not editable (the right-click is answered by the panel's popup).
			TimelineEvent::SelectionChanged
			| TimelineEvent::TrackSelected { .. }
			| TimelineEvent::ContextMenuRequested { .. }
			| TimelineEvent::ZoomChanged(_) => {}
			TimelineEvent::TrackToggleRequested { track, toggle } => {
				// The header toggles map onto the undoable track flag
				// setters. The muted flag doubles as the video/subtitle
				// visibility toggle (Olive parity); the model has no solo
				// flag yet, so solo requests are inert.
				let (Some(t), Some(project)) = (self.tracks.get(*track), self.project.clone())
				else {
					return;
				};
				let result = match toggle {
					TrackHeaderEvent::ToggleLock => {
						graphops::set_track_locked(&project, t.track, !t.locked)
					}
					TrackHeaderEvent::ToggleMute => {
						graphops::set_track_muted(&project, t.track, !t.muted)
					}
					TrackHeaderEvent::ToggleVisibility => {
						graphops::set_track_muted(&project, t.track, t.visible)
					}
					TrackHeaderEvent::ToggleSolo => Ok(()),
				};
				self.apply_edit(result, "toggle track flag", cx);
			}
			TimelineEvent::WorkAreaPreview { start, end } => {
				self.set_workarea_preview(*start, *end, cx);
			}
			TimelineEvent::WorkAreaCommitted {
				start,
				end,
				old_start,
				old_end,
			} => {
				self.commit_workarea(*old_start, *old_end, *start, *end, cx);
			}
		}
	}

	fn split_clip(&mut self, clip: ClipId, time: Frame, cx: &mut Context<Self>) {
		let (Some(block), Some(project)) = (self.clip_block(clip), self.project.clone()) else {
			return;
		};
		if self.clip_track_locked(block) {
			return;
		}
		let result = graphops::split_clip(&project, block, time.0);
		self.apply_edit(result, "split clip", cx);
	}

	fn split_at_playhead(&mut self, cx: &mut Context<Self>) {
		let frame = self.clock_frame(Monitor::Program, cx);
		let Some(project) = self.project.clone() else {
			return;
		};
		let targets: Vec<NodeId> = self
			.tracks
			.iter()
			.filter(|track| !track.locked)
			.flat_map(|track| {
				track.clips.iter().filter_map(|clip| {
					if clip.range.start.0 < frame.0 && frame.0 < clip.range.end.0 {
						Some(clip.block)
					} else {
						None
					}
				})
			})
			.collect();
		// ONE preserving-links split for the whole set: linked pairs (the
		// A/V drop) get their rear halves linked too — the per-block plain
		// split left the rear halves unlinked.
		let result = graphops::split_clips_preserving_links(&project, &targets, frame.0);
		self.apply_edit(result, "split at playhead", cx);
	}

	fn workarea(&self) -> Option<(Frame, Frame)> {
		let session = self.editor_session()?;
		let tb = self.time_base()?;
		let (enabled, range) = graphops::workarea_state(&session);
		if !enabled {
			return None;
		}
		Some((
			Frame(graphops::rational_to_ts(range.in_, tb)),
			Frame(graphops::rational_to_ts(range.out, tb)),
		))
	}

	fn add_marker_at_playhead(&mut self, cx: &mut Context<Self>) {
		let (Some(session), Some(tb)) = (self.editor_session(), self.time_base()) else {
			return;
		};
		let frame = self.clock_frame(Monitor::Program, cx);
		let time = graphops::ts_to_rational(frame.0, tb);
		let result = graphops::marker_add(&session, time, "", 0);
		self.apply_edit(result, "add marker", cx);
	}

	fn remove_marker_at_playhead(&mut self, cx: &mut Context<Self>) {
		let (Some(session), Some(tb)) = (self.editor_session(), self.time_base()) else {
			return;
		};
		let frame = self.clock_frame(Monitor::Program, cx);
		let time = graphops::ts_to_rational(frame.0, tb);
		// Removing a marker that is not there is a benign no-op for the menu
		// action; only rebuild on success.
		let Ok(()) = graphops::marker_remove(&session, time) else {
			return;
		};
		self.apply_edit(Ok(()), "remove marker", cx);
	}

	fn set_workarea_preview(&mut self, start: Frame, end: Frame, cx: &mut Context<Self>) {
		let (Some(session), Some(tb)) = (self.editor_session(), self.time_base()) else {
			return;
		};
		// Live, non-undoable: the engine workarea tracks the drag so other
		// reads (export, snap) stay current; no timeline rebuild needed — the
		// band itself is widget-local state.
		graphops::workarea_set(
			&session,
			true,
			TimeRange::new(
				graphops::ts_to_rational(start.0, tb),
				graphops::ts_to_rational(end.0, tb),
			),
		);
		cx.notify();
	}

	fn commit_workarea(
		&mut self,
		old_start: Frame,
		old_end: Frame,
		start: Frame,
		end: Frame,
		cx: &mut Context<Self>,
	) {
		let (Some(session), Some(tb)) = (self.editor_session(), self.time_base()) else {
			return;
		};
		let result = graphops::workarea_set_undoable(
			&session,
			true,
			TimeRange::new(
				graphops::ts_to_rational(start.0, tb),
				graphops::ts_to_rational(end.0, tb),
			),
			TimeRange::new(
				graphops::ts_to_rational(old_start.0, tb),
				graphops::ts_to_rational(old_end.0, tb),
			),
		);
		self.apply_edit(result, "set workarea", cx);
	}

	fn clear_workarea(&mut self, cx: &mut Context<Self>) {
		let (Some(session), Some(tb)) = (self.editor_session(), self.time_base()) else {
			return;
		};
		let (old_start, old_end) = self.workarea().unwrap_or((Frame::ZERO, Frame::ZERO));
		let result = graphops::workarea_set_undoable(
			&session,
			false,
			TimeRange::new(
				graphops::ts_to_rational(old_start.0, tb),
				graphops::ts_to_rational(old_end.0, tb),
			),
			TimeRange::new(
				graphops::ts_to_rational(old_start.0, tb),
				graphops::ts_to_rational(old_end.0, tb),
			),
		);
		self.apply_edit(result, "clear workarea", cx);
	}

	fn delete_clip(&mut self, clip: ClipId, ripple: bool, cx: &mut Context<Self>) {
		let (Some(block), Some(project)) = (self.clip_block(clip), self.project.clone()) else {
			return;
		};
		if self.clip_track_locked(block) {
			return;
		}
		let result = if ripple {
			graphops::ripple_delete_clip(&project, block)
		} else {
			graphops::delete_clip(&project, block)
		};
		self.apply_edit(
			result,
			if ripple {
				"ripple delete clip"
			} else {
				"delete clip"
			},
			cx,
		);
	}

	/// Copy the selected clips to the engine clipboard (C++ Copy): the
	/// clipboard holds footage/range/speed/track-kind per clip, in
	/// timeline order.
	fn clipboard_copy(&mut self, clips: Vec<ClipId>, _cx: &mut Context<Self>) {
		let Some(project) = self.project.clone() else {
			return;
		};
		let blocks: Vec<NodeId> = clips
			.iter()
			.filter_map(|id| graphops::id_of(id.0))
			.collect();
		self.clipboard = graphops::copy_clips(&project, &blocks);
	}

	/// Copy then gap-delete the selected clips (C++ Cut: copy + Delete,
	/// leaving gaps — ripple deletion is the separate Ripple Delete
	/// action).
	fn clipboard_cut(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		self.clipboard_copy(clips.clone(), cx);
		for id in clips {
			self.delete_clip(id, false, cx);
		}
	}

	/// Paste the clipboard at the program playhead (C++ Paste): one
	/// undoable entry, clips keep their relative offsets and links.
	fn clipboard_paste(&mut self, cx: &mut Context<Self>) {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			return;
		};
		if self.clipboard.is_empty() {
			return;
		}
		let playhead = self.clock_frame(Monitor::Program, cx).0.max(0);
		let items = self.clipboard.clone();
		let result = graphops::paste_clips(&project, seq, &items, playhead).map(|_| ());
		self.apply_edit(result, "paste", cx);
	}

	/// Paste the clipboard at the program playhead with ripple (C++ Paste
	/// Insert): the tail shifts right by the pasted span, one undoable
	/// entry.
	fn clipboard_paste_insert(&mut self, cx: &mut Context<Self>) {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			return;
		};
		if self.clipboard.is_empty() {
			return;
		}
		let playhead = self.clock_frame(Monitor::Program, cx).0.max(0);
		let items = self.clipboard.clone();
		let result = graphops::paste_clips_insert(&project, seq, &items, playhead).map(|_| ());
		self.apply_edit(result, "paste insert", cx);
	}

	/// Duplicate the clips right after the selection's end (C++
	/// Duplicate): copy + paste anchored at the latest out point.
	fn duplicate_clips(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			return;
		};
		let blocks: Vec<NodeId> = clips
			.iter()
			.filter_map(|id| graphops::id_of(id.0))
			.filter(|&b| !self.clip_track_locked(b))
			.collect();
		let result = graphops::duplicate_clips(&project, seq, &blocks).map(|_| ());
		self.apply_edit(result, "duplicate", cx);
	}

	/// Rename the clips (C++ Rename): the node label, one undoable entry.
	fn rename_clips(&mut self, clips: Vec<ClipId>, name: String, cx: &mut Context<Self>) {
		let Some(project) = self.project.clone() else {
			return;
		};
		let blocks: Vec<NodeId> = clips
			.iter()
			.filter_map(|id| graphops::id_of(id.0))
			.collect();
		let result = graphops::rename_blocks(&project, &blocks, &name);
		self.apply_edit(result, "rename", cx);
	}

	/// Toggle the enabled flag of the clips (C++ Enable/Disable): the new
	/// state is the inverse of the first clip's, applied to the whole
	/// selection in one undoable entry.
	fn toggle_clips_enabled(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		let Some(project) = self.project.clone() else {
			return;
		};
		let blocks: Vec<NodeId> = clips
			.iter()
			.filter_map(|id| graphops::id_of(id.0))
			.filter(|&b| !self.clip_track_locked(b))
			.collect();
		if blocks.is_empty() {
			return;
		}
		let enabled = !project.block_enabled(blocks[0]);
		let result = graphops::set_blocks_enabled(&project, &blocks, enabled);
		self.apply_edit(result, "enable/disable clips", cx);
	}

	/// Nudge the clips (and their linked groups) by `delta` frames (C++
	/// Nudge Left/Right): one undoable entry per linked group move.
	fn nudge_clips(&mut self, clips: Vec<ClipId>, delta: i64, cx: &mut Context<Self>) {
		let Some(project) = self.project.clone() else {
			return;
		};
		// Expand to the transitive link group and keep one representative
		// per group (the links move together inside move_clip_with_links).
		let mut seen = std::collections::BTreeSet::new();
		let mut first_err = Ok(());
		for id in clips {
			let Some(block) = graphops::id_of(id.0) else {
				continue;
			};
			if !seen.insert(block) {
				continue;
			}
			if self.clip_track_locked(block) {
				continue;
			}
			let (links, in_ts) = {
				let links = project
					.block_links(block)
					.into_iter()
					.filter(|l| seen.insert(*l))
					.collect::<Vec<_>>();
				let tb = self.time_base().unwrap_or((1, 25));
				let in_ts = graphops::rational_to_ts(project.block_in(block), tb);
				(links, in_ts)
			};
			let new_in = in_ts + delta;
			if new_in < 0 {
				continue;
			}
			let result = graphops::move_clip_with_links(&project, block, None, new_in, &links);
			if result.is_err() {
				first_err = result;
				break;
			}
		}
		self.apply_edit(first_err, "nudge", cx);
	}

	/// Trim the clips' in/out edge to the playhead (C++ Move In/Out to
	/// Playhead; with `ripple` the track follows — Ripple to In/Out).
	fn trim_selection_to_playhead(
		&mut self,
		clips: Vec<ClipId>,
		in_edge: bool,
		ripple: bool,
		cx: &mut Context<Self>,
	) {
		let Some(project) = self.project.clone() else {
			return;
		};
		let playhead = self.clock_frame(Monitor::Program, cx).0.max(0);
		let mut first_err = Ok(());
		for id in clips {
			let Some(block) = graphops::id_of(id.0) else {
				continue;
			};
			if self.clip_track_locked(block) {
				continue;
			}
			// Skip when the playhead is outside the clip (or on the wrong
			// side of the trimmed edge).
			let (in_ts, out_ts) = {
				let Some(tb) = self.time_base() else {
					return;
				};
				if project.node_kind(block).is_none() {
					continue;
				}
				(
					graphops::rational_to_ts(project.block_in(block), tb),
					graphops::rational_to_ts(project.block_out(block), tb),
				)
			};
			let valid = if in_edge {
				playhead > in_ts && playhead < out_ts
			} else {
				playhead > in_ts && playhead < out_ts
			};
			if !valid {
				continue;
			}
			let result = if ripple {
				graphops::ripple_trim_clip(&project, block, in_edge, playhead)
			} else if in_edge {
				graphops::trim_clip(&project, block, playhead, out_ts)
			} else {
				graphops::trim_clip(&project, block, in_ts, playhead)
			};
			if result.is_err() {
				first_err = result;
				break;
			}
		}
		self.apply_edit(first_err, "trim to playhead", cx);
	}

	/// Roll the boundary at the clips' in/out edge to the playhead (C++
	/// Edit to In/Out).
	fn roll_selection_to_playhead(
		&mut self,
		clips: Vec<ClipId>,
		at_in: bool,
		cx: &mut Context<Self>,
	) {
		let Some(project) = self.project.clone() else {
			return;
		};
		let playhead = self.clock_frame(Monitor::Program, cx).0.max(0);
		let mut first_err = Ok(());
		for id in clips {
			let Some(block) = graphops::id_of(id.0) else {
				continue;
			};
			if self.clip_track_locked(block) {
				continue;
			}
			let result = graphops::roll_clip_edge_to(&project, block, at_in, playhead);
			if result.is_err() {
				first_err = result;
				break;
			}
		}
		self.apply_edit(first_err, "roll edit", cx);
	}

	/// Delete the workarea's in/out range on every unlocked track (C++
	/// Delete/Ripple Delete In to Out).
	fn delete_in_to_out(&mut self, ripple: bool, cx: &mut Context<Self>) {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			return;
		};
		let Some((in_point, out_point)) = self.workarea() else {
			return;
		};
		let result = graphops::delete_area(
			&project,
			seq,
			in_point.0.max(0),
			out_point.0.max(0),
			ripple,
		);
		self.apply_edit(
			result,
			if ripple {
				"ripple delete in/out"
			} else {
				"delete in/out"
			},
			cx,
		);
	}

	/// Open an undo group for a multi-write parameter gesture (a
	/// color-wheel drag's per-move writes join it, closing as one entry).
	fn begin_param_drag(&mut self, name: &str, cx: &mut Context<Self>) {
		if !quercus_engine_rs::node::undo_group_begin(name) {
			println!("[real engine] begin param drag: failed");
		}
		cx.notify();
	}

	/// Close the gesture's undo group.
	fn end_param_drag(&mut self, cx: &mut Context<Self>) {
		if !quercus_engine_rs::node::undo_group_end() {
			println!("[real engine] end param drag: failed");
		}
		cx.notify();
	}

	/// Set the clips' speed, span unchanged (C++ Speed/Duration).
	fn set_clips_speed(&mut self, clips: Vec<ClipId>, speed: f64, cx: &mut Context<Self>) {
		let Some(project) = self.project.clone() else {
			return;
		};
		let blocks: Vec<NodeId> = clips
			.iter()
			.filter_map(|id| graphops::id_of(id.0))
			.filter(|&b| !self.clip_track_locked(b))
			.collect();
		let result = graphops::set_clips_speed(&project, &blocks, speed);
		self.apply_edit(result, "speed/duration", cx);
	}

	/// Nest the clips into a new sequence and replace them with one
	/// nested clip spanning their range (C++ Nest).
	fn nest_clips(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			return;
		};
		let blocks: Vec<NodeId> = clips
			.iter()
			.filter_map(|id| graphops::id_of(id.0))
			.filter(|&b| !self.clip_track_locked(b))
			.collect();
		let result = graphops::nest_clips(&project, seq, &blocks).map(|_| ());
		self.apply_edit(result, "nest", cx);
	}

	fn can_undo(&self) -> bool {
		self.project.is_some() && quercus_engine_rs::node::undo_can_undo()
	}

	fn can_redo(&self) -> bool {
		self.project.is_some() && quercus_engine_rs::node::undo_can_redo()
	}

	fn undo(&mut self, cx: &mut Context<Self>) {
		if self.project.is_some() {
			quercus_engine_rs::node::undo();
			self.apply_stack_change(cx);
		}
	}

	fn redo(&mut self, cx: &mut Context<Self>) {
		if self.project.is_some() {
			quercus_engine_rs::node::redo();
			self.apply_stack_change(cx);
		}
	}

	fn history_entries(&self) -> Vec<super::HistoryEntry> {
		if self.project.is_none() {
			return Vec::new();
		}
		// The C++ `HistoryModel` lists every row of the engine stack (done
		// first, then the redoable tail); labels are read through the same
		// two-stage contract the C++ `oakengine_undo_command_text` uses.
		let count = quercus_engine_rs::node::undo_count();
		(0..count)
			.map(|row| super::HistoryEntry {
				name: quercus_engine_rs::node::undo_command_name(row).unwrap_or_default(),
				done: quercus_engine_rs::node::undo_command_done(row).unwrap_or(false),
			})
			.collect()
	}

	fn history_index(&self) -> i64 {
		if self.project.is_some() {
			quercus_engine_rs::node::undo_index()
		} else {
			0
		}
	}

	fn jump_history(&mut self, index: i64, cx: &mut Context<Self>) {
		if self.project.is_some() {
			quercus_engine_rs::node::undo_jump(index);
			self.apply_stack_change(cx);
		}
	}

	fn new_project(&mut self, cx: &mut Context<Self>) {
		let project = graphops::create_project();
		self.adopt_project(project, cx);
	}

	fn open_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
		let ext = path
			.extension()
			.and_then(|e| e.to_str())
			.map(|e| e.to_ascii_lowercase());
		match ext.as_deref() {
			Some("otio") | Some("fcpxml") => self.open_interchange(&path, cx),
			_ => self.open_ove(&path, cx),
		}
	}

	fn export_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
		if self.project.is_none() {
			return Err("no project open".into());
		}
		let ext = path
			.extension()
			.and_then(|e| e.to_str())
			.map(|e| e.to_ascii_lowercase());
		match ext.as_deref() {
			Some("otio") | Some("fcpxml") => self.export_interchange(&path, cx),
			_ => self.export_ove(&path, cx),
		}
	}

	fn close_project(&mut self, cx: &mut Context<Self>) {
		self.drop_project();
		cx.notify();
	}

	fn import_footage(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
		let Some(project) = self.project.clone() else {
			return Err("no project open".into());
		};
		graphops::import_footage(&project, &path)?;
		// The material bin reads the folder tree live from the graph, so a
		// notify is enough for the explorer to list the new entry.
		cx.notify();
		// 全局代理开关开启时，新素材的代理在后台自动生成。
		self.autostart_proxies(cx);
		Ok(())
	}

	fn entry_path(&self, id: u64) -> Option<PathBuf> {
		let project = self.project.clone()?;
		let footage = graphops::id_of(id)?;
		if project.node_kind(footage) != Some(NodeKind::Footage) {
			return None;
		}
		project
			.footage_filename(footage)
			.filter(|filename| !filename.is_empty())
			.map(PathBuf::from)
	}

	fn project_entry_name(&self, id: u64) -> Option<SharedString> {
		let project = self.project_ref()?;
		let node = graphops::id_of(id)?;
		project.node_kind(node)?;
		Some(project.node_label(node).into())
	}

	fn replace_footage(
		&mut self,
		id: u64,
		path: PathBuf,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		if !path.is_file() {
			return Err(format!("file does not exist: {}", path.display()));
		}
		let Some(session) = self.editor_session() else {
			return Err("no project open".into());
		};
		let Some(footage) = graphops::id_of(id) else {
			return Err("unknown project entry".into());
		};
		session.execute(EditCommand::ReplaceFootage {
			footage: quercus_engine_rs::node::NodeId::from_raw(footage.raw()),
			path: path.to_string_lossy().into_owned(),
		})?;
		cx.notify();
		Ok(())
	}

	fn rename_entry(&mut self, id: u64, new_name: String, cx: &mut Context<Self>) {
		let Some(session) = self.editor_session() else {
			return;
		};
		let Some(node) = graphops::id_of(id) else {
			return;
		};
		let result = session
			.execute(EditCommand::RenameBlocks {
				clips: vec![quercus_engine_rs::node::NodeId::from_raw(node.raw())],
				name: new_name,
			})
			.map(|_| ());
		self.apply_edit(result, "rename entry", cx);
	}

	fn delete_entry(&mut self, id: u64, cx: &mut Context<Self>) {
		let Some(session) = self.editor_session() else {
			return;
		};
		let Some(node) = graphops::id_of(id) else {
			return;
		};
		// A folder removal is already one command; a footage/sequence
		// entry removes the node itself (the C++ Delete behaviour: the
		// node and its connections; timeline clips referencing a removed
		// footage keep their blocks but their decode source is gone —
		// matching the C++ path, which is undoable).
		let result = session
			.execute(EditCommand::DeleteEntry {
				node: quercus_engine_rs::node::NodeId::from_raw(node.raw()),
			})
			.map(|_| ());
		self.apply_edit(result, "delete entry", cx);
	}

	fn drop_footage(
		&mut self,
		id: u64,
		_track_kind: TrackKind,
		track_index: usize,
		time: Frame,
		cx: &mut Context<Self>,
	) {
		let Some(project) = self.project.clone() else {
			return;
		};
		// The explorer's entry id IS the footage node's stable identity
		// (`projectbrowser`).
		let Some(footage) = graphops::id_of(id) else {
			println!("[real engine] drop footage: entry {id} is not a footage node");
			return;
		};
		// Sequence entry drop: a NESTED-SEQUENCE clip (one row, not the
		// sequence's tracks — a sequence is a frame, not a folder of
		// tracks, on the timeline). The dropped clip feeds from the
		// sequence node (TEXTURE_INPUT, like `clip_connected_sequence`
		// resolves), so a multicam source sequence drops as a single
		// multi-cam clip the panel detects.
		//
		// Text entry drop: a GENERATOR clip — the clip reads the text
		// node's output through `tex_in` instead of decoding media.
		let is_sequence = project.node_kind(footage) == Some(NodeKind::Sequence);
		let is_text =
			graphops::node_type_id(&project, footage) == graphops::TEXT_FOOTAGE_TYPE_ID;
		if is_sequence {
			self.drop_sequence_entry(&project, footage, track_index, time, cx);
			return;
		}
		if is_text {
			self.drop_text_entry(&project, footage, track_index, time, cx);
			return;
		}
		let Some(filename) = project.footage_filename(footage) else {
			println!("[real engine] drop footage: entry {id} is not a footage node");
			return;
		};
		let video_streams = project.footage_video_stream_count(footage);
		let total_streams = project.footage_total_stream_count(footage);
		let seconds = graphops::footage_duration_seconds(&project, footage);
		// Empty timeline: dropping footage auto-creates a sequence sized to
		// the footage's first video stream (the NLE convention); pure-audio
		// footage falls back to the default sequence format.
		let seq = match self.sequence {
			Some(seq) => seq,
			None => {
				let Some(seq) = self.create_sequence_for_drop(&project, footage, cx) else {
					return;
				};
				seq
			}
		};
		// Media type from the probed stream list (the probe is real since
		// the import fills it); fall back to the extension only when no
		// streams were recorded (legacy projects loaded without a probe).
		let (has_video, has_audio) = if total_streams > 0 {
			(video_streams > 0, total_streams > video_streams)
		} else if crate::oakui::filename_is_audio(&filename) {
			(false, true)
		} else {
			(true, false)
		};
		// Track policy (see the `AppEngine::drop_footage` docs): use the
		// pointed display track when its kind matches, otherwise auto-select
		// the topmost track of the footage's kind; reject when there is none.
		// A video-with-audio file needs BOTH a video and an audio track —
		// missing kinds are created (the NLE convention — Premiere
		// auto-creates on drop).
		let ensure_track = |this: &mut Self, kind: TrackKind, cx: &mut Context<Self>| {
			if let Some(track) = this.tracks.get(track_index) {
				if track.kind == kind {
					return Some(track_index);
				}
			}
			if let Some(index) = this.tracks.iter().position(|t| t.kind == kind) {
				return Some(index);
			}
			this.add_track(kind, cx);
			match this.tracks.iter().position(|t| t.kind == kind) {
				Some(index) => Some(index),
				None => {
					println!(
						"[real engine] drop footage: could not add a {:?} track for \"{}\"",
						kind, filename
					);
					None
				}
			}
		};
		let (kind, track_index_facade) = if has_video && has_audio {
			let Some(video_target) = ensure_track(self, TrackKind::Video, cx) else {
				return;
			};
			// The A/V drop pairs the audio with the SAME track number as
			// the video (V2 → A2, V1 → A1): the group lands together and
			// later drops no longer pile every audio clip onto A1. A
			// missing same-numbered audio track (e.g. A2 was removed) is
			// recreated so the pair stays aligned; the user is free to
			// rearrange afterwards.
			let audio_target = loop {
				let video_number = self.tracks[video_target].track_index;
				if let Some(index) = self
					.tracks
					.iter()
					.position(|t| t.kind == TrackKind::Audio && t.track_index == video_number)
				{
					break index;
				}
				let audio_count = self
					.tracks
					.iter()
					.filter(|t| t.kind == TrackKind::Audio)
					.count();
				if audio_count > video_number {
					// Same-numbered audio tracks have run out without a
					// hit (should not happen: counting is contiguous).
					println!(
						"[real engine] drop footage: no audio track A{} for \"{}\"",
						video_number + 1,
						filename
					);
					return;
				}
				self.add_track(TrackKind::Audio, cx);
				let after = self
					.tracks
					.iter()
					.filter(|t| t.kind == TrackKind::Audio)
					.count();
				if after == audio_count {
					// The track add failed: give up this drop.
					println!(
						"[real engine] drop footage: could not create audio track A{} for \"{}\"",
						video_number + 1,
						filename
					);
					return;
				}
			};
			let (video_index, audio_index) = (
				self.tracks[video_target].track_index,
				self.tracks[audio_target].track_index,
			);
			// Clip length: the footage's probed duration when available;
			// otherwise a 10-second default.
			let fps = self.frame_rate();
			let fps_f = fps.num as f64 / fps.den.max(1) as f64;
			let length = match seconds {
				Some(s) => (s * fps_f).round().max(1.0) as i64,
				None => (10.0 * fps_f).round().max(1.0) as i64,
			};
			let in_ts = time.0.max(0);
			// One undoable "Add Clip" entry: the video clip plus its linked
			// audio clip at the same range (the NLE A/V drop).
			let result = graphops::place_footage_clips_linked(
				&project,
				seq,
				footage,
				&[
					(DomainTrackKind::Video, video_index),
					(DomainTrackKind::Audio, audio_index),
				],
				in_ts,
				in_ts + length,
				0,
			)
			.map(|_| ());
			self.apply_edit(result, "drop footage", cx);
			return;
		} else {
			let footage_kind = if has_video {
				TrackKind::Video
			} else {
				TrackKind::Audio
			};
			let Some(target) = ensure_track(self, footage_kind, cx) else {
				return;
			};
			let track = &self.tracks[target];
			(track_type_of(track.kind), track.track_index)
		};
		// Clip length: the footage's probed duration when available;
		// otherwise a 10-second default.
		let fps = self.frame_rate();
		let fps_f = fps.num as f64 / fps.den.max(1) as f64;
		let length = match seconds {
			Some(s) => (s * fps_f).round().max(1.0) as i64,
			None => (10.0 * fps_f).round().max(1.0) as i64,
		};
		let in_ts = time.0.max(0);
		let result = graphops::place_footage_clip(
			&project,
			seq,
			footage,
			kind,
			track_index_facade,
			in_ts,
			in_ts + length,
			0,
		)
		.map(|_| ());
		// The placement pushes ONE undoable "Add Clip" entry; the refresh
		// also invalidates the cached rendered frames.
		self.apply_edit(result, "drop footage", cx);
	}

	/// The footage's length in sequence frames (the drop ghost's extent):
	/// the probed duration times the frame rate; `None` when the entry is
	/// not footage or was never probed.
	fn footage_length_frames(&self, id: u64) -> Option<i64> {
		let project = self.project_ref()?;
		let node = graphops::id_of(id)?;
		let seconds = graphops::footage_duration_seconds(project, node)?;
		let fps = self.frame_rate();
		let fps_f = fps.num as f64 / fps.den.max(1) as f64;
		Some((seconds * fps_f).round().max(1.0) as i64)
	}

	fn footage_video_params(&self, id: u64) -> Option<(u32, u32, FrameRate, bool)> {
		let project = self.project_ref()?;
		let node = graphops::id_of(id)?;
		let vp = project.footage_video_params(node, 0)?;
		Some((
			vp.width.max(1) as u32,
			vp.height.max(1) as u32,
			FrameRate::new(
				vp.frame_rate.num.max(1) as u32,
				vp.frame_rate.den.max(1) as u32,
			),
			vp.interlaced != 0,
		))
	}

	// --- project library (M13 D4) --------------------------------------

	fn storage_bound(&self) -> bool {
		self.editor_session()
			.map(|session| session.storage_bound())
			.unwrap_or(false)
	}

	fn storage_last_error(&self) -> Option<String> {
		self.editor_session()?.storage_last_error()
	}

	fn library_projects(&self) -> Result<Vec<LibraryProject>, String> {
		graphops::library_list()
	}

	fn library_create_project(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
		let uuid = graphops::library_create(name)?;
		self.open_library_project(&uuid, cx)
	}

	fn library_open_project(&mut self, uuid: &str, cx: &mut Context<Self>) -> Result<(), String> {
		self.open_library_project(uuid, cx)
	}

	fn library_delete_project(&mut self, uuid: &str) -> Result<(), String> {
		graphops::library_delete(uuid).map_err(|e| format!("failed to delete the project: {e}"))
	}

	fn library_rename_project(&mut self, uuid: &str, name: &str) -> Result<(), String> {
		graphops::library_rename(uuid, name)
			.map_err(|e| format!("failed to rename the project: {e}"))
	}

	fn library_duplicate_project(&mut self, uuid: &str) -> Result<(), String> {
		graphops::library_duplicate(uuid)
			.map(|_| ())
			.map_err(|e| format!("failed to duplicate the project: {e}"))
	}

	/// 另存为: duplicate the open project into the library under a new
	/// (named) row, then open that copy as the current project. The
	/// duplicator applies the new name to the copy's row and projectname.
	fn save_project_as(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
		let Some(project) = self.project.clone() else {
			return Err("no project open".to_string());
		};
		let current_uuid = project.uuid();
		let new_uuid = graphops::library_duplicate(&current_uuid)
			.map_err(|e| format!("duplicate failed: {e}"))?;
		graphops::library_rename(&new_uuid, name.trim())
			.map_err(|e| format!("rename copy failed: {e}"))?;
		// Open the new row (adopt switches the current project).
		self.open_library_project(&new_uuid, cx)?;
		Ok(())
	}

	fn library_import_project(&mut self, path: PathBuf) -> Result<String, String> {
		graphops::library_import(&path)
			.map_err(|e| format!("failed to import \"{}\": {e}", path.display()))
	}

	fn library_export_project(&mut self, uuid: &str, path: PathBuf) -> Result<(), String> {
		graphops::library_export(uuid, &path).map_err(|e| {
			format!(
				"failed to export the project to \"{}\": {e}",
				path.display()
			)
		})
	}

	fn set_use_proxy_media(&mut self, enabled: bool, cx: &mut Context<Self>) {
		config_set_bool(CONFIG_KEY_USE_PROXY, enabled);
		// Every footage's preview media may change: drop the rendered
		// frames so the next pull re-resolves original/proxy.
		self.invalidate_preview_frames(cx);
	}

	/// Sets the playback resolution divider (the viewer `Playback
	/// Resolution ▸` menu): the preview geometry changes, so every cached
	/// and in-flight preview frame is stale.
	fn set_playback_divider(&mut self, divider: i64, cx: &mut Context<Self>) {
		config_set_int("PlaybackDivider", divider.clamp(1, 8));
		self.invalidate_preview_frames(cx);
	}

	/// The project's OCIO config override (the 项目属性 color tab; "" = the
	/// app default config).
	fn project_ocio_config(&self) -> String {
		self.project_ref()
			.and_then(|p| p.setting(PROJECT_SETTING_OCIO_CONFIG))
			.unwrap_or_default()
	}

	fn set_project_ocio_config(
		&mut self,
		path: String,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(project) = self.project.clone() else {
			return Err("no project open".to_string());
		};
		let trimmed = path.trim().to_string();
		// Validate first — the dialog stays open on Err (the C++ accept()
		// refuses an invalid config the same way). Applying is the
		// process-wide color config reload plus a full frame invalidation.
		let path = (!trimmed.is_empty()).then_some(trimmed.as_str());
		quercus_engine_rs::state::color_set_up_default_config(path)
			.map_err(|e| e.to_string())?;
		if trimmed.is_empty() {
			project.unset_setting(PROJECT_SETTING_OCIO_CONFIG);
		} else {
			project.set_setting(PROJECT_SETTING_OCIO_CONFIG, &trimmed);
		}
		super::displaycolor::invalidate();
		self.invalidate_rendered_frames();
		cx.notify();
		Ok(())
	}

	fn project_cache_location(&self) -> (i32, String) {
		self.project_ref()
			.map(|p| {
				let (mode, path) = p.cache_location();
				// Clamp: the dialog's combo indexes by this value.
				(mode.clamp(0, 2), path)
			})
			.unwrap_or((0, String::new()))
	}

	fn set_project_cache_location(
		&mut self,
		setting: i32,
		custom_path: String,
		cx: &mut Context<Self>,
	) {
		let Some(project) = self.project.clone() else {
			return;
		};
		{
			let mode = setting.clamp(0, 2);
			let path = if mode == 2 {
				custom_path.trim().to_string()
			} else {
				String::new()
			};
			let _ = project.set_cache_location(mode, &path);
		}
		cx.notify();
	}

	fn project_color_settings(&self) -> (String, String, String) {
		// The project-format defaults: ACEScg working space, sRGB gamut
		// and transfer (the values the former colormath defaults encoded).
		let working_default = "acescg".to_string();
		let gamut_default = "srgb".to_string();
		let transfer_default = "srgb".to_string();
		let Some(project) = self.project_ref() else {
			return (working_default, gamut_default, transfer_default);
		};
		let get = |key: &str, default: &str| project.setting(key).unwrap_or_else(|| default.to_string());
		(
			get("workingcolorspace", &working_default),
			get("outputgamut", &gamut_default),
			get("outputtransfer", &transfer_default),
		)
	}

	fn set_project_color_settings(
		&mut self,
		working: String,
		gamut: String,
		transfer: String,
		cx: &mut Context<Self>,
	) {
		let Some(project) = self.project.clone() else {
			return;
		};
		project.set_setting("workingcolorspace", &working);
		project.set_setting("outputgamut", &gamut);
		project.set_setting("outputtransfer", &transfer);
		// The app-side transforms read the process global; the workers pick
		// the new settings up with the next graph upload.
		let _ = quercus_engine_rs::state::color_set_pipeline_settings(
			&working, &gamut, &transfer,
		);
		// The workers derive their pipeline colors from the uploaded project
		// snapshot; a settings change alone does not bump the undo-stack
		// revision (the manager dedups re-uploads on it), so push an explicit
		// resync — the workers re-deserialize the rewritten snapshot, adopt
		// the new working space/output spec and drop their cached frames.
		let revision = quercus_engine_rs::node::undo_index().max(0) as u64;
		let _ = quercus_engine_rs::render::manager_resync_graph_snapshot(&project, revision);
		// The pipeline colorspace changed: every cached frame (CPU image,
		// GPU texture, preview slot) was rendered under the old space.
		super::displaycolor::invalidate();
		self.invalidate_rendered_frames();
		self.reapply_display_policy_to_windows(cx);
		cx.notify();
	}

	fn entry_is_sequence(&self, id: u64) -> bool {
		let Some(project) = self.project_ref() else {
			return false;
		};
		let Some(node) = graphops::id_of(id) else {
			return false;
		};
		project.node_kind(node) == Some(NodeKind::Sequence)
	}

	fn open_sequence_id(&mut self, id: u64, cx: &mut Context<Self>) {
		let Some(project) = self.project.clone() else {
			return;
		};
		let Some(node) = graphops::id_of(id) else {
			return;
		};
		let is_seq = project.node_kind(node) == Some(NodeKind::Sequence);
		if !is_seq {
			return;
		}
		// Switch the current sequence: the timeline rebuilds against the
		// opened sequence (the "open a sequence entry" behavior — same
		// refresh as project open).
		if self.sequence == Some(node) {
			return;
		}
		self.activate_sequence(node);
		graphops::ensure_sequences_mounted(&project);
		self.refresh_sequence_info();
		self.rebuild_timeline();
		self.invalidate_rendered_frames();
		self.push_graph_snapshot();
		cx.notify();
	}

	fn sequence_parameters(&self, id: u64) -> Option<SequenceParameters> {
		let project = self.project_ref()?;
		let node = graphops::id_of(id)?;
		let (width, height) = project.sequence_geometry(node)?;
		let rate = project.sequence_frame_rate(node);
		let interlaced = project.sequence_interlaced(node)?;
		Some(SequenceParameters {
			name: project.node_label(node),
			format: VideoFormat {
				width: width.max(1) as u32,
				height: height.max(1) as u32,
				rate: FrameRate::new(rate.num.max(1) as u32, rate.den.max(1) as u32),
			},
			interlaced,
		})
	}

	fn create_sequence_with_params(
		&mut self,
		name: String,
		format: VideoFormat,
		interlaced: bool,
		cx: &mut Context<Self>,
	) -> Result<u64, String> {
		let (Some(project), _) = (self.project.clone(), self.sequence) else {
			return Err(crate::i18n::tr("seqprops.error.no_project").to_string());
		};
		let name = if name.trim().is_empty() {
			"Sequence 1".to_string()
		} else {
			name.trim().to_string()
		};
		let seq = graphops::create_sequence_with_params(
			&project,
			&name,
			Some((
				format.width as i32,
				format.height as i32,
				Rational::new(i64::from(format.rate.num), i64::from(format.rate.den)),
				interlaced,
			)),
		);
		self.activate_sequence(seq);
		self.refresh_sequence_info();
		self.rebuild_timeline();
		self.push_graph_snapshot();
		cx.notify();
		Ok(seq.raw())
	}

	fn create_folder(&mut self, name: String, cx: &mut Context<Self>) -> Result<u64, String> {
		let Some(project) = self.project.clone() else {
			return Err(crate::i18n::tr("seqprops.error.no_project").to_string());
		};
		let name = if name.trim().is_empty() {
			let count = graphops::folder_ids(&project).len();
			format!("Folder {}", count + 1)
		} else {
			name.trim().to_string()
		};
		let id = graphops::create_folder(&project, &name)?;
		self.apply_edit(Ok(()), "new folder", cx);
		Ok(id.raw())
	}

	fn create_text_footage(&mut self, cx: &mut Context<Self>) -> Result<u64, String> {
		let Some(project) = self.project.clone() else {
			return Err(crate::i18n::tr("seqprops.error.no_project").to_string());
		};
		let id = graphops::create_text_footage_node(&project)?;
		self.apply_edit(Ok(()), "add text footage", cx);
		Ok(id.raw())
	}

	fn drop_generator_clip(
		&mut self,
		type_id: &str,
		track_index: usize,
		time: Frame,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(project) = self.project.clone() else {
			return Err("no project open".to_string());
		};
		let Some(host_seq) = self.sequence else {
			return Err("no sequence open".to_string());
		};
		// The effect-library generator becomes a node in the project, then
		// a generator clip feeds from it (the text-clip drop's shape).
		let generator = quercus_engine_rs::node::create_generator(&project, type_id)
			.ok_or_else(|| format!("effect \"{type_id}\" is not a generator"))?;
		let video_target = if self
			.tracks
			.get(track_index)
			.is_some_and(|t| t.kind == TrackKind::Video)
		{
			track_index
		} else if let Some(index) = self.tracks.iter().position(|t| t.kind == TrackKind::Video) {
			index
		} else {
			return Err("no video track".to_string());
		};
		let video_index = self.tracks[video_target].track_index;
		// Five seconds of the host sequence's frames (the drop's default
		// extent — a generator clip has no media length to take one from).
		let (in_ts, length_ts) = {
			let tb = graphops::sequence_time_base(&project, host_seq)
				.ok_or_else(|| "host sequence has no frame rate".to_string())?;
			let fps_f = tb.1.max(1) as f64 / tb.0.max(1) as f64;
			((time.0.max(0)), ((5.0 * fps_f).round() as i64).max(1))
		};
		let placed = graphops::place_generator_clip(
			&project,
			host_seq,
			generator,
			video_index,
			in_ts,
			in_ts + length_ts,
		);
		let result = placed.map(|_| ());
		self.apply_edit(result.clone(), "drop generator clip", cx);
		result
	}

	fn drop_transition_at(
		&mut self,
		_type_id: &str,
		track_index: usize,
		time: Frame,
		snap_frames: i64,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(project) = self.project.clone() else {
			return Err("no project open".to_string());
		};
		let Some(host_seq) = self.sequence else {
			return Err("no sequence open".to_string());
		};
		let video_target = if self
			.tracks
			.get(track_index)
			.is_some_and(|t| t.kind == TrackKind::Video)
		{
			track_index
		} else if let Some(index) = self.tracks.iter().position(|t| t.kind == TrackKind::Video) {
			index
		} else {
			return Err("no video track".to_string());
		};
		let video_index = self.tracks[video_target].track_index;
		// Plan under the graph lock, execute AFTER dropping it: the
		// add_transition_* builders lock the project internally (a held
		// guard deadlocks them).
		enum DropPlan {
			Seam(NodeId, NodeId),
			Edge(NodeId, bool),
		}
		let (plan, half) = {
			let Some(track) = graphops::track_ids(&project, host_seq, DomainTrackKind::Video)
				.get(video_index)
				.copied()
			else {
				return Err("the target track is not in the sequence".to_string());
			};
			// The time base is the SEQUENCE's frame rate (tracks carry no
			// video params of their own — `sequence_time_base` on a track
			// id always returns None).
			let Some(tb) = graphops::sequence_time_base(&project, host_seq) else {
				return Err("the sequence has no frame rate".to_string());
			};
			// Snap to the nearest clip edge on the target track (within one
			// second): a junction when the edge is a contiguous cut,
			// otherwise a single-sided head/tail transition.
			let time_r = graphops::ts_to_rational(time.0.max(0), tb);
			let mut best: Option<(Rational, NodeId, bool)> = None;
			for clip in graphops::clip_ids(&project, track) {
				let Some((in_r, out_r, _)) = graphops::clip_range(&project, clip) else {
					continue;
				};
				for (edge, start_edge) in [(in_r, true), (out_r, false)] {
					let dist = if edge > time_r {
						edge - time_r
					} else {
						time_r - edge
					};
					if best.as_ref().is_none_or(|(d, ..)| dist < *d) {
						best = Some((dist, clip, start_edge));
					}
				}
			}
			let Some((dist, clip, start_edge)) = best else {
				return Err("the target track has no clips".to_string());
			};
			let snap = graphops::ts_to_rational(snap_frames.max(1), tb);
			if dist > snap {
				return Err(format!(
					"no clip edge within {} frames of the drop point",
					snap_frames
				));
			}
			let Some((in_r, out_r, _)) = graphops::clip_range(&project, clip) else {
				return Err("the edge clip has no range".to_string());
			};
			let blocks: Vec<NodeId> = (0..project.track_block_count(track))
				.map(|index| project.track_block_at(track, index))
				.collect();
			let index = blocks.iter().position(|&b| b == clip);
			let prev = index
				.and_then(|i| i.checked_sub(1))
				.and_then(|i| blocks.get(i))
				.copied();
			let next = index.and_then(|i| blocks.get(i + 1)).copied();
			let prev_touch = prev.is_some_and(|b| {
				project.node_kind(b) == Some(NodeKind::Clip)
					&& project.block_out(b) == in_r
			});
			let next_touch = next.is_some_and(|b| {
				project.node_kind(b) == Some(NodeKind::Clip)
					&& project.block_in(b) == out_r
			});
			// The default transition's half-length, one frame at least.
			let fps = tb.1 as f64 / tb.0.max(1) as f64;
			let half_frames = (0.5 * fps).round().max(1.0) as i64;
			let half = graphops::ts_to_rational(half_frames, tb);
			let plan = if start_edge && prev_touch {
				DropPlan::Seam(prev.unwrap(), clip)
			} else if !start_edge && next_touch {
				DropPlan::Seam(clip, next.unwrap())
			} else {
				DropPlan::Edge(clip, start_edge)
			};
			(plan, half)
		};
		let placed = match plan {
			DropPlan::Seam(out_clip, in_clip) => {
				graphops::add_transition_at_seam(&project, out_clip, in_clip, half)
			}
			DropPlan::Edge(clip, start_edge) => {
				graphops::add_transition_at_edge(&project, clip, start_edge, half + half)
			}
		};
		let result = placed.map(|_| ());
		self.apply_edit(result.clone(), "drop transition", cx);
		result
	}

	fn update_sequence_parameters(
		&mut self,
		id: u64,
		name: String,
		format: VideoFormat,
		interlaced: bool,
		cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(project) = self.project.clone() else {
			return Err(crate::i18n::tr("seqprops.error.no_project").to_string());
		};
		let Some(node) = graphops::id_of(id) else {
			return Err(crate::i18n::tr("seqprops.error.no_sequence").to_string());
		};
		graphops::set_sequence_parameters(
			&project,
			node,
			name.trim(),
			format.width as i32,
			format.height as i32,
			Rational::new(i64::from(format.rate.num), i64::from(format.rate.den)),
			interlaced,
		)?;
		self.apply_edit(Ok(()), "set sequence parameters", cx);
		Ok(())
	}

	fn proxy_rows(&self) -> Vec<super::engine::ProxyFootageRow> {
		let Some(session) = self.editor_session() else {
			return Vec::new();
		};
		session
			.snapshot()
			.footages
			.into_iter()
			.filter_map(|footage| {
				let status = session.proxy_status(footage.id)?;
				let id = footage.id.raw();
				let state = if self.proxy_runs.iter().any(|run| run.footage == id) {
					super::engine::ProxyMediaState::Generating
				} else {
					proxy_media_state(status.state)
				};
				Some(super::engine::ProxyFootageRow {
					id,
					name: footage.name,
					state,
					enabled: status.enabled,
					has_custom: status.has_custom_params,
					can_generate: status.can_generate,
					has_proxy: status.has_proxy,
				})
			})
			.collect()
	}

	fn proxy_state(&self, id: u64) -> Option<super::engine::ProxyMediaState> {
		if self.proxy_runs.iter().any(|run| run.footage == id) {
			return Some(super::engine::ProxyMediaState::Generating);
		}
		self.editor_session()?
			.proxy_status(quercus_engine_rs::node::NodeId::from_raw(id))
			.map(|status| proxy_media_state(status.state))
	}

	fn proxy_row(&self, id: u64) -> Option<super::engine::ProxyFootageRow> {
		self.proxy_rows().into_iter().find(|row| row.id == id)
	}

	fn proxy_generate(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
		if self.proxy_runs.iter().any(|run| run.footage == id) {
			return Err("a proxy is already generating for this footage".into());
		}
		let session = self.editor_session().ok_or("no project open")?;
		let footage = quercus_engine_rs::node::NodeId::from_raw(id);
		let task = session.generate_proxy(footage, &Self::proxy_cache_path())?;
		self.proxy_runs.push(ProxyRun { footage: id, session, task });
		cx.notify();
		Ok(())
	}

	fn proxy_task_progress(&self) -> Option<(String, f64)> {
		self.proxy_runs
			.first()
			.map(|run| (run.task.title(), run.task.progress().clamp(0.0, 1.0)))
	}

	fn proxy_delete(&mut self, id: u64, cx: &mut Context<Self>) {
		if self.proxy_runs.iter().any(|run| run.footage == id) {
			return;
		}
		// 删除代理 = 明确不要了：同时移出自动生成队列（否则递补时
		// 会立刻重新生成一个）。
		self.proxy_queue.retain(|&q| q != id);
		let Some(session) = self.editor_session() else {
			return;
		};
		if session
			.delete_proxy(quercus_engine_rs::node::NodeId::from_raw(id))
			.is_ok()
		{
			self.invalidate_preview_frames(cx);
		}
	}

	fn proxy_set_enabled(&mut self, id: u64, enabled: bool, cx: &mut Context<Self>) {
		let Some(session) = self.editor_session() else {
			return;
		};
		if session
			.set_proxy_enabled(quercus_engine_rs::node::NodeId::from_raw(id), enabled)
			.is_err()
		{
			return;
		}
		// 启用而代理未就绪时立即后台生成（禁用则是 opt-out，
		// autostart_proxies 对有记录路径但未启用的素材不会重启）。
		if enabled {
			self.autostart_proxies(cx);
		}
		self.invalidate_preview_frames(cx);
	}

	fn proxy_reveal(&self, id: u64) {
		let Some(proxy_path) = self
			.editor_session()
			.and_then(|session| session.proxy_path(quercus_engine_rs::node::NodeId::from_raw(id)))
			.filter(|path| !path.is_empty())
		else {
			return;
		};
		let path = std::path::Path::new(&proxy_path);
		if path.exists() {
			let _ = std::process::Command::new("open")
				.arg("-R")
				.arg(path)
				.spawn();
		}
	}

	fn proxy_set_custom_params(
		&mut self,
		id: u64,
		params: super::engine::ProxyParamsUi,
		cx: &mut Context<Self>,
	) {
		let Some(footage) = self.footage_of(id) else {
			return;
		};
		if let Some(session) = self.editor_session() {
			let mut sdk = quercus_engine_rs::codec::ProxyParams::default();
			sdk.width = params.width;
			sdk.height = params.height;
			sdk.divider = params.divider;
			sdk.crf = params.crf;
			sdk.include_audio = i32::from(params.include_audio);
			if sdk.set_preset(&params.preset).is_err() {
				return;
			}
			let _ = session.execute(EditCommand::SetFootageProxyParams {
				footage: quercus_engine_rs::node::NodeId::from_raw(footage.raw()),
				params: sdk,
			});
		}
		cx.notify();
	}

	fn proxy_clear_custom_params(&mut self, id: u64, cx: &mut Context<Self>) {
		let Some(footage) = self.footage_of(id) else {
			return;
		};
		if let Some(session) = self.editor_session() {
			let _ = session.execute(EditCommand::ClearFootageProxyParams {
				footage: quercus_engine_rs::node::NodeId::from_raw(footage.raw()),
			});
		}
		cx.notify();
	}

	fn proxy_custom_params(&self, id: u64) -> Option<super::engine::ProxyParamsUi> {
		let session = self.editor_session()?;
		let node = graphops::id_of(id)?;
		let params = session.project().footage_custom_proxy_params(
			quercus_engine_rs::node::NodeId::from_raw(node.raw()),
		)?;
		Some(proxy_ui_from_sdk(&params))
	}

	fn proxy_effective_params(&self, id: u64) -> super::engine::ProxyParamsUi {
		let Some(session) = self.editor_session() else {
			return proxy_params_from_config();
		};
		let Some(node) = graphops::id_of(id) else {
			return proxy_params_from_config();
		};
		session
			.project()
			.footage_effective_proxy_params(quercus_engine_rs::node::NodeId::from_raw(node.raw()))
			.map(|params| proxy_ui_from_sdk(&params))
			.unwrap_or_else(proxy_params_from_config)
	}

	fn clip_footage_entries(&self, clips: &[ClipId]) -> Vec<super::engine::ProxyFootageRow> {
		let Some(project) = self.project.as_ref() else {
			return Vec::new();
		};
		let footage = {
			let mut seen: Vec<NodeId> = Vec::new();
			for clip in clips {
				let Some(node) = graphops::id_of(clip.0) else {
					continue;
				};
				let Some(footage) = graphops::find_input_footage(project, node) else {
					continue;
				};
				if !seen.contains(&footage) {
					seen.push(footage);
				}
			}
			seen
		};
		footage
			.into_iter()
			.filter_map(|footage| self.proxy_row(footage.raw()))
			.collect()
	}

	fn sync_eligibility(&self, clips: &[ClipId]) -> super::engine::SyncEligibility {
		let mut eligibility = super::engine::SyncEligibility::default();
		let (Some(session), Some(cache)) = (self.editor_session(), self.waveform_cache()) else {
			return eligibility;
		};
		for clip in clips {
			let Some(info) = session.clip_sync_info(quercus_engine_rs::node::NodeId::from_raw(clip.0)) else {
				continue;
			};
			if info.has_source_start_time {
				eligibility.source_time += 1;
			}
			if let Some(waveform) = cache.get(clip.0) {
				if super::waveformsync::waveform_sync_eligible(&waveform, info.media_length_seconds) {
					eligibility.waveform += 1;
				}
			}
		}
		eligibility
	}

	fn sync_clips_by_source_time(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		self.sync_clips_by_source_time_internal(&clips);
		cx.notify();
	}

	fn sync_clips_by_waveform(
		&mut self,
		clips: Vec<ClipId>,
		adjust_speed: bool,
		cx: &mut Context<Self>,
	) {
		self.sync_clips_by_waveform_internal(&clips, adjust_speed);
		cx.notify();
	}

	fn toggle_clip_links(&mut self, clips: Vec<ClipId>, cx: &mut Context<Self>) {
		let Some(project) = self.project.clone() else {
			return;
		};
		// Only clip blocks can be linked (the C++ `block_as_clip` filter).
		let blocks: Vec<NodeId> = clips
			.iter()
			.filter_map(|c| graphops::id_of(c.0))
			.filter(|&n| project.node_kind(n) == Some(NodeKind::Clip))
			.collect();
		if blocks.len() < 2 {
			return;
		}
		// Toggle rule: fully linked internally → unlink; otherwise link
		// (deviation from the C++ crude "any member has ANY link" check,
		// which made split halves — carrying inherited A/V links —
		// impossible to link to each other). The undo restores the prior
		// internal topology; links to nodes outside the set stay untouched.
		let all_linked = blocks.iter().enumerate().all(|(i, &a)| {
			blocks[i + 1..]
				.iter()
				.all(|&b| project.block_links(a).contains(&b))
		});
		let result = graphops::set_clips_linked(&project, &blocks, !all_linked);
		self.apply_edit(
			result,
			if all_linked {
				"unlink clips"
			} else {
				"link clips"
			},
			cx,
		);
	}

	fn start_export(&mut self, format: i32, path: PathBuf) -> Result<ExportSession, String> {
		let settings = super::engine::ExportSettings {
			format,
			..Default::default()
		};
		self.start_export_with(&settings, path)
	}

	fn start_export_with(
		&mut self,
		settings: &super::engine::ExportSettings,
		path: PathBuf,
	) -> Result<ExportSession, String> {
		let (Some(project), Some(seq)) = (self.project.clone(), self.sequence) else {
			return Err("no sequence open".into());
		};
		let session = self.editor_session().ok_or("no sequence open")?;
		let workarea = self.workarea().map(|(s, e)| (s.0, e.0));
		let params = super::renderops::encoding_params_with_settings(
			&project,
			seq,
			settings,
			&path,
			workarea,
			self.sequence_length().0,
		)?;
		super::renderops::spawn_export(
			&session,
			quercus_engine_rs::node::NodeId::from_raw(seq.raw()),
			&path.to_string_lossy(),
			params,
		)
	}

	fn sequence_entries(&self) -> Vec<(u64, SharedString)> {
		/// DFS the bin in explorer order, keeping sequence leaves.
		fn collect(
			project: &ProjectRef,
			entries: Vec<ProjectEntry>,
			out: &mut Vec<(u64, SharedString)>,
		) {
			for entry in entries {
				if entry.is_dir {
					collect(
						project,
						crate::oakui::projectbrowser::children(project, entry.id),
						out,
					);
					continue;
				}
				let is_seq = graphops::id_of(entry.id)
					.is_some_and(|node| project.node_kind(node) == Some(NodeKind::Sequence));
				if is_seq {
					out.push((entry.id, entry.name));
				}
			}
		}
		let Some(project) = self.project_ref() else {
			return Vec::new();
		};
		let mut out = Vec::new();
		collect(
			project,
			crate::oakui::projectbrowser::roots(project),
			&mut out,
		);
		out
	}

	fn current_sequence_id(&self) -> Option<u64> {
		self.sequence.map(|seq| seq.raw())
	}

	fn start_export_of(
		&mut self,
		id: u64,
		settings: &super::engine::ExportSettings,
		path: PathBuf,
	) -> Result<ExportSession, String> {
		let Some(project) = self.project.clone() else {
			return Err("no project open".into());
		};
		let session = self.editor_session().ok_or("no project open")?;
		let Some(seq) = graphops::id_of(id) else {
			return Err("the entry is not a sequence".into());
		};
		if project.node_kind(seq) != Some(NodeKind::Sequence) {
			return Err("the entry is not a sequence".into());
		}
		// The work area belongs to the OPEN sequence; a bin export of a
		// different sequence renders it whole.
		let workarea = match self.sequence {
			Some(current) if current == seq => self.workarea().map(|(s, e)| (s.0, e.0)),
			_ => None,
		};
		let length_frames = {
			let length = graphops::sequence_length(&project, seq);
			graphops::sequence_time_base(&project, seq)
				.map(|tb| graphops::rational_to_ts(length, tb))
				.unwrap_or(0)
		};
		let params = super::renderops::encoding_params_with_settings(
			&project,
			seq,
			settings,
			&path,
			workarea,
			length_frames,
		)?;
		super::renderops::spawn_export(
			&session,
			quercus_engine_rs::node::NodeId::from_raw(seq.raw()),
			&path.to_string_lossy(),
			params,
		)
	}

	fn multicam_state(&self) -> Option<MulticamState> {
		self.multicam_state_internal()
	}

	fn multicam_angle_frame(
		&mut self,
		source: i32,
		_cx: &mut Context<Self>,
	) -> Option<Arc<RenderImage>> {
		self.multicam_angle_frame_internal(source)
	}

	fn multicam_eligible(&self, clips: &[ClipId]) -> bool {
		let Some(session) = self.editor_session() else {
			return false;
		};
		clips.iter().any(|clip| {
			session
				.multicam_connected_sequence(quercus_engine_rs::node::NodeId::from_raw(clip.0))
				.is_some()
		})
	}

	fn multicam_enabled_on_selection(&self, clips: &[ClipId]) -> bool {
		let Some(session) = self.editor_session() else {
			return false;
		};
		clips.iter().any(|clip| {
			session
				.multicam_state(quercus_engine_rs::node::NodeId::from_raw(clip.0))
				.is_some()
		})
	}

	fn multicam_enable_selected(
		&mut self,
		clips: Vec<ClipId>,
		enabled: bool,
		cx: &mut Context<Self>,
	) {
		let Some(session) = self.editor_session() else {
			return;
		};
		let clips = clips
			.into_iter()
			.map(|clip| quercus_engine_rs::node::NodeId::from_raw(clip.0))
			.collect();
		let result = session
			.execute(EditCommand::SetMulticamEnabled { clips, enabled })
			.map(|_| ());
		self.apply_edit(result, "multicam enable/disable", cx);
	}

	fn add_default_transition(
		&mut self,
		clips: Vec<ClipId>,
		cx: &mut Context<Self>,
	) -> Result<usize, String> {
		let Some(project) = self.project.clone() else {
			return Err("default transition: no project is open".to_string());
		};
		let Some(seq) = self.sequence else {
			return Err("default transition: no sequence is open".to_string());
		};
		let Some(tb) = self.time_base() else {
			return Err("default transition: the sequence has no frame rate".to_string());
		};
		// The preference is the WHOLE transition length; a default
		// transition is symmetric, so each wedge takes half of it (at least
		// one frame — `ts_to_rational` needs a positive frame count).
		let seconds: f64 = config_get_string(CONFIG_KEY_DEFAULT_TRANSITION_SEC)
			.parse()
			.unwrap_or_else(|_| {
				DEFAULT_TRANSITION_SEC
					.parse()
					.expect("the compiled-in default parses")
			});
		let fps = tb.1 as f64 / tb.0.max(1) as f64;
		let half_frames = (seconds * fps / 2.0).round().max(1.0) as i64;
		let half = graphops::ts_to_rational(half_frames, tb);
		let blocks: Vec<NodeId> = clips
			.iter()
			.filter_map(|id| {
				let block = self.clip_block(*id)?;
				(!self.clip_track_locked(block)).then_some(block)
			})
			.collect();
		let result = graphops::add_default_transition(&project, seq, &blocks, half);
		self.apply_edit(result.clone().map(|_| ()), "add default transition", cx);
		result
	}

	fn multicam_switch_to(&mut self, source: i32, split_clip: bool, cx: &mut Context<Self>) {
		let Some(session) = self.editor_session() else {
			return;
		};
		let Some(sequence) = self.sequence else {
			return;
		};
		let Some(state) = self.multicam_state_internal() else {
			return;
		};
		if source < 0 || source >= state.source_count {
			return;
		}
		// No-op guard: switching to the ALREADY current source must not
		// push an undo command. The playback-time "revision advances every
		// frame" report traced back to the multicam panel queue replaying
		// the same switch on every tick — each replay pushed a useless
		// undo entry, re-uploaded the whole 130 KB graph snapshot to every
		// worker and re-serialized it per frame (the "picture barely
		// updates" freeze).
		if state.current_source == source {
			return;
		}
		let sequence = quercus_engine_rs::node::NodeId::from_raw(sequence.raw());
		let result = session
			.execute(EditCommand::SwitchMulticam {
				clip: quercus_engine_rs::node::NodeId::from_raw(state.clip_id),
				source,
				split_clip,
				playhead: session.project().sequence_playhead(sequence),
			})
			.map(|_| ());
		self.apply_edit(result, "multicam switch", cx);
	}

	// --- multicam wizard ------------------------------------------------

	fn multicam_wizard_footage(&self) -> Option<Vec<WizardFootage>> {
		let session = self.editor_session()?;
		let project = session.project();
		Some(
			session
				.snapshot()
				.root
				.children
				.into_iter()
				.filter(|entry| entry.kind == Some(quercus_engine_rs::node::NodeKind::Footage))
				.filter_map(|entry| {
					let filename = project.footage_filename(entry.id)?;
					if filename.is_empty() {
						return None;
					}
					let duration = project.footage_duration(entry.id);
					let source_timecode = project
						.footage_source_start_time(entry.id)
						.map(|time| time.to_f64().round() as i64);
					Some(WizardFootage {
						id: entry.id.raw(),
						name: entry.name.into(),
						source_timecode,
						duration_s: (!duration.is_null()).then(|| duration.to_f64()),
						has_audio: Some(project.footage_audio_stream_count(entry.id) > 0),
					})
				})
				.collect(),
		)
	}

	fn multicam_wizard_sync_offsets(
		&self,
		selected: &[WizardFootage],
	) -> Result<Vec<WizardSyncOffset>, String> {
		let Some(reference) = selected.first() else {
			return Ok(Vec::new());
		};
		// One angle is enough: nothing to align.
		if selected.len() == 1 {
			return Ok(vec![WizardSyncOffset {
				footage: reference.id,
				offset_s: 0.0,
				confidence: Some(1.0),
			}]);
		}
		// Extract each angle's RMS envelope (100 ms windows) through the
		// audio waveform extractor over the footage's media file. The
		// reference angle is angle 0.
		let source_paths: Vec<(u64, String)> = {
			let project = self.project_ref().ok_or("no project open")?;
			let mut out = Vec::new();
			for entry in selected {
				let node = graphops::id_of(entry.id).ok_or("angle missing")?;
				if project.node_kind(node) != Some(NodeKind::Footage) {
					return Err("angle is not footage".to_string());
				}
				let filename = project.footage_filename(node).ok_or("angle is not footage")?;
				out.push((entry.id, filename));
			}
			out
		};
		let mut envelopes: Vec<Vec<f64>> = Vec::new();
		for (_, filename) in &source_paths {
			// The SDK takes an audio-stream ordinal, not the container stream ID.
			let envelope = crate::oakui::waveform::extract_audio_envelope(filename, 0)
				.ok_or_else(|| format!("failed to extract audio from {filename}"))?;
			envelopes.push(envelope);
		}
		let reference_envelope = envelopes[0].clone();
		let offsets: Vec<quercus_engine_rs::audio_analysis::OffsetEstimate> = envelopes[1..]
			.iter()
			.map(|candidate| {
				quercus_engine_rs::audio_analysis::estimate_envelope_offset(
					&reference_envelope,
					candidate,
					&[],
					&[],
					1,
					50,
				)
			})
			.collect();
		let mut out = vec![WizardSyncOffset {
			footage: reference.id,
			offset_s: 0.0,
			confidence: Some(1.0),
		}];
		for (entry, offset) in selected[1..].iter().zip(offsets.iter()) {
			out.push(WizardSyncOffset {
				footage: entry.id,
				// Envelopes use 100 ms windows and window_samples=1 above.
				// Timeline placement uses the wizard's opposite lag convention:
				// positive means the candidate clip starts later on the timeline.
				offset_s: -(offset.offset_samples as f64) * 0.1,
				confidence: offset.valid.then_some(offset.confidence),
			});
		}
		Ok(out)
	}

	fn multicam_create_sequence(
		&mut self,
		selected: Vec<WizardFootage>,
		offsets: Vec<f64>,
		name: String,
		cx: &mut Context<Self>,
	) -> Result<u64, String> {
		let Some(session) = self.editor_session() else {
			return Err(crate::i18n::tr("seqprops.error.no_project").to_string());
		};
		let Some(host_sequence) = self.sequence else {
			return Err("no sequence open".into());
		};
		let angles = selected
			.iter()
			.map(|angle| quercus_engine_rs::node::NodeId::from_raw(angle.id))
			.collect();
		let result = session.execute(EditCommand::CreateMulticamSequence {
			host_sequence: quercus_engine_rs::node::NodeId::from_raw(host_sequence.raw()),
			angles,
			offsets,
			name,
		});
		match result {
			Ok(_) => {
				self.apply_edit(Ok(()), "create multicam sequence", cx);
				Ok(host_sequence.raw())
			}
			Err(error) => {
				self.apply_edit(Err(error.clone()), "create multicam sequence", cx);
				Err(error)
			}
		}
	}

	fn backend_name(&self) -> &'static str {
		"real"
	}
}

impl RealEngine {
	/// Drops a text-project entry on the timeline as a GENERATOR clip: the
	/// clip spans five seconds from the drop point and its `tex_in` reads
	/// the text node, so the generator re-renders at whatever time the
	/// clip spans (no media to decode and no footage length to clamp to).
	/// The target track policy is the sequence drop's: the pointed display
	/// row when it is a video track, else the first video track. Undoable
	/// as one "Add Clip".
	fn drop_text_entry(
		&mut self,
		project: &ProjectRef,
		text: NodeId,
		track_index: usize,
		time: Frame,
		cx: &mut Context<Self>,
	) {
		let Some(host_seq) = self.sequence else {
			println!("[real engine] drop text: no sequence open");
			return;
		};
		let video_target = if self
			.tracks
			.get(track_index)
			.is_some_and(|t| t.kind == TrackKind::Video)
		{
			track_index
		} else if let Some(index) = self.tracks.iter().position(|t| t.kind == TrackKind::Video) {
			index
		} else {
			println!("[real engine] drop text: no video track");
			return;
		};
		let video_index = self.tracks[video_target].track_index;
		// Five seconds of the host sequence's frames (the drop's default
		// extent — the text clip has no media length to take one from).
		let (in_ts, length_ts) = {
			let Ok(tb) = graphops::sequence_time_base(project, host_seq)
				.ok_or_else(|| "host sequence has no frame rate".to_string())
			else {
				println!("[real engine] drop text: host has no frame rate");
				return;
			};
			let fps_f = tb.1.max(1) as f64 / tb.0.max(1) as f64;
			((time.0.max(0)), ((5.0 * fps_f).round() as i64).max(1))
		};
		let placed = graphops::place_text_clip(
			project,
			host_seq,
			text,
			video_index,
			in_ts,
			in_ts + length_ts,
		);
		let result = placed.map(|_| ());
		self.apply_edit(result, "drop text", cx);
	}

	/// Drops a sequence-project entry on the timeline as a NESTED-SEQUENCE
	/// clip: ONE video clip on the video track (not the sequence's tracks —
	/// a frame, not a folder, of tracks), fed from the sequence node. This
	/// is also how a multicam source sequence lands in the timeline: the
	/// clip is the single multi-cam row the panel detects. Undoable as one
	/// "Add Clip".
	fn drop_sequence_entry(
		&mut self,
		project: &ProjectRef,
		source_seq: NodeId,
		track_index: usize,
		time: Frame,
		cx: &mut Context<Self>,
	) {
		let Some(host_seq) = self.sequence else {
			println!("[real engine] drop sequence: no sequence open");
			return;
		};
		// The target video track: the pointed display row when it is a
		// video track, else the first video track (the footage policy).
		let video_target = if self
			.tracks
			.get(track_index)
			.is_some_and(|t| t.kind == TrackKind::Video)
		{
			track_index
		} else if let Some(index) = self.tracks.iter().position(|t| t.kind == TrackKind::Video) {
			index
		} else {
			println!("[real engine] drop sequence: no video track");
			return;
		};
		let video_index = self.tracks[video_target].track_index;
		// The clip length: the source sequence's length converted into the
		// host sequence's frame timestamps (the drop ghost's extent).
		let (in_ts, length_ts) = {
			let Ok(tb) = graphops::sequence_time_base(project, host_seq)
				.ok_or_else(|| "host sequence has no frame rate".to_string())
			else {
				println!("[real engine] drop sequence: host has no frame rate");
				return;
			};
			let fps_f = tb.1.max(1) as f64 / tb.0.max(1) as f64;
			let source_seconds = graphops::sequence_length(project, source_seq).to_f64();
			let length_ts = ((source_seconds * fps_f).round() as i64).max(1);
			(time.0.max(0), length_ts)
		};
		let placed = graphops::place_nested_sequence_clip(
			project,
			host_seq,
			source_seq,
			video_index,
			in_ts,
			in_ts + length_ts,
		);
		let placed = match placed {
			Ok(clip) => clip,
			Err(e) => {
				self.apply_edit(Err(e.clone()), "drop sequence", cx);
				return;
			}
		};
		// A source sequence with a multicam node drops as the multi-cam
		// clip itself (its texture feeds from the multicam output, so the
		// panel detects it — "未检测到多机位片段" when it nur fed the
		// raw source sequence).
		// Collect the candidate multicam ids under the lock FIRST, then
		// match each with `multicam_sequence` OUTSIDE the guard (that
		// helper locks the project itself — a live guard re-enters the
		// non-recursive mutex, exactly the wizard-freeze shape).
		let candidates = graphops::multicam_nodes(project);
		let session = self.editor_session();
		let mc_for_source = candidates.iter().copied().find(|&mc| {
			session.as_ref().is_some_and(|session| {
				session
					.multicam_sequence(quercus_engine_rs::node::NodeId::from_raw(mc.raw()))
					.map(|sequence| sequence.raw())
					== Some(source_seq.raw())
			})
		});
		if let Some(mc) = mc_for_source {
			// Rewire the dropped clip's texture input from the nested
			// sequence to the multicam's output (the panel then detects a
			// multicam clip).
			let _ = project.disconnect(placed, "tex_in", -1);
			let result = if project.connect(mc, placed, "tex_in", -1) {
				Ok(())
			} else {
				Err("connect multicam to dropped clip failed".to_string())
			};
			self.apply_edit(result, "drop sequence (multicam)", cx);
		} else {
			self.apply_edit(Ok(()), "drop sequence", cx);
		}
	}

}

// ---------------------------------------------------------------------------
// Format dispatch (pure, unit tested)
// ---------------------------------------------------------------------------

/// Classifies a project-file extension for open/save dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectFormat {
	/// `.ove` / `.ovexml` — the native serializer.
	Ove,
	/// `.otio` — OpenTimelineIO JSON.
	Otio,
	/// `.fcpxml` — Final Cut Pro XML.
	Fcpxml,
}

impl ProjectFormat {
	/// The format for a file path, by extension (unknown → [`Ove`]
	/// (ProjectFormat::Ove), matching the app's default serializer).
	pub fn of(path: &PathBuf) -> Self {
		let ext = path
			.extension()
			.and_then(|e| e.to_str())
			.map(|e| e.to_ascii_lowercase());
		match ext.as_deref() {
			Some("otio") => ProjectFormat::Otio,
			Some("fcpxml") => ProjectFormat::Fcpxml,
			_ => ProjectFormat::Ove,
		}
	}
}

impl RealEngine {
	/// Re-declares the display color policy (and the content colorspace) on
	/// every open window after the project's color pipeline settings changed.
	/// Runs deferred: [`Self::set_project_color_settings`] is committed from
	/// inside the settings dialog's window update, where a direct
	/// `update_window` would fail; `defer` re-enters the app after that
	/// update completes.
	fn reapply_display_policy_to_windows(&self, cx: &mut Context<Self>) {
		let windows = cx.windows();
		cx.defer(move |app| {
			for handle in windows {
				let _ = app.update_window(handle, |_root, window, _app| {
					crate::oakui::displaycolor::apply_to_window(window);
				});
			}
		});
	}

	/// Auto-creates a sequence for a footage drop onto an empty timeline (see
	/// `drop_footage`): the format mirrors the footage's first video stream;
	/// pure-audio footage uses the default sequence format. Returns the new
	/// sequence's node, or `None` when creation failed (errors are logged, not
	/// propagated — the drop itself is a no-op in that case).
	fn create_sequence_for_drop(
		&mut self,
		project: &ProjectRef,
		footage: NodeId,
		cx: &mut Context<Self>,
	) -> Option<NodeId> {
		let Some(session) = self.editor_session() else {
			return None;
		};
		let params = session.project().footage_video_params(
			quercus_engine_rs::node::NodeId::from_raw(footage.raw()),
			0,
		);
		match params {
			Some(_) => match session.execute(EditCommand::CreateSequenceFromFootage {
				footage: quercus_engine_rs::node::NodeId::from_raw(footage.raw()),
				name: "Sequence 1".into(),
			}) {
				Ok(result) => {
					if let Some(id) = result.primary.and_then(|id| graphops::id_of(id.raw())) {
						self.activate_sequence(id);
					}
					self.refresh_sequence_info();
					self.rebuild_timeline();
					self.sequence
				}
				Err(err) => {
					println!("[real engine] drop footage: auto-create sequence failed: {err}");
					None
				}
			},
			None => {
				let seq = graphops::create_sequence(project, "Sequence 1");
				self.activate_sequence(seq);
				self.refresh_sequence_info();
				self.rebuild_timeline();
				self.push_graph_snapshot();
				cx.notify();
				Some(seq)
			}
		}
	}

	/// Opens a `.ove` / `.ovexml` project through the module serializer.
	fn open_ove(&mut self, path: &PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
		let project = graphops::load_ove(path)
			.map_err(|e| format!("failed to load \"{}\": {e}", path.display()))?;
		// The module serializer cannot parse every legacy document (e.g. the
		// `<olive>`-rooted format skips its nested `<project>` body), which
		// loads "successfully" with no content; surface it instead of
		// pretending the project opened.
		if project.node_count() == 0 {
			println!(
				"[real engine] warning: \"{}\" loaded but contained no parseable content; starting from an empty project",
				path.display()
			);
		}
		self.adopt_project(project, cx);
		Ok(())
	}

	/// Opens an `.otio` / `.fcpxml` project through the SDK task API and adopts
	/// the exact loaded Arc through the migration handle.
	fn open_interchange(&mut self, path: &PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
		let task = quercus_engine_rs::task::load_otio(&path.to_string_lossy())
			.ok_or_else(|| "failed to start the interchange load".to_string())?;
		if !task.wait() {
			let error = task.error();
			let error = if error.is_empty() {
				"the task failed".into()
			} else {
				error
			};
			return Err(format!("failed to load \"{}\": {error}", path.display()));
		}
		let loaded = task.take_project().ok_or_else(|| {
			format!(
				"loaded \"{}\" but the loader returned no project",
				path.display()
			)
		})?;
		self.adopt_project(loaded, cx);
		Ok(())
	}

	/// Exports the project to `path` through the OVE serializer (the .ove
	/// branch of 导出工程文件…; the write-through library stays the primary
	/// persistence, this only writes a file).
	fn export_ove(&mut self, path: &Path, _cx: &mut Context<Self>) -> Result<(), String> {
		let Some(project) = self.project.clone() else {
			return Err("no project open".into());
		};
		graphops::save_ove(&project, path)
			.map_err(|e| format!("failed to export the project: {e}"))?;
		// The save records the target filename (legacy save side effect);
		// refresh the display name to match.
		let (name, filename) = (project.name(), project.filename());
		if !name.is_empty() && name != "(untitled)" {
			self.project_info.name = name;
		}
		if !filename.is_empty() {
			self.project_info.path = PathBuf::from(filename);
		}
		Ok(())
	}

	/// Exports as `.otio` / `.fcpxml` through the SDK's unified task API.
	fn export_interchange(
		&mut self,
		path: &PathBuf,
		_cx: &mut Context<Self>,
	) -> Result<(), String> {
		let Some(session) = self.editor_session() else {
			return Err("no project open".into());
		};
		let filename = path.to_string_lossy().into_owned();
		let task = quercus_engine_rs::task::save_otio(&session.project(), &filename)
			.ok_or_else(|| "failed to start the interchange export".to_string())?;
		if !task.wait() {
			let error = task.error();
			let error = if error.is_empty() {
				"the task failed".into()
			} else {
				error
			};
			return Err(format!("failed to export \"{}\": {error}", path.display()));
		}
		self.project_info.path = path.clone();
		Ok(())
	}

	/// Opens the library row `uuid` through the oakstorage database backend
	/// and adopts it (the adopt binds the project to the write-through
	/// session, continuing the row's journal from its head seq).
	fn open_library_project(&mut self, uuid: &str, cx: &mut Context<Self>) -> Result<(), String> {
		let project = graphops::library_open(uuid)
			.map_err(|e| format!("failed to open the library project: {e}"))?;
		self.adopt_project(project, cx);
		// The project's module name is filename-derived ("(untitled)" for a
		// library row); display the library row name instead.
		if let Ok(rows) = graphops::library_list() {
			if let Some(row) = rows.iter().find(|row| row.uuid == uuid) {
				self.project_info.name = row.name.clone();
			}
		}
		Ok(())
	}
}

/// Builds the export-format list: (format id, display name, extension) from
/// the SDK encoding enumeration.
///
/// Pure helper so the export dialog can be unit tested.
pub fn encoding_formats() -> Vec<(i32, String, String)> {
	let mut out = Vec::new();
	for i in 0..quercus_engine_rs::codec::Format::count() {
		let Some(format) = quercus_engine_rs::codec::Format::from_i32(i) else {
			continue;
		};
		out.push((i, format.name(), format.extension()));
	}
	out
}

/// The format id of the default export container: MPEG-4 Video (`.mp4`).
pub const EXPORT_FORMAT_MP4: i32 = 2;

// ---------------------------------------------------------------------------
// Config (preferences) — the oak_core config store directly
// ---------------------------------------------------------------------------

/// The config key selecting the renderer backend (worker `create_renderer`
/// backend id).
pub const CONFIG_KEY_RENDERER_BACKEND: &str = "GraphicsBackend";
/// The config key holding the UI theme (`"dark"` / `"light"`; the app
/// defaults to dark when the key is absent).
pub const CONFIG_KEY_THEME: &str = "Theme";
/// The config key overriding the disk cache directory (empty = the
/// platform default `<config dir>/mediacache`; honored by oak_core's
/// `default_disk_cache_path`, so oakrender/oaknode caches follow it).
pub const CONFIG_KEY_DISK_CACHE_PATH: &str = "DiskCachePath";
/// The config key toggling proxy media use (`UseProxyMedia`, bool).
pub const CONFIG_KEY_USE_PROXY: &str = "UseProxyMedia";
/// The config key holding the explicit ffmpeg executable path the proxy
/// transcode should use (`FFmpegPath`; empty = auto-detect on PATH and
/// the common install locations).
pub const CONFIG_KEY_FFMPEG_PATH: &str = "FFmpegPath";
/// The config key holding the proxy resolution divider (`ProxyDivider`,
/// int; 1 = full resolution, 2/4/8/16 = 1/2 … 1/16). oakcodec's
/// `ProxyManager::proxy_params_from_config` reads it for generation.
pub const CONFIG_KEY_PROXY_DIVIDER: &str = "ProxyDivider";
/// The config key holding the project snapshot interval in seconds
/// (`Storage/SnapshotIntervalSec`; the write-through era's "auto-save
/// interval" — oakstorage's snapshot thread reads it every pass, default
/// 600, ≤ 0 snapshots every dirty save).
pub const CONFIG_KEY_SNAPSHOT_INTERVAL_SEC: &str = "Storage/SnapshotIntervalSec";
/// The config key holding the default transition length in seconds
/// (`DefaultTransitionLength`, decimal string; consumed by the engine's
/// add-default-transition command — currently a module stub).
pub const CONFIG_KEY_DEFAULT_TRANSITION_SEC: &str = "DefaultTransitionLength";
/// The config key holding the audio output device NAME (empty = system
/// default; C++ parity `AudioOutput`).
pub const CONFIG_KEY_AUDIO_OUTPUT: &str = "AudioOutput";
/// The config key holding the audio input device NAME (`AudioInput`).
pub const CONFIG_KEY_AUDIO_INPUT: &str = "AudioInput";

/// The default snapshot interval (seconds), mirroring oakstorage's
/// compiled-in default.
pub const DEFAULT_SNAPSHOT_INTERVAL_SEC: i64 = 600;
/// The default transition length (seconds).
pub const DEFAULT_TRANSITION_SEC: &str = "0.5";

/// Loads the persisted configuration from disk (once at startup, before
/// any preference is read).
pub fn config_load() {
	Config::global().load().ok();
}

/// Persists the configuration to disk (the app calls it on exit).
pub fn config_save() {
	Config::global().save().ok();
}

/// Reads a config string from the store (empty when missing).
pub fn config_get_string(key: &str) -> String {
	Config::global().get(key).unwrap_or_default()
}

/// Writes a config string to the store.
pub fn config_set_string(key: &str, value: &str) {
	let _ = Config::global().set(key, value);
}

/// Reads a config integer from the store (`default` when the key is
/// missing or not an integer).
pub fn config_get_int(key: &str, default: i64) -> i64 {
	Config::global().get_i64(key, default)
}

/// Writes a config integer to the store.
pub fn config_set_int(key: &str, value: i64) {
	let _ = Config::global().set_i64(key, value);
}

/// Reads a config boolean through the string accessor (the store parses
/// `"true"`/`"false"` for registered bool keys).
pub fn config_get_bool(key: &str, default: bool) -> bool {
	match config_get_string(key).as_str() {
		"true" => true,
		"false" => false,
		_ => default,
	}
}

/// Writes a config boolean (see [`config_get_bool`]).
pub fn config_set_bool(key: &str, value: bool) {
	config_set_string(key, if value { "true" } else { "false" });
}

/// The renderer backends offered in the preferences dialog, in display
/// order. The first entry is the built-in default.
pub fn renderer_backends() -> Vec<&'static str> {
	vec!["opengl", "metal", "vulkan", "none"]
}

/// The proxy resolution dividers offered in the preferences dialog, in
/// display order (1 = full resolution).
pub fn proxy_dividers() -> Vec<i64> {
	vec![1, 2, 4, 8, 16]
}

/// Whether the persisted theme is dark (the default).
pub fn theme_is_dark() -> bool {
	config_get_string(CONFIG_KEY_THEME) != "light"
}

/// Persists the theme choice (applied live by the caller).
pub fn set_theme_dark(dark: bool) {
	config_set_string(CONFIG_KEY_THEME, if dark { "dark" } else { "light" });
}

// ---------------------------------------------------------------------------
// Audio devices (preferences + startup wiring) — through the SDK audio facade
// ---------------------------------------------------------------------------

/// The host's audio output device names in enumeration order (the index is
/// what the engine's `set_output_device` takes).
pub fn audio_output_devices() -> Vec<String> {
	quercus_engine_rs::audio::output_device_names().to_vec()
}

/// The host's audio input device names (see [`audio_output_devices`]).
pub fn audio_input_devices() -> Vec<String> {
	quercus_engine_rs::audio::input_device_names().to_vec()
}

/// Selects the output device by NAME (empty = system default), persists the
/// choice to `AudioOutput`, and applies it live: the output stream reopens
/// on the device with the next pushed samples.
pub fn set_audio_output_device(name: &str) {
	config_set_string(CONFIG_KEY_AUDIO_OUTPUT, name);
	let index = if name.is_empty() {
		-1
	} else {
		quercus_engine_rs::audio::device_index_by_name(name, true).unwrap_or(-1)
	};
	let _ = quercus_engine_rs::audio::set_output_device(index);
}

/// Selects the input device by NAME (empty = system default) and persists
/// the choice to `AudioInput` (used by recording).
pub fn set_audio_input_device(name: &str) {
	config_set_string(CONFIG_KEY_AUDIO_INPUT, name);
	let index = if name.is_empty() {
		-1
	} else {
		quercus_engine_rs::audio::device_index_by_name(name, false).unwrap_or(-1)
	};
	let _ = quercus_engine_rs::audio::set_input_device(index);
}

/// The configured output device name, validated against the live
/// enumeration (empty when the configured device is gone).
pub fn audio_output_device() -> String {
	let name = config_get_string(CONFIG_KEY_AUDIO_OUTPUT);
	if name.is_empty() || audio_output_devices().contains(&name) {
		name
	} else {
		String::new()
	}
}

/// The configured input device name (see [`audio_output_device`]).
pub fn audio_input_device() -> String {
	let name = config_get_string(CONFIG_KEY_AUDIO_INPUT);
	if name.is_empty() || audio_input_devices().contains(&name) {
		name
	} else {
		String::new()
	}
}

/// Brings up the AudioManager singleton and applies the persisted device
/// choices. Called once at startup; without an instance `push_to_output`
/// fails silently and playback stays video-only.
pub fn audio_init_from_config() {
	let _ = quercus_engine_rs::audio::create_instance();
	let output = config_get_string(CONFIG_KEY_AUDIO_OUTPUT);
	if !output.is_empty() {
		set_audio_output_device(&output);
	}
	let input = config_get_string(CONFIG_KEY_AUDIO_INPUT);
	if !input.is_empty() {
		set_audio_input_device(&input);
	}
}

// ---------------------------------------------------------------------------
// Project library (M13 D4: the write-through database the manager browses)
// ---------------------------------------------------------------------------

/// The config key selecting the storage backend (see
/// `crates/oakstorage/src/writethrough.rs`).
pub const CONFIG_KEY_STORAGE_BACKEND: &str = "Storage/Backend";
/// The config key holding the PostgreSQL connection string (`Storage/PgUrl`,
/// `user:pass@host:5432/dbname`; used when the backend is `"pg"` — see
/// `crates/oakstorage/src/writethrough.rs`).
pub const CONFIG_KEY_PG_URL: &str = "Storage/PgUrl";

/// Enables the SQLite write-through library unless the user configured the
/// backend explicitly (any existing value — including "off" — wins over the
/// app's default). The library path defaults to `<system data
/// directory>/library.db`.
pub fn configure_storage() {
	if config_get_string(CONFIG_KEY_STORAGE_BACKEND).is_empty() {
		config_set_string(CONFIG_KEY_STORAGE_BACKEND, "sqlite");
	}
}

/// Flushes every bound project (write-through + snapshot) and stops the
/// snapshot thread. The app calls this on exit.
pub fn storage_flush() {
	quercus_engine_rs::storage::flush();
}

/// The library rows, most recently modified first (the project manager's
/// data source).
pub fn library_list() -> Result<Vec<LibraryProject>, String> {
	graphops::library_list()
}

#[cfg(test)]
mod session_smoke_tests {
	use super::*;
	use crate::oakui::engine::AppEngine;
	use crate::widgets::timeline::{Frame, TimelineDataSource};
	use gpui_kit::TestAppContext;

	fn test_media() -> std::path::PathBuf {
		std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/demo.mp4")
	}

	/// The live SDK session survives a full open → import → undo/redo →
	/// marker → work-area round trip through the real engine.
	#[gpui_kit::test]
	async fn real_engine_edits_undo_and_auxiliary_state(cx: &mut TestAppContext) {
		cx.update(|cx| {
			gpui_kit::init(cx);
			cx.init_colors();
		});
		let engine = cx.update(|cx| cx.new(RealEngine::create));
		cx.update(|app| {
			engine.update(app, |engine, cx| engine.new_project(cx));
			engine.update(app, |engine, cx| {
				assert!(engine.current_sequence_id().is_some(), "new project has a sequence");
				assert!(engine.track_count() > 0, "default track layout");
				let before = engine.history_entries().len();
				engine
					.import_footage(test_media(), cx)
					.expect("import the demo footage");
				let after_import = engine.history_entries().len();
				assert_eq!(after_import, before + 1, "import is one undo entry");
				engine.undo(cx);
				assert_eq!(
					engine.history_index(),
					after_import as i64 - 1,
					"undo steps back one row"
				);
				engine.redo(cx);
				assert_eq!(
					engine.history_index(),
					after_import as i64,
					"redo restores the row"
				);

				assert!(engine.markers().is_empty());
				engine.add_marker_at_playhead(cx);
				assert_eq!(engine.markers().len(), 1);
				engine.undo(cx);
				assert!(engine.markers().is_empty(), "marker undo");

				engine.set_workarea_preview(Frame(2), Frame(5), cx);
				engine.commit_workarea(Frame(2), Frame(5), Frame(2), Frame(5), cx);
				assert_eq!(engine.workarea(), Some((Frame(2), Frame(5))));
			});
		});
	}
}
