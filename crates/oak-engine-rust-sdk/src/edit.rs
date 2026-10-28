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
	AddTrack {
		kind: TrackKind,
	},
	RemoveTrack {
		track: NodeId,
	},
	SetTrackHeight {
		track: NodeId,
		height: f64,
	},
	SetTrackMuted {
		track: NodeId,
		muted: bool,
	},
	SetTrackLocked {
		track: NodeId,
		locked: bool,
	},
	// Project and sequence structure. Custom sequence format options must be
	// either all `Some` or all `None`; partial formats are rejected.
	CreateSequence {
		name: String,
		width: Option<i32>,
		height: Option<i32>,
		rate: Option<Rational>,
		interlaced: Option<bool>,
	},
	SetSequenceParameters {
		sequence: NodeId,
		name: String,
		width: i32,
		height: i32,
		rate: Rational,
		interlaced: bool,
	},
	SetSequencePlayhead {
		sequence: NodeId,
		time: Rational,
	},
	CreateFolder {
		name: String,
	},
	// Project-bin and media operations.
	CreateTextFootage,
	ImportFootage {
		path: String,
	},
	EnsureSequencesMounted,
	ReprobeUnprobedFootage,
	// Clip creation and placement on the current sequence.
	CreateAdjustmentLayer {
		track: NodeId,
		in_ts: i64,
		out_ts: i64,
	},
	PlaceFootageClip {
		footage: NodeId,
		kind: TrackKind,
		track_index: usize,
		in_ts: i64,
		out_ts: i64,
		media_in_ts: i64,
	},
	PlaceNestedSequenceClip {
		source: NodeId,
		track_index: usize,
		in_ts: i64,
		out_ts: i64,
	},
	PlaceTextClip {
		text: NodeId,
		track_index: usize,
		in_ts: i64,
		out_ts: i64,
	},
	PlaceGeneratorClip {
		generator: NodeId,
		track_index: usize,
		in_ts: i64,
		out_ts: i64,
	},
	PlaceFootageClipsLinked {
		footage: NodeId,
		placements: Vec<(TrackKind, usize)>,
		in_ts: i64,
		out_ts: i64,
		media_in_ts: i64,
	},
	// Transitions. `half` is the duration extending to each side of a seam.
	AddTransitionAtSeam {
		previous: NodeId,
		next: NodeId,
		half: Rational,
	},
	AddTransitionAtEdge {
		clip: NodeId,
		start_edge: bool,
		length: Rational,
	},
	AddDefaultTransition {
		clips: Vec<NodeId>,
		half: Rational,
	},
	SetTransitionLength {
		clip: NodeId,
		start_edge: bool,
		frame: Rational,
		length: Rational,
	},
	// Timeline editing. Ripple commands also move later clips where applicable.
	SplitClip {
		clip: NodeId,
		time_ts: i64,
	},
	SplitClipsPreservingLinks {
		clips: Vec<NodeId>,
		time_ts: i64,
	},
	TrimClip {
		clip: NodeId,
		new_in_ts: i64,
		new_out_ts: i64,
	},
	RippleTrimClip {
		clip: NodeId,
		start_edge: bool,
		new_frame: i64,
	},
	RollEdit {
		track: NodeId,
		a: NodeId,
		b: NodeId,
		new_frame: i64,
	},
	SlideClip {
		clip: NodeId,
		new_start: i64,
	},
	SlipClip {
		clip: NodeId,
		new_media_in: i64,
	},
	MoveClip {
		clip: NodeId,
		new_in_ts: i64,
	},
	MoveClipToTrack {
		clip: NodeId,
		track: NodeId,
		new_in_ts: i64,
	},
	MoveClipWithLinks {
		clip: NodeId,
		track: Option<NodeId>,
		new_in_ts: i64,
		linked: Vec<NodeId>,
	},
	SetClipsLinked {
		clips: Vec<NodeId>,
		linked: bool,
	},
	DeleteClip {
		clip: NodeId,
	},
	RippleDeleteClip {
		clip: NodeId,
	},
	DeleteArea {
		in_ts: i64,
		out_ts: i64,
		ripple: bool,
	},
	RollClipEdgeTo {
		clip: NodeId,
		at_in: bool,
		frame_ts: i64,
	},
	// Sequence-local markers and work area.
	MarkerAdd {
		time: Rational,
		name: String,
		color: i32,
	},
	MarkerRemove {
		time: Rational,
	},
	WorkareaSet {
		enabled: bool,
		range: TimeRange,
	},
	WorkareaSetUndoable {
		enabled: bool,
		range: TimeRange,
		old_range: TimeRange,
	},
	// Generic node-graph edits used by the node editor.
	RemoveNode {
		node: NodeId,
	},
	Connect {
		from: NodeId,
		to: NodeId,
		input_id: String,
	},
	Disconnect {
		to: NodeId,
		input_id: String,
	},
	SetContextPosition {
		node: NodeId,
		context: NodeId,
		x: f64,
		y: f64,
	},
	// Clipboard and composite clip operations.
	CopyClips {
		clips: Vec<NodeId>,
	},
	PasteClips {
		playhead_ts: i64,
	},
	SetBlocksEnabled {
		clips: Vec<NodeId>,
		enabled: bool,
	},
	SetClipsSpeed {
		clips: Vec<NodeId>,
		speed: f64,
	},
	RenameBlocks {
		clips: Vec<NodeId>,
		name: String,
	},
	DuplicateClips {
		clips: Vec<NodeId>,
	},
	PasteClipsInsert {
		playhead_ts: i64,
	},
	NestClips {
		clips: Vec<NodeId>,
	},
	// Effect-chain and static parameter operations. Insert/move indices use
	// closest-to-source-first chain order and clamp to the valid end.
	EffectInsert {
		host: NodeId,
		index: usize,
		type_id: String,
	},
	EffectRemove {
		host: NodeId,
		effect: NodeId,
	},
	EffectMove {
		host: NodeId,
		effect: NodeId,
		new_index: usize,
	},
	EffectSetEnabled {
		effect: NodeId,
		enabled: bool,
	},
	/// Sets one static POD input value; `element = -1` addresses a scalar input.
	SetNodeStandardValue {
		node: NodeId,
		input_id: String,
		element: i32,
		value: NodeValue,
	},
	/// Inserts or replaces a complete keyframe and its interpolation handles.
	UpsertNodeKeyframe {
		node: NodeId,
		input_id: String,
		element: i32,
		time: Rational,
		value: NodeValue,
		interpolation: KeyframeInterpolation,
		bezier_in: (f64, f64),
		bezier_out: (f64, f64),
	},
	/// Removes the keyframe at an exact rational time.
	RemoveNodeKeyframe {
		node: NodeId,
		input_id: String,
		element: i32,
		time: Rational,
	},
	/// Applies oak-node's standard/keyframed value-at-time mutation rules.
	SetNodeValueAtTime {
		node: NodeId,
		input_id: String,
		element: i32,
		time: Rational,
		value: NodeValue,
	},
	/// Triggers an OFX push-button action. This is not an undoable value edit.
	PushNodeButton {
		node: NodeId,
		input_id: String,
	},
	/// Sets a Text/StrCombo/Parametric static value without a POD payload.
	SetNodeTextValue {
		node: NodeId,
		input_id: String,
		element: i32,
		value: String,
	},
	/// Replaces and reprobes a footage node's media without changing its ID.
	ReplaceFootage {
		footage: NodeId,
		path: String,
	},
	/// Deletes a project-bin entry and its folder mount, restoring both on undo.
	DeleteEntry {
		node: NodeId,
	},
	/// Replaces a footage node's custom proxy generation parameters.
	SetFootageProxyParams {
		footage: NodeId,
		params: crate::codec::ProxyParams,
	},
	/// Clears the per-footage override and restores global proxy parameters.
	ClearFootageProxyParams {
		footage: NodeId,
	},
	/// Creates a sequence using the footage's first video-stream format.
	CreateSequenceFromFootage {
		footage: NodeId,
		name: String,
	},
	/// Deletes proxy files and clears their footage association without adding
	/// an undo entry, because file contents cannot be restored by undo.
	DeleteProxy {
		footage: NodeId,
	},
	/// Changes the per-footage proxy playback switch without starting a worker.
	SetProxyEnabled {
		footage: NodeId,
		enabled: bool,
	},
	/// Enables or disables multicam on sequence-connected clips as one undo entry.
	SetMulticamEnabled {
		clips: Vec<NodeId>,
		enabled: bool,
	},
	/// Switches video and linked AFV audio, optionally splitting at the playhead.
	SwitchMulticam {
		clip: NodeId,
		source: i32,
		split_clip: bool,
		playhead: crate::types::Rational,
	},
	/// Builds a multicam source sequence and rewires a host clip on another sequence.
	CreateMulticamSequence {
		host_sequence: NodeId,
		angles: Vec<NodeId>,
		offsets: Vec<f64>,
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
	pub track_index: usize,
	pub media_in_ts: i64,
	/// Original timeline start, used to preserve relative placement.
	pub start_ts: i64,
	pub length_ts: i64,
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
