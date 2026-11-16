//! The viewer widget: a fullscreen preview with a transport bar.
//!
//! The picture comes from a [`SurfaceSource`] (in Oak, the W3 bridge's
//! IOSurface-backed pixel buffer); the playhead position is polled from the
//! host's [`PlaybackClock`] on a ~60 Hz timer, and every transport action is
//! emitted as a [`ViewerEvent`] request (the engine applies it and the clock
//! reflects it). Timecode formatting reuses `crate::widgets::timeline::time`.

pub mod clock;
pub mod transport;

pub use clock::*;
pub use transport::*;

use crate::widgets::timeline::{FrameRate, TimeDisplay, format_timecode};
use gpui_kit::{
	AnyElement, App, AsyncWindowContext, Bounds, ClickEvent, Context, DevicePixels, Entity,
	EventEmitter, FocusHandle, Focusable, KeyDownEvent, KeyUpEvent, Keystroke, MouseButton,
	MouseMoveEvent, ObjectFit, Pixels, Point, Render, RenderImage, Rgba, SharedString, Size,
	Window, canvas, colors::DefaultColors, div, img, prelude::*, px,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::widgets::{icons, tooltip::tooltip_view};

/// One GPU-backed viewer frame: the engine's F32 pipeline output uploaded
/// as an RGBA16F texture (see `crates/oak-app/src/oakui/gpu.rs`), indexed by
/// the `RenderImage` id it replaces. The texture carries the 10-bit content
/// that `RenderImage`'s BGRA8 bytes cannot, so the viewer samples it
/// straight into a 10-bit swapchain instead of going through the 8-bit
/// sprite atlas.
#[derive(Clone)]
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
struct GpuFrameEntry {
	/// The GPU texture, type-erased (an `Arc<wgpu::Texture>`).
	texture: Arc<dyn std::any::Any + Send + Sync>,
	/// The texture's dimensions in device pixels.
	size: Size<DevicePixels>,
	/// Insertion-order stamp (the oldest entry is evicted first).
	seq: u64,
}

/// The registered GPU frames, keyed by `RenderImage::id.0`. The engines
/// insert an entry next to every image they hand to `set_cpu_frame`; the
/// viewer looks it up there and uses the texture as a
/// [`SurfaceSource::Texture`] when present. Entries are looked up *without*
/// removal, so a cached image keeps reusing its texture on every hit.
/// Bounded: past [`GPU_FRAMES_CAP`] the oldest entry is evicted.
static GPU_FRAMES: Mutex<Option<HashMap<usize, GpuFrameEntry>>> = Mutex::new(None);
/// Insertion counter for FIFO eviction.
static GPU_FRAMES_SEQ: AtomicU64 = AtomicU64::new(0);
/// Upper bound on registered frames (a playback window plus a couple of
/// cached proxy frames; the newest 16 are enough).
const GPU_FRAMES_CAP: usize = 16;

/// Register the GPU texture standing in for the `RenderImage` with id
/// `image_id`. Called by the engine right after it produces the image, so
/// the viewer's `set_cpu_frame` can switch the picture to the texture.
pub fn register_gpu_frame(
	image_id: usize,
	texture: Arc<dyn std::any::Any + Send + Sync>,
	size: Size<DevicePixels>,
) {
	let seq = GPU_FRAMES_SEQ.fetch_add(1, Ordering::Relaxed);
	if let Ok(mut frames) = GPU_FRAMES.lock() {
		let frames = frames.get_or_insert_with(HashMap::new);
		if frames.len() >= GPU_FRAMES_CAP {
			if let Some(oldest) = frames.values().map(|e| e.seq).min() {
				frames.retain(|_, e| e.seq != oldest);
			}
		}
		frames.insert(image_id, GpuFrameEntry { texture, size, seq });
	}
}

/// Look up the GPU frame registered for `image_id`, if any. Does not remove
/// the entry: the same image is reused on every cache hit.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn take_gpu_frame(image_id: usize) -> Option<GpuFrameEntry> {
	GPU_FRAMES
		.lock()
		.ok()?
		.get_or_insert_with(HashMap::new)
		.get(&image_id)
		.cloned()
}

/// Drop every registered GPU frame (the display-color generation bump
/// invalidates the CPU images they belong to).
pub fn clear_gpu_frames() {
	if let Ok(mut frames) = GPU_FRAMES.lock() {
		frames.get_or_insert_with(HashMap::new).clear();
	}
}

