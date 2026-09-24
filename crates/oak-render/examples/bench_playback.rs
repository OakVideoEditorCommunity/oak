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

//! Real-footage playback benchmark: renders `N` sequential frames of a
//! real media file through the real oak-worker pool **or** the M1/M4
//! thread pipeline at the app's preview proxy size, mimicking the
//! playback pre-render window (Playback priority, immediate release).
//! Reports throughput, completion latency, first-frame latency and CPU
//! time (self + children) so the M4 acceptance comparison between the two
//! backends is reproducible.
//!
//! Run from the repo root:
//!
//! ```sh
//! cargo run --release -p oakrender --example bench_playback -- <media> [frames] [workers] [long_edge] [processes|pipeline]
//! ```
//!
//! `frames` defaults to 240, `workers` to the adaptive policy, `long_edge`
//! to 480 (the app's preview proxy size) and the backend to `processes`.
//! Set `OAK_BENCH_GENERATE=1` to synthesize a 1080p/25 fps 10 s clip at
//! `<media>` when the file does not exist.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oak_core::{PixelFormat, Rational};
use oak_render::procpool::{DispatcherConfig, ProcessDispatcher};
use oak_render::ticket::{Completion, Producer, TicketPayload, TicketResult, VideoTicketParams};
use oak_render::worker::{Job, JobDispatch, JobSchedule};

/// The comparison frame format: both backends must produce the same
/// pixels for the numbers to be comparable (the app's 8-bit preview path
/// uses BGRA8, but the process pool would then quantize — the F32 slots
/// are the like-for-like path, and what the 10-bit preview uses).
const BENCH_FORMAT: PixelFormat = PixelFormat::F32;

/// Locate the oak-worker binary (see bench_process).
fn worker_bin() -> PathBuf {
	if let Ok(p) = std::env::var("OAK_WORKER_BIN") {
		return PathBuf::from(p);
	}
	if let Ok(exe) = std::env::current_exe() {
		if let Some(examples) = exe.parent() {
			if let Some(profile) = examples.parent() {
				let candidate = profile.join(format!("oak-worker{}", std::env::consts::EXE_SUFFIX));
				if candidate.exists() {
					return candidate;
				}
			}
		}
	}
	PathBuf::from("oak-worker")
}

/// `(user, system)` CPU seconds of this process and its children (Unix
/// `getrusage`; the benchmark's CPU accounting is Unix-only, the rest of
/// the harness runs everywhere).
#[cfg(unix)]
fn cpu_times() -> (f64, f64) {
	fn rusage(who: i32) -> (f64, f64) {
		let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
		if unsafe { libc::getrusage(who, &mut usage) } != 0 {
			return (0.0, 0.0);
		}
		let seconds = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
		(seconds(usage.ru_utime), seconds(usage.ru_stime))
	}
	let self_times = rusage(libc::RUSAGE_SELF);
	let children = rusage(libc::RUSAGE_CHILDREN);
	(self_times.0 + children.0, self_times.1 + children.1)
}

/// Windows has no `getrusage`; the harness still builds and runs, it just
/// reports zero CPU seconds (`cargo test` compiles examples on every OS).
#[cfg(not(unix))]
fn cpu_times() -> (f64, f64) {
	(0.0, 0.0)
}

/// One footage ticket over the whole timeline.
fn footage_params(media: &str, time: Rational, width: i32, height: i32) -> VideoTicketParams {
	VideoTicketParams {
		viewer: 1,
		project: String::new(),
		time,
		force_size: Some((width, height)),
		force_format: Some(BENCH_FORMAT),
		cache: None,
		cache_dir: None,
		cache_id: None,
		cache_timebase: None,
		footage: Some((media.to_string(), 0)),
		montage: Vec::new(),
		adjustments: Vec::new(),
	}
}

