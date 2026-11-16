use super::*;

/// Command-line arguments the app accepts.
#[derive(Debug, Clone, Default)]
pub(crate) struct AppArgs {
	/// A project file to open at startup.
	pub(crate) project: Option<PathBuf>,
	/// Force the mock engine.
	pub(crate) mock: bool,
}

impl AppArgs {
	/// The pure parse step: `oakapp [project.ove] [--mock]`. The first
	/// positional wins; later positionals are ignored. `oak_engine` is
	/// the `OAK_ENGINE` override ("mock", case-insensitive). `--help` is
	/// skipped here — `from_env` handles it (print + exit).
	pub(crate) fn parse_args(
		argv: impl IntoIterator<Item = std::ffi::OsString>,
		oak_engine: Option<&str>,
	) -> Self {
		let mut args = AppArgs::default();
		for arg in argv {
			let text = arg.to_string_lossy();
			match text.as_ref() {
				"--mock" => args.mock = true,
				"--help" | "-h" => {}
				_ if args.project.is_none() => args.project = Some(arg.into()),
				other => println!("[app] ignoring unknown argument {other:?}"),
			}
		}
		if oak_engine
			.map(|v| v.eq_ignore_ascii_case("mock"))
			.unwrap_or(false)
		{
			args.mock = true;
		}
		args
	}

	/// Parses `std::env::args` plus the `OAK_ENGINE` override:
	/// `oakapp [project.ove] [--mock]`.
	pub(crate) fn from_env() -> Self {
		// --help prints and exits before the pure parse.
		if std::env::args_os()
			.skip(1)
			.any(|a| a == "--help" || a == "-h")
		{
			println!("oakapp — Oak Video Editor");
			println!("usage: oakapp [project.ove] [--mock]");
			println!("  --mock   use the mock engine (or set OAK_ENGINE=mock)");
			std::process::exit(0);
		}
		Self::parse_args(
			std::env::args_os().skip(1),
			std::env::var("OAK_ENGINE").ok().as_deref(),
		)
	}
}

/// Create the window's root view: a freshly constructed [`OakApp`] for the
/// `E` backend.
///
/// `window` is passed straight through to [`OakApp::new`], which needs it to
/// read window scale/theme state while building the shell; `initial` is the
/// project path from the command line (or `None`), opened only after the
/// layout is up; `cx` is the gpui app context used to mint the entity.
/// Returns the `Entity<OakApp<E>>` that the window (and [`run_with`]) keeps
/// as its root — a handle, so callers can later `update` it on the app
/// context.
///
/// Thin by design: the constructor runs inside `cx.new`, so the entity is
/// created immediately on the calling thread rather than lazily.
pub(crate) fn build_root<E: AppEngine>(
	window: &mut Window,
	initial: Option<PathBuf>,
	cx: &mut App,
) -> Entity<OakApp<E>> {
	cx.new(|cx| OakApp::new(window, initial, cx))
}

/// The crate entry point: applies the olive-dark theme and opens the main
/// window, running on the real engine by default (mock with `--mock` /
/// `OAK_ENGINE=mock` / the `mock-engine` feature).
pub fn run() {
	let args = AppArgs::from_env();
	let use_mock = args.mock || cfg!(feature = "mock-engine");
	if use_mock {
		run_with::<MockEngine>(args.clone());
	} else {
		run_with::<RealEngine>(args);
	}
}