/// A request emitted by the viewer.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerEvent {
	/// Start playback.
	PlayRequested {
		/// The viewer's stable id.
		control: usize,
	},
	/// Pause playback.
	PauseRequested {
		/// The viewer's stable id.
		control: usize,
	},
	/// Step the playhead by `delta` frames.
	StepRequested {
		/// The viewer's stable id.
		control: usize,
		/// Frames to step (negative steps backward).
		delta: i64,
	},
	/// Set the loop-in point at the playhead.
	InPointRequested {
		/// The viewer's stable id.
		control: usize,
	},
	/// Set the loop-out point at the playhead.
	OutPointRequested {
		/// The viewer's stable id.
		control: usize,
	},
	/// Clear the loop range.
	ClearRangeRequested {
		/// The viewer's stable id.
		control: usize,
	},
	/// Toggle the safe-frame overlay.
	ToggleSafeFramesRequested {
		/// The viewer's stable id.
		control: usize,
	},
	/// Toggle the zoom (contain vs cover).
	ToggleZoomRequested {
		/// The viewer's stable id.
		control: usize,
	},
	/// A pointer (pen) event inside the picture area, emitted so the host
	/// can forward it to an OFX interact. `position` is local to the
	/// picture area (top-left origin, logical pixels).
	InteractPointer {
		/// Whether this is a move, a press or a release.
		kind: InteractPointerKind,
		/// Position in the picture area's local pixels (top-left origin).
		position: Point<f32>,
		/// The button involved in a press/release, if any.
		button: Option<MouseButton>,
		/// Whether a pointer button is held at this event (the pen-down
		/// state for a move).
		pressed: bool,
	},
	/// A key press/release while the picture area has keyboard focus,
	/// emitted for OFX interact forwarding. Keys consumed by a global
	/// keybinding never reach this event (gpui dispatches bindings before
	/// the focus-path key handlers).
	InteractKey {
		/// true = press, false = release.
		down: bool,
		/// The keystroke (modifiers + key name).
		keystroke: Keystroke,
	},
	/// A colour sampled from the picture by the eyedropper (only emitted
	/// while armed; the host routes it back to the picker that armed it).
	EyedropperPick {
		/// The sampled colour (0..1 components).
		color: Rgba,
	},
}

/// The kind of a pointer event inside the picture area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractPointerKind {
	/// A mouse move (possibly with a button held).
	Move,
	/// A mouse button press.
	Down,
	/// A mouse button release.
	Up,
}

/// The zoom state of the viewer picture.
///
/// [`ViewerZoom::Fit`] fits the whole frame inside the picture area (like the
/// old contain mode); [`ViewerZoom::Level`] renders the frame at a fixed
/// multiple of its native resolution, clipped to the picture area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerZoom {
	/// Fit the entire frame (contain).
	Fit,
	/// A fixed multiple of the native resolution (index into
	/// [`VIEWER_ZOOM_LEVELS`]).
	Level(usize),
}

impl ViewerZoom {
	/// The next zoom in the cycle used by the toolbar zoom button: Fit → 100%
	/// → 200% → Fit; any other level steps up by one.
	pub fn next(self) -> Self {
		match self {
			ViewerZoom::Fit => ViewerZoom::Level(4),
			ViewerZoom::Level(i) if i + 1 >= VIEWER_ZOOM_LEVELS.len() => ViewerZoom::Fit,
			ViewerZoom::Level(i) if i == 3 => ViewerZoom::Level(7),
			ViewerZoom::Level(i) => ViewerZoom::Level(i + 1),
		}
	}
}

/// The safe-frame margin overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SafeMargins {
	/// No overlay.
	Off,
	/// The standard 90% × 80% action-safe margins.
	On,
	/// Custom horizontal/vertical fractions of the frame (e.g. 0.9 × 0.8).
	Custom(f32, f32),
}

impl SafeMargins {
	/// The next state in the toolbar cycle: Off → On → Off; custom behaves
	/// like On (cycles back to Off).
	pub fn next(self) -> Self {
		match self {
			SafeMargins::Off => SafeMargins::On,
			SafeMargins::On | SafeMargins::Custom(_, _) => SafeMargins::Off,
		}
	}

	/// The frame fractions the overlay covers, or `None` when disabled.
	pub fn ratio(self) -> Option<(f32, f32)> {
		match self {
			SafeMargins::Off => None,
			SafeMargins::On => Some((0.9, 0.8)),
			SafeMargins::Custom(w, h) => Some((w, h)),
		}
	}
}

/// How the audio waveform overlay behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveformMode {
	/// Show the waveform only while playing.
	Automatic = 0,
	/// Show the waveform always (never hide it).
	Only = 1,
	/// Show both the waveform and the picture.
	Both = 2,
}

impl WaveformMode {
	/// Returns the persisted configuration code for this waveform mode.
	///
	/// The codes are `0` (Automatic), `1` (Only) and `2` (Both);
	/// [`WaveformMode::from_config_value`] is the inverse. The value is stored
	/// in the viewer preferences.
	pub fn config_value(self) -> i32 {
		self as i32
	}

	/// Reconstruct from a persisted config value, clamping out-of-range
	/// values back to [`WaveformMode::Automatic`].
	pub fn from_config_value(value: i32) -> Self {
		match value.clamp(0, 2) {
			1 => WaveformMode::Only,
			2 => WaveformMode::Both,
			_ => WaveformMode::Automatic,
		}
	}
}

/// The zoom levels offered by the context menu, as multiples of the native
/// resolution. Index 4 is 100% and index 7 is 200% — those are the stops the
/// toolbar zoom button cycles through.
pub const VIEWER_ZOOM_LEVELS: [f32; 10] =
	[0.10, 0.25, 0.50, 0.75, 1.00, 1.25, 1.50, 2.00, 4.00, 8.00];

/// A smoothed per-second frame-rate meter for the `Show FPS` overlay.
#[derive(Debug, Clone, Copy)]
pub struct FpsCounter {
	last: Option<std::time::Instant>,
	smoothed: f32,
	samples: u32,
}

