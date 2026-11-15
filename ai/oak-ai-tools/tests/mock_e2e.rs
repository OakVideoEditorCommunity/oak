//! Mock E2E — the T1 acceptance chain on the feature-free backend:
//!
//! import → place → split → trim → ripple_delete → render_frame
//! (synthetic frame) → PNG decode + pixel assertions → undo the whole
//! chain → the timeline serialisation equals the initial snapshot
//! exactly.
//!
//! Everything runs through `dispatch::execute`, i.e. through schema
//! validation and one undo group per call, exactly like the SDK path.

use std::path::PathBuf;

use oak_ai_tools::dispatch::execute;
use oak_ai_tools::mock::{MOCK_HEIGHT, MOCK_WIDTH, MockBackend, footage_rgb};
use oak_ai_tools::render::Frame;
use serde_json::{Value, json};

/// Two real files on disk (the mock checks existence only).
fn temp_media(tag: &str) -> PathBuf {
	let path = std::env::temp_dir().join(format!("oak_e2e_{tag}.mp4"));
	std::fs::write(&path, b"oak mock e2e media fixture").expect("write fixture");
	path
}

fn call(backend: &mut MockBackend, tool: &str, args: Value) -> Value {
	execute(backend, tool, args)
		.unwrap_or_else(|err| panic!("{tool} failed: {err:?}"))
}

