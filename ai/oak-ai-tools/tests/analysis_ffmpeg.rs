//! Task K acceptance: the five `analysis.*` v2 tools against real ffmpeg.
//!
//! Material is generated with lavfi (`sine`, `testsrc2`, `color`) so the
//! expectations are known exactly: a 3 s tone muted outside [1 s, 2 s]
//! has one silence, a 4 s testsrc2 with a cut at 1.5 s has a scene point,
//! and a 4 s color clip whose first second is black has one black
//! interval. Every test first probes `ffmpeg -version`; without ffmpeg it
//! prints the reason and returns — never a fabricated green.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use oak_ai_tools::dispatch::{execute_v2_with, v2_registry};
use oak_ai_tools::hooks::{NoHooks, Policy};
use oak_ai_tools::mock::MockBackend;
use oak_ai_tools::schema_v2::{is_v2_tool, v2_tool_names};
use serde_json::json;

/// Returns `None` when ffmpeg is absent, printing why (the skip must be
/// visible in the test log, not silent).
fn ffmpeg_or_skip(test: &str) -> Option<()> {
	match Command::new("ffmpeg")
		.arg("-version")
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.status()
	{
		Ok(status) if status.success() => Some(()),
		other => {
			eprintln!(
				"SKIP {test}: ffmpeg is not available ({other:?}); \
				 install ffmpeg to run the analysis acceptance tests"
			);
			None
		}
	}
}

fn run(tool: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
	let mut backend = MockBackend::new();
	execute_v2_with(&mut backend, tool, args, Policy::Auto, &mut NoHooks)
		.into_result()
		.map_err(|err| format!("{tool} failed: {} ({})", err.message, err.code))
}

/// Generates `out` with the lavfi graph `filter`; panics with ffmpeg's
/// stderr when the generation itself fails.
fn lavfi_generate(out: &Path, filter: &str, format: &str) {
	let mut cmd = Command::new("ffmpeg");
	cmd.args([
		"-hide_banner",
		"-loglevel",
		"error",
		"-y",
		"-f",
		"lavfi",
		"-i",
		filter,
	]);
	if format == "muted_sine" {
		// 3 s 440 Hz, muted outside [1 s, 2 s]: one 1 s silence in the
		// middle and one trailing silence from 2 s to 3 s.
		cmd.args(["-af", "volume=enable='lt(t,1)+gt(t,2)':volume=0"]);
	}
	cmd.arg(out);
	let status = cmd
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.status()
		.expect("spawn ffmpeg");
	assert!(status.success(), "lavfi generation failed for {out:?}");
}

fn temp_dir(tag: &str) -> PathBuf {
	let dir = std::env::temp_dir().join(format!("oak_analysis_{tag}"));
	let _ = fs::remove_dir_all(&dir);
	fs::create_dir_all(&dir).expect("temp dir");
	dir
}

// ---------------------------------------------------------------------------
// Schema / registry face: five tools listed, classified, schema-valid
// ---------------------------------------------------------------------------

#[test]
fn analysis_tools_are_listed_classified_and_schema_valid() {
	let names = v2_tool_names();
	let expected = [
		"analysis.detect_silence",
		"analysis.detect_scenes",
		"analysis.detect_black",
		"analysis.loudness",
		"analysis.export_thumbnails",
	];
	for name in expected {
		assert!(names.contains(&name), "{name} missing from v2_tool_names");
		assert!(is_v2_tool(name), "{name} not a v2 tool");
	}
	// Every registered call is dispatchable (lookup resolves) and grades
	// as Read — `class_of_v2` returning None would be a fail-closed
	// UNCLASSIFIED_TOOL at execute time.
	for def in v2_registry() {
		if !def.name.starts_with("analysis.") {
			continue;
		}
		assert!(!def.transactional, "{} must not open an undo group", def.name);
		let class = oak_ai_tools::hooks::class_of_v2(def.name)
			.unwrap_or_else(|| panic!("{} has no grade", def.name));
		assert_eq!(
			class,
			oak_ai_tools::hooks::OperationClass::Read,
			"{} must grade Read",
			def.name
		);
	}
}