impl FpsCounter {
	/// A fresh counter with no samples.
	pub fn new() -> Self {
		Self {
			last: None,
			smoothed: 0.0,
			samples: 0,
		}
	}

	/// Records that a frame was presented at `now` and updates the smoothed
	/// rate.
	///
	/// The first call only starts the clock. Later calls derive an instant rate
	/// from the elapsed time and apply 0.9/0.1 exponential smoothing;
	/// non-positive intervals (a repeated timestamp) are ignored. Call once per
	/// painted frame while measuring.
	///
	/// # Arguments
	///
	/// * `now` - Monotonic timestamp of the frame (usually `Instant::now()`).
	pub fn record(&mut self, now: std::time::Instant) {
		if let Some(last) = self.last {
			let dt = now.duration_since(last).as_secs_f32();
			if dt > 0.0 {
				let instant = 1.0 / dt;
				self.smoothed = if self.samples == 0 {
					instant
				} else {
					self.smoothed * 0.9 + instant * 0.1
				};
				self.samples += 1;
			}
		}
		self.last = Some(now);
	}

	/// The smoothed rate, or `0.0` before the second frame.
	pub fn value(&self) -> f32 {
		self.smoothed
	}
}

impl Default for FpsCounter {
	fn default() -> Self {
		Self::new()
	}
}

/// The picture source of a [`ViewerWidget`].
///
/// gpui-kit has no external-texture surface on Oak's platforms (its
/// `SurfaceSource` is macOS CoreVideo only), so the viewer always paints
/// CPU frames through the sprite atlas (`img`). The 10-bit GPU-texture
/// path the oak-gpui fork carried (`SurfaceSource::Texture`) needs a
/// renderer-level replacement before it can return.
#[derive(Clone)]
pub enum ViewerFrameSource {
	/// A platform surface: a CoreVideo pixel buffer on macOS, or a GPU
	/// texture handle on Linux/FreeBSD.
	Surface(gpui_kit::SurfaceSource),
	/// A CPU-side frame as raw bytes in a [`RenderImage`] (BGRA8, row-major,
	/// top-to-bottom), uploaded through gpui's sprite atlas on every
	/// platform — the path to use when no platform surface is available.
	CpuFrame(Arc<RenderImage>),
}

/// The viewer widget.
pub struct ViewerWidget<C: PlaybackClock> {
	control: usize,
	clock: Entity<C>,
	frame_rate: FrameRate,
	transport: TransportState,
	frame_source: Option<ViewerFrameSource>,
	/// The most recent CPU frame handed to [`ViewerWidget::set_cpu_frame`],
	/// kept alongside a GPU surface so the pixel-size / eyedropper consumers
	/// (which read raw bytes) keep working on the 10-bit surface path.
	cpu_image: Option<Arc<RenderImage>>,
	focus_handle: FocusHandle,
	safe_margins: SafeMargins,
	zoom: ViewerZoom,
	show_fps: bool,
	fps: FpsCounter,
	/// The picture area's bounds in window pixels, captured each render
	/// (the interact pointer forwarding maps window events to the picture's
	/// local pixels from these).
	picture_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
	/// Whether the eyedropper is armed: the cursor becomes a crosshair and a
	/// click samples the pixel under it instead of forwarding an interact
	/// pointer event.
	eyedropper_armed: bool,
	video_fullscreen: bool,
	controls_visible: bool,
	previous_zoom: Option<ViewerZoom>,
	last_pointer_move: Option<std::time::Instant>,
}

impl<C: PlaybackClock> ViewerWidget<C> {
	/// Creates a viewer widget driven by `clock`.
	///
	/// Seeds the local frame-rate mirror from the clock and spawns a 16 ms
	/// timer that polls the clock and repaints on change. Initial state: `Fit`
	/// zoom, safe margins off, FPS overlay hidden, controls visible and no frame
	/// source.
	///
	/// # Arguments
	///
	/// * `control` - Stable control id reported in viewer events and used in
	///   element ids.
	/// * `clock` - Playback clock the viewer follows; stored and polled for the
	///   current frame and playing state.
	/// * `window` - Window used to spawn the polling timer.
	/// * `cx` - Context used to create the focus handle.
	pub fn new(
		control: usize,
		clock: Entity<C>,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let frame_rate = clock.read(cx).frame_rate();

		// Poll the engine clock on a timer and reflect it locally.
		let this = cx.weak_entity();
		window
			.spawn(cx, async move |cx: &mut AsyncWindowContext| {
				loop {
					cx.background_executor()
						.timer(std::time::Duration::from_millis(16))
						.await;
					let _ = cx.update(|_window, app| {
						if let Some(this) = this.upgrade() {
							this.update(app, |this, cx| this.poll_clock(cx));
						}
					});
				}
			})
			.detach();

		Self {
			control,
			clock,
			frame_rate,
			transport: TransportState::new(),
			frame_source: None,
			cpu_image: None,
			focus_handle: cx.focus_handle(),
			safe_margins: SafeMargins::Off,
			zoom: ViewerZoom::Fit,
			show_fps: false,
			fps: FpsCounter::new(),
			picture_bounds: Rc::new(Cell::new(None)),
			eyedropper_armed: false,
			video_fullscreen: false,
			controls_visible: true,
			previous_zoom: None,
			last_pointer_move: None,
		}
	}