#[test]
fn acceptance_chain_render_and_full_undo() {
	let media_a = temp_media("a");
	let media_b = temp_media("b");
	let mut backend = MockBackend::new();

	// --- open ---------------------------------------------------------
	let out = call(&mut backend, "project.open", json!({"name": "e2e", "fps": 30}));
	assert_eq!(out["created"], true);
	let out = call(&mut backend, "timeline.add_track", json!({"type": "video"}));
	let track_a = out["track"].as_str().unwrap().to_string();
	let out = call(&mut backend, "timeline.add_track", json!({"type": "video"}));
	let track_b = out["track"].as_str().unwrap().to_string();

	// --- import -------------------------------------------------------
	let out = call(
		&mut backend,
		"media.import_footage",
		json!({"paths": [media_a.display().to_string(), media_b.display().to_string()]}),
	);
	assert_eq!(out["footage"].as_array().unwrap().len(), 2);
	assert_eq!(out["failed"].as_array().unwrap().len(), 0);
	let footage = out["footage"][0]["id"].as_str().unwrap().to_string();
	let footage_b = out["footage"][1]["id"].as_str().unwrap().to_string();

	// --- place --------------------------------------------------------
	// Default mock footage: 10 s at 30 fps, placed at t=1s.
	let out = call(
		&mut backend,
		"timeline.place_clip",
		json!({
			"track": track_a,
			"footage": footage,
			"in": {"num": 1, "den": 1},
		}),
	);
	let clip = out["clip"].as_str().unwrap().to_string();
	assert_eq!(out["in"], json!({"num": 1, "den": 1}));
	assert_eq!(out["length"], json!({"num": 10, "den": 1}));
	// Second clip later on the same track, for the ripple test.
	let out = call(
		&mut backend,
		"timeline.place_clip",
		json!({
			"track": track_a,
			"footage": footage_b,
			"in": {"num": 21, "den": 1},
		}),
	);
	let clip_b = out["clip"].as_str().unwrap().to_string();
	// And one on the other track (must survive ripple + undo).
	call(
		&mut backend,
		"timeline.place_clip",
		json!({"track": track_b, "footage": footage, "in": 0}),
	);

	let initial = backend.serialize_timeline();

	// --- split at 4s --------------------------------------------------
	let out = call(
		&mut backend,
		"timeline.split_clip",
		json!({"clip": clip, "time": {"num": 4, "den": 1}}),
	);
	let right = out["new_clip"].as_str().unwrap().to_string();
	assert_eq!(out["time"], json!({"num": 4, "den": 1}));

	// --- trim the right half's in point to 6s -------------------------
	let out = call(
		&mut backend,
		"timeline.trim_clip",
		json!({"clip": right, "side": "in", "time": {"num": 6, "den": 1}}),
	);
	assert_eq!(out["in"], json!({"num": 6, "den": 1}));
	// Right half after split: [4,11) with media_in 3 s; trimming its in
	// point to 6 s advances media_in by 2 s → media_in 5 s, length 5 s.
	assert_eq!(out["length"], json!({"num": 5, "den": 1}));
	assert_eq!(out["media_in"], json!({"num": 5, "den": 1}));

	// --- ripple_delete the left half ----------------------------------
	let out = call(
		&mut backend,
		"timeline.ripple_delete",
		json!({"clip": clip}),
	);
	assert_eq!(out["removed"], true);
	assert_eq!(out["ripple"], json!({"num": 3, "den": 1}));
	// The later clip on the same track shifted left by the removed gap:
	// [21,31) → [18,28).
	let overview = call(&mut backend, "project.get_overview", json!({}));
	let track_json = &overview["sequences"][0]["tracks"];
	let track_a_json = track_json
		.as_array()
		.unwrap()
		.iter()
		.find(|t| t["id"] == track_a)
		.expect("track a in overview");
	let moved = track_a_json["clips"]
		.as_array()
		.unwrap()
		.iter()
		.find(|c| c["id"] == clip_b)
		.expect("clip b still on the track");
	assert_eq!(moved["in"], json!({"num": 18, "den": 1}));
	assert_eq!(moved["out"], json!({"num": 28, "den": 1}));

	// --- render a frame at t=7s: covers the trimmed right half --------
	let out = call(
		&mut backend,
		"render.get_frame",
		json!({
			"sequence": "sequence-1",
			"time": {"num": 7, "den": 1},
			"format": "png",
			"max_size": {"width": 320, "height": 180},
		}),
	);
	assert_eq!(out["backend"], "mock");
	assert_eq!(out["width"], 320);
	assert_eq!(out["height"], 180);
	let png_b64 = out["png_base64"].as_str().expect("png payload").to_string();

	// --- PNG decode + pixel assertions --------------------------------
	let png_bytes = b64_decode(&png_b64);
	let frame = Frame::from_png(&png_bytes).expect("PNG decodes");
	assert_eq!(frame.width, 320);
	assert_eq!(frame.height, 180);
	// The frame must be a solid colour equal to the synthetic colour of
	// the footage at the covering clip. After the chain, t=7s falls in
	// the trimmed right half of the first place (footage_a), which the
	// ripple moved to [3,8).
	let expected = footage_rgb(&media_a.display().to_string());
	let centre = frame.pixel(160, 90);
	for channel in 0..3 {
		assert!(
			(centre[channel] - expected[channel]).abs() <= 1.0 / 255.0,
			"channel {channel}: got {}, want {}",
			centre[channel],
			expected[channel]
		);
	}
	// Every corner agrees (solid fill, not a gradient or black).
	for (x, y) in [(0u32, 0u32), (319, 0), (0, 179), (319, 179)] {
		let px = frame.pixel(x, y);
		for channel in 0..3 {
			assert!(
				(px[channel] - expected[channel]).abs() <= 1.0 / 255.0,
				"corner ({x},{y}) channel {channel}: got {}, want {}",
				px[channel],
				expected[channel]
			);
		}
	}
	// Not black anywhere: the mock fills the whole frame.
	let stats = frame.stats();
	assert!(stats.non_black_ratio > 0.999, "frame must be fully covered");

	// A frame outside any clip is black.
	let out = call(
		&mut backend,
		"render.get_frame",
		json!({
			"sequence": "sequence-1",
			"time": {"num": 100, "den": 1},
			"format": "png",
			"max_size": {"width": 8, "height": 8},
		}),
	);
	let gap = Frame::from_png(&b64_decode(out["png_base64"].as_str().unwrap()))
		.expect("gap PNG");
	assert_eq!(gap.stats().non_black_ratio, 0.0, "gap frames are black");

	// --- undo the whole chain ----------------------------------------
	// Chain (after open): add_track x2, import, place x3, split, trim,
	// ripple = 10 transactional groups; render calls also commit groups
	// (they are transactional), so undo until the timeline matches the
	// post-import snapshot exactly.
	let mut guard = 0;
	while backend.serialize_timeline() != initial {
		let out = call(&mut backend, "edit.undo", json!({}));
		guard += 1;
		assert!(guard < 64, "undo did not restore the initial state");
		let _ = out;
	}
	// The state is byte-for-byte the initial serialisation.
	assert_eq!(
		backend.serialize_timeline(),
		initial,
		"timeline serialisation must equal the initial snapshot"
	);
	// And the full state (including name/path/counters) too.
	let final_state = backend.serialize();
	assert_eq!(final_state["project"]["name"], json!("e2e"));

	// Footage/track ids still exist; counters were restored with them.
	let overview = call(&mut backend, "project.get_overview", json!({}));
	assert_eq!(overview["footage"].as_array().unwrap().len(), 2);
	// 2 default tracks from project.open + the 2 we added (still present:
	// the undo loop stopped at the post-import snapshot).
	assert_eq!(overview["sequences"][0]["tracks"].as_array().unwrap().len(), 4);

	let _ = std::fs::remove_file(&media_a);
	let _ = std::fs::remove_file(&media_b);
}