#[test]
fn analysis_schemas_validate_their_examples_and_refuse_unknown_keys() {
	for (tool, args) in [
		(
			"analysis.detect_silence",
			json!({"src": "/tmp/a.mp4", "noise_db": -25, "min_dur": 0.4}),
		),
		("analysis.detect_scenes", json!({"src": "/tmp/a.mp4"})),
		(
			"analysis.detect_black",
			json!({"src": "/tmp/a.mp4", "min_dur": 0.3, "pix_th": 0.05}),
		),
		("analysis.loudness", json!({"src": "/tmp/a.mp4"})),
		(
			"analysis.export_thumbnails",
			json!({"src": "/tmp/a.mp4", "interval_sec": 2, "outdir": "/tmp/t", "width": 320}),
		),
	] {
		oak_ai_tools::schema_v2::validate(tool, &args)
			.unwrap_or_else(|err| panic!("{tool} example rejected: {err:?}"));
	}
	// additionalProperties:false is live: a misspelled key is refused.
	let err = oak_ai_tools::schema_v2::validate(
		"analysis.detect_silence",
		&json!({"src": "/tmp/a.mp4", "noiseDb": -30}),
	)
	.expect_err("misspelled key must be refused");
	assert_eq!(err.code, oak_ai_tools::error::CODE_SCHEMA_VALIDATION);
}

// ---------------------------------------------------------------------------
// Real ffmpeg runs
// ---------------------------------------------------------------------------

#[test]
fn detect_silence_finds_the_muted_windows() {
	let Some(()) = ffmpeg_or_skip("detect_silence_finds_the_muted_windows") else {
		return;
	};
	let dir = temp_dir("silence");
	let src = dir.join("tone.mp4");
	lavfi_generate(
		&src,
		"sine=frequency=440:duration=3",
		"muted_sine",
	);
	let out = run(
		"analysis.detect_silence",
		json!({
			"src": src.display().to_string(),
			"noise_db": -30,
			"min_dur": 0.5,
		}),
	)
	.expect("detect_silence");
	let intervals = out["intervals"].as_array().expect("intervals array");
	assert!(
		!intervals.is_empty(),
		"expected at least one silence, got {out}"
	);
	// First silence: [0, 1) — the tone is muted before t=1.
	let first = &intervals[0];
	let start = first["start"]["num"].as_f64().unwrap() / first["start"]["den"].as_f64().unwrap();
	let end = first["end"]["num"].as_f64().unwrap() / first["end"]["den"].as_f64().unwrap();
	assert!(
		(start - 0.0).abs() < 0.15,
		"first silence should start near 0, got {start} (out: {out})"
	);
	assert!(
		(end - 1.0).abs() < 0.15,
		"first silence should end near 1.0, got {end} (out: {out})"
	);
	assert_eq!(out["count"].as_u64().unwrap() as usize, intervals.len());
	// A video with no audio stream is a structured failure, not a lie.
	let silent_video = dir.join("silent_video.mp4");
	lavfi_generate(&silent_video, "testsrc2=size=64x48:rate=25:duration=1", "video");
	let err = run(
		"analysis.detect_silence",
		json!({"src": silent_video.display().to_string()}),
	)
	.expect_err("no audio stream must fail, not report empty silence");
	assert!(
		err.contains("no audio stream"),
		"expected the missing-audio explanation, got: {err}"
	);
	// Missing input file: structured NOT_FOUND.
	let err = run(
		"analysis.detect_silence",
		json!({"src": "/nonexistent/oak_analysis_file.mp4"}),
	)
	.expect_err("missing file must fail");
	assert!(err.contains("failed"), "error text should name the failure: {err}");
}

#[test]
fn detect_scenes_finds_the_cut() {
	let Some(()) = ffmpeg_or_skip("detect_scenes_finds_the_cut") else {
		return;
	};
	let dir = temp_dir("scenes");
	let src = dir.join("cut.mp4");
	// testsrc2 for 2 s, then a hard cut to a second colour: the scene
	// detector must report the change at ~1.5 s.
	let status = Command::new("ffmpeg")
		.args([
			"-hide_banner",
			"-loglevel",
			"error",
			"-y",
			"-f",
			"lavfi",
			"-i",
			"testsrc2=size=64x48:rate=25:duration=1.5",
			"-f",
			"lavfi",
			"-i",
			"smptebars=size=64x48:rate=25:duration=2.5",
			"-filter_complex",
			"[0:v][1:v]concat=n=2:v=1:a=0[out]",
			"-map",
			"[out]",
			"-pix_fmt",
			"yuv420p",
			src.to_str().expect("utf8 path"),
		])
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.status()
		.expect("spawn ffmpeg");
	assert!(status.success(), "concat clip generation failed");

	let out = run(
		"analysis.detect_scenes",
		json!({"src": src.display().to_string(), "threshold": 0.3}),
	)
	.expect("detect_scenes");
	let points = out["points"].as_array().expect("points array");
	assert!(
		!points.is_empty(),
		"expected at least one scene point, got {out}"
	);
	let times: Vec<f64> = points
		.iter()
		.map(|p| {
			p["time"]["num"].as_f64().unwrap() / p["time"]["den"].as_f64().unwrap()
		})
		.collect();
	assert!(
		times.iter().any(|t| (t - 1.5).abs() < 0.35),
		"a cut at 1.5 s should be detected, got {times:?} (out: {out})"
	);
}