	pub fn set_video_fullscreen(&mut self, fullscreen: bool, cx: &mut Context<Self>) {
		if self.video_fullscreen == fullscreen {
			return;
		}
		self.video_fullscreen = fullscreen;
		if fullscreen {
			self.previous_zoom = Some(self.zoom);
			self.zoom = ViewerZoom::Level(4);
			self.controls_visible = true;
			self.last_pointer_move = Some(std::time::Instant::now());
		} else {
			self.zoom = self.previous_zoom.take().unwrap_or(ViewerZoom::Fit);
			self.controls_visible = true;
			self.last_pointer_move = None;
		}
		cx.notify();
	}

	/// The current transport state.
	pub fn transport(&self) -> TransportState {
		self.transport
	}

	/// Set the picture source (the bridge's pixel buffer) and repaint.
	pub fn set_frame_source(
		&mut self,
		source: Option<gpui_kit::SurfaceSource>,
		cx: &mut Context<Self>,
	) {
		self.frame_source = source.map(ViewerFrameSource::Surface);
		cx.notify();
	}

	/// Set the picture source to a CPU-side frame and repaint.
	///
	/// This is the path for non-macOS platforms and engines that decode to
	/// raw pixels instead of platform surfaces: hand in a
	/// [`RenderImage`](gpui_kit::RenderImage) whose bytes are BGRA8 (the same
	/// format gpui's `img` element uses) and the viewer uploads it through
	/// the sprite atlas. `None` clears the picture (showing the placeholder).
	///
	/// When a GPU frame was registered for the image's id (the 10-bit
	/// display path — the engine uploads the same pixels as an RGBA16F
	/// texture), the picture switches to that texture instead and skips the
	/// 8-bit sprite atlas entirely; the BGRA8 image is kept around as the
	/// CPU fallback for pixel-size and eyedropper consumers. Without a
	/// registered texture (no window/GPU, e.g. tests) the BGRA8 image is
	/// used, as before.
	pub fn set_cpu_frame(&mut self, frame: Option<Arc<RenderImage>>, cx: &mut Context<Self>) {
		self.cpu_image = frame.clone();
		self.frame_source = match frame {
			Some(image) => {
				// The 10-bit GPU-texture upgrade is only available where
				// `SurfaceSource::Texture` exists (linux/freebsd); elsewhere the
				// BGRA8 CPU frame is used as-is.
				#[cfg(any(target_os = "linux", target_os = "freebsd"))]
				let source = take_gpu_frame(image.id.0).map(|entry| {
					ViewerFrameSource::Surface(gpui_kit::SurfaceSource::Texture {
						texture: entry.texture,
						size: entry.size,
					})
				});
				#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
				let source: Option<ViewerFrameSource> = None;
				Some(source.unwrap_or(ViewerFrameSource::CpuFrame(image)))
			}
			None => None,
		};
		cx.notify();
	}

	/// The picture area's current bounds in window pixels, or `None` before
	/// the picture was first painted. The host uses these to convert the
	/// [`ViewerEvent::InteractPointer`] positions (picture-local) into the
	/// interact's viewport coordinates.
	pub fn picture_bounds(&self) -> Option<Bounds<Pixels>> {
		self.picture_bounds.get()
	}

	/// Whether the eyedropper is armed: the cursor becomes a crosshair and a
	/// click samples the pixel under it into a [`ViewerEvent::EyedropperPick`]
	/// instead of forwarding an interact pointer event.
	pub fn eyedropper_armed(&self) -> bool {
		self.eyedropper_armed
	}

	/// Arms or disarms the eyedropper and repaints when the state changes.
	///
	/// While armed, a click in the picture samples the pixel and emits
	/// [`ViewerEvent::EyedropperPick`] instead of forwarding an interact pointer
	/// event. An identical call is a no-op, so a host may poll this every frame.
	///
	/// # Arguments
	///
	/// * `armed` - `true` to arm the eyedropper, `false` to disarm it.
	/// * `cx` - Context notified only when the value changes.
	pub fn set_eyedropper_armed(&mut self, armed: bool, cx: &mut Context<Self>) {
		if self.eyedropper_armed == armed {
			return;
		}
		self.eyedropper_armed = armed;
		cx.notify();
	}

	/// The current safe-margin overlay state.
	pub fn safe_margins(&self) -> SafeMargins {
		self.safe_margins
	}

	/// Sets the safe-margin overlay and repaints.
	///
	/// # Arguments
	///
	/// * `margins` - Overlay state: off, the standard 90% x 80% margins, or a
	///   custom horizontal/vertical fraction pair.
	/// * `cx` - Context notified after the change.
	pub fn set_safe_margins(&mut self, margins: SafeMargins, cx: &mut Context<Self>) {
		self.safe_margins = margins;
		cx.notify();
	}

	/// The current zoom state.
	pub fn zoom(&self) -> ViewerZoom {
		self.zoom
	}

