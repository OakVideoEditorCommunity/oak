//! Owned, engine-side representation of a semantic edit.
//!
//! Unlike the C ABI parameter union, this enum gives every operation named,
//! typed fields and owns its strings and vectors. Raw node identities are
//! validated during dispatch, immediately before the domain operation runs.
//! Integer fields ending in `_ts`, and the `new_frame`/`new_start` fields,
//! use the current sequence's frame timebase; wire rationals do not.

use super::{WireRational, WireTimeRange};

/// A decoded ABI command. This is intentionally separate from the SDK's
/// public `EditCommand`, which uses safe `NodeId` and native rational types.
#[derive(Clone, Debug)]
pub enum EditCommand {
	// Tracks.
	AddTrack {
		kind: i32,
	},
	RemoveTrack {
		track: u64,
	},
	SetTrackHeight {
		track: u64,
		height: f64,
	},
	SetTrackMuted {
		track: u64,
		muted: bool,
	},
	SetTrackLocked {
		track: u64,
		locked: bool,
	},
	// Project, sequence and media-bin structure.
	CreateSequence {
		name: String,
		width: Option<i32>,
		height: Option<i32>,
		rate: Option<WireRational>,
		interlaced: Option<bool>,
	},
	SetSequenceParameters {
		sequence: u64,
		name: String,
		width: i32,
		height: i32,
		rate: WireRational,
		interlaced: bool,
	},
	SetSequencePlayhead {
		sequence: u64,
		time: WireRational,
	},
	CreateFolder {
		name: String,
	},
	CreateTextFootage,
	ImportFootage {
		path: String,
	},
	EnsureSequencesMounted,
	ReprobeUnprobedFootage,
	// Clip placement and generator-based blocks.
	CreateAdjustmentLayer {
		track: u64,
		in_ts: i64,
		out_ts: i64,
	},
	PlaceFootageClip {
		footage: u64,
		kind: i32,
		track_index: usize,
		in_ts: i64,
		out_ts: i64,
		media_in_ts: i64,
	},
	PlaceNestedSequenceClip {
		source: u64,
		track_index: usize,
		in_ts: i64,
		out_ts: i64,
	},
	PlaceTextClip {
		text: u64,
		track_index: usize,
		in_ts: i64,
		out_ts: i64,
	},
	PlaceGeneratorClip {
		generator: u64,
		track_index: usize,
		in_ts: i64,
		out_ts: i64,
	},
	PlaceFootageClipsLinked {
		footage: u64,
		placements: Vec<(i32, usize)>,
		in_ts: i64,
		out_ts: i64,
		media_in_ts: i64,
	},
	// Transitions.
	AddTransitionAtSeam {
		previous: u64,
		next: u64,
		half: WireRational,
	},
	AddTransitionAtEdge {
		clip: u64,
		start_edge: bool,
		length: WireRational,
	},
	AddDefaultTransition {
		clips: Vec<u64>,
		half: WireRational,
	},
	SetTransitionLength {
		clip: u64,
		start_edge: bool,
		frame: WireRational,
		length: WireRational,
	},
	// Clip trimming, movement and deletion.
	SplitClip {
		clip: u64,
		time_ts: i64,
	},
	SplitClipsPreservingLinks {
		clips: Vec<u64>,
		time_ts: i64,
	},
	TrimClip {
		clip: u64,
		new_in_ts: i64,
		new_out_ts: i64,
	},
	RippleTrimClip {
		clip: u64,
		start_edge: bool,
		new_frame: i64,
	},
	RollEdit {
		track: u64,
		a: u64,
		b: u64,
		new_frame: i64,
	},
	SlideClip {
		clip: u64,
		new_start: i64,
	},
	SlipClip {
		clip: u64,
		new_media_in: i64,
	},
	MoveClip {
		clip: u64,
		new_in_ts: i64,
	},
	MoveClipToTrack {
		clip: u64,
		track: u64,
		new_in_ts: i64,
	},
	MoveClipWithLinks {
		clip: u64,
		track: Option<u64>,
		new_in_ts: i64,
		linked: Vec<u64>,
	},
	SetClipsLinked {
		clips: Vec<u64>,
		linked: bool,
	},
	DeleteClip {
		clip: u64,
	},
	RippleDeleteClip {
		clip: u64,
	},
	DeleteArea {
		in_ts: i64,
		out_ts: i64,
		ripple: bool,
	},
	RollClipEdgeTo {
		clip: u64,
		at_in: bool,
		frame_ts: i64,
	},
	// Sequence-local auxiliary state.
	MarkerAdd {
		time: WireRational,
		name: String,
		color: i32,
	},
	MarkerRemove {
		time: WireRational,
	},
	WorkareaSet {
		enabled: bool,
		range: WireTimeRange,
	},
	WorkareaSetUndoable {
		enabled: bool,
		range: WireTimeRange,
		old_range: WireTimeRange,
	},
	// Generic graph operations.
	RemoveNode {
		node: u64,
	},
	Connect {
		from: u64,
		to: u64,
		input_id: String,
	},
	Disconnect {
		to: u64,
		input_id: String,
	},
	SetContextPosition {
		node: u64,
		context: u64,
		x: f64,
		y: f64,
	},
	// Clipboard and composite edit-menu operations.
	CopyClips {
		clips: Vec<u64>,
	},
	PasteClips {
		playhead_ts: i64,
	},
	SetBlocksEnabled {
		clips: Vec<u64>,
		enabled: bool,
	},
	SetClipsSpeed {
		clips: Vec<u64>,
		speed: f64,
	},
	RenameBlocks {
		clips: Vec<u64>,
		name: String,
	},
	DuplicateClips {
		clips: Vec<u64>,
	},
	PasteClipsInsert {
		playhead_ts: i64,
	},
	NestClips {
		clips: Vec<u64>,
	},
	// Effect-chain and static parameter operations.
	EffectInsert {
		host: u64,
		index: usize,
		type_id: String,
	},
	EffectRemove {
		host: u64,
		effect: u64,
	},
	EffectMove {
		host: u64,
		effect: u64,
		new_index: usize,
	},
	EffectSetEnabled {
		effect: u64,
		enabled: bool,
	},
	SetNodeStandardValue {
		node: u64,
		input_id: String,
		element: i32,
		value: oak_node::value::OakNodeValue,
	},
	UpsertNodeKeyframe {
		node: u64,
		input_id: String,
		element: i32,
		time: WireRational,
		value: oak_node::value::OakNodeValue,
		interpolation: i32,
		bezier_in: (f64, f64),
		bezier_out: (f64, f64),
	},
	RemoveNodeKeyframe {
		node: u64,
		input_id: String,
		element: i32,
		time: WireRational,
	},
	SetNodeValueAtTime {
		node: u64,
		input_id: String,
		element: i32,
		time: WireRational,
		value: oak_node::value::OakNodeValue,
	},
	PushNodeButton {
		node: u64,
		input_id: String,
	},
	SetNodeTextValue {
		node: u64,
		input_id: String,
		element: i32,
		value: String,
	},
	ReplaceFootage {
		footage: u64,
		path: String,
	},
	DeleteEntry {
		node: u64,
	},
}
