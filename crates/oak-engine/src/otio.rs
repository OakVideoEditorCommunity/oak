//! C ABI for oak-otio (prefix `oak_otio_`): the OpenTimelineIO JSON and
//! FCPXML interchange binding.
//!
//! Two layers:
//!
//! - **Document level** (string in/out): parse/serialize OTIO JSON
//!   ([`oak_otio_document_from_json`]/[`oak_otio_document_to_json`]) and
//!   FCPXML ([`oak_otio_timelines_from_fcpxml`]/[`oak_otio_timelines_to_fcpxml`]).
//!   File IO is the consumer's business (the SDK wraps it).
//! - **Structured level**: the object graph — timeline → tracks →
//!   composables (clip/gap/transition) — as refcounted owned handles.
//!   Traversal clones out of the parent; edits write back through
//!   `set_track`/`set_child` (or `append_child`). This is the surface a
//!   functional plugin uses to inspect an imported interchange document
//!   or build a timeline from scratch (e.g. an AI rough cut) and export
//!   it.
//!
//! Conventions (shared with the rest of the engine ABI): string
//! arguments are `(ptr, len)` borrows; string outputs are
//! snprintf-style (`copy_str_out`); handles are refcounted with a typed
//! `add_ref`/`release` pair; all entry points are null-safe and never
//! panic across the boundary.

use std::sync::Mutex;

use oak_otio::model::{
	Clip, Composable, ExternalReference, Gap, MediaReference, RationalTime, Serializable,
	TimeRange, Track, Transition,
};
use oak_otio::model::Timeline;

use crate::codec::{copy_str_out, str_arg};
use crate::handle::NativeMirror;

// ---------------------------------------------------------------------------
// POD mirrors
// ---------------------------------------------------------------------------

/// `repr(C)` mirror of oak-otio's `RationalTime` (a time value and its
/// frame rate).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OakOtioRationalTime {
	pub value: f64,
	pub rate: f64,
}

impl NativeMirror for OakOtioRationalTime {
	type Native = RationalTime;

	fn from_native(t: &RationalTime) -> Self {
		Self {
			value: t.value(),
			rate: t.rate(),
		}
	}

	fn to_native(&self) -> RationalTime {
		RationalTime::new(self.value, self.rate)
	}
}

/// `repr(C)` mirror of oak-otio's `TimeRange` (a start time and a
/// duration).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OakOtioTimeRange {
	pub start_time: OakOtioRationalTime,
	pub duration: OakOtioRationalTime,
}

impl NativeMirror for OakOtioTimeRange {
	type Native = TimeRange;

	fn from_native(r: &TimeRange) -> Self {
		Self {
			start_time: OakOtioRationalTime::from_native(&r.start_time()),
			duration: OakOtioRationalTime::from_native(&r.duration()),
		}
	}

	fn to_native(&self) -> TimeRange {
		TimeRange::new(self.start_time.to_native(), self.duration.to_native())
	}
}

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// A parsed OTIO document (the root `Serializable`).
pub struct OakOtioDocument {
	inner: Mutex<Serializable>,
}

/// A list of timelines (the FCPXML document unit: one timeline per
/// sequence; also the builder for FCPXML export).
pub struct OakOtioTimelines {
	inner: Mutex<Vec<Timeline>>,
}

/// A timeline (name, global start time, tracks).
pub struct OakOtioTimeline {
	inner: Mutex<Timeline>,
}

/// A track (kind + ordered composables).
pub struct OakOtioTrack {
	inner: Mutex<Track>,
}

/// A track/stack child: a clip, gap, or transition.
pub struct OakOtioComposable {
	inner: Mutex<Composable>,
}

crate::export_handle!(
	OakOtioDocument,
	oak_otio_document_add_ref,
	oak_otio_document_release
);
crate::export_handle!(
	OakOtioTimelines,
	oak_otio_timelines_add_ref,
	oak_otio_timelines_release
);
crate::export_handle!(
	OakOtioTimeline,
	oak_otio_timeline_add_ref,
	oak_otio_timeline_release
);
crate::export_handle!(OakOtioTrack, oak_otio_track_add_ref, oak_otio_track_release);
crate::export_handle!(
	OakOtioComposable,
	oak_otio_composable_add_ref,
	oak_otio_composable_release
);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
	m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `oak_otio_document_kind` result: the root is a Timeline.