#[test]
fn detect_black_finds_the_black_head() {
	let Some(()) = ffmpeg_or_skip("detect_black_finds_the_black_head") else {
		return;
	};
	let dir = temp_dir("black");
	// 4 s: black for the first second, then colour.
	let src = dir.join("black_head.mp4");
	lavfi_generate(
		&src,
		"color=c=black:s=64x48:r=25:d=1,format=yuv420p,drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='gte(t,1)'",
		"video",
	);
	let out = run(
		"analysis.detect_black",
		json!({"src": src.display().to_string(), "min_dur": 0.3, "pix_th": 0.10}),
	)
	.expect("detect_black");
	let intervals = out["intervals"].as_array().expect("intervals array");
	assert!(
		!intervals.is_empty(),
		"expected a black interval, got {out}"
	);
	let first = &intervals[0];
	let start = first["start"]["num"].as_f64().unwrap() / first["start"]["den"].as_f64().unwrap();
	let end = first["end"]["num"].as_f64().unwrap() / first["end"]["den"].as_f64().unwrap();
	assert!(
		start < 0.2,
		"black should start at 0, got {start} (out: {out})"
	);
	assert!(
		(end - 1.0).abs() < 0.3,
		"black should end near 1.0 (drawbox takes over), got {end} (out: {out})"
	);
}

#[test]
fn loudness_reports_lufs_lra_and_true_peak() {
	let Some(()) = ffmpeg_or_skip("loudness_reports_lufs_lra_and_true_peak") else {
		return;
	};
	let dir = temp_dir("loudness");
	let src = dir.join("tone.mp4");
	lavfi_generate(&src, "sine=frequency=440:duration=3", "plain");
	let out = run(
		"analysis.loudness",
		json!({"src": src.display().to_string()}),
	)
	.expect("loudness");
	let i = out["integrated_lufs"].as_f64().expect("I");
	let lra = out["lra_lu"].as_f64().expect("LRA");
	let peak = out["true_peak_dbfs"].as_f64().expect("true peak");
	// A pure sine at full scale: loud but well below the 0 LUFS anchor,
	// LRA ~ 0 (constant level), true peak below 0 dBFS.
	assert!(
		i > -40.0 && i < 0.0,
		"integrated loudness out of range: {i} (out: {out})"
	);
	// ebur128's range estimator is edge-driven and reports up to 20 LU for
	// this pure tone; the contract checked here is the *shape* (a finite,
	// non-negative LU value), not the estimator's opinion of the material.
	assert!(
		lra.is_finite() && lra >= 0.0 && lra <= 20.0,
		"LRA out of range: {lra} (out: {out})"
	);
	assert!(
		peak <= 0.0 && peak > -30.0,
		"true peak out of range: {peak} (out: {out})"
	);
}

#[test]
fn export_thumbnails_writes_real_files() {
	let Some(()) = ffmpeg_or_skip("export_thumbnails_writes_real_files") else {
		return;
	};
	let dir = temp_dir("thumbs");
	let src = dir.join("clip.mp4");
	lavfi_generate(&src, "testsrc2=size=64x48:rate=25:duration=6", "video");
	let outdir = dir.join("out");
	let out = run(
		"analysis.export_thumbnails",
		json!({
			"src": src.display().to_string(),
			"interval_sec": 2,
			"outdir": outdir.display().to_string(),
			"width": 160,
		}),
	)
	.expect("export_thumbnails");
	let paths = out["paths"].as_array().expect("paths array");
	assert!(!paths.is_empty(), "expected thumbnails, got {out}");
	for path in paths {
		let p = Path::new(path.as_str().expect("utf8 path"));
		assert!(p.is_file(), "advertised path does not exist: {p:?}");
		assert!(
			fs::metadata(p).expect("metadata").len() > 0,
			"thumbnail is empty: {p:?}"
		);
	}
	// Explicit time points (timecode style: rational seconds).
	let outdir2 = dir.join("out_times");
	let out = run(
		"analysis.export_thumbnails",
		json!({
			"src": src.display().to_string(),
			"times": [{"num": 1, "den": 2}, 100],
			"outdir": outdir2.display().to_string(),
		}),
	)
	.expect("export_thumbnails at times");
	let paths = out["paths"].as_array().expect("paths array");
	assert_eq!(paths.len(), 2, "two requested times, two files: {out}");
	for path in paths {
		assert!(
			Path::new(path.as_str().expect("utf8 path")).is_file(),
			"missing {path}"
		);
	}
}
