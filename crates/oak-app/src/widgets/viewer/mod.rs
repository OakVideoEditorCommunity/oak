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
	EventEmitter, FocusHandle, Focusable, Keystroke, KeyDownEvent, KeyUpEvent, MouseButton,
	MouseMoveEvent, ObjectFit, Point, Pixels, Render, RenderImage, Rgba, SharedString, Size,
	Window, canvas, colors::DefaultColors, div, img, prelude::*, px,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::widgets::{icons, tooltip::tooltip_view};

/// One GPU-backed viewer frame: the engine's F32 pipeline output uploaded
/// as an RGBA16F texture (see `crates/oak-app/src/oakui/gpu.rs`), indexed by
/// the `RenderImage` id it replaces. The texture carries the 10-bit content
/// that `RenderImage`'s BGRA8 bytes cannot, so the viewer samples it
/// straight into a 10-bit swapchain instead of going through the 8-bit
/// sprite atlas.
#[derive(Clone)]
#[cfg_attr(
	not(any(target_os = "linux", target_os = "freebsd")),
	allow(dead_code)
)]
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
		frames.insert(
			image_id,
			GpuFrameEntry {
				texture,
				size,
				seq,
			},
		);
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
	/// The persisted config value (`0`/`1`/`2`).
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
pub const VIEWER_ZOOM_LEVELS: [f32; 10] = [
	0.10, 0.25, 0.50, 0.75, 1.00, 1.25, 1.50, 2.00, 4.00, 8.00,
];

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

	/// Record a new frame at `now` and update the smoothed rate.
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
}