pub const OAK_OTIO_DOC_TIMELINE: i32 = 0;
/// `oak_otio_document_kind` result: the root is a SerializableCollection.
pub const OAK_OTIO_DOC_COLLECTION: i32 = 1;
/// `oak_otio_document_kind` result: an unrecognized root schema (kept
/// whole for round-tripping).
pub const OAK_OTIO_DOC_RAW: i32 = 2;

/// `oak_otio_composable_kind` result: a clip.
pub const OAK_OTIO_COMPOSABLE_CLIP: i32 = 0;
/// `oak_otio_composable_kind` result: a gap.
pub const OAK_OTIO_COMPOSABLE_GAP: i32 = 1;
/// `oak_otio_composable_kind` result: a transition.
pub const OAK_OTIO_COMPOSABLE_TRANSITION: i32 = 2;
/// `oak_otio_composable_kind` result: anything else (nested track/stack,
/// or an unrecognized schema kept whole).
pub const OAK_OTIO_COMPOSABLE_OTHER: i32 = 3;

// ---------------------------------------------------------------------------
// Document (OTIO JSON root)
// ---------------------------------------------------------------------------

/// Parses an OTIO JSON document. Null on invalid input or a parse
/// error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_document_from_json(
	json: *const u8,
	json_len: usize,
) -> *mut OakOtioDocument {
	let Some(json) = (unsafe { str_arg(json, json_len) }) else {
		return std::ptr::null_mut();
	};
	match oak_otio::from_json_string(json) {
		Ok(root) => crate::handle::into_ffi(OakOtioDocument {
			inner: Mutex::new(root),
		}) as *mut OakOtioDocument,
		Err(_) => std::ptr::null_mut(),
	}
}

/// Serializes the document to OTIO JSON, snprintf-style; -1 on a null
/// handle or a serialization error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_document_to_json(
	doc: *const OakOtioDocument,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if doc.is_null() {
		return -1;
	}
	let root = lock(&unsafe { &*doc }.inner);
	match root.to_json_string() {
		Ok(json) => {
			drop(root);
			copy_str_out(&json, buf, buf_len)
		}
		Err(_) => -1,
	}
}

/// The root kind: [`OAK_OTIO_DOC_TIMELINE`], [`OAK_OTIO_DOC_COLLECTION`]
/// or [`OAK_OTIO_DOC_RAW`]; -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_document_kind(doc: *const OakOtioDocument) -> i32 {
	if doc.is_null() {
		return -1;
	}
	match &*lock(&unsafe { &*doc }.inner) {
		Serializable::Timeline(_) => OAK_OTIO_DOC_TIMELINE,
		Serializable::SerializableCollection(_) => OAK_OTIO_DOC_COLLECTION,
		Serializable::Raw(_) => OAK_OTIO_DOC_RAW,
	}
}

/// Clones the root timeline out of the document (null when the root
/// is not a timeline).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_document_timeline(
	doc: *const OakOtioDocument,
) -> *mut OakOtioTimeline {
	if doc.is_null() {
		return std::ptr::null_mut();
	}
	let root = lock(&unsafe { &*doc }.inner);
	match root.as_timeline().cloned() {
		Some(tl) => {
			drop(root);
			crate::handle::into_ffi(OakOtioTimeline {
				inner: Mutex::new(tl),
			}) as *mut OakOtioTimeline
		}
		None => std::ptr::null_mut(),
	}
}

/// The number of timeline children of a collection root; 0 for other
/// roots, -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_document_collection_timeline_count(
	doc: *const OakOtioDocument,
) -> i32 {
	if doc.is_null() {
		return -1;
	}
	let root = lock(&unsafe { &*doc }.inner);
	match root.as_collection() {
		Some(c) => c
			.children()
			.iter()
			.filter(|s| matches!(***s, Serializable::Timeline(_)))
			.count() as i32,
		None => 0,
	}
}

/// Clones the `index`-th timeline child of a collection root (index
/// among timelines only); null on a null handle, a non-collection
/// root, or an out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_document_collection_timeline_at(
	doc: *const OakOtioDocument,
	index: i32,
) -> *mut OakOtioTimeline {
	if doc.is_null() || index < 0 {
		return std::ptr::null_mut();
	}
	let root = lock(&unsafe { &*doc }.inner);
	let Some(collection) = root.as_collection() else {
		return std::ptr::null_mut();
	};
	let timeline = collection
		.children()
		.iter()
		.filter_map(|s| s.as_timeline())
		.nth(index as usize)
		.cloned();
	match timeline {
		Some(tl) => {
			drop(root);
			crate::handle::into_ffi(OakOtioTimeline {
				inner: Mutex::new(tl),
			}) as *mut OakOtioTimeline
		}
		None => std::ptr::null_mut(),
	}
}

