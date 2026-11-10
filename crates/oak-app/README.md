# oak-app

The Oak editor application: window shell, dockable panels, and the engine
gateway (`oakui`) with two backends — the mock engine (demo data) and the
real engine driving the `oak*` module crates directly.

Run it with `cargo run --bin oak-editor` (append `-- --mock` for the demo
engine, or a `.ove` path to open a project at startup).

## OPP/1 plugin host (T7.2 P1.2)

`src/opp/` wires the [OPP/1](../zh/plans/completed/external-plugin-protocol.md)
external-plugin protocol into the real engine: `src/opp/host.rs` implements
`oak_extensions::HostServices` (declared in the frozen
`crates/oak-extensions/src/host_services.rs`), `src/opp/dispatch.rs` maps one
wire request onto it, and `src/opp/discover.rs` finds and starts plugin
processes. `oak-extensions` never depends on `oak-app` — the trait is the
only contract between the two halves (see
`docs/zh/plans/opp-implementation-notes.md` §2).

### Plugin discovery

| Item | Value |
|---|---|
| Manifest filename | `plugin.toml` |
| Default search directory | `$OAK_HOME/plugins`, else `~/.oak/plugins` |
| Enable switch | `OAK_OPP_PLUGINS` — unset/empty = off (the GUI does not start plugins in P1.2) |
| `OAK_OPP_PLUGINS=1` (or `true`/`yes`) | scan the default directory |
| `OAK_OPP_PLUGINS=/path/a:/path/b` | scan exactly these `:`-separated directories |
| Layout | each plugin is a **child directory** of a search directory holding `plugin.toml`; the executable is resolved relative to that directory (`Manifest::resolve_command`) |

A search directory that itself contains `plugin.toml` is treated as a single
plugin. Invalid or unloadable manifests are skipped and reported through
`discover_with_errors` / the `failed` list of `start_enabled` — one broken
plugin never takes the host down. `start_enabled()` is the only entry point
P1.2 exposes; the GUI toggle arrives with P2.

### Wired methods (route into the engine)

| Domain | Method | Engine call |
|---|---|---|
| project | `get_info` | `graphops::project_name` + `sequence_ids` + `sequence_video_params` + `sequence_length` |
| project | `open` | `graphops::load_ove` — **does not enter the undo stack** (notes §1): the token serializes and gates confirmation only |
| project | `save` | `graphops::save_ove` |
| media | `list_footage` | `graphops::footage_ids` + `footage_duration_seconds` — **`path` is always `null`**: the per-clip filename (`clip_media_filename`, spec §1.3) is not wired yet, only id/name/duration come back |
| media | `import_footage` | `graphops::import_footage` |
| timeline | `get_structure` | `track_ids` + `clip_ids` + `clip_range` + `find_input_footage` |
| timeline | `add_track` | `graphops::add_track` (undoable `TimelineAddTrackCommand`) |
| timeline | `place_clip` | `graphops::place_footage_clip` (track id → kind/index, media in = 0). The wire carries no duration, so the clip length comes from the footage's probed duration; **when that cannot be probed the call fails with `INVALID_PARAMS` and a structured reason instead of guessing a length** |
| timeline | `split_clip` | `graphops::split_clip` (returns the left-half id) |
| timeline | `trim_clip` | `graphops::trim_clip` (omitted side keeps its position) |
| timeline | `move_clip` | `graphops::move_clip` / `move_clip_to_track` by whether the track changes |
| timeline | `ripple_delete` | `graphops::ripple_delete_clip` |
| node | `list_types` | `oak_node::factory::Factory::global().entries()` — **`category` is the `Category` enum's `Debug` form** (e.g. `Effect`), not a localized display label; `label` is the node's display name |
| node | `get_params` | `effectchain::effect_params` |
| render | `get_frame` | `renderops::render_sequence_frame` → PNG (`inline_base64`); sequence branch only, `CpuF32` backend only |
| render | `get_thumbnails` | loop of `render_sequence_frame` at each clip's in-point, capped at 64 |
| render | `get_audio_levels` | `renderops::render_audio_range` → peak buckets |
| edit | `begin` / `commit` / `abort` | `oak_undo::global::group_begin` / `group_end` / `group_abort` (single-holder token) |
| edit | `undo` / `redo` | `oak_undo::global::undo` / `redo` |

`events.subscribe` / `events.unsubscribe` / `session.*` are served by the P1
protocol handler in `oak-extensions` (`opp::dispatch` falls through to it);
an `events.*` call that reaches `HostServices` instead means the dispatch
layer did not run.

### Not wired (structured `Unsupported`, never a fake success)

Every method below answers `METHOD_NOT_FOUND` with
`data = {"unsupported": true, "method": …, "reason": …, "spec": …}`:

| Method | Why it is not wired |
|---|---|
| `media.probe` | no standalone probe entry point in this crate; spec §1.3 lands it by calling `oak_codec` directly |
| `timeline.delete_clip` | only the ripple form is mapped; the non-ripple function is unconfirmed (spec §1.4) |
| `timeline.add_transition` | needs the opposite block of the seam; the inference rule (`GAP_TRANSITION_SEAM`) is undefined |
| `timeline.add_marker`, `timeline.set_workarea` | `marker_list_create`/`workarea_create` only mint handles; persistence (sidecar vs graph node) is an open deviation item (notes §6) |
| `node.add_effect`, `node.set_param` | engine setters take a gpui `Context` (UI thread); the host runs on the IO thread (notes §3) |
| `node.set_keyframe` | `set_value_at_time_command` needs an engine `NodeValue` + element index; the wire's JSON value is not mapped |
| `node.remove` | `effectchain::remove` needs the chain's host node; the wire passes only the effect id |
| `render.get_frame` (footage branch, shm/GPU frames) | footage decode lives outside this crate; reading shm slots/GPU textures needs the attachment path P1.2 does not have |
| `export.start`, `export.cancel` | `ExportSession` carries a non-`Send` event receiver and `EncodingParams` has no `Default`; both need the notes §3 event pump |
| `events.subscribe` / `events.unsubscribe` | handled at the dispatch layer by the P1 session |

### Tests

```bash
cargo test --offline -p oak-app opp::
```

- `timeline_add_track_reaches_the_engine` — a method really reaches the engine
  (structure grows, the undo row lands **after** `edit_commit`, because an
  open group does not yet appear in `count()`);
- `unmapped_method_answers_structured_unsupported` — unmapped methods return
  the structured `Unsupported`;
- `project_open_never_enters_the_undo_stack` — notes §1: save + open run
  inside one transaction (both are confirmation-class) and neither pushes an
  undo row, nor does the committing empty group;
- `transaction_is_single_holder` — protocol §6 rule 1.

All four take the crate-wide `graphops::test_lock()` and zero the global undo
stack first (`oak_undo` is process-global), matching what the `graphops`,
`effectchain` and `displaycolor` tests already do — so a plain
`cargo test --offline -p oak-app` run is safe without extra threading flags.

Linking these tests needs the local `cc` wrapper when the system
`libfontconfig.a` (non-PIC) would otherwise win the `-L` ordering race; see
the "Environment requirements" section of [`ai/oak-ai-tools/README.md`](../../ai/oak-ai-tools/README.md).
