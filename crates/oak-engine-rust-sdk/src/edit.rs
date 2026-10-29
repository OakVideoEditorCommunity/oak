//! Strongly typed semantic editing over the native engine ABI.

mod abi;
mod execute;

#[cfg(test)]
mod tests;

use crate::editor::{KeyframeInterpolation, TrackKind};
use crate::node::{NodeId, NodeValue};
use crate::types::{Rational, TimeRange};

/// One semantic mutation accepted by [`EditorSession::execute`](crate::editor::EditorSession::execute).
///
/// Commands without an explicit sequence act on the session's current sequence.
/// Fields ending in `_ts`, plus `new_start`, `new_media_in` and `new_frame`, are
/// integer frame timestamps in that sequence's timebase. Fields using [`Rational`]
/// are absolute times or durations and do not use that integer frame convention.
/// Each call executes the command once; the returned revision can be used to
/// detect a subsequent undo, redo or edit.
#[derive(Clone, Debug)]
pub enum EditCommand {
	// Track structure and state.
	/// Adds a track of the given kind to the current sequence.
	AddTrack {
		/// The `kind` field.
		kind: TrackKind,
	},
	/// Removes a track and its list entry.
	RemoveTrack {
		/// The `track` field.
		track: NodeId,
	},
	/// Sets a track's display height.
	SetTrackHeight {
		/// The `track` field.
		track: NodeId,
		/// The `height` field.
		height: f64,
	},
	/// Mutes an audio track or hides a video/subtitle track.
	SetTrackMuted {
		/// The `track` field.
		track: NodeId,
		/// The `muted` field.
		muted: bool,
	},
	/// Locks a track against clip edits.
	SetTrackLocked {
		/// The `track` field.
		track: NodeId,
		/// The `locked` field.
		locked: bool,
	},
	// Project and sequence structure. Custom sequence format options must be
	// either all `Some` or all `None`; partial formats are rejected.
	/// Creates a sequence and makes it current.
	CreateSequence {
		/// The `name` field.
		name: String,
		/// The `width` field.
		width: Option<i32>,
		/// The `height` field.
		height: Option<i32>,
		/// The `rate` field.
		rate: Option<Rational>,
		/// The `interlaced` field.
		interlaced: Option<bool>,
	},
	/// Renames a sequence and sets its video format.
	SetSequenceParameters {
		/// The `sequence` field.
		sequence: NodeId,
		/// The `name` field.
		name: String,
		/// The `width` field.
		width: i32,
		/// The `height` field.
		height: i32,
		/// The `rate` field.
		rate: Rational,
		/// The `interlaced` field.
		interlaced: bool,
	},
	/// Moves the sequence playhead.
	SetSequencePlayhead {
		/// The `sequence` field.
		sequence: NodeId,
		/// The `time` field.
		time: Rational,
	},
	/// Creates a bin folder under the project root.
	CreateFolder {
		/// The `name` field.
		name: String,
	},
	// Project-bin and media operations.
	/// Creates a text generator entry in the project root.
	CreateTextFootage,
	/// Imports a media file into the project root.
	ImportFootage {
		/// The `path` field.
		path: String,
	},
	/// Mounts orphaned sequences under the project root.
	EnsureSequencesMounted,
	/// Reprobes footage that lacks stream metadata.
	ReprobeUnprobedFootage,
	// Clip creation and placement on the current sequence.
	/// Creates an adjustment layer on a video track.
	CreateAdjustmentLayer {
		/// The `track` field.
		track: NodeId,
		/// The `in_ts` field.
		in_ts: i64,
		/// The `out_ts` field.
		out_ts: i64,
	},
	/// Places a footage clip on a track.
	PlaceFootageClip {
		/// The `footage` field.
		footage: NodeId,
		/// The `kind` field.
		kind: TrackKind,
		/// The `track_index` field.
		track_index: usize,
		/// The `in_ts` field.
		in_ts: i64,
		/// The `out_ts` field.
		out_ts: i64,
		/// The `media_in_ts` field.
		media_in_ts: i64,
	},
	/// Places a nested-sequence clip on the host timeline.
	PlaceNestedSequenceClip {
		/// The `source` field.
		source: NodeId,
		/// The `track_index` field.
		track_index: usize,
		/// The `in_ts` field.
		in_ts: i64,
		/// The `out_ts` field.
		out_ts: i64,
	},
	/// Places a text generator clip.
	PlaceTextClip {
		/// The `text` field.
		text: NodeId,
		/// The `track_index` field.
		track_index: usize,
		/// The `in_ts` field.
		in_ts: i64,
		/// The `out_ts` field.
		out_ts: i64,
	},
	/// Places a generator clip.
	PlaceGeneratorClip {
		/// The `generator` field.
		generator: NodeId,
		/// The `track_index` field.
		track_index: usize,
		/// The `in_ts` field.
		in_ts: i64,
		/// The `out_ts` field.
		out_ts: i64,
	},
	/// Places linked video/audio clips for one footage.
	PlaceFootageClipsLinked {
		/// The `footage` field.
		footage: NodeId,
		/// The `placements` field.
		placements: Vec<(TrackKind, usize)>,
		/// The `in_ts` field.
		in_ts: i64,
		/// The `out_ts` field.
		out_ts: i64,
		/// The `media_in_ts` field.
		media_in_ts: i64,
	},
	// Transitions. `half` is the duration extending to each side of a seam.
	/// Adds a transition at a clip seam.
	AddTransitionAtSeam {
		/// The `previous` field.
		previous: NodeId,
		/// The `next` field.
		next: NodeId,
		/// The `half` field.
		half: Rational,
	},
	/// Adds a single-sided transition at a clip edge.
	AddTransitionAtEdge {
		/// The `clip` field.
		clip: NodeId,
		/// The `start_edge` field.
		start_edge: bool,
		/// The `length` field.
		length: Rational,
	},
	/// Adds default transitions around the selected clips.
	AddDefaultTransition {
		/// The `clips` field.
		clips: Vec<NodeId>,
		/// The `half` field.
		half: Rational,
	},
	/// Resizes one transition wedge.
	SetTransitionLength {
		/// The `clip` field.
		clip: NodeId,
		/// The `start_edge` field.
		start_edge: bool,
		/// The `frame` field.
		frame: Rational,
		/// The `length` field.
		length: Rational,
	},
	// Timeline editing. Ripple commands also move later clips where applicable.
	/// Splits a clip at a frame timestamp.
	SplitClip {
		/// The `clip` field.
		clip: NodeId,
		/// The `time_ts` field.
		time_ts: i64,
	},
	/// Splits clips while preserving A/V links.
	SplitClipsPreservingLinks {
		/// The `clips` field.
		clips: Vec<NodeId>,
		/// The `time_ts` field.
		time_ts: i64,
	},
	/// Trims a clip's timeline range.
	TrimClip {
		/// The `clip` field.
		clip: NodeId,
		/// The `new_in_ts` field.
		new_in_ts: i64,
		/// The `new_out_ts` field.
		new_out_ts: i64,
	},
	/// Ripple-trims a clip edge.
	RippleTrimClip {
		/// The `clip` field.
		clip: NodeId,
		/// The `start_edge` field.
		start_edge: bool,
		/// The `new_frame` field.
		new_frame: i64,
	},
	/// Rolls the shared boundary of adjacent clips.
	RollEdit {
		/// The `track` field.
		track: NodeId,
		/// The `a` field.
		a: NodeId,
		/// The `b` field.
		b: NodeId,
		/// The `new_frame` field.
		new_frame: i64,
	},
	/// Slides a clip while keeping its media window.
	SlideClip {
		/// The `clip` field.
		clip: NodeId,
		/// The `new_start` field.
		new_start: i64,
	},
	/// Slips a clip's media window.
	SlipClip {
		/// The `clip` field.
		clip: NodeId,
		/// The `new_media_in` field.
		new_media_in: i64,
	},
	/// Moves a clip within its track.
	MoveClip {
		/// The `clip` field.
		clip: NodeId,
		/// The `new_in_ts` field.
		new_in_ts: i64,
	},
	/// Moves a clip to another track.
	MoveClipToTrack {
		/// The `clip` field.
		clip: NodeId,
		/// The `track` field.
		track: NodeId,
		/// The `new_in_ts` field.
		new_in_ts: i64,
	},
	/// Moves a clip and its linked partners.
	MoveClipWithLinks {
		/// The `clip` field.
		clip: NodeId,
		/// The `track` field.
		track: Option<NodeId>,
		/// The `new_in_ts` field.
		new_in_ts: i64,
		/// The `linked` field.
		linked: Vec<NodeId>,
	},
	/// Links or unlinks a set of clips.
	SetClipsLinked {
		/// The `clips` field.
		clips: Vec<NodeId>,
		/// The `linked` field.
		linked: bool,
	},
	/// Deletes a clip leaving a gap.
	DeleteClip {
		/// The `clip` field.
		clip: NodeId,
	},
	/// Deletes a clip and closes the gap.
	RippleDeleteClip {
		/// The `clip` field.
		clip: NodeId,
	},
	/// Deletes a timeline range across tracks.
	DeleteArea {
		/// The `in_ts` field.
		in_ts: i64,
		/// The `out_ts` field.
		out_ts: i64,
		/// The `ripple` field.
		ripple: bool,
	},
	/// Rolls a clip edge to a frame.
	RollClipEdgeTo {
		/// The `clip` field.
		clip: NodeId,
		/// The `at_in` field.
		at_in: bool,
		/// The `frame_ts` field.
		frame_ts: i64,
	},
	// Sequence-local markers and work area.
	/// Adds a marker at a time.
	MarkerAdd {
		/// The `time` field.
		time: Rational,
		/// The `name` field.
		name: String,
		/// The `color` field.
		color: i32,
	},
	/// Removes the marker at a time.
	MarkerRemove {
		/// The `time` field.
		time: Rational,
	},
	/// Sets the work area without pushing an undo entry.
	WorkareaSet {
		/// The `enabled` field.
		enabled: bool,
		/// The `range` field.
		range: TimeRange,
	},
	/// Sets the work area as one undo entry.
	WorkareaSetUndoable {
		/// The `enabled` field.
		enabled: bool,
		/// The `range` field.
		range: TimeRange,
		/// The `old_range` field.
		old_range: TimeRange,
	},
	// Generic node-graph edits used by the node editor.
	/// Removes a node from the project graph.
	RemoveNode {
		/// The `node` field.
		node: NodeId,
	},
	/// Connects a node output to an input.
	Connect {
		/// The `from` field.
		from: NodeId,
		/// The `to` field.
		to: NodeId,
		/// The `input_id` field.
		input_id: String,
	},
	/// Disconnects a node input.
	Disconnect {
		/// The `to` field.
		to: NodeId,
		/// The `input_id` field.
		input_id: String,
	},
	/// Stores a node's position in the graph context.
	SetContextPosition {
		/// The `node` field.
		node: NodeId,
		/// The `context` field.
		context: NodeId,
		/// The `x` field.
		x: f64,
		/// The `y` field.
		y: f64,
	},
	// Clipboard and composite clip operations.
	/// Captures clips into the session clipboard.
	CopyClips {
		/// The `clips` field.
		clips: Vec<NodeId>,
	},
	/// Pastes the clipboard at the playhead.
	PasteClips {
		/// The `playhead_ts` field.
		playhead_ts: i64,
	},
	/// Enables or disables timeline blocks.
	SetBlocksEnabled {
		/// The `clips` field.
		clips: Vec<NodeId>,
		/// The `enabled` field.
		enabled: bool,
	},
	/// Sets clip playback speed.
	SetClipsSpeed {
		/// The `clips` field.
		clips: Vec<NodeId>,
		/// The `speed` field.
		speed: f64,
	},
	/// Renames timeline blocks.
	RenameBlocks {
		/// The `clips` field.
		clips: Vec<NodeId>,
		/// The `name` field.
		name: String,
	},
	/// Duplicates clips.
	DuplicateClips {
		/// The `clips` field.
		clips: Vec<NodeId>,
	},
	/// Pastes with a ripple insert.
	PasteClipsInsert {
		/// The `playhead_ts` field.
		playhead_ts: i64,
	},
	/// Nests clips into a new sequence.
	NestClips {
		/// The `clips` field.
		clips: Vec<NodeId>,
	},
	// Effect-chain and static parameter operations. Insert/move indices use
	// closest-to-source-first chain order and clamp to the valid end.
	/// Inserts an effect into a clip's chain.
	EffectInsert {
		/// The `host` field.
		host: NodeId,
		/// The `index` field.
		index: usize,
		/// The `type_id` field.
		type_id: String,
	},
	/// Removes an effect from a chain.
	EffectRemove {
		/// The `host` field.
		host: NodeId,
		/// The `effect` field.
		effect: NodeId,
	},
	/// Reorders an effect in a chain.
	EffectMove {
		/// The `host` field.
		host: NodeId,
		/// The `effect` field.
		effect: NodeId,
		/// The `new_index` field.
		new_index: usize,
	},
	/// Enables or disables an effect.
	EffectSetEnabled {
		/// The `effect` field.
		effect: NodeId,
		/// The `enabled` field.
		enabled: bool,
	},
	/// Sets one static POD input value; `element = -1` addresses a scalar input.
	SetNodeStandardValue {
		/// The `node` field.
		node: NodeId,
		/// The `input_id` field.
		input_id: String,
		/// The `element` field.
		element: i32,
		/// The `value` field.
		value: NodeValue,
	},
	/// Inserts or replaces a complete keyframe and its interpolation handles.
	UpsertNodeKeyframe {
		/// The `node` field.
		node: NodeId,
		/// The `input_id` field.
		input_id: String,
		/// The `element` field.
		element: i32,
		/// The `time` field.
		time: Rational,
		/// The `value` field.
		value: NodeValue,
		/// The `interpolation` field.
		interpolation: KeyframeInterpolation,
		/// The `bezier_in` field.
		bezier_in: (f64, f64),
		/// The `bezier_out` field.
		bezier_out: (f64, f64),
	},
	/// Removes the keyframe at an exact rational time.
	RemoveNodeKeyframe {
		/// The `node` field.
		node: NodeId,
		/// The `input_id` field.
		input_id: String,
		/// The `element` field.
		element: i32,
		/// The `time` field.
		time: Rational,
	},
	/// Applies oak-node's standard/keyframed value-at-time mutation rules.
	SetNodeValueAtTime {
		/// The `node` field.
		node: NodeId,
		/// The `input_id` field.
		input_id: String,
		/// The `element` field.
		element: i32,
		/// The `time` field.
		time: Rational,
		/// The `value` field.
		value: NodeValue,
	},
	/// Triggers an OFX push-button action. This is not an undoable value edit.
	PushNodeButton {
		/// The `node` field.
		node: NodeId,
		/// The `input_id` field.
		input_id: String,
	},
	/// Sets a Text/StrCombo/Parametric static value without a POD payload.
	SetNodeTextValue {
		/// The `node` field.
		node: NodeId,
		/// The `input_id` field.
		input_id: String,
		/// The `element` field.
		element: i32,
		/// The `value` field.
		value: String,
	},
	/// Replaces and reprobes a footage node's media without changing its ID.
	ReplaceFootage {
		/// The `footage` field.
		footage: NodeId,
		/// The `path` field.
		path: String,
	},
	/// Deletes a project-bin entry and its folder mount, restoring both on undo.
	DeleteEntry {
		/// The `node` field.
		node: NodeId,
	},
	/// Replaces a footage node's custom proxy generation parameters.
	SetFootageProxyParams {
		/// The `footage` field.
		footage: NodeId,
		/// The `params` field.
		params: crate::codec::ProxyParams,
	},
	/// Clears the per-footage override and restores global proxy parameters.
	ClearFootageProxyParams {
		/// The `footage` field.
		footage: NodeId,
	},
	/// Creates a sequence using the footage's first video-stream format.
	CreateSequenceFromFootage {
		/// The `footage` field.
		footage: NodeId,
		/// The `name` field.
		name: String,
	},
	/// Deletes proxy files and clears their footage association without adding
	/// an undo entry, because file contents cannot be restored by undo.
	DeleteProxy {
		/// The `footage` field.
		footage: NodeId,
	},
	/// Changes the per-footage proxy playback switch without starting a worker.
	SetProxyEnabled {
		/// The `footage` field.
		footage: NodeId,
		/// The `enabled` field.
		enabled: bool,
	},
	/// Enables or disables multicam on sequence-connected clips as one undo entry.
	SetMulticamEnabled {
		/// The `clips` field.
		clips: Vec<NodeId>,
		/// The `enabled` field.
		enabled: bool,
	},
	/// Switches video and linked AFV audio, optionally splitting at the playhead.
	SwitchMulticam {
		/// The `clip` field.
		clip: NodeId,
		/// The `source` field.
		source: i32,
		/// The `split_clip` field.
		split_clip: bool,
		/// The `playhead` field.
		playhead: crate::types::Rational,
	},
	/// Builds a multicam source sequence and rewires a host clip on another sequence.
	CreateMulticamSequence {
		/// The `host_sequence` field.
		host_sequence: NodeId,
		/// The `angles` field.
		angles: Vec<NodeId>,
		/// The `offsets` field.
		offsets: Vec<f64>,
		/// The `name` field.
		name: String,
	},
}