	/// Sets the picture zoom and repaints.
	///
	/// # Arguments
	///
	/// * `zoom` - [`ViewerZoom::Fit`] to fit the frame, or
	///   [`ViewerZoom::Level`]`(index)` for a fixed multiple of native
	///   resolution (index into [`VIEWER_ZOOM_LEVELS`]).
	/// * `cx` - Context notified after the change.
	pub fn set_zoom(&mut self, zoom: ViewerZoom, cx: &mut Context<Self>) {
		self.zoom = zoom;
		cx.notify();
	}

	/// Whether the frame-rate overlay is shown.
	pub fn show_fps(&self) -> bool {
		self.show_fps
	}

	/// Shows or hides the frame-rate overlay and repaints.
	///
	/// # Arguments
	///
	/// * `show` - `true` to draw the smoothed FPS counter over the picture.
	/// * `cx` - Context notified after the change.
	pub fn set_show_fps(&mut self, show: bool, cx: &mut Context<Self>) {
		self.show_fps = show;
		cx.notify();
	}

	/// The frame's native size in pixels when the source carries one (CPU
	/// frames), `None` for platform surfaces. Used to size the safe-frame
	/// overlay relative to the actual picture. On the 10-bit GPU-surface path
	/// the kept CPU image supplies the size.
	fn frame_pixel_size(&self) -> Option<(f32, f32)> {
		match &self.frame_source {
			Some(ViewerFrameSource::CpuFrame(image)) => {
				let size = image.size(0);
				Some((size.width.0 as f32, size.height.0 as f32))
			}
			_ => None,
		}
	}

	/// Emits a pointer event for the OFX interact forwarding path.
	///
	/// Converts the `Pixels` position to `f32` picture-local pixels and emits
	/// [`ViewerEvent::InteractPointer`] with the same kind, button and pressed
	/// state. The host forwards it to the active interact's pen actions.
	///
	/// # Arguments
	///
	/// * `kind` - Move, press or release.
	/// * `position` - Pointer position in the picture area's local pixels
	///   (top-left origin).
	/// * `button` - Button for a press/release, `None` for a move.
	/// * `pressed` - Whether a button is held (the pen-down state for a move).
	/// * `cx` - Context that receives the event.
	fn emit_interact_pointer(
		&mut self,
		kind: InteractPointerKind,
		position: Point<Pixels>,
		button: Option<MouseButton>,
		pressed: bool,
		cx: &mut Context<Self>,
	) {
		cx.emit(ViewerEvent::InteractPointer {
			kind,
			position: Point::new(f32::from(position.x), f32::from(position.y)),
			button,
			pressed,
		});
	}