/// Wraps a timeline as a document (for JSON serialization).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_document_from_timeline(
	timeline: *const OakOtioTimeline,
) -> *mut OakOtioDocument {
	if timeline.is_null() {
		return std::ptr::null_mut();
	}
	let tl = lock(&unsafe { &*timeline }.inner).clone();
	crate::handle::into_ffi(OakOtioDocument {
		inner: Mutex::new(Serializable::Timeline(tl)),
	}) as *mut OakOtioDocument
}

// ---------------------------------------------------------------------------
// Timelines (FCPXML document unit / export builder)
// ---------------------------------------------------------------------------

/// A new, empty timeline list (ref count 1).
#[unsafe(no_mangle)]
pub extern "C" fn oak_otio_timelines_new() -> *mut OakOtioTimelines {
	crate::handle::into_ffi(OakOtioTimelines {
		inner: Mutex::new(Vec::new()),
	}) as *mut OakOtioTimelines
}

/// Parses an FCPXML document (one timeline per `<sequence>`). Null on
/// invalid input or a parse error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timelines_from_fcpxml(
	xml: *const u8,
	xml_len: usize,
) -> *mut OakOtioTimelines {
	let Some(xml) = (unsafe { str_arg(xml, xml_len) }) else {
		return std::ptr::null_mut();
	};
	match oak_otio::from_fcpxml_string(xml) {
		Ok(timelines) => crate::handle::into_ffi(OakOtioTimelines {
			inner: Mutex::new(timelines),
		}) as *mut OakOtioTimelines,
		Err(_) => std::ptr::null_mut(),
	}
}

/// Serializes the timeline list to FCPXML, snprintf-style; -1 on a
/// null handle or a serialization error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timelines_to_fcpxml(
	timelines: *const OakOtioTimelines,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if timelines.is_null() {
		return -1;
	}
	let list = lock(&unsafe { &*timelines }.inner);
	match oak_otio::to_fcpxml_string(&list) {
		Ok(xml) => {
			drop(list);
			copy_str_out(&xml, buf, buf_len)
		}
		Err(_) => -1,
	}
}

/// Number of timelines in the list; -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timelines_count(timelines: *const OakOtioTimelines) -> i32 {
	if timelines.is_null() {
		return -1;
	}
	lock(&unsafe { &*timelines }.inner).len() as i32
}

/// Clones the timeline at `index` out of the list; null on a null
/// handle or an out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timelines_at(
	timelines: *const OakOtioTimelines,
	index: i32,
) -> *mut OakOtioTimeline {
	if timelines.is_null() || index < 0 {
		return std::ptr::null_mut();
	}
	let list = lock(&unsafe { &*timelines }.inner);
	let Some(tl) = list.get(index as usize).cloned() else {
		return std::ptr::null_mut();
	};
	drop(list);
	crate::handle::into_ffi(OakOtioTimeline {
		inner: Mutex::new(tl),
	}) as *mut OakOtioTimeline
}

/// Replaces the timeline at `index` with a copy of `timeline`
/// (write-back after editing). False on a null handle or an
/// out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timelines_set(
	timelines: *mut OakOtioTimelines,
	index: i32,
	timeline: *const OakOtioTimeline,
) -> bool {
	if timelines.is_null() || timeline.is_null() || index < 0 {
		return false;
	}
	let tl = lock(&unsafe { &*timeline }.inner).clone();
	let mut list = lock(&unsafe { &*timelines }.inner);
	let Some(slot) = list.get_mut(index as usize) else {
		return false;
	};
	*slot = tl;
	true
}

/// Appends a copy of `timeline` to the list. False on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timelines_add(
	timelines: *mut OakOtioTimelines,
	timeline: *const OakOtioTimeline,
) -> bool {
	if timelines.is_null() || timeline.is_null() {
		return false;
	}
	let tl = lock(&unsafe { &*timeline }.inner).clone();
	lock(&unsafe { &*timelines }.inner).push(tl);
	true
}

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

/// A new named timeline with an empty tracks stack (ref count 1).
/// Null on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_new(
	name: *const u8,
	name_len: usize,
) -> *mut OakOtioTimeline {
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return std::ptr::null_mut();
	};
	crate::handle::into_ffi(OakOtioTimeline {
		inner: Mutex::new(Timeline::new(name)),
	}) as *mut OakOtioTimeline
}