fn report(entries: &[(i64, Instant, Instant)], start: Instant, elapsed: Duration, cpu: (f64, f64)) {
	let completed = entries.len();
	let throughput = completed as f64 / elapsed.as_secs_f64();
	let mut latencies: Vec<f64> = entries
		.iter()
		.map(|(_, submit, done)| (*done - *submit).as_secs_f64() * 1000.0)
		.collect();
	latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
	let first = entries
		.iter()
		.map(|(_, _, done)| (*done - start).as_secs_f64() * 1000.0)
		.fold(f64::INFINITY, f64::min);

	let report = |name: &str, value: String| println!("{name:<38} {value}");
	report("frames completed", completed.to_string());
	report("total wall time", format!("{:.2} s", elapsed.as_secs_f64()));
	report(
		"throughput",
		format!(
			"{throughput:.1} fps ({:.1} ms/frame)",
			1000.0 / throughput.max(f64::EPSILON)
		),
	);
	report("first-frame latency", format!("{first:.1} ms"));
	if !latencies.is_empty() {
		let mean = latencies.iter().sum::<f64>() / latencies.len() as f64;
		report("completion latency mean", format!("{mean:.1} ms"));
		report(
			"completion latency p50/p95/max",
			format!(
				"{:.1} / {:.1} / {:.1} ms",
				latencies[latencies.len() / 2],
				latencies[((latencies.len() as f64 * 0.95) as usize).min(latencies.len() - 1)],
				latencies.last().unwrap()
			),
		);
	}
	report("cpu user + sys", format!("{:.2} + {:.2} s", cpu.0, cpu.1));
	report(
		"main-heap frame copies",
		oak_render::procpool::main_heap_frame_copies().to_string(),
	);
}

/// Process-pool playback: post every frame at Playback priority and pump.
fn run_processes(media: &str, frames: usize, width: i32, height: i32, workers: Option<usize>) {
	let config = DispatcherConfig {
		worker_bin: Some(worker_bin()),
		workers: workers.unwrap_or(0),
		slots_per_worker: 8,
		width,
		height,
		slot_format: BENCH_FORMAT as i32,
		batch_size: 0,
		graph_snapshot: None,
		handshake_timeout_ms: 30_000,
	};
	let dispatcher = ProcessDispatcher::new(config).expect("dispatcher config");
	dispatcher.start().expect("workers start + handshake");
	println!(
		"oak-worker pool: {} worker(s), {frames} x {width}x{height} {BENCH_FORMAT:?} frames of {media}",
		dispatcher.worker_count()
	);

	let cpu_start = cpu_times();
	let results = Arc::new(Mutex::new(Vec::<(i64, Instant, Instant)>::new()));
	let start = Instant::now();
	for i in 0..frames {
		let results = results.clone();
		let dc = dispatcher.clone();
		let frame = i as i64;
		let media_clone = media.to_string();
		let job = Job {
			node_identity: 1,
			time: Rational::new(frame, 25),
			params: Arc::new(footage_params(
				&media_clone,
				Rational::new(frame, 25),
				width,
				height,
			)),
			audio: None,
			produce: Arc::new(|_, _| {
				Err(oak_render::error::Error::Failed(
					"process backend does not use the in-process producer".into(),
				))
			}),
			done: Box::new(move |result: TicketResult| match result {
				Ok(TicketPayload::ShmFrame(f)) => {
					results.lock().unwrap_or_else(|e| e.into_inner()).push((
						frame,
						start,
						Instant::now(),
					));
					dc.release_frame(&f);
				}
				Ok(TicketPayload::ShmAudio(a)) => {
					dc.release_audio_frame(&a);
				}
				Ok(other) => {
					eprintln!("unexpected payload: {other:?}");
				}
				Err(e) => {
					eprintln!("frame {frame} failed: {e}");
				}
			}),
			schedule: JobSchedule::playback(frame, frame, 0),
			cancelled: None,
		};
		if !dispatcher.post(job) {
			eprintln!("post refused at frame {frame}");
			break;
		}
	}

	let deadline = Instant::now() + Duration::from_secs(300);
	loop {
		dispatcher.poll();
		let done = results.lock().unwrap_or_else(|e| e.into_inner()).len();
		if done >= frames {
			break;
		}
		if Instant::now() > deadline {
			eprintln!("timeout: {done}/{frames} completions");
			break;
		}
		std::thread::sleep(Duration::from_millis(2));
	}
	let elapsed = start.elapsed();
	// Children (the worker pool) are only accounted at wait(): shut the
	// pool down before reading RUSAGE_CHILDREN, then report.
	dispatcher.shutdown();
	let cpu_end = cpu_times();
	let entries: Vec<(i64, Instant, Instant)> = results
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.drain(..)
		.collect();
	report(
		&entries,
		start,
		elapsed,
		(cpu_end.0 - cpu_start.0, cpu_end.1 - cpu_start.1),
	);
}