	/// Sample the pixel under `position` (window pixels) from the current CPU
	/// frame and emit [`ViewerEvent::EyedropperPick`]. No-op without a CPU
	/// frame, when the frame carries no bytes, or when the point lands in the
	/// letterbox. Only called while armed, by the picture's mouse-down handler.
	/// The kept CPU image serves both the CPU-frame and the 10-bit GPU-surface
	/// paths (the surface has no CPU-readable bytes).
	fn sample_eyedropper(&self, position: Point<Pixels>, cx: &mut Context<Self>) {
		let Some(bounds) = self.picture_bounds.get() else {
			return;
		};
		let Some(image) = self.cpu_image.as_ref() else {
			return;
		};
		let size = image.size(0);
		let (w, h) = (size.width.0 as u32, size.height.0 as u32);
		let Some(bytes) = image.as_bytes(0) else {
			return;
		};
		let local = position - bounds.origin;
		let (box_w, box_h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
		let u = f32::from(local.x) / box_w;
		let v = f32::from(local.y) / box_h;
		let Some((uu, vv)) = contain_uv(w, h, box_w, box_h, u, v) else {
			return;
		};
		let Some(color) = sample_bgra8(bytes, w, h, uu, vv) else {
			return;
		};
		cx.emit(ViewerEvent::EyedropperPick { color });
	}

	fn poll_clock(&mut self, cx: &mut Context<Self>) {
		let clock = self.clock.read(cx);
		let frame = clock.current_frame();
		let playing = clock.is_playing();
		if self.video_fullscreen
			&& self
				.last_pointer_move
				.is_some_and(|last| last.elapsed().as_secs() >= 2)
		{
			self.controls_visible = false;
			cx.notify();
		}
		if frame != self.transport.frame || playing != self.transport.playing {
			self.fps.record(std::time::Instant::now());
			self.transport.frame = frame;
			self.transport.playing = playing;
			cx.notify();
		}
	}

	/// Emits `event` to the viewer's observers and schedules a repaint.
	///
	/// Used by the input and render paths so hosts receive pointer, key,
	/// eyedropper and playback events from a single place.
	///
	/// # Arguments
	///
	/// * `event` - Event to broadcast to [`ViewerEvent`] subscribers.
	/// * `cx` - Context that emits the event and is notified.
	fn emit(&mut self, event: ViewerEvent, cx: &mut Context<Self>) {
		cx.emit(event);
		cx.notify();
	}
}

/// Map a point inside a `box_w` x `box_h` widget to unit coordinates in an
/// `img_w` x `img_h` image laid out with `ObjectFit::Contain` (the `img`
/// default). Returns `None` when the point falls in the letterbox.
fn contain_uv(
	img_w: u32,
	img_h: u32,
	box_w: f32,
	box_h: f32,
	u: f32,
	v: f32,
) -> Option<(f32, f32)> {
	let (w, h) = (img_w as f32, img_h as f32);
	if w <= 0.0 || h <= 0.0 || box_w <= 0.0 || box_h <= 0.0 {
		return None;
	}
	let scale = (box_w / w).min(box_h / h);
	let cw = w * scale;
	let ch = h * scale;
	let ox = (box_w - cw) / 2.0;
	let oy = (box_h - ch) / 2.0;
	let px = u * box_w - ox;
	let py = v * box_h - oy;
	if px < 0.0 || py < 0.0 || px > cw || py > ch {
		return None;
	}
	Some((px / cw, py / ch))
}

/// Sample a BGRA8 frame (row-major, top-to-bottom — gpui's `img` byte order)
/// at unit coordinates (0..1 each); out-of-range coordinates clamp to the
/// nearest edge pixel. `None` for an empty frame or truncated bytes.
fn sample_bgra8(bytes: &[u8], w: u32, h: u32, u: f32, v: f32) -> Option<Rgba> {
	if w == 0 || h == 0 {
		return None;
	}
	let x = (u.clamp(0.0, 1.0) * (w - 1) as f32).round() as u32;
	let y = (v.clamp(0.0, 1.0) * (h - 1) as f32).round() as u32;
	let i = ((y * w + x) * 4) as usize;
	let px = bytes.get(i..i + 4)?;
	Some(Rgba {
		r: px[2] as f32 / 255.0,
		g: px[1] as f32 / 255.0,
		b: px[0] as f32 / 255.0,
		a: px[3] as f32 / 255.0,
	})
}

impl<C: PlaybackClock> EventEmitter<ViewerEvent> for ViewerWidget<C> {}

impl<C: PlaybackClock> Focusable for ViewerWidget<C> {
	fn focus_handle(&self, _cx: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

impl<C: PlaybackClock> Render for ViewerWidget<C> {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let colors = cx.default_colors().clone();
		let timecode =
			format_timecode(self.transport.frame, self.frame_rate, TimeDisplay::Timecode);

		// The picture area: surface (or placeholder), safe frames and zoom.
		let mut picture = div()
			.id("gpui-widgets-viewer-picture")
			.debug_selector(|| "gpui-widgets-viewer-picture".into())
			.flex_1()
			.min_w_0()
			.min_h_0()
			.overflow_hidden()
			.relative()
			.bg(gpui_kit::Hsla {
				h: 0.0,
				s: 0.0,
				l: 0.0,
				a: 1.0,
			})
			// The picture is focusable so keyboard events reach the OFX
			// interact forwarding while the user is interacting with the
			// picture (the panel decides what an active interact consumes).
			.focusable()
			.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
				if this.video_fullscreen {
					this.controls_visible = true;
					this.last_pointer_move = Some(std::time::Instant::now());
					cx.notify();
				}
				// While armed the pointer is a crosshair; the eyedropper
				// only reacts to a click, so moves are swallowed.
				if this.eyedropper_armed {
					return;
				}
				if let Some(bounds) = this.picture_bounds.get() {
					this.emit_interact_pointer(
						InteractPointerKind::Move,
						event.position - bounds.origin,
						None,
						event.pressed_button.is_some(),
						cx,
					);
				}
			}))
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(|this, event: &gpui_kit::MouseDownEvent, _window, cx| {
					if this.eyedropper_armed {
						this.sample_eyedropper(event.position, cx);
						return;
					}
					if let Some(bounds) = this.picture_bounds.get() {
						this.emit_interact_pointer(
							InteractPointerKind::Down,
							event.position - bounds.origin,
							Some(event.button),
							true,
							cx,
						);
					}
				}),
			)
			.on_mouse_up(
				MouseButton::Left,
				cx.listener(|this, event: &gpui_kit::MouseUpEvent, _window, cx| {
					if this.eyedropper_armed {
						return;
					}
					if let Some(bounds) = this.picture_bounds.get() {
						this.emit_interact_pointer(
							InteractPointerKind::Up,
							event.position - bounds.origin,
							Some(event.button),
							false,
							cx,
						);
					}
				}),
			)
			.on_key_down(cx.listener(|_this, event: &KeyDownEvent, _window, cx| {
				cx.emit(ViewerEvent::InteractKey {
					down: true,
					keystroke: event.keystroke.clone(),
				});
			}))
			.on_key_up(cx.listener(|_this, event: &KeyUpEvent, _window, cx| {
				cx.emit(ViewerEvent::InteractKey {
					down: false,
					keystroke: event.keystroke.clone(),
				});
			}));

		if self.eyedropper_armed {
			picture = picture.cursor_crosshair();
		}

		if let Some(source) = &self.frame_source {
			let picture_element: AnyElement = match self.zoom {
				// Fit renders the whole frame, contained inside the area.
				ViewerZoom::Fit => match source {
					ViewerFrameSource::Surface(surface_source) => {
						gpui_kit::surface(surface_source.clone())
							.size_full()
							.object_fit(ObjectFit::Contain)
							.into_any()
					}
					ViewerFrameSource::CpuFrame(image) => img(image.clone())
						.size_full()
						.min_w_0()
						.min_h_0()
						.object_fit(ObjectFit::Contain)
						.into_any(),
				},
				// A zoom level renders at a fixed multiple of the native
				// resolution. Only a CPU frame carries its pixel size; a
				// platform surface has no resolution here, so it falls back
				// to fit.
				ViewerZoom::Level(i) => match source {
					ViewerFrameSource::Surface(surface_source) => {
						gpui_kit::surface(surface_source.clone())
							.size_full()
							.object_fit(ObjectFit::Contain)
							.into_any()
					}
					ViewerFrameSource::CpuFrame(image) => {
						let scale = VIEWER_ZOOM_LEVELS[i];
						let size = image.size(0);
						div()
							.absolute()
							.left_0()
							.right_0()
							.top_0()
							.bottom_0()
							.flex()
							.items_center()
							.justify_center()
							.child(
								img(image.clone())
									.w(px(size.width.0 as f32 * scale))
									.h(px(size.height.0 as f32 * scale))
									.object_fit(ObjectFit::Fill),
							)
							.into_any()
					}
				},
			};
			picture = picture.child(picture_element);
		} else {
			picture = picture.child(
				div()
					.size_full()
					.flex()
					.items_center()
					.justify_center()
					.text_color(colors.disabled)
					.child(crate::widgets::i18n::tr(
						"viewer.no_frame_source",
						"No frame source",
					)),
			);
		}

		if let (Some((frame_w, frame_h)), Some((margin_w, margin_h))) =
			(self.frame_pixel_size(), self.safe_margins.ratio())
		{
			picture = picture.child(
				div()
					.absolute()
					.left_0()
					.right_0()
					.top_0()
					.bottom_0()
					.flex()
					.items_center()
					.justify_center()
					.child(
						div()
							.w(px(frame_w * margin_w))
							.h(px(frame_h * margin_h))
							.border_1()
							.border_color(colors.selected),
					),
			);
		}

		if self.show_fps {
			picture = picture.child(
				div()
					.absolute()
					.right_1()
					.top_1()
					.px_1()
					.py_0p5()
					.rounded_md()
					.bg(colors.container)
					.text_xs()
					.text_color(colors.text)
					.child(format!("{:.1} fps", self.fps.value())),
			);
		}

		// Capture the picture area's bounds each frame so the pointer
		// handlers above can map window coordinates to picture-local
		// pixels. The canvas paints nothing; it only records bounds.
		let picture_bounds = self.picture_bounds.clone();
		picture = picture.child(
			canvas(
				move |bounds, _window, _cx| {
					picture_bounds.set(Some(bounds));
				},
				|_bounds, (), _window, _cx| {},
			)
			.absolute()
			.left_0()
			.right_0()
			.top_0()
			.bottom_0(),
		);

		// Transport bar. The transport controls are icon buttons (16px icon
		// on a 24px hit target, localized tooltips); without a registered
		// icon resolver the buttons fall back to the glyph labels below.
		let playing = self.transport.playing;
		let play_label = if playing { "⏸" } else { "▶" };
		let in_icon = icons::path("prev", cx);
		let step_back_icon = icons::path("rew", cx);
		let play_icon = icons::path(if playing { "pause" } else { "play" }, cx);
		let step_forward_icon = icons::path("ff", cx);
		let out_icon = icons::path("next", cx);
		let transport_bar = div()
			.flex()
			.items_center()
			.gap_2()
			.px_2()
			.py_0p5()
			.bg(colors.container)
			.border_t_1()
			.border_color(colors.border)
			.child(transport_button(
				"gpui-widgets-viewer-in",
				in_icon,
				"⏮",
				crate::widgets::i18n::tr("viewer.in_point", "入点"),
				false,
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.emit(
						ViewerEvent::InPointRequested {
							control: this.control,
						},
						cx,
					);
				}),
			))
			.child(transport_button(
				"gpui-widgets-viewer-step-back",
				step_back_icon,
				"⏪",
				crate::widgets::i18n::tr("viewer.step_back", "上一帧"),
				false,
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.emit(
						ViewerEvent::StepRequested {
							control: this.control,
							delta: -1,
						},
						cx,
					);
				}),
			))
			.child(transport_button(
				"gpui-widgets-viewer-play",
				play_icon,
				play_label,
				if playing {
					crate::widgets::i18n::tr("viewer.pause", "暂停")
				} else {
					crate::widgets::i18n::tr("viewer.play", "播放")
				},
				true,
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					let event = if this.transport.playing {
						ViewerEvent::PauseRequested {
							control: this.control,
						}
					} else {
						ViewerEvent::PlayRequested {
							control: this.control,
						}
					};
					this.emit(event, cx);
				}),
			))
			.child(transport_button(
				"gpui-widgets-viewer-step-forward",
				step_forward_icon,
				"⏩",
				crate::widgets::i18n::tr("viewer.step_forward", "下一帧"),
				false,
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.emit(
						ViewerEvent::StepRequested {
							control: this.control,
							delta: 1,
						},
						cx,
					);
				}),
			))
			.child(transport_button(
				"gpui-widgets-viewer-out",
				out_icon,
				"⏭",
				crate::widgets::i18n::tr("viewer.out_point", "出点"),
				false,
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.emit(
						ViewerEvent::OutPointRequested {
							control: this.control,
						},
						cx,
					);
				}),
			))
			.child(transport_button(
				"gpui-widgets-viewer-clear-range",
				None,
				x_glyph(colors.text),
				crate::widgets::i18n::tr("viewer.clear_range", "清除入出点"),
				false,
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.emit(
						ViewerEvent::ClearRangeRequested {
							control: this.control,
						},
						cx,
					);
				}),
			))
			// The current timecode reads in the design's bright green.
			.child(
				div()
					.px_2()
					.text_xs()
					.text_color(gpui_kit::hsla(0.33, 0.75, 0.62, 1.0))
					.child(timecode),
			)
			.child(div().flex_1())
			.child(button(
				"gpui-widgets-viewer-safe",
				crate::widgets::i18n::tr("viewer.safe_frames", "安全框"),
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.safe_margins = this.safe_margins.next();
					this.emit(
						ViewerEvent::ToggleSafeFramesRequested {
							control: this.control,
						},
						cx,
					);
				}),
			))
			.child(button(
				"gpui-widgets-viewer-zoom",
				crate::widgets::i18n::tr("viewer.zoom", "缩放"),
				&colors,
				cx.listener(|this, _event: &ClickEvent, _window, cx| {
					this.zoom = this.zoom.next();
					this.emit(
						ViewerEvent::ToggleZoomRequested {
							control: this.control,
						},
						cx,
					);
				}),
			));

		let root = div()
			.size_full()
			.flex()
			.flex_col()
			.min_w_0()
			.overflow_hidden()
			.child(picture);
		if !self.video_fullscreen || self.controls_visible {
			if self.video_fullscreen {
				root.child(
					div()
						.absolute()
						.left_0()
						.right_0()
						.bottom_0()
						.child(transport_bar),
				)
				.into_any_element()
			} else {
				root.child(transport_bar).into_any_element()
			}
		} else {
			root.into_any_element()
		}
	}
}