/// The timeline name, snprintf-style; -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_name(
	timeline: *const OakOtioTimeline,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if timeline.is_null() {
		return -1;
	}
	let name = lock(&unsafe { &*timeline }.inner).name().to_string();
	copy_str_out(&name, buf, buf_len)
}

/// Renames the timeline. False on a null handle or invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_set_name(
	timeline: *mut OakOtioTimeline,
	name: *const u8,
	name_len: usize,
) -> bool {
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return false;
	};
	if timeline.is_null() {
		return false;
	}
	lock(&unsafe { &*timeline }.inner).set_name(name);
	true
}

/// Reads the global start time into `out`. False when unset (or on a
/// null handle/output).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_global_start_time(
	timeline: *const OakOtioTimeline,
	out: *mut OakOtioRationalTime,
) -> bool {
	if timeline.is_null() || out.is_null() {
		return false;
	}
	let tl = lock(&unsafe { &*timeline }.inner);
	let Some(start) = tl.global_start_time() else {
		return false;
	};
	unsafe { *out = OakOtioRationalTime::from_native(&start) };
	true
}

/// Sets the global start time. False on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_set_global_start_time(
	timeline: *mut OakOtioTimeline,
	start: OakOtioRationalTime,
) -> bool {
	if timeline.is_null() {
		return false;
	}
	lock(&unsafe { &*timeline }.inner).set_global_start_time(start.to_native());
	true
}

/// Serializes the timeline alone to OTIO JSON, snprintf-style; -1 on
/// a null handle or a serialization error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_to_json(
	timeline: *const OakOtioTimeline,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if timeline.is_null() {
		return -1;
	}
	let tl = lock(&unsafe { &*timeline }.inner);
	match tl.to_json_string() {
		Ok(json) => {
			drop(tl);
			copy_str_out(&json, buf, buf_len)
		}
		Err(_) => -1,
	}
}

/// Number of tracks; -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_track_count(timeline: *const OakOtioTimeline) -> i32 {
	if timeline.is_null() {
		return -1;
	}
	lock(&unsafe { &*timeline }.inner).tracks().children().len() as i32
}

/// Clones the track at `index` (a timeline's tracks are
/// `Composable::Track` children of its tracks stack); null on a null
/// handle, an out-of-range index, or a non-track child.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_track_at(
	timeline: *const OakOtioTimeline,
	index: i32,
) -> *mut OakOtioTrack {
	if timeline.is_null() || index < 0 {
		return std::ptr::null_mut();
	}
	let tl = lock(&unsafe { &*timeline }.inner);
	let track = tl
		.tracks()
		.children()
		.get(index as usize)
		.and_then(|c| c.as_track())
		.cloned();
	match track {
		Some(track) => {
			drop(tl);
			crate::handle::into_ffi(OakOtioTrack {
				inner: Mutex::new(track),
			}) as *mut OakOtioTrack
		}
		None => std::ptr::null_mut(),
	}
}

/// Replaces the track at `index` with a copy of `track` (write-back
/// after editing). False on a null handle or an out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_set_track(
	timeline: *mut OakOtioTimeline,
	index: i32,
	track: *const OakOtioTrack,
) -> bool {
	if timeline.is_null() || track.is_null() || index < 0 {
		return false;
	}
	let track = lock(&unsafe { &*track }.inner).clone();
	lock(&unsafe { &*timeline }.inner)
		.tracks_mut()
		.set_child(index as usize, Composable::Track(track))
}

/// Appends a copy of `track` to the tracks stack. False on a null
/// handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_append_track(
	timeline: *mut OakOtioTimeline,
	track: *const OakOtioTrack,
) -> bool {
	if timeline.is_null() || track.is_null() {
		return false;
	}
	let track = lock(&unsafe { &*track }.inner).clone();
	lock(&unsafe { &*timeline }.inner)
		.tracks_mut()
		.append_child(Composable::Track(track));
	true
}

/// Removes the track at `index`. False on a null handle or an
/// out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_timeline_remove_track(
	timeline: *mut OakOtioTimeline,
	index: i32,
) -> bool {
	if timeline.is_null() || index < 0 {
		return false;
	}
	lock(&unsafe { &*timeline }.inner)
		.tracks_mut()
		.remove_child(index as usize)
}

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

/// A new track of the given kind ("Video", "Audio", ...), ref count 1.
/// Null on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_track_new(kind: *const u8, kind_len: usize) -> *mut OakOtioTrack {
	let Some(kind) = (unsafe { str_arg(kind, kind_len) }) else {
		return std::ptr::null_mut();
	};
	crate::handle::into_ffi(OakOtioTrack {
		inner: Mutex::new(Track::new(kind)),
	}) as *mut OakOtioTrack
}