/// A clip captured by the session clipboard.
///
/// All timestamp fields retain the source sequence's integer frame timebase.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardClip {
	/// Source footage, nested sequence or generator node.
	pub footage: NodeId,
	/// Track list on which this clip should be pasted.
	pub kind: TrackKind,
	/// The `track_index` field.
	pub track_index: usize,
	/// The `media_in_ts` field.
	pub media_in_ts: i64,
	/// Original timeline start, used to preserve relative placement.
	pub start_ts: i64,
	/// The `length_ts` field.
	pub length_ts: i64,
	/// The `speed` field.
	pub speed: f64,
}

/// Owned output from one semantic edit.
///
/// A command populates only the fields relevant to its result; for example,
/// creating one clip sets `primary`, paste sets `ids`, and copy sets `clipboard`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditResult {
	/// Main created or affected node, if the command has one.
	pub primary: Option<NodeId>,
	/// Ordered IDs for commands that create or affect several nodes.
	pub ids: Vec<NodeId>,
	/// Command-specific integer output, such as a track index or result count.
	pub value: i64,
	/// Command-specific rational output, such as a clamped transition length.
	pub rational: Option<Rational>,
	/// Global undo revision observed after the command completed.
	pub revision: i64,
	/// Owned clipboard contents returned by copy operations.
	pub clipboard: Vec<ClipboardClip>,
}