/// A transport icon button: a 16px icon on a 24px hit target with a
/// localized tooltip. Falls back to the `fallback` glyph when `icon` is
/// `None` (no resolver registered, or no file for the name). `primary` marks
/// the design's accent-filled play/pause button; the rest are flat buttons
/// that surface on hover.
fn transport_button(
	id: &'static str,
	icon: Option<std::path::PathBuf>,
	fallback: impl IntoElement,
	tooltip: SharedString,
	primary: bool,
	colors: &gpui_kit::colors::Colors,
	on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
	let background = colors.selected;
	let hover = colors.container;
	let mut el = div()
		.id(id)
		.debug_selector(move || id.into())
		.w(px(22.0))
		.h(px(22.0))
		.flex()
		.items_center()
		.justify_center()
		.rounded_md()
		.cursor_pointer()
		.text_color(if primary {
			colors.selected_text
		} else {
			colors.text
		})
		.when(primary, |style| style.bg(background))
		.hover(move |style| {
			if primary {
				style.bg(background)
			} else {
				style.bg(hover)
			}
		})
		.tooltip(move |window, cx| tooltip_view(tooltip.clone(), window, cx))
		.on_click(on_click);
	if let Some(path) = icon {
		el = el.child(img(path).w(px(16.0)).h(px(16.0)));
	} else {
		el = el.child(fallback);
	}
	el
}