/// M4 thread-pipeline playback: the same Playback jobs on the single
/// render/decode threads; the pipeline prefetches each frame's decode on
/// post and orders the queue by priority.
fn run_pipeline(media: &str, frames: usize, width: i32, height: i32) {
	let backend = oak_render::pipeline::PipelineBackend::new().expect("pipeline start");
	println!("thread pipeline: 1 render + 1 decode thread, {frames} x {width}x{height} F32 frames of {media}");

	let cpu_start = cpu_times();
	let results = Arc::new(Mutex::new(Vec::<(i64, Instant, Instant)>::new()));
	let start = Instant::now();
	for i in 0..frames {
		let frame = i as i64;
		let results = results.clone();
		let submitted = Instant::now();
		let done: Completion = Box::new(move |result: TicketResult| {
			if let Ok(TicketPayload::Video(_)) = result {
				results.lock().unwrap_or_else(|e| e.into_inner()).push((
					frame,
					submitted,
					Instant::now(),
				));
			}
		});
		let producer: Producer = Arc::new(|time, params| {
			oak_render::eval::render_produced_frame(time, params).map(TicketPayload::Video)
		});
		let job = Job {
			node_identity: 1,
			time: Rational::new(frame, 25),
			params: Arc::new(footage_params(
				media,
				Rational::new(frame, 25),
				width,
				height,
			)),
			audio: None,
			produce: producer,
			done,
			schedule: JobSchedule::playback(frame, frame, 0),
			cancelled: None,
		};
		// The blocking post is the pipeline's backpressure: once the render
		// queue is full the submitter waits (the app's window is capped by
		// `preview_window_capacity`).
		if !backend.post(job) {
			eprintln!("post refused at frame {frame}");
			break;
		}
	}

	let deadline = Instant::now() + Duration::from_secs(300);
	loop {
		let done = results.lock().unwrap_or_else(|e| e.into_inner()).len();
		if done >= frames {
			break;
		}
		if Instant::now() > deadline {
			eprintln!("timeout: {done}/{frames} completions");
			break;
		}
		std::thread::sleep(Duration::from_millis(2));
	}
	let elapsed = start.elapsed();
	let cpu_end = cpu_times();
	let entries: Vec<(i64, Instant, Instant)> = results
		.lock()
		.unwrap_or_else(|e| e.into_inner())
		.drain(..)
		.collect();
	report(
		&entries,
		start,
		elapsed,
		(cpu_end.0 - cpu_start.0, cpu_end.1 - cpu_start.1),
	);
	backend.shutdown();
}

fn main() {
	let media = std::env::args()
		.nth(1)
		.unwrap_or_else(|| "tests/demo.mp4".to_string());
	let frames: usize = std::env::args()
		.nth(2)
		.and_then(|s| s.parse().ok())
		.unwrap_or(240);
	let workers: Option<usize> = std::env::args().nth(3).and_then(|s| s.parse().ok());
	let long_edge: i32 = std::env::args()
		.nth(4)
		.and_then(|s| s.parse().ok())
		.unwrap_or(480);
	let mode = std::env::args()
		.nth(5)
		.unwrap_or_else(|| "processes".to_string());

	// The app's preview proxy size: the sequence's aspect scaled to the
	// long edge (demo.mp4 is 16:9 1080p).
	let (width, height) = ((long_edge as f64 * 16.0 / 9.0).round() as i32, long_edge);

	if !std::path::Path::new(&media).exists() && std::env::var_os("OAK_BENCH_GENERATE").is_some() {
		oak_codec::testmedia::write_test_clip(std::path::Path::new(&media), 1920, 1080, 250, 25)
			.expect("generate the 1080p benchmark clip");
		println!("generated 1080p/25 fps test media at {media}");
	}

	match mode.as_str() {
		"pipeline" => run_pipeline(&media, frames, width, height),
		_ => run_processes(&media, frames, width, height, workers),
	}
}