/// The track kind ("Video", "Audio", ...), snprintf-style; -1 on a
/// null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_track_kind(
	track: *const OakOtioTrack,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if track.is_null() {
		return -1;
	}
	let kind = lock(&unsafe { &*track }.inner).kind().to_string();
	copy_str_out(&kind, buf, buf_len)
}

/// Number of children (clips/gaps/transitions); -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_track_child_count(track: *const OakOtioTrack) -> i32 {
	if track.is_null() {
		return -1;
	}
	lock(&unsafe { &*track }.inner).children().len() as i32
}

/// Clones the child at `index` out of the track; null on a null
/// handle or an out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_track_child_at(
	track: *const OakOtioTrack,
	index: i32,
) -> *mut OakOtioComposable {
	if track.is_null() || index < 0 {
		return std::ptr::null_mut();
	}
	let track = lock(&unsafe { &*track }.inner);
	let Some(child) = track.children().get(index as usize).map(|c| (**c).clone()) else {
		return std::ptr::null_mut();
	};
	drop(track);
	crate::handle::into_ffi(OakOtioComposable {
		inner: Mutex::new(child),
	}) as *mut OakOtioComposable
}

/// Replaces the child at `index` with a copy of `child` (write-back
/// after editing). False on a null handle or an out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_track_set_child(
	track: *mut OakOtioTrack,
	index: i32,
	child: *const OakOtioComposable,
) -> bool {
	if track.is_null() || child.is_null() || index < 0 {
		return false;
	}
	let child = lock(&unsafe { &*child }.inner).clone();
	lock(&unsafe { &*track }.inner).set_child(index as usize, child)
}

/// Appends a copy of `child` to the track. False on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_track_append_child(
	track: *mut OakOtioTrack,
	child: *const OakOtioComposable,
) -> bool {
	if track.is_null() || child.is_null() {
		return false;
	}
	let child = lock(&unsafe { &*child }.inner).clone();
	lock(&unsafe { &*track }.inner).append_child(child);
	true
}

/// Removes the child at `index`. False on a null handle or an
/// out-of-range index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_track_remove_child(track: *mut OakOtioTrack, index: i32) -> bool {
	if track.is_null() || index < 0 {
		return false;
	}
	lock(&unsafe { &*track }.inner).remove_child(index as usize)
}

// ---------------------------------------------------------------------------
// Composable (clip / gap / transition)
// ---------------------------------------------------------------------------

/// The composable kind: [`OAK_OTIO_COMPOSABLE_CLIP`],
/// [`OAK_OTIO_COMPOSABLE_GAP`], [`OAK_OTIO_COMPOSABLE_TRANSITION`] or
/// [`OAK_OTIO_COMPOSABLE_OTHER`]; -1 on a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_composable_kind(c: *const OakOtioComposable) -> i32 {
	if c.is_null() {
		return -1;
	}
	match &*lock(&unsafe { &*c }.inner) {
		Composable::Clip(_) => OAK_OTIO_COMPOSABLE_CLIP,
		Composable::Gap(_) => OAK_OTIO_COMPOSABLE_GAP,
		Composable::Transition(_) => OAK_OTIO_COMPOSABLE_TRANSITION,
		_ => OAK_OTIO_COMPOSABLE_OTHER,
	}
}

/// The composable name ("" for unknown blocks), snprintf-style; -1 on
/// a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_composable_name(
	c: *const OakOtioComposable,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if c.is_null() {
		return -1;
	}
	let name = lock(&unsafe { &*c }.inner).name().to_string();
	copy_str_out(&name, buf, buf_len)
}

/// Reads the source range of clips and gaps into `out`. False for
/// other kinds, an unset range, or a null handle/output.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_composable_source_range(
	c: *const OakOtioComposable,
	out: *mut OakOtioTimeRange,
) -> bool {
	if c.is_null() || out.is_null() {
		return false;
	}
	let c = lock(&unsafe { &*c }.inner);
	let Some(range) = c.source_range() else {
		return false;
	};
	unsafe { *out = OakOtioTimeRange::from_native(&range) };
	true
}