/// A small painted ✕ (clear-range / close), drawn with a canvas so it stays
/// crisp and theme-colored instead of relying on a font glyph that may
/// rasterize faintly or not at all.
fn x_glyph(color: gpui_kit::Rgba) -> impl IntoElement {
	use gpui_kit::{Bounds, PathBuilder, Pixels, canvas, point, px};

	canvas(
		move |_bounds, _window, _cx| (),
		move |bounds: Bounds<Pixels>, (), window, cx| {
			let _ = cx;
			// Two diagonal strokes across the 16px box, with a small inset so
			// the mark reads as a clean X.
			let inset = px(4.0);
			for stroke in [true, false] {
				let mut path = PathBuilder::stroke(px(1.5));
				let (x0, x1) = if stroke {
					(bounds.left() + inset, bounds.right() - inset)
				} else {
					(bounds.right() - inset, bounds.left() + inset)
				};
				path.move_to(point(x0, bounds.top() + inset));
				path.line_to(point(x1, bounds.bottom() - inset));
				if let Ok(path) = path.build() {
					window.paint_path(path, color);
				}
			}
		},
	)
}

/// A small labeled button, styled as the design's bordered chip (the "适合 /
/// 安全框" controls at the transport bar's right end).
fn button(
	id: &'static str,
	label: impl IntoElement,
	colors: &gpui_kit::colors::Colors,
	on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
	let border = colors.border;
	let hover = colors.separator;
	div()
		.id(id)
		.debug_selector(move || id.into())
		.px_2()
		.py_0p5()
		.rounded_md()
		.border_1()
		.border_color(border)
		.text_color(colors.text)
		.cursor_pointer()
		.hover(move |style| style.bg(hover))
		.on_click(on_click)
		.child(label)
}

#[cfg(test)]
mod tests;
