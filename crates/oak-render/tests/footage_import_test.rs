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

//! M5 acceptance: the zero-copy hardware-decode import.
//!
//! With a shared GPU context installed (the app's render device), a
//! hardware-decoded frame is imported as planar GPU textures and resolved
//! to working-space RGBA on the GPU — `HW_TRANSFERS` (the CPU download
//! counter) must not move. Turning the import switch off must give the
//! same pixels through the CPU staging path (within decoder/swscale
//! rounding; the threshold is documented at the comparison).
//!
//! The assertions need an importable *hardware decoder* (VAAPI/NVDEC/
//! D3D11VA/VideoToolbox), not just a GPU context: the lavapipe CI runner
//! has software Vulkan but no `/dev/dri`, so the decoder never produces
//! an importable surface and every test here logs a `SKIP:` line and
//! returns. The staging fallback is covered by the existing decode tests;
//! the real-hardware acceptance run is recorded in
//! `docs/zh/plans/render-pipeline-threads-m5-branch-coverage.txt` (M5
//! platform rows) and has to be repeated on a VAAPI/D3D11VA/VideoToolbox
//! machine.

use std::sync::Mutex;

use oak_core::texture::Texture;
use oak_core::{PixelFormat, Rational};

mod common;

/// Tests in this binary share the process-wide eval frame cache, the
/// import switch and the shared GPU context slot; serialize them.
static SERIAL: Mutex<()> = Mutex::new(());

/// Forces the CPU staging decode (`OAK_GPU_IMPORT=0`) for the reference
/// frame, and restores the previous value on drop: the variable is
/// process-wide, so a mid-test panic (`expect("staging decode")`) must not
/// leak `"0"` into later tests, and an operator-preset value must survive
/// the test. The tests serialize on `SERIAL`, so the override is
/// race-free here (mirrors `SoftwareDecodeGuard` in
/// `render_threads_test.rs`).
struct StagingDecodeGuard {
	prev: Option<String>,
}

impl StagingDecodeGuard {
	fn set() -> Self {
		let prev = std::env::var("OAK_GPU_IMPORT").ok();
		std::env::set_var("OAK_GPU_IMPORT", "0");
		Self { prev }
	}
}

impl Drop for StagingDecodeGuard {
	fn drop(&mut self) {
		match &self.prev {
			Some(p) => std::env::set_var("OAK_GPU_IMPORT", p),
			None => std::env::remove_var("OAK_GPU_IMPORT"),
		}
	}
}

fn clip_path(tag: &str) -> std::path::PathBuf {
	std::env::temp_dir().join(format!(
		"oakrender_import_{tag}_{}.mp4",
		std::process::id()
	))
}

fn write_clip(tag: &str) -> std::path::PathBuf {
	let path = clip_path(tag);
	oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10).expect("test clip generation");
	path
}

fn pin_legacy_working_space() {
	oak_core::color::set_pipeline_color_settings(
		oak_core::colormath::WorkingColorSpace::SrgbLegacy,
		oak_core::colormath::OutputColorSpec::default(),
	);
}

/// Sample the decoded F32 frame at a pixel.
fn sample(frame: &oak_core::texture::Frame, x: usize, y: usize) -> [f32; 4] {
	assert_eq!(
		frame.format,
		PixelFormat::F32,
		"sample() interprets the frame bytes as f32"
	);
	let stride = frame.linesize_bytes();
	let off = y * stride + x * 16;
	let mut out = [0f32; 4];
	for (i, channel) in out.iter_mut().enumerate() {
		*channel =
			f32::from_le_bytes(frame.data[off + i * 4..off + i * 4 + 4].try_into().unwrap());
	}
	out
}

