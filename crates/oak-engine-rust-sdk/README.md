# oak-engine-rust-sdk

Safe Rust SDK for the Oak video engine.

The SDK is a thin, owned-value layer over the engine's stable C ABI: it links
`oak-engine` as a cdylib (`#[link(name = "oak_engine")]`) and never uses the
engine's Rust API. A program that depends on this crate (and indirectly on
`oak-engine`) can build a complete non-linear editor without touching any
other Oak crate.

```toml
[dependencies]
oak-engine-rust-sdk = "0.5"
```

## Quick start

```rust,ignore
use oak_engine_rust_sdk::editor::{EditorSession, TrackKind};
use oak_engine_rust_sdk::edit::EditCommand;

let session = EditorSession::new()?;                 // a project in memory
let sequence = session.create_sequence("Demo")?;     // default V1/V2/A1/A2 tracks
let footage = session
    .execute(EditCommand::ImportFootage { path: "clip.mp4".into() })?
    .primary
    .expect("imported footage");
let clip = session
    .execute(EditCommand::PlaceFootageClip {
        footage,
        kind: TrackKind::Video,
        track_index: 0,
        in_ts: 0,
        out_ts: 50,
        media_in_ts: 0,
    })?
    .primary
    .expect("placed clip");
session.save("demo.ove")?;
```

A complete workflow (import → edit → effects/keyframes → preview/audio →
undo/redo → save/reopen → export) lives in
[`examples/sdk_editor`](../../examples/sdk_editor). Its only Oak dependency is
this crate.

## Lifecycle

- **`EditorSession`** owns a project, its current sequence, the clipboard,
  markers and the work area. Create one with `new`, `open`, or
  `open_project_from_library`; `from_project` wraps an existing `Project`
  handle (the session and the handle then refer to the same document).
- `EditorSession::close` releases engine-side session state; `Drop` does the
  same. Dropping a session does not invalidate `Project` handles obtained
  from it, so background work may keep a project alive safely.
- One process may create several sessions over the same project (e.g. a
  program and a source viewer). Edits are serialized by the engine.
- Call `close` (or drop the session) before tearing down the engine. The
  engine's global undo stack is shared by all sessions by design.

## Ownership

Every value returned across the ABI owns its data:

- `Project`, `PreviewFrame`, `AudioBlock`, `VideoFrame`, `Thumbnail`,
  `Waveform`, `Curve`, `ParameterSnapshot` and `GraphSnapshot` are RAII
  handles or owned copies. `Clone` adds a reference where the engine
  supports it; `Drop` releases exactly one.
- Frame and audio buffers are copied out of engine memory at the boundary.
  No pointer returned by this crate borrows engine memory.
- Task results (`take_project`, `take_video`, `take_audio`,
  `take_thumbnail`, `take_waveform`) can be transferred **once**. The second
  `take` returns `None`.
- `Task`, `TaskCancellation` and `PreviewFrameRequest` are `Send`; a worker
  thread may own them. Do not block a UI thread on `wait` when a poll will
  do.
- `Project` and `EditorSession` are `Send + Sync` handles. `NodeId` is a
  stable packed token; identities are **not** preserved across save/reopen,
  so resolve nodes from a snapshot after loading.

## Threading

- Rendering, audio mixing and export run on engine workers. The SDK never
  silently renders on the caller's thread: `Task::wait` blocks the caller,
  and `PreviewSession::request_frame`/`request_audio` return pollable
  requests.
- Poll task state from the UI (`Task::state`, `progress`, `title`), and
  transfer results when they succeed.
- Callbacks (`edit` subscriptions, undo observers, OFX progress) are invoked
  on the engine thread that fires them; the SDK serializes Rust callbacks
  and catches panics. Keep callbacks short and never re-enter the engine
  from one.
- A preview generation (`PreviewSession::generation`) identifies stale
  results after seek/invalidate. Discard frames whose generation no longer
  matches.

## Errors

- Fallible calls return `Result<T, String>` or `Result<T, StorageError>`.
  The string is the engine's human-readable error; stable numeric codes are
  not part of the Rust surface yet.
- Queries return `None`/empty for stale ids instead of panicking.
- Null/invalid handles fail closed: a dropped engine or session yields
  errors rather than undefined behavior.

## Tasks and events

| API | Purpose |
|---|---|
| `task::export` | encode a sequence to a file; `Task::cancel` cooperatively stops it |
| `task::render_video` / `render_playback_video` / `render_track_video` | owned preview frames |
| `task::render_audio` | owned audio blocks |
| `task::extract_waveform` | waveform peaks |
| `task::render_thumbnail` | RGBA thumbnail |
| `task::load_otio` / `save_otio` | OTIO/FCPXML interchange |
| `editor::EditorSession::subscribe_edit` | per-command notifications |
| `node::add_undo_observer` | undo-stack changes |
| `ofx::set_progress_handler` | plugin progress events |

## C ABI

The C header is generated from this crate's extern declarations and the
`#[repr(C)]` mirrors:

- `crates/oak-engine/include/oak_engine.h`
- regenerate with `python3 tooling/gen_c_header.py --write`
- CI checks it with `python3 tooling/gen_c_header.py --check`

Layouts of the common mirrors are asserted by `_Static_assert` in the header.

## Publishing

`oak-engine` must be published before this crate (the SDK declares a
versioned path dependency). See
[`docs/zh/sdk-guide.md`](../../docs/zh/sdk-guide.md#publishing) for the
release order and `cargo package` checks.

## License

GPL-3.0-or-later.