/// A clip (ref count 1). `url` attaches an external media reference
/// (null/empty → a missing reference); `source_range` is optional
/// (null → unset). Null on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_clip_new(
	name: *const u8,
	name_len: usize,
	url: *const u8,
	url_len: usize,
	source_range: *const OakOtioTimeRange,
) -> *mut OakOtioComposable {
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return std::ptr::null_mut();
	};
	let mut clip = Clip::new(name);
	if let Some(url) = unsafe { str_arg(url, url_len) }.filter(|u| !u.is_empty()) {
		clip.set_media_reference(MediaReference::ExternalReference(ExternalReference::new(
			url, None,
		)));
	}
	if !source_range.is_null() {
		clip.set_source_range(unsafe { &*source_range }.to_native());
	}
	crate::handle::into_ffi(OakOtioComposable {
		inner: Mutex::new(Composable::Clip(clip)),
	}) as *mut OakOtioComposable
}

/// A gap (ref count 1). `source_range` null → an empty range. Null on
/// invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_gap_new(
	name: *const u8,
	name_len: usize,
	source_range: *const OakOtioTimeRange,
) -> *mut OakOtioComposable {
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return std::ptr::null_mut();
	};
	let range = if source_range.is_null() {
		TimeRange::default()
	} else {
		unsafe { &*source_range }.to_native()
	};
	crate::handle::into_ffi(OakOtioComposable {
		inner: Mutex::new(Composable::Gap(Gap::new(range, name))),
	}) as *mut OakOtioComposable
}

/// A transition (ref count 1). Null on invalid input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_transition_new(
	name: *const u8,
	name_len: usize,
	in_offset: OakOtioRationalTime,
	out_offset: OakOtioRationalTime,
) -> *mut OakOtioComposable {
	let Some(name) = (unsafe { str_arg(name, name_len) }) else {
		return std::ptr::null_mut();
	};
	let mut transition = Transition::new(name);
	transition.set_in_offset(in_offset.to_native());
	transition.set_out_offset(out_offset.to_native());
	crate::handle::into_ffi(OakOtioComposable {
		inner: Mutex::new(Composable::Transition(transition)),
	}) as *mut OakOtioComposable
}

/// Sets the source range of a clip. False on a null handle or a
/// non-clip composable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_clip_set_source_range(
	c: *mut OakOtioComposable,
	range: OakOtioTimeRange,
) -> bool {
	if c.is_null() {
		return false;
	}
	match &mut *lock(&unsafe { &*c }.inner) {
		Composable::Clip(clip) => {
			clip.set_source_range(range.to_native());
			true
		}
		_ => false,
	}
}

/// The clip's media URL, snprintf-style; -1 on a null handle, a
/// non-clip composable, or a non-external (e.g. missing) media
/// reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_clip_media_url(
	c: *const OakOtioComposable,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if c.is_null() {
		return -1;
	}
	let c = lock(&unsafe { &*c }.inner);
	let url = match &*c {
		Composable::Clip(clip) => match clip.media_reference() {
			Some(MediaReference::ExternalReference(r)) => Some(r.target_url().to_string()),
			_ => None,
		},
		_ => None,
	};
	match url {
		Some(url) => {
			drop(c);
			copy_str_out(&url, buf, buf_len)
		}
		None => -1,
	}
}

/// Points the clip's media reference at `url` (external reference).
/// False on a null handle, invalid input, or a non-clip composable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_clip_set_media_url(
	c: *mut OakOtioComposable,
	url: *const u8,
	url_len: usize,
) -> bool {
	let Some(url) = (unsafe { str_arg(url, url_len) }) else {
		return false;
	};
	if c.is_null() {
		return false;
	}
	match &mut *lock(&unsafe { &*c }.inner) {
		Composable::Clip(clip) => {
			clip.set_media_reference(MediaReference::ExternalReference(ExternalReference::new(
				url, None,
			)));
			true
		}
		_ => false,
	}
}

/// Whether the clip is enabled: 0/1, -1 on a null handle or a
/// non-clip composable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_clip_enabled(c: *const OakOtioComposable) -> i32 {
	if c.is_null() {
		return -1;
	}
	match &*lock(&unsafe { &*c }.inner) {
		Composable::Clip(clip) => clip.enabled() as i32,
		_ => -1,
	}
}

/// Sets the clip's enabled flag. False on a null handle or a
/// non-clip composable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_clip_set_enabled(
	c: *mut OakOtioComposable,
	enabled: bool,
) -> bool {
	if c.is_null() {
		return false;
	}
	match &mut *lock(&unsafe { &*c }.inner) {
		Composable::Clip(clip) => {
			clip.set_enabled(enabled);
			true
		}
		_ => false,
	}
}