#[test]
fn hardware_import_is_zero_copy_and_matches_staging() {
	let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
	pin_legacy_working_space();

	// The import needs the render device the app installs; without a GPU
	// there is nothing to test (the staging path is the fallback).
	let Some(ctx) = common::gpu_context_for_import() else {
		// The helper logged `SKIP:` (or panicked under OAK_REQUIRE_GPU).
		return;
	};
	oak_core::backend::GpuContext::install_shared(Some(ctx.clone()));

	oak_codec::gpuinterop::reset_import_counters();
	let transfers_before = oak_codec::hwdecode::HW_TRANSFERS.load(std::sync::atomic::Ordering::Relaxed);

	let path = write_clip("zero");
	let imported = oak_render::eval::render_footage_frame(
		&path.to_string_lossy(),
		0,
		Rational::new(0, 1),
		(0, 0), // native size: the import cannot resize
		PixelFormat::F32,
	)
	.expect("decode frame 0");

	if oak_codec::gpuinterop::HW_IMPORTS.load(std::sync::atomic::Ordering::Relaxed) == 0 {
		eprintln!(
			"SKIP: hardware import unavailable; imports={} fallbacks={} transfers={}",
			oak_codec::gpuinterop::HW_IMPORTS.load(std::sync::atomic::Ordering::Relaxed),
			oak_codec::gpuinterop::HW_IMPORT_FALLBACKS.load(std::sync::atomic::Ordering::Relaxed),
			oak_codec::hwdecode::HW_TRANSFERS.load(std::sync::atomic::Ordering::Relaxed),
		);
		let _ = std::fs::remove_file(&path);
		return;
	}

	eprintln!(
		"import took the frame: imports={} transfers={} (before {transfers_before})",
		oak_codec::gpuinterop::HW_IMPORTS.load(std::sync::atomic::Ordering::Relaxed),
		oak_codec::hwdecode::HW_TRANSFERS.load(std::sync::atomic::Ordering::Relaxed),
	);

	// The import took the frame: the result is GPU-resident and no CPU
	// download happened.
	assert!(
		matches!(imported, Texture::Gpu { .. }),
		"imported decode must resolve to a GPU texture, got {imported:?}"
	);
	assert_eq!(
		oak_codec::hwdecode::HW_TRANSFERS.load(std::sync::atomic::Ordering::Relaxed),
		transfers_before,
		"the zero-copy path must not call av_hwframe_transfer_data"
	);
	let imported_frame = imported.to_frame().expect("download resolved frame");
	assert_eq!((imported_frame.width, imported_frame.height), (64, 64));

	// Staging reference: the same media copied to a second path (a
	// distinct eval cache key) decoded with the import switch off.
	let staging_path = clip_path("staging");
	std::fs::copy(&path, &staging_path).expect("copy clip");
	let staging_guard = StagingDecodeGuard::set();
	let staging = oak_render::eval::render_footage_frame(
		&staging_path.to_string_lossy(),
		0,
		Rational::new(0, 1),
		(0, 0),
		PixelFormat::F32,
	)
	.expect("staging decode");
	drop(staging_guard);
	let Texture::Cpu(staging_frame) = &staging else {
		panic!("staging decode must stay on the CPU: {staging:?}");
	};
	assert_eq!((staging_frame.width, staging_frame.height), (64, 64));

	// Same content (the GPU pass uses the frame's own matrix/range and
	// bilinear chroma sampling; the CPU path uses swscale's chroma
	// filtering, so the two differ slightly). 0.08 is the real
	// hardware-vs-software decode precedent
	// (`oak-codec/src/realmedia_tests.rs::hardware_decode_matches_software_decode`);
	// the M5 reference hardware (RTX 5070 Ti + nvidia-vaapi-driver)
	// measures 0.009 here, so the threshold has an order of magnitude of
	// headroom.
	let mut max_diff = 0.0f32;
	for y in 0..64 {
		for x in 0..64 {
			let a = sample(&imported_frame, x, y);
			let b = sample(staging_frame, x, y);
			for c in 0..3 {
				max_diff = max_diff.max((a[c] - b[c]).abs());
			}
		}
	}
	eprintln!("sRGB import vs staging: max diff {max_diff}");
	assert!(
		max_diff < 0.08,
		"import and staging decodes diverge (max channel diff {max_diff})"
	);

	// The known test pattern survives the GPU path: left half red, right
	// half blue on frame 0.
	let [r, g, b, a] = sample(&imported_frame, 8, 32);
	assert!(r > 0.5 && g < 0.4 && b < 0.4, "left half red: {r},{g},{b}");
	assert!(a > 0.9, "opaque: {a}");
	let [r, g, b, _] = sample(&imported_frame, 56, 32);
	assert!(b > 0.5 && r < 0.4 && g < 0.4, "right half blue: {r},{g},{b}");

	let _ = std::fs::remove_file(&path);
	let _ = std::fs::remove_file(&staging_path);
}

#[test]
fn hardware_import_applies_the_source_to_working_lut() {
	let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
	// ACEScg working space: the import path must run the source→working
	// transform on the GPU (the baked 3D LUT), matching the CPU path's
	// exact per-pixel `decode_to_acescg`.
	oak_core::color::set_pipeline_color_settings(
		oak_core::colormath::WorkingColorSpace::AcesCg,
		oak_core::colormath::OutputColorSpec::default(),
	);

	let Some(ctx) = common::gpu_context_for_import() else {
		// The helper logged `SKIP:` (or panicked under OAK_REQUIRE_GPU).
		return;
	};
	oak_core::backend::GpuContext::install_shared(Some(ctx.clone()));

	oak_codec::gpuinterop::reset_import_counters();
	let path = write_clip("aces");
	let imported = oak_render::eval::render_footage_frame(
		&path.to_string_lossy(),
		0,
		Rational::new(0, 1),
		(0, 0),
		PixelFormat::F32,
	)
	.expect("decode frame 0");
	if oak_codec::gpuinterop::HW_IMPORTS.load(std::sync::atomic::Ordering::Relaxed) == 0 {
		eprintln!("SKIP: hardware import unavailable; skipping assertions");
		let _ = std::fs::remove_file(&path);
		return;
	}
	let imported_frame = imported.to_frame().expect("download resolved frame");

	let staging_path = clip_path("aces_staging");
	std::fs::copy(&path, &staging_path).expect("copy clip");
	let staging_guard = StagingDecodeGuard::set();
	let staging = oak_render::eval::render_footage_frame(
		&staging_path.to_string_lossy(),
		0,
		Rational::new(0, 1),
		(0, 0),
		PixelFormat::F32,
	)
	.expect("staging decode");
	drop(staging_guard);
	let Texture::Cpu(staging_frame) = &staging else {
		panic!("staging decode must stay on the CPU: {staging:?}");
	};

	// The GPU applies the LUT (interpolated); the CPU runs the exact
	// per-pixel transform, so a small interpolation difference is
	// expected — far below a visible grade mismatch.
	let mut max_diff = 0.0f32;
	for y in 0..64 {
		for x in 0..64 {
			let a = sample(&imported_frame, x, y);
			let b = sample(staging_frame, x, y);
			for c in 0..3 {
				max_diff = max_diff.max((a[c] - b[c]).abs());
			}
		}
	}
	eprintln!("ACEScg import vs staging: max diff {max_diff}");
	assert!(
		max_diff < 0.05,
		"working-space LUT diverges from the CPU transform: {max_diff}"
	);

	let _ = std::fs::remove_file(&path);
	let _ = std::fs::remove_file(&staging_path);
}