impl<C: PlaybackClock> ViewerWidget<C> {
	/// Create a viewer driven by `clock`.
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
		}
	}

	/// The current transport state.
	pub fn transport(&self) -> TransportState {
		self.transport
	}

	/// Set the picture source (the bridge's pixel buffer) and repaint.
	pub fn set_frame_source(&mut self, source: Option<gpui_kit::SurfaceSource>, cx: &mut Context<Self>) {
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

	/// Arm / disarm the eyedropper and repaint. No-op when unchanged, so
	/// calling this every frame from a host poll is cheap.
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

	/// Set the safe-margin overlay state and repaint.
	pub fn set_safe_margins(&mut self, margins: SafeMargins, cx: &mut Context<Self>) {
		self.safe_margins = margins;
		cx.notify();
	}

	/// The current zoom state.
	pub fn zoom(&self) -> ViewerZoom {
		self.zoom
	}

	/// Set the zoom state and repaint.
	pub fn set_zoom(&mut self, zoom: ViewerZoom, cx: &mut Context<Self>) {
		self.zoom = zoom;
		cx.notify();
	}

	/// Whether the frame-rate overlay is shown.
	pub fn show_fps(&self) -> bool {
		self.show_fps
	}

	/// Toggle the frame-rate overlay and repaint.
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
		if frame != self.transport.frame || playing != self.transport.playing {
			self.fps.record(std::time::Instant::now());
			self.transport.frame = frame;
			self.transport.playing = playing;
			cx.notify();
		}
	}

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
					ViewerFrameSource::Surface(surface_source) => gpui_kit::surface(surface_source.clone())
						.size_full()
						.object_fit(ObjectFit::Contain)
						.into_any(),
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
					ViewerFrameSource::Surface(surface_source) => gpui_kit::surface(surface_source.clone())
						.size_full()
						.object_fit(ObjectFit::Contain)
						.into_any(),
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
					.child(crate::widgets::i18n::tr("viewer.no_frame_source", "No frame source")),
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

		div()
			.size_full()
			.flex()
			.flex_col()
			.min_w_0()
			.overflow_hidden()
			.child(picture)
			.child(transport_bar)
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
mod tests {
	use super::*;
	use crate::widgets::timeline::{Frame, FrameRate};
	use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, px, size};

	struct MockClock {
		frame: Frame,
		playing: bool,
	}
	impl PlaybackClock for MockClock {
		fn current_frame(&self) -> Frame {
			self.frame
		}
		fn is_playing(&self) -> bool {
			self.playing
		}
		fn frame_rate(&self) -> FrameRate {
			FrameRate::new(30, 1)
		}
	}

	struct Host {
		viewer: Entity<ViewerWidget<MockClock>>,
		events: Vec<ViewerEvent>,
	}
	impl Render for Host {
		fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
			div().size_full().child(self.viewer.clone())
		}
	}

	#[gpui_kit::test]
	async fn play_button_emits_play_request(cx: &mut TestAppContext) {
		let (cx, host) = make_host(cx);
		// The play button sits on the left of the transport bar at the bottom.
		let play = cx
			.debug_bounds("gpui-widgets-viewer-play")
			.expect("play button rendered");
		cx.simulate_click(play.center(), Modifiers::none());
		cx.run_until_parked();

		let requested = cx.read(|app| {
			host.read(app)
				.events
				.iter()
				.any(|e| matches!(e, ViewerEvent::PlayRequested { control: 1 }))
		});
		assert!(requested, "expected a PlayRequested event");
	}

	#[test]
	fn timecode_formatting_reuses_timeline() {
		let frame = Frame(3000);
		let text = format_timecode(frame, FrameRate::new(30, 1), TimeDisplay::Timecode);
		assert_eq!(text, "00:01:40:00");
	}

	#[gpui_kit::test]
	async fn cpu_frame_source_renders_without_a_platform_surface(cx: &mut TestAppContext) {
		// The CPU-frame path (for non-macOS platforms without CVPixelBuffer)
		// accepts raw BGRA8 bytes in a RenderImage and renders through the
		// sprite atlas — no SurfaceSource involved.
		use gpui_kit::RenderImage;
		use image::{Frame, RgbaImage};

		// A 2x2 opaque red frame, converted RGBA -> BGRA as gpui expects.
		let mut rgba = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
		for pixel in rgba.chunks_exact_mut(4) {
			pixel.swap(0, 2);
		}
		let frame = Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(
			Frame::new(rgba),
			1,
		)));

		let (cx, host) = make_host(cx);
		cx.update(|window, app| {
			host.read(app)
				.viewer
				.clone()
				.update(app, |viewer, cx| viewer.set_cpu_frame(Some(frame), cx));
			let _ = window.draw(app);
		});
		cx.run_until_parked();

		let is_cpu = cx.read(|app| {
			matches!(
				host.read(app).viewer.read(app).frame_source,
				Some(ViewerFrameSource::CpuFrame(_))
			)
		});
		assert!(is_cpu, "the frame source should be the CPU-frame variant");

		// Clearing the CPU frame falls back to the placeholder.
		cx.update(|window, app| {
			host.read(app)
				.viewer
				.clone()
				.update(app, |viewer, cx| viewer.set_cpu_frame(None, cx));
			let _ = window.draw(app);
		});
		cx.run_until_parked();
		let is_none = cx.read(|app| host.read(app).viewer.read(app).frame_source.is_none());
		assert!(is_none);
	}

	// The surface path exists only where `SurfaceSource::Texture` does
	// (Linux/FreeBSD); on macOS/Windows a registered GPU frame stays on
	// the CPU-frame path by design.
	#[cfg(any(target_os = "linux", target_os = "freebsd"))]
	#[gpui_kit::test]
	async fn registered_gpu_frame_switches_to_surface(cx: &mut TestAppContext) {
		// The 10-bit display path: the engine registers an RGBA16F texture
		// under the RenderImage's id, so handing that image to the viewer
		// takes the surface path (sampled straight into the swapchain) —
		// the BGRA8 bytes are only the CPU fallback. Any Send+Sync value
		// stands in for the wgpu texture here; the registry never touches
		// its contents (headless tests have no GPU).
		use gpui_kit::{DevicePixels, RenderImage, Size};
		use image::{Frame, RgbaImage};

		let rgba = RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]));
		let frame = Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(
			Frame::new(rgba),
			1,
		)));
		register_gpu_frame(
			frame.id.0,
			Arc::new(0u32),
			Size {
				width: DevicePixels::from(2),
				height: DevicePixels::from(2),
			},
		);

		let (cx, host) = make_host(cx);
		cx.update(|window, app| {
			host.read(app)
				.viewer
				.clone()
				.update(app, |viewer, cx| viewer.set_cpu_frame(Some(frame), cx));
			let _ = window.draw(app);
		});
		cx.run_until_parked();

		let is_surface = cx.read(|app| {
			matches!(
				host.read(app).viewer.read(app).frame_source,
				Some(ViewerFrameSource::Surface(_))
			)
		});
		assert!(is_surface, "a registered GPU frame must take the surface path");

		// Clearing the CPU frame drops the surface source too.
		cx.update(|window, app| {
			host.read(app)
				.viewer
				.clone()
				.update(app, |viewer, cx| viewer.set_cpu_frame(None, cx));
			let _ = window.draw(app);
		});
		cx.run_until_parked();
		let is_none = cx.read(|app| host.read(app).viewer.read(app).frame_source.is_none());
		assert!(is_none);
	}

	#[test]
	fn contain_uv_maps_letterbox() {
		// A 4x3 image inside a 72x48 box: scale = min(18, 16) = 16, so the
		// content is 64x48, letterboxed by 4px on each side.
		assert_eq!(contain_uv(4, 3, 72.0, 48.0, 0.5, 0.5), Some((0.5, 0.5)));
		assert_eq!(contain_uv(4, 3, 72.0, 48.0, 0.0, 0.5), None, "left letterbox");
		assert_eq!(contain_uv(4, 3, 72.0, 48.0, 1.0, 0.5), None, "right letterbox");
		// The content's right edge sits at u = 4/72 + 64/72.
		let right = contain_uv(4, 3, 72.0, 48.0, 68.0 / 72.0, 0.5).unwrap();
		assert!((right.0 - 1.0).abs() < 1e-6 && (right.1 - 0.5).abs() < 1e-6);
		// Degenerate boxes / images sample nothing.
		assert_eq!(contain_uv(0, 3, 72.0, 48.0, 0.5, 0.5), None);
		assert_eq!(contain_uv(4, 3, 0.0, 48.0, 0.5, 0.5), None);
	}

	#[test]
	fn sample_bgra8_reads_corners() {
		// A 4x2 BGRA frame: red top-left, green top-right, blue bottom-left,
		// white/128 bottom-right.
		let mut bytes = vec![0u8; 4 * 2 * 4];
		let put = |bytes: &mut [u8], x: u32, y: u32, b: u8, g: u8, r: u8, a: u8| {
			let i = ((y * 4 + x) * 4) as usize;
			bytes[i..i + 4].copy_from_slice(&[b, g, r, a]);
		};
		put(&mut bytes, 0, 0, 0, 0, 255, 255); // red
		put(&mut bytes, 3, 0, 0, 255, 0, 255); // green
		put(&mut bytes, 0, 1, 255, 0, 0, 255); // blue
		put(&mut bytes, 3, 1, 255, 255, 255, 128); // white half-alpha

		let close = |a: Rgba, b: Rgba| {
			(a.r - b.r).abs() < 1e-6
				&& (a.g - b.g).abs() < 1e-6
				&& (a.b - b.b).abs() < 1e-6
				&& (a.a - b.a).abs() < 1e-6
		};

		let red = Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
		let green = Rgba { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
		let blue = Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
		let white_half = Rgba { r: 1.0, g: 1.0, b: 1.0, a: 128.0 / 255.0 };
		assert!(close(sample_bgra8(&bytes, 4, 2, 0.0, 0.0).unwrap(), red), "top-left");
		assert!(close(sample_bgra8(&bytes, 4, 2, 1.0, 0.0).unwrap(), green), "top-right");
		assert!(close(sample_bgra8(&bytes, 4, 2, 0.0, 1.0).unwrap(), blue), "bottom-left");
		assert!(
			close(sample_bgra8(&bytes, 4, 2, 1.0, 1.0).unwrap(), white_half),
			"bottom-right translucent"
		);

		// Out-of-range coordinates clamp to the nearest edge pixel.
		assert!(close(sample_bgra8(&bytes, 4, 2, 2.0, -0.5).unwrap(), green), "clamps u/v");
		assert!(close(sample_bgra8(&bytes, 4, 2, -1.0, 2.0).unwrap(), blue), "clamps negative");
		// (0.5, 0.5) rounds to pixel (2, 1), which was never set (transparent
		// black) — exercises the rounding path.
		let mid = sample_bgra8(&bytes, 4, 2, 0.5, 0.5).unwrap();
		assert!(mid.r.abs() < 1e-6 && mid.a.abs() < 1e-6, "middle pixel unset");
		// Truncated bytes sample nothing: pixel (2, 1) needs bytes 24..28, so
		// a 27-byte buffer leaves the range short.
		assert!(sample_bgra8(&bytes[..27], 4, 2, 0.5, 0.5).is_none());
	}

	/// While armed, clicking the picture samples the pixel under the cursor
	/// into an [`ViewerEvent::EyedropperPick`] and swallows the interact
	/// pointer events.
	#[gpui_kit::test]
	async fn armed_eyedropper_click_samples_the_picture(cx: &mut TestAppContext) {
		use gpui_kit::RenderImage;
		use image::{Frame, RgbaImage};

		// A 2x2 frame: red / green / blue / white corners, RGBA -> BGRA as
		// gpui expects.
		let mut rgba = RgbaImage::new(2, 2);
		rgba.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
		rgba.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
		rgba.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
		rgba.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
		for pixel in rgba.chunks_exact_mut(4) {
			pixel.swap(0, 2);
		}
		let frame = Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(
			Frame::new(rgba),
			1,
		)));

		let (cx, host) = make_host(cx);
		cx.update(|window, app| {
			host.read(app)
				.viewer
				.clone()
				.update(app, |viewer, cx| {
					viewer.set_cpu_frame(Some(frame), cx);
					viewer.set_eyedropper_armed(true, cx);
				});
			let _ = window.draw(app);
		});
		cx.run_until_parked();

		// The 2x2 square is contained in the (wide) picture area, so the
		// picture centre maps to image pixel (1, 1) = white.
		let picture = cx
			.debug_bounds("gpui-widgets-viewer-picture")
			.expect("picture painted");
		cx.simulate_click(picture.center(), Modifiers::none());
		cx.run_until_parked();

		let (pick, no_interact) = cx.read(|app| {
			let events = &host.read(app).events;
			(
				events.iter().find_map(|e| match e {
					ViewerEvent::EyedropperPick { color } => Some(*color),
					_ => None,
				}),
				!events
					.iter()
					.any(|e| matches!(e, ViewerEvent::InteractPointer { .. })),
			)
		});
		let pick = pick.expect("an EyedropperPick should be emitted");
		assert!(
			(pick.r - 1.0).abs() < 1e-6 && (pick.g - 1.0).abs() < 1e-6 && (pick.b - 1.0).abs() < 1e-6,
			"the picture centre should sample white, got {pick:?}"
		);
		assert!(no_interact, "an armed click must not forward interact pointers");
	}

	fn make_host(cx: &mut TestAppContext) -> (&'static mut VisualTestContext, Entity<Host>) {
		cx.update(|cx| cx.init_colors());
		let window = cx.open_window(size(px(640.0), px(420.0)), |window, cx| {
			let clock = cx.new(|_| MockClock {
				frame: Frame(0),
				playing: false,
			});
			let viewer = cx.new(|cx| ViewerWidget::new(1, clock, window, cx));
			let host = Host {
				viewer,
				events: Vec::new(),
			};
			cx.subscribe(
				&host.viewer,
				|host: &mut Host,
				 _v: Entity<ViewerWidget<MockClock>>,
				 event: &ViewerEvent,
				 _cx: &mut Context<Host>| {
					host.events.push(event.clone());
				},
			)
			.detach();
			host
		});
		cx.run_until_parked();
		let host = window.root(cx).unwrap();
		let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
		(cx, host)
	}

	#[gpui_kit::test]
	async fn step_button_emits_step_request(cx: &mut TestAppContext) {
		let (cx, host) = make_host(cx);
		let step = cx
			.debug_bounds("gpui-widgets-viewer-step-forward")
			.expect("step button rendered");
		cx.simulate_click(step.center(), Modifiers::none());
		cx.run_until_parked();

		let requested = cx.read(|app| {
			host.read(app)
				.events
				.iter()
				.any(|e| matches!(e, ViewerEvent::StepRequested { delta: 1, .. }))
		});
		assert!(requested, "expected a StepRequested(+1) event");
	}

	#[gpui_kit::test]
	async fn safe_frame_toggle_emits_and_switches(cx: &mut TestAppContext) {
		let (cx, host) = make_host(cx);
		let toggle = cx
			.debug_bounds("gpui-widgets-viewer-safe")
			.expect("safe-frame button rendered");
		cx.simulate_click(toggle.center(), Modifiers::none());
		cx.run_until_parked();

		let (requested, shown) = cx.read(|app| {
			let host = host.read(app);
			(
				host.events
					.iter()
					.any(|e| matches!(e, ViewerEvent::ToggleSafeFramesRequested { .. })),
				host.viewer.read(app).safe_margins,
			)
		});
		assert!(requested, "expected a ToggleSafeFramesRequested event");
		assert_eq!(shown, SafeMargins::On, "safe margins should now be shown");
	}

	#[test]
	fn zoom_cycles_fit_through_levels() {
		// The toolbar zoom button cycles Fit → 100% → 200% → Fit.
		assert_eq!(ViewerZoom::Fit.next(), ViewerZoom::Level(4));
		assert_eq!(ViewerZoom::Level(4).next(), ViewerZoom::Level(5));
		assert_eq!(ViewerZoom::Level(3).next(), ViewerZoom::Level(7));
		assert_eq!(ViewerZoom::Level(7).next(), ViewerZoom::Level(8));
		assert_eq!(
			ViewerZoom::Level(VIEWER_ZOOM_LEVELS.len() - 1).next(),
			ViewerZoom::Fit
		);
	}

	#[test]
	fn safe_margins_cycle_and_ratio() {
		assert_eq!(SafeMargins::Off.next(), SafeMargins::On);
		assert_eq!(SafeMargins::On.next(), SafeMargins::Off);
		assert_eq!(SafeMargins::Custom(0.5, 0.5).next(), SafeMargins::Off);
		assert_eq!(SafeMargins::Off.ratio(), None);
		assert_eq!(SafeMargins::On.ratio(), Some((0.9, 0.8)));
		assert_eq!(SafeMargins::Custom(0.7, 0.6).ratio(), Some((0.7, 0.6)));
	}

	#[test]
	fn waveform_mode_config_round_trips() {
		assert_eq!(WaveformMode::Automatic.config_value(), 0);
		assert_eq!(WaveformMode::Only.config_value(), 1);
		assert_eq!(WaveformMode::Both.config_value(), 2);
		assert_eq!(WaveformMode::from_config_value(0), WaveformMode::Automatic);
		assert_eq!(WaveformMode::from_config_value(1), WaveformMode::Only);
		assert_eq!(WaveformMode::from_config_value(2), WaveformMode::Both);
		// Out-of-range values clamp to the nearest valid mode.
		assert_eq!(WaveformMode::from_config_value(-5), WaveformMode::Automatic);
		assert_eq!(WaveformMode::from_config_value(9), WaveformMode::Both);
	}

	#[test]
	fn fps_counter_smooths_measured_rate() {
		let mut counter = FpsCounter::new();
		let t0 = std::time::Instant::now();
		counter.record(t0);
		assert_eq!(counter.value(), 0.0, "no rate before a second sample");
		// 100ms apart => 10 fps instant.
		counter.record(t0 + std::time::Duration::from_millis(100));
		assert!((counter.value() - 10.0).abs() < 1.0);
		counter.record(t0 + std::time::Duration::from_millis(200));
		assert!((counter.value() - 10.0).abs() < 1.5);
	}
}