/// The transition type ("SMPTE_Dissolve", ...), snprintf-style; -1 on
/// a null handle or a non-transition composable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_transition_type(
	c: *const OakOtioComposable,
	buf: *mut u8,
	buf_len: usize,
) -> i32 {
	if c.is_null() {
		return -1;
	}
	let c = lock(&unsafe { &*c }.inner);
	match &*c {
		Composable::Transition(t) => {
			let ty = t.transition_type().to_string();
			drop(c);
			copy_str_out(&ty, buf, buf_len)
		}
		_ => -1,
	}
}

/// Reads a transition offset (`is_in` selects in/out) into `out`.
/// False on a null handle/output or a non-transition composable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_transition_offset(
	c: *const OakOtioComposable,
	is_in: bool,
	out: *mut OakOtioRationalTime,
) -> bool {
	if c.is_null() || out.is_null() {
		return false;
	}
	let c = lock(&unsafe { &*c }.inner);
	let Composable::Transition(t) = &*c else {
		return false;
	};
	let offset = if is_in { t.in_offset() } else { t.out_offset() };
	unsafe { *out = OakOtioRationalTime::from_native(&offset) };
	true
}

/// Sets a transition offset (`is_in` selects in/out). False on a null
/// handle or a non-transition composable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oak_otio_transition_set_offset(
	c: *mut OakOtioComposable,
	is_in: bool,
	offset: OakOtioRationalTime,
) -> bool {
	if c.is_null() {
		return false;
	}
	match &mut *lock(&unsafe { &*c }.inner) {
		Composable::Transition(t) => {
			if is_in {
				t.set_in_offset(offset.to_native());
			} else {
				t.set_out_offset(offset.to_native());
			}
			true
		}
		_ => false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn arg(s: &str) -> (*const u8, usize) {
		(s.as_ptr(), s.len())
	}

	/// Builds a one-track timeline (clip + gap) through the C ABI and
	/// reads it back through the document layer.
	#[test]
	fn build_traverse_and_json_roundtrip() {
		unsafe {
			let (n, nl) = arg("Rough Cut");
			let tl = oak_otio_timeline_new(n, nl);
			assert!(!tl.is_null());
			assert!(oak_otio_timeline_set_global_start_time(
				tl,
				OakOtioRationalTime {
					value: 0.0,
					rate: 24.0
				}
			));

			let (k, kl) = arg("Video");
			let track = oak_otio_track_new(k, kl);
			let range = OakOtioTimeRange {
				start_time: OakOtioRationalTime {
					value: 10.0,
					rate: 24.0,
				},
				duration: OakOtioRationalTime {
					value: 48.0,
					rate: 24.0,
				},
			};
			let (cn, cnl) = arg("clip1");
			let (u, ul) = arg("file:///tmp/a.mov");
			let clip = oak_otio_clip_new(cn, cnl, u, ul, &range);
			assert!(!clip.is_null());
			assert_eq!(oak_otio_composable_kind(clip), OAK_OTIO_COMPOSABLE_CLIP);
			assert!(oak_otio_track_append_child(track, clip));
			oak_otio_composable_release(clip);

			let (gn, gnl) = arg("gap");
			let gap = oak_otio_gap_new(gn, gnl, std::ptr::null());
			assert!(oak_otio_track_append_child(track, gap));
			oak_otio_composable_release(gap);

			assert!(oak_otio_timeline_append_track(tl, track));
			oak_otio_track_release(track);

			// Traverse back.
			assert_eq!(oak_otio_timeline_track_count(tl), 1);
			let track = oak_otio_timeline_track_at(tl, 0);
			assert!(!track.is_null());
			assert_eq!(oak_otio_track_child_count(track), 2);
			let child = oak_otio_track_child_at(track, 0);
			assert_eq!(oak_otio_composable_kind(child), OAK_OTIO_COMPOSABLE_CLIP);
			let mut buf = [0u8; 64];
			let n = oak_otio_clip_media_url(child, buf.as_mut_ptr(), buf.len());
			assert_eq!(
				std::str::from_utf8(&buf[..n as usize]).unwrap(),
				"file:///tmp/a.mov"
			);
			let mut got = OakOtioTimeRange::default();
			assert!(oak_otio_composable_source_range(child, &mut got));
			assert_eq!(got.duration.value, 48.0);
			oak_otio_composable_release(child);
			oak_otio_track_release(track);

			// JSON roundtrip via the document layer.
			let doc = oak_otio_document_from_timeline(tl);
			let len = oak_otio_document_to_json(doc, std::ptr::null_mut(), 0);
			assert!(len > 0);
			let mut buf = vec![0u8; len as usize + 1];
			assert_eq!(
				oak_otio_document_to_json(doc, buf.as_mut_ptr(), buf.len()),
				len
			);
			buf.truncate(len as usize);
			oak_otio_document_release(doc);
			oak_otio_timeline_release(tl);

			let doc2 = oak_otio_document_from_json(buf.as_ptr(), buf.len());
			assert!(!doc2.is_null());
			assert_eq!(oak_otio_document_kind(doc2), OAK_OTIO_DOC_TIMELINE);
			let tl2 = oak_otio_document_timeline(doc2);
			assert!(!tl2.is_null());
			let n = oak_otio_timeline_name(tl2, buf.as_mut_ptr(), buf.len());
			assert_eq!(
				std::str::from_utf8(&buf[..n as usize]).unwrap(),
				"Rough Cut"
			);
			oak_otio_timeline_release(tl2);
			oak_otio_document_release(doc2);
		}
	}

	/// FCPXML: build → serialize → parse → the structure survives.
	#[test]
	fn fcpxml_roundtrip() {
		unsafe {
			let (n, nl) = arg("FCP Sequence");
			let tl = oak_otio_timeline_new(n, nl);
			let (k, kl) = arg("Video");
			let track = oak_otio_track_new(k, kl);
			let range = OakOtioTimeRange {
				start_time: OakOtioRationalTime {
					value: 0.0,
					rate: 25.0,
				},
				duration: OakOtioRationalTime {
					value: 50.0,
					rate: 25.0,
				},
			};
			let (cn, cnl) = arg("shot");
			let (u, ul) = arg("file:///tmp/shot.mov");
			let clip = oak_otio_clip_new(cn, cnl, u, ul, &range);
			assert!(oak_otio_track_append_child(track, clip));
			assert!(oak_otio_timeline_append_track(tl, track));

			let list = oak_otio_timelines_new();
			assert!(oak_otio_timelines_add(list, tl));
			let len = oak_otio_timelines_to_fcpxml(list, std::ptr::null_mut(), 0);
			assert!(len > 0);
			let mut xml = vec![0u8; len as usize + 1];
			assert_eq!(
				oak_otio_timelines_to_fcpxml(list, xml.as_mut_ptr(), xml.len()),
				len
			);
			xml.truncate(len as usize);

			let back = oak_otio_timelines_from_fcpxml(xml.as_ptr(), xml.len());
			assert!(!back.is_null());
			assert_eq!(oak_otio_timelines_count(back), 1);
			let tl2 = oak_otio_timelines_at(back, 0);
			let mut buf = [0u8; 64];
			let n = oak_otio_timeline_name(tl2, buf.as_mut_ptr(), buf.len());
			assert_eq!(
				std::str::from_utf8(&buf[..n as usize]).unwrap(),
				"FCP Sequence"
			);
			assert_eq!(oak_otio_timeline_track_count(tl2), 1);

			oak_otio_timeline_release(tl2);
			oak_otio_timelines_release(back);
			oak_otio_composable_release(clip);
			oak_otio_track_release(track);
			oak_otio_timeline_release(tl);
			oak_otio_timelines_release(list);
		}
	}

	#[test]
	fn null_and_invalid_input_fail_closed() {
		unsafe {
			assert!(oak_otio_document_from_json(std::ptr::null(), 0).is_null());
			assert!(oak_otio_document_from_json("{".as_ptr(), 1).is_null());
			assert!(oak_otio_timelines_from_fcpxml("nope".as_ptr(), 4).is_null());
			assert_eq!(oak_otio_document_kind(std::ptr::null()), -1);
			assert_eq!(oak_otio_timeline_track_count(std::ptr::null()), -1);
			assert!(oak_otio_document_timeline(std::ptr::null()).is_null());
			assert!(
				oak_otio_clip_new(std::ptr::null(), 0, std::ptr::null(), 0, std::ptr::null())
					.is_null()
			);
			// Unknown kind: media URL is only for clips.
			let (n, nl) = arg("t");
			let tr = oak_otio_transition_new(
				n,
				nl,
				OakOtioRationalTime::default(),
				OakOtioRationalTime::default(),
			);
			assert_eq!(oak_otio_clip_media_url(tr, std::ptr::null_mut(), 0), -1);
			assert!(!oak_otio_clip_set_enabled(tr, false));
			oak_otio_composable_release(tr);
		}
	}
}
