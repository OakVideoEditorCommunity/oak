//! Capability gaps of [`SdkBackend`](super::SdkBackend), as stable ids.
//!
//! Each constant below is reported through
//! [`ToolError::unsupported`](crate::error::ToolError::unsupported), so a
//! model that hits a gap gets `{code: "UNSUPPORTED", gap: "<id>", message}`
//! and can re-plan. The README's gap table is the human-readable twin of
//! this file (and the only place the *why* is written out in prose).
//!
//! The rule these ids enforce is the plan's iron rule: a backend never
//! reports success for an operation it cannot really perform.
//!
//! All of these were verified against the SDK sources
//! (`crates/oak-engine-rust-sdk/src`) and the engine's exported symbols —
//! they are ABI-level absences, not "not implemented yet on our side".

/// `timeline.add_marker`: the engine exports no marker API.
///
/// Verified: neither `oak_timeline_marker_*` nor any `marker` entry point
/// exists in `crates/oak-engine/src`, so a marker cannot be added, listed or
/// persisted through the SDK. (The undoable marker commands live in
/// `oak-timeline` but are not reachable over the C ABI.)
pub const GAP_TIMELINE_MARKERS: &str = "GAP_TIMELINE_MARKERS";

/// `timeline.add_transition`: no seam model in the frozen tool face.
///
/// The SDK *does* expose `block::create_transition`,
/// `timeline::transition_set_offsets` and `node::connect`, but an engine
/// transition is a block that spans a **seam between two blocks**
/// (`transition_input::OUT_BLOCK` / `IN_BLOCK`), with the duration split into
/// `in_offset`/`out_offset`. The frozen tool face names one clip edge plus a
/// total duration and does not say which neighbour is on the other side, how
/// the duration is split across the seam, or what to do when the clip has no
/// neighbour. Wiring a guess would claim a transition the renderer may
/// ignore, so this is an explicit gap for the lead to rule on.
pub const GAP_TRANSITION_SEAM: &str = "GAP_TRANSITION_SEAM";

/// `render.get_audio_levels`: the engine exposes no sequence audio mix.
///
/// Verified: `oak_render::eval::render_audio_samples` (the playback/mix
/// entry point) has no C export — see
/// `docs/zh/plans/engine-sdk-capability-gaps.md` "音频采样渲染". The SDK's
/// only audio path is `codec::DecoderSession::retrieve_audio`, i.e. one
/// media file at a time; it cannot be turned into a sequence mix without
/// reading each block's connected source, which the SDK cannot do either
/// ([`GAP_CLIP_SOURCE_QUERY`]).
pub const GAP_RENDER_AUDIO_SAMPLES: &str = "GAP_RENDER_AUDIO_SAMPLES";

/// Informational: the SDK cannot read a block's connected inputs.
///
/// There is `oak_node_connect` / `oak_node_disconnect` but no edge query, so
/// a clip's source footage cannot be recovered from the document. The
/// backend therefore remembers `clip → footage` when `timeline.place_clip`
/// runs, and `project.get_overview` reports `"footage": null` for clips it
/// never placed (e.g. after `project.open` of an existing file).
pub const GAP_CLIP_SOURCE_QUERY: &str = "GAP_CLIP_SOURCE_QUERY";

/// `node.list_types` with a `node` id: no parameter enumeration.
///
/// The type catalogue (`node.list_types` without `node`) is real — it is
/// built from the engine's node factory table. Reading back one instance's
/// live parameters is not: the SDK bridges `standard_value(node, input,
/// element)` (one known input id at a time) but exports no input list, so a
/// parameter inventory cannot be produced. Reporting an empty parameter list
/// would be a false success.
pub const GAP_NODE_INPUT_ENUM: &str = "GAP_NODE_INPUT_ENUM";

/// `node.set_param` with a string value: strings are not bridged.
///
/// `oak_node_set_standard_value` takes the POD `OakNodeValue`, whose bridge
/// rejects string-carried types (`OakNodeValue::to_node_value` has no STRING
/// arm). Numeric / boolean / rational / vector / colour parameters are
/// implemented; text parameters are not.
pub const GAP_NODE_STRING_PARAM: &str = "GAP_NODE_STRING_PARAM";

/// `timeline.add_track` with an explicit `index`: append only.
///
/// `oak_track_add(project, sequence, kind)` appends to the sequence's track
/// list of that media kind; the SDK exposes no "insert at index" track
/// command. An `index` equal to the current append position is accepted (it
/// is what the call does), anything else is this gap.
pub const GAP_TRACK_INSERT_INDEX: &str = "GAP_TRACK_INSERT_INDEX";