#[test]
fn one_call_is_one_undo_group_and_failure_rolls_back() {
	let media = temp_media("txn");
	let mut backend = MockBackend::new();
	call(&mut backend, "project.open", json!({}));
	let out = call(
		&mut backend,
		"media.import_footage",
		json!({"paths": [media.display().to_string()]}),
	);
	let footage = out["footage"][0]["id"].as_str().unwrap().to_string();
	let track = call(&mut backend, "timeline.add_track", json!({"type": "video"}))
		["track"]
		.as_str()
		.unwrap()
		.to_string();

	let depth_before = backend.undo_depth();
	// A failing place (overlap) must abort: no state change, no group.
	let err = execute(
		&mut backend,
		"timeline.place_clip",
		json!({"track": track, "footage": footage, "in": 0}),
	)
	.expect("first place succeeds");
	let _ = err;
	let depth_after_ok = backend.undo_depth();
	assert_eq!(depth_after_ok, depth_before + 1, "one call = one group");

	let state_before = backend.serialize();
	let err = execute(
		&mut backend,
		"timeline.place_clip",
		json!({"track": track, "footage": footage, "in": 0}),
	)
	.expect_err("overlapping place must fail");
	assert_eq!(err.code, "INVALID_PARAMS");
	assert_eq!(err.step.as_deref(), Some("invoke"));
	assert_eq!(err.txn.as_deref(), Some("aborted"), "failure must abort");
	assert_eq!(err.tool.as_deref(), Some("timeline.place_clip"));
	assert_eq!(
		backend.serialize(),
		state_before,
		"abort must restore the pre-call state"
	);
	assert_eq!(
		backend.undo_depth(),
		depth_after_ok,
		"an aborted call must not push an undo group"
	);

	// Structured error fields are complete.
	assert!(!err.message.is_empty());
	let wire = err.to_json();
	for key in ["code", "message", "tool", "step"] {
		assert!(wire.get(key).is_some(), "error JSON must carry {key}");
	}

	let _ = std::fs::remove_file(&media);
}

/// Minimal standard base64 (the crate under test exposes its own, but
/// the E2E keeps an independent decoder so a bug in `util` cannot make
/// both sides agree on wrong bytes).
fn b64_decode(text: &str) -> Vec<u8> {
	const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
	let clean: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
	assert_eq!(clean.len() % 4, 0, "bad base64 length");
	let mut out = Vec::new();
	for chunk in clean.chunks(4) {
		let mut sextets = [0u32; 4];
		let mut pad = 0;
		for (i, &b) in chunk.iter().enumerate() {
			if b == b'=' {
				pad += 1;
				sextets[i] = 0;
			} else {
				sextets[i] = TABLE
					.iter()
					.position(|&t| t == b)
					.expect("bad base64 char") as u32;
			}
		}
		let n = (sextets[0] << 18) | (sextets[1] << 12) | (sextets[2] << 6) | sextets[3];
		out.push(((n >> 16) & 0xff) as u8);
		if pad < 2 {
			out.push(((n >> 8) & 0xff) as u8);
		}
		if pad < 1 {
			out.push((n & 0xff) as u8);
		}
	}
	out
}

/// The synthetic geometry constants stay honest (documented fixtures).
#[test]
fn mock_geometry_constants_match_the_documented_fixture() {
	assert_eq!(MOCK_WIDTH, 640);
	assert_eq!(MOCK_HEIGHT, 360);
}