#[test]
fn montage_native_size_with_host_gpu_composites_the_clip() {
	let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
	pin_legacy_working_space();

	// This is the M5 audit regression: a sequence montage at the clip's
	// native size with a host GPU installed used to import the clip and
	// then silently skip it in the CPU compositor, producing an
	// all-transparent black sequence. The montage path must stage on
	// purpose and composite the clip.
	let Some(ctx) = common::gpu_context_for_import() else {
		// The helper logged `SKIP:` (or panicked under OAK_REQUIRE_GPU).
		return;
	};
	oak_core::backend::GpuContext::install_shared(Some(ctx));

	oak_codec::gpuinterop::reset_import_counters();
	let path = write_clip("montage_native");

	// First import the frame at native size through the normal
	// single-footage path: the planar texture is now cached under the
	// (64, 64) key that the montage request will use.
	let single = oak_render::eval::render_footage_frame(
		&path.to_string_lossy(),
		0,
		Rational::new(0, 1),
		(64, 64),
		PixelFormat::F32,
	)
	.expect("single-footage decode");
	if oak_codec::gpuinterop::HW_IMPORTS.load(std::sync::atomic::Ordering::Relaxed) == 0 {
		eprintln!("SKIP: hardware import unavailable on this machine; skipping montage assertions");
		let _ = std::fs::remove_file(&path);
		return;
	}
	assert!(
		matches!(single, Texture::Gpu { .. }),
		"the pre-step must produce the imported GPU texture"
	);
	let imports_after_single =
		oak_codec::gpuinterop::HW_IMPORTS.load(std::sync::atomic::Ordering::Relaxed);
	let transfers_after_single =
		oak_codec::hwdecode::HW_TRANSFERS.load(std::sync::atomic::Ordering::Relaxed);

	let params = oak_render::ticket::VideoTicketParams {
		viewer: 1,
		project: String::new(),
		time: Rational::new(0, 1),
		force_size: Some((64, 64)), // native: the import-triggering shape
		force_format: None,
		cache: None,
		cache_dir: None,
		cache_id: None,
		cache_timebase: None,
		footage: None,
		montage: vec![oak_render::ticket::MontageClip {
			filename: path.to_string_lossy().into_owned(),
			stream_index: 0,
			in_time: Rational::new(0, 1),
			out_time: Rational::new(10, 1),
			media_in: Rational::new(0, 1),
			gain: 1.0,
			effects: Vec::new(),
		}],
		adjustments: Vec::new(),
	};

	let texture = oak_render::eval::render_produced_frame(Rational::new(0, 1), &params)
		.expect("montage render");
	let frame = texture.to_frame().expect("montage frame");
	assert_eq!((frame.width, frame.height), (64, 64));
	assert!(
		!frame.data.iter().all(|&b| b == 0),
		"montage must not be transparent black"
	);
	// Known pattern: left half red, right half blue.
	let [r, g, b, a] = sample(&frame, 8, 32);
	assert!(r > 0.5 && g < 0.4 && b < 0.4, "left half red: {r},{g},{b}");
	assert!(a > 0.9, "opaque: {a}");
	let [r, g, b, _] = sample(&frame, 56, 32);
	assert!(b > 0.5 && r < 0.4 && g < 0.4, "right half blue: {r},{g},{b}");

	assert_eq!(
		oak_codec::gpuinterop::HW_IMPORTS.load(std::sync::atomic::Ordering::Relaxed),
		imports_after_single,
		"the CPU montage compositor must stage on purpose (no new imports)"
	);
	// The staged request must not have been served by the cached planar
	// texture: it re-decoded through the CPU scaler (a hardware frame
	// transfer happened for the staging path).
	assert!(
		oak_codec::hwdecode::HW_TRANSFERS.load(std::sync::atomic::Ordering::Relaxed)
			> transfers_after_single,
		"the staged montage decode must produce CPU pixels"
	);

	let _ = std::fs::remove_file(&path);
}