/// Run the editor's main event loop on the `E` engine backend.
///
/// `args` is the parsed command line ([`AppArgs`]): its `project` path, when
/// present, is opened after the shell is built (and suppresses the startup
/// project manager). The function owns `AppArgs` by value because the gpui
/// closure must be `move`. `E` picks the engine implementation; [`run`]
/// selects [`MockEngine`] or [`RealEngine`].
///
/// Startup order inside the gpui closure (each step's comments explain why
/// it must happen here): install gpui-kit's theme, load `config.ini`, init
/// the UI language, configure the project library storage backend, bring up
/// the audio manager with the persisted devices, scan/register OFX plugins
/// (failures only log), surface the persisted display bit depth through
/// `OAK_DISPLAY_BIT_DEPTH` so the wgpu layer can pick the swapchain format
/// before the surface exists, then open a 1600x900 centered window with app
/// id `oak` and the embedded icon. In the window callback it declares the
/// display-colour mapping, registers the window's wgpu device on
/// Linux/FreeBSD (for the RGBA16F path), sets the compact 14px rem size and
/// builds the root entity with [`build_root`]. Afterwards it optionally
/// shows the project manager, starts the (deferred) update check, activates
/// the app, and on the last window close shuts OFX down, saves the config,
/// flushes the storage backlog and quits.
///
/// No `Result`: fatal startup failures panic (see below) and normal
/// termination is the event loop returning. Plugin, config and storage
/// problems degrade to logs.
///
/// # Panics
///
/// Panics with `the embedded app icon must decode` when the compiled-in
/// `assets/appicon/icon.png` is not a valid image, and with
/// `failed to open the main window` when the platform window cannot be
/// created. Also panics if the `OAK_DISPLAY_BIT_DEPTH` environment override
/// cannot be set (it is set before any thread/window exists, so the
/// `set_var` is sound in practice).
fn run_with<E: AppEngine>(args: AppArgs) {
	let initial = args.project.clone();
	gpui_kit::application()
		.with_assets(gpui_kit::assets::Assets)
		.run(move |cx: &mut App| {
			// gpui-kit's component layer (the dock chrome, the text inputs)
			// renders from its own theme global — install it before any window
			// exists. `apply_theme` (per window setup and the theme toggle)
			// maps the Oak palette onto it.
			gpui_kit::init(cx);
			// Load the persisted preferences (config.ini) before anything reads
			// them — the language, the theme, the storage backend and the audio
			// devices all come from the config store.
			crate::oakui::real::config_load();
			// Restore the persisted UI language (config `Language` key) before the
			// first window renders.
			crate::i18n::init();
			// M13 D4: enable the write-through project library (SQLite at the
			// default location) unless the user configured the backend
			// explicitly.
			crate::oakui::real::configure_storage();
			// Bring up the audio manager and apply the persisted device choices
			// (without an instance, playback pushes fail silently).
			crate::oakui::real::audio_init_from_config();
			// Stage 6b: wire the optional OFX plugin host — scan the standard
			// plugin paths, register every discovered plugin into the node
			// factory (effect library / add-effect menu), install the render
			// executor, and register the progress / viewer-time bridges. All
			// failures degrade to logs; plugins are optional.
			let plugin_count = crate::oakui::ofx::init();
			if plugin_count > 0 {
				println!("[ofx] registered {plugin_count} OFX plugin node type(s)");
			}
			// Display color management: the platform layer must know who maps
			// the pixels to the display (the single-mapping rule). Declared
			// per-window below, right after the window exists, and re-declared
			// whenever the preference changes (see displaycolor::apply_to_window).
			// Display bit depth: the wgpu window layer must pick the swapchain
			// format before the surface is created, so surface the persisted
			// choice through the process environment. Read by gpui_wgpu as
			// OAK_DISPLAY_BIT_DEPTH ("8" opts into the 8-bit pair; anything
			// else requests 10-bit).
			let bit_depth = quercus_engine_rs::core::DisplayBitDepth::from_config_string(
				&crate::oakui::real::config_get_string(
					quercus_engine_rs::core::CONFIG_KEY_DISPLAY_BIT_DEPTH,
				),
			);
			// SAFETY: single-threaded startup, before any window exists.
			unsafe { std::env::set_var("OAK_DISPLAY_BIT_DEPTH", bit_depth.to_config_string()) };
			cx.init_colors();
			let bounds = Bounds::centered(None, size(px(1600.0), px(900.0)), cx);
			let initial = initial.clone();
			let show_manager = initial.is_none();
			// The window/taskbar icon, per platform: X11 takes the pixels up
			// front (_NET_WM_ICON, decoded here from the embedded PNG);
			// Wayland resolves the icon from oak.desktop via the app id;
			// Windows reads it from the exe's icon resource (see build.rs).
			let icon =
				image::load_from_memory(include_bytes!("../../../../assets/appicon/icon.png"))
					.expect("the embedded app icon must decode")
					.into_rgba8();
			let mut root_slot = None;
			let window = cx
				.open_window(
					WindowOptions {
						window_bounds: Some(WindowBounds::Windowed(bounds)),
						app_id: Some("oak".into()),
						icon: Some(Arc::new(icon)),
						..Default::default()
					},
					|window, cx| {
						// Declare who maps this window's pixels to the display
						// before the first frame paints.
						crate::oakui::displaycolor::apply_to_window(window);
						// The 10-bit display path: hand the window's wgpu device
						// to the engine so it can upload RGBA16F textures the
						// renderer samples straight into the swapchain (no 8-bit
						// quantization). Linux/FreeBSD only — `gpu_context` is
						// `None` on other platforms, where the BGRA8 CPU path
						// stays in effect.
						#[cfg(any(target_os = "linux", target_os = "freebsd"))]
						if let Some(Ok(ctx)) = window
							.gpu_context()
							.map(|c| c.downcast::<(Arc<wgpu::Device>, Arc<wgpu::Queue>)>())
						{
							crate::oakui::gpu::register_context(ctx.0, ctx.1);
						}
						// Compact pro-app text metrics: gpui's default rem is
						// 16px (desktop-app large); 14px matches the design's
						// density. All rem-based text scales; px spacing is
						// unaffected.
						window.set_rem_size(px(14.0));
						let root = build_root::<E>(window, initial, cx);
						root_slot = Some(root.clone());
						root
					},
				)
				.expect("failed to open the main window");
			let _ = window;

			// No project on the command line: the DaVinci-style project manager
			// greets instead of an empty shell. Drives the ROOT ENTITY (not the
			// window handle): a window update borrows the window, and building a
			// modal inside it would re-enter it (spawn_modal needs a free
			// `update_window`).
			if show_manager {
				if let Some(root) = &root_slot {
					root.update(cx, |app, cx| app.show_project_manager(cx));
				}
			}

			// Startup update check (unless disabled in the preferences): the
			// fetch is async and the prompt defers behind the project manager
			// when that is up.
			if let Some(root) = &root_slot {
				root.update(cx, |app, cx| app.start_update_check(cx));
			}

			cx.activate(true);
			cx.on_window_closed(|cx, _| {
				if cx.windows().is_empty() {
					crate::oakui::ofx::shutdown();
					// Persist the preferences (config.ini), then drain the
					// write-through backlog (save + snapshot of every still-bound
					// project) and stop oakstorage's snapshot thread.
					crate::oakui::real::config_save();
					crate::oakui::real::storage_flush();
					cx.quit();
				}
			})
			.detach();
		});
}
