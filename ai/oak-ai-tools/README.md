# oak-ai-tools

The T1 **tool execution layer** of the Oak AI editing pipeline
(`docs/zh/plans/ai-editing-feature-plan.md`, milestone T1).

It turns the frozen T0 tool face (`ai/schema/tools/*.json`, 22 tools) into
callable operations:

```
model tool call
   → schema validation (ai/schema/tools/*.json, draft-07)
   → typed argument parsing (rational seconds / frame index only)
   → one undo group per call (begin → invoke → commit / abort)
   → engine backend (mock or the real SDK)
   → result, or a structured {code, message, tool, step[, gap][, txn]} error
```

Two invariants drive the design:

* **one tool call = one undo group**, and a failed call changes nothing
  (the dispatcher aborts, the backend validates before mutating);
* **no fake success**: a backend that cannot perform an operation returns
  `UNSUPPORTED` with a stable gap id instead of an empty or invented result
  (plan §5 iron rules).

## Layout

| path | contents |
|---|---|
| `src/schema.rs` | compiles the frozen JSON Schemas, validates arguments |
| `src/timecode.rs` | the two accepted time shapes (`{num, den}` rational seconds, integer frame index), rejected bare floats |
| `src/tools/*` | typed argument parsers shared by both backends |
| `src/dispatch.rs` | the 22-name registry (name → schema + backend method), `execute()` / `execute_with()` |
| `src/txn.rs` | `TxnOps` + `txn::run` / `run_gated` (begin/commit/abort, `GroupGuard`) |
| `src/hooks.rs` | plan §4 adapter hooks: `Policy` + the 22-tool grading table, `before_commit` / `render_done` / `edit_failed` |
| `src/backend.rs` | the `EngineBackend` trait (22 tools + `time_rate`) |
| `src/mock.rs` | `MockBackend`: in-memory timeline model, no engine dependency |
| `src/render.rs` | `Frame` (F32 RGBA), PNG transport, golden-frame assertions |
| `src/sdk/` | `SdkBackend`: the real engine through `oak-engine-rust-sdk` |

## Feature gates

| feature | links | what you get |
|---|---|---|
| `default = []` | no engine, no FFmpeg, no OCIO | `MockBackend`, `dispatch`, `txn`, `render`, schemas, and `tests/mock_e2e.rs` — builds and tests on any machine |
| `sdk` | `oak-engine-rust-sdk` → `crates/oak-engine` (cdylib) → the project FFmpeg + OCIO | `SdkBackend` (`src/sdk/`) and `tests/sdk_e2e.rs` — needs the project FFmpeg (`FFMPEG_DIR`) and the static OCIO build |

The `sdk` feature is the only thing in this crate that can fail to build on a
machine: it links a cdylib, which needs the project FFmpeg prefix. See
`docs/build.md` for `tooling/install-deps.sh` + `tooling/ffmpeg/build-ffmpeg.sh`.
The first `--features sdk` build compiles the whole engine; expect it to be
long (tens of minutes on a cold target dir) and use a background job.

## Build and test

```sh
cd ai/oak-ai-tools

# default feature set: no engine dependency, must be green anywhere
cargo test

# the real engine backend (needs FFMPEG_DIR; see docs/build.md)
cargo test --features sdk

# just the type caches, without running
cargo check --features sdk --all-targets
```

`FFMPEG_DIR` is inherited from the repository's `.cargo/config.toml`
(`FFMPEG_DIR = .cache/ffmpeg`), whose `relative = true` resolves against the
repository root, so the commands above work from this directory too.

The crate is a **standalone workspace** (`[workspace]` in `Cargo.toml`): the
repository root only lists `crates/*` and `examples/*`, and `ai/*` must stay
out of the upstream build. Consequently this directory has its own
`Cargo.lock`, and the root's `[patch.crates-io]` is mirrored here (same OCIO
fork, same rev).

## Environment requirements (linking the engine)

`cargo test --features sdk` builds `crates/oak-engine` as a **cdylib**, so every
archive that ends up on that link line must be position independent.

**Phenomenon.** On a host whose system packages ship *static* fontconfig /
freetype / harfbuzz, the link of `liboak_engine.so` fails with:

```
rust-lld: error: relocation R_X86_64_32 cannot be used against local symbol; recompile with -fPIC
... /usr/lib/x86_64-linux-gnu/libfontconfig.a(fcxml.o): ...
```

**Judgment (what it is and is not).** The failure is a `-L` **directory
ordering** problem, not a bad project archive:

1. The repository's `.cache/ffmpeg/lib/libfontconfig.a` *is* PIC — the archives
   there are built by `tooling/ffmpeg/build-deps.sh` with `CFLAGS=-fPIC`
   (`build-deps.sh:71`). Reproduce the difference yourself:

   ```sh
   REPO=$(git rev-parse --show-toplevel)   # run from anywhere in the repo
   mkdir -p /tmp/fcrepro && cd /tmp/fcrepro && mkdir s c
   (cd s && ar x /usr/lib/x86_64-linux-gnu/libfontconfig.a fccfg.o)
   (cd c && ar x "$REPO"/.cache/ffmpeg/lib/libfontconfig.a fccfg.o)
   readelf -r s/fccfg.o | grep -c R_X86_64_32   # system: non-zero (90 here)
   readelf -r c/fccfg.o | grep -c R_X86_64_32   # cached : 0
   ```
2. On the failing link line the `-L/usr/lib/x86_64-linux-gnu` entries (emitted
   by unrelated build scripts: alsa, pipewire, system-deps) come **before**
   `-L<repo>/.cache/ffmpeg/lib`, and `-lfontconfig` is inside
   `-Wl,--whole-archive`. Link search order is decided by the `-L` list, so the
   system's **non-PIC** archive wins over the cached PIC one. The same holds for
   `-lfreetype` and `-lharfbuzz` wherever both copies exist.
3. Ruled out by experiment: `LIBRARY_PATH` does not change `-L` precedence, and
   `RUSTFLAGS='-C link-arg=-L…'` cannot fix it (rustc places those args after
   the libraries). Only `-L` ordering matters.

**Consequence for this environment.** A host that has no static
`libfontconfig.a`/`libfreetype.a` in the system directories links fine with no
extra setup; a host that does must get the `.cache/ffmpeg/lib` search path
*before* the system path for this one cdylib link. **This crate does not ship a
workaround** — this round was verified on one machine with a local-only `cc`
wrapper on `PATH` (not part of the repository, deliberately not documented here
as a standard practice), and the fix candidates are the two named above: link
the seven prefix archives by absolute path in `crates/oak-ffmpeg-link/build.rs`,
or document the ordering requirement where the engine's build is described
(`docs/build.md`). This crate changes neither (out of its write scope).

## Backends

The two backends implement one trait, and `dispatch::execute` cannot tell
them apart. They differ only where reality does:

| | `MockBackend` (default) | `SdkBackend` (`sdk`) |
|---|---|---|
| document | in-memory model | one real `oak_engine` project |
| undo | snapshot stack | the engine's undo groups (`oak_undo_group_*`) |
| media metadata | fixed synthetic fixture (640×360, 30 fps, 10 s), flagged `"synthetic": true` | the engine's probe (real width/height/fps/duration) |
| `render.get_frame` | solid colour derived from the file path | real decode + composite (`oak_render_frame_f32`) |
| `render.get_audio_levels` | zeros, with a note | `UNSUPPORTED` (`GAP_RENDER_AUDIO_SAMPLES`) |
| `timeline.add_marker` | in-memory markers | `UNSUPPORTED` (`GAP_TIMELINE_MARKERS`) |
| `export.start` | acknowledges only, writes nothing | real H.264/MP4 export, run to completion |
| errors | identical codes/steps/`txn` markers | identical codes/steps/`txn` markers |

`"backend": "mock"` / `"backend": "sdk"` in the result payload is the only
tag a caller sees, and it says which of the two produced the answer.

### Documented semantic notes (SDK backend)

* **ids** are opaque tool-face ids (`sequence-1`, `track-2`, `clip-3`,
  `node-4`) minted by the backend and mapped onto the engine's `NodeId`
  tokens. They are invalidated by `project.open`.
* **`project.open`** clears the engine undo stack (the engine's history is
  process-wide and must not refer to a replaced project) and creates the
  default sequence, so the tool face always has an active sequence. When
  `fps` is given it is applied to that sequence; without `fps` the engine's
  default (29.97) is kept, while the mock defaults to 30.
* **`timeline.place_clip`** rejects an overlapping placement with
  `INVALID_PARAMS` (the same observable contract as the mock) instead of
  using the engine's ripple-insert placement. Stray clip nodes created by an
  aborted `place_clip` stay unmounted in the graph: node creation is not
  undoable in the engine, but nothing is mounted on a track, so the timeline
  state visible through the tool face is unaffected.
* **`timeline.place_clip`** returns `length = footage duration − media_in`
  (the frozen face has no `out`; the clip is shortened with
  `timeline.trim_clip`), matching the mock's rule.
* **`render.get_frame`** renders at `max_size` when given, otherwise at the
  target's native geometry capped to 320×180 (16:9 box, aspect preserved).
* **`render.get_thumbnails`** resolves `range` on the *target's own* grid
  (footage → media frame rate, sequence → sequence frame rate) and renders
  each thumbnail through `render.get_frame`; the mock resolves both against
  the sequence grid.
* **`export.start`** runs the engine export task to completion inside the
  call (the SDK's task handle must outlive the call) and reports
  `{started, completed, state, bytes}`; a failed task is a
  `BACKEND_FAILED` error carrying the engine's message.
* **`edit.undo`** maps onto the engine history: `NOTHING_TO_UNDO` when
  `oak_undo_can_undo()` is false (row 0 is the engine's empty "New/Open
  Project" row), and `remaining` excludes that row.
* **`project.get_overview`** reports effects as `{id, kind}` without
  parameter values (see the gap table) and `markers: []` (markers cannot
  exist).
* **`media.probe` duration fallback (documented trade-off, not engine
  semantics)**: the container duration is reported only when the probe
  provides one (`duration_source: "container"`). When it does not —
  the ffmpeg probe on this round's generated mp4 returns no container
  duration — the backend falls back to the **video stream's
  `duration × time base`** and says so with
  `duration_source: "video stream time base"`; if neither exists the field
  is `null`, never a guessed number. This is a probe-level convenience, not
  an engine statement about media length, and `tests/sdk_e2e.rs` pins it
  with `assert_eq!(probe["duration_source"], "video stream time base")`
  so the fallback cannot change silently.
* **`timeline.trim_clip` head trim follows the engine** (`block_trim`,
  `TrimIn`): the timeline OUT point stays fixed, the media in point shifts
  with it, and the freed span becomes a **gap block** the tool face cannot
  address (it reports such elements as `kind: "gap"`); tail trim keeps the
  in point and moves the out point, media untouched.
* **`timeline.ripple_delete`** is the closed-gap contract: the clip is
  unlinked, every later block of the track is shifted left by the removed
  length with the undoable move primitive, and the gaps this operation
  creates are unlinked again — all inside the call's undo group, so a
  rollback restores them. The engine's `track_ripple_remove_block` alone
  would only unlink (leaving a hole) and `TimelineRippleRemoveAreaCommand`
  deliberately leaves a successor starting at/after the range untouched,
  so neither primitive equals "the gap is closed".

## Hooks (plan §4: confirmation gate / readback / failure feedback)

The execution layer owns three interception points (`src/hooks.rs`); none of
them depends on the OPP event face.

### The three policies

| policy | reads (`project.get_overview`, `render.*`, `media.list_footage`, `media.probe`, `node.list_types`) | edits (timeline / node / `edit.undo`) | project + export (`project.open`, `project.save`, `media.import_footage`, `export.start`) |
|---|---|---|---|
| `Policy::ProposalOnly` | run (they cannot change state) | **never run** — a `Proposal` is produced instead; the backend is not entered at all | **never run** — proposal only |
| `Policy::Confirm` | run, gate not consulted | run, gate asked **after the body, before commit**; `Reject`/`Proposed` aborts the undo group | gate asked **before the body**; nothing is opened, so a rejection leaves no trace |
| `Policy::Auto` | run | run | run |

* `ProposalOnly` is the "show me, apply nothing" mode (issue #77 Q5). Its
  zero-side-effect guarantee is tested against a spy backend: `edits == 0`,
  `begins == 0`, identical `serialize()` and identical undo depth.
* `Confirm`'s gate point follows the class: `edit` is graded
  `BeforeCommit` (the undo group is a complete rollback, so the gate can
  veto what it just saw), `project`/`export` are `BeforeInvoke` (files are
  written outside any undo group). `edit.undo` has no transaction at all
  (OPP/1 §6), so it is always gated before the body.
* Reads are `auto` in every mode — the agent needs them to plan.

Grading lives in `hooks::TOOL_CLASSES` (22 entries). `tests/hooks_policy.rs`
asserts it is set-equal to `ai/schema/tools/*.json` **and** to
`dispatch::tool_names()`; a tool added to either side without a grading
entry turns the suite red. A tool with no entry is refused with
`UNCLASSIFIED_TOOL` (fail-closed).

### Injecting a policy and a gate

`dispatch::execute(tool, args)` is unchanged: it runs `Policy::Auto` with
`hooks::NoHooks`, i.e. exactly the pre-hooks behaviour. Use
`dispatch::execute_with(backend, tool, args, policy, hooks)` to inject
both; `hooks` is any type implementing the three callbacks (the `Hooks`
trait bundles them):

```rust
struct Ui {
    decided: Vec<String>,   // what the gate was asked about
    pixels: hooks::PixelReadback,
}

impl hooks::BeforeCommit for Ui {
    fn before_commit(&mut self, pending: &hooks::PendingEdit<'_>) -> hooks::Decision {
        self.decided.push(pending.tool.to_string());
        // Approve | Reject { reason } | Proposed { reason } — a real host
        // asks the user here (or, under ProposalOnly, always proposes).
        hooks::Decision::Approve
    }
}

impl hooks::RenderDone for Ui {
    fn render_done(&mut self, event: &hooks::RenderDoneEvent<'_>) -> hooks::ReadbackVerdict {
        // Decode the PNG, check geometry / byte count / black ratio;
        // an LLM judgement would sit right here behind the same verdict.
        self.pixels.render_done(event)
    }
}

impl hooks::EditFailed for Ui {
    fn edit_failed(&mut self, event: &hooks::FailureEvent<'_>) -> hooks::Feedback {
        hooks::Feedback::from_error(event.error.clone())
    }
}
```

The outcome is a `GatedOutcome`: `Applied { result }` / `Proposed {
proposal }` / `Rejected { reason, error }` / `Failed { error, feedback }`.
`GatedOutcome::into_result()` collapses it onto the old
`Result<Value, ToolError>` surface (a proposal becomes the structured
`PROPOSAL_ONLY` error, so a proposal can never masquerade as a success).

### The three callbacks

* **`before_commit`** answers `Approve` / `Reject { reason }` /
  `Proposed { reason }`. A `Reject` at commit time aborts the open group
  (`USER_REJECTED`, `step: "confirm"`, `txn: "aborted"`); before the body
  there is nothing to abort and no group was opened.
* **`render_done`** fires after `render.get_frame` /
  `render.get_thumbnails` returned and before their undo group commits.
  `hooks::PixelReadback` is the built-in pixel half: it decodes
  `png_base64`, checks the declared geometry and `png_bytes`, and
  optionally requires a non-black ratio. A failed assertion aborts the
  group and reports `READBACK_ASSERTION_FAILED` (`step: "readback"`).
* **`edit_failed`** fires after the abort for every failure. It returns a
  `Feedback` = the **existing** `{code, message, tool, step, gap, txn}`
  error object plus two advisory keys, `retryable` and `hint` — no second
  error shape. `tests/hooks_replan.rs` runs the closed loop: a stale clip
  id fails with `NOT_FOUND` → state restored → the hint points at
  `project.get_overview` → the re-read gives the live id and midpoint →
  the retry commits.

### Hooks tests

`tests/hooks_policy.rs` (policy matrix, gate points, zero-side-effect
proof, grading-table drift), `tests/hooks_readback.rs` (trigger order,
abort-on-failure, bgra8 path), `tests/hooks_replan.rs` (fail → abort →
feedback → re-plan → success).

## Tool coverage matrix

`SdkBackend` implements every method of `EngineBackend`. "gap" means the
method returns `UNSUPPORTED` with that gap id — never a silent success.

| tool / method | SDK backend | note |
|---|---|---|
| `project.open` | implemented | manager + active-project initialisation, undo clear |
| `project.save` | implemented | real XML write, byte count read back |
| `project.get_overview` | implemented | live traversal of bin → sequence → track list → track → block |
| `media.probe` | implemented | `codec::decoder_probe` |
| `media.import_footage` | implemented | per-path failures reported in `failed` |
| `media.list_footage` | implemented | |
| `timeline.add_track` | implemented (append only) | gap `GAP_TRACK_INSERT_INDEX` for a non-append `index` |
| `timeline.place_clip` | implemented | validates before mutating; overlap rejected |
| `timeline.split_clip` | implemented | new block located by set difference on the track |
| `timeline.trim_clip` | implemented | in → `block_trim(TrimIn)`, out → `block_set_length` |
| `timeline.move_clip` | implemented (same sequence) | cross-sequence move is `INVALID_PARAMS` |
| `timeline.ripple_delete` | implemented | `track_ripple_remove_block`, then the later blocks of the track are shifted left by the removed length and the filler gaps unlinked (see the ripple note above) |
| `timeline.add_transition` | **gap** `GAP_TRANSITION_SEAM` | see below |
| `timeline.add_marker` | **gap** `GAP_TIMELINE_MARKERS` | no marker API in the C ABI |
| `node.add_effect` | implemented | type validated against the engine factory table |
| `node.set_param` | implemented for numeric / bool / rational / vector / colour | gap `GAP_NODE_STRING_PARAM` for strings; the write is read back and compared |
| `node.list_types` | implemented for the type catalogue | gap `GAP_NODE_INPUT_ENUM` when `node` is given |
| `render.get_frame` | implemented | real pixels, PNG or `bgra8` |
| `render.get_thumbnails` | implemented | loops `render.get_frame` |
| `render.get_audio_levels` | **gap** `GAP_RENDER_AUDIO_SAMPLES` | no sequence audio mix in the ABI |
| `export.start` | implemented | H.264/MP4, synchronous |
| `edit.undo` | implemented | engine history |
| `txn_begin` / `txn_commit` / `txn_abort` | implemented | `oak_undo_group_begin/end/abort` |
| `time_rate` | implemented | sequence / track / clip → sequence rate, footage → media rate |

## Known gaps (SDK capability, `UNSUPPORTED` + gap id)

The ids are the constants in `src/sdk/gaps.rs`; each carries the same
explanation in its doc comment. All of them are **ABI-level absences**,
verified against `crates/oak-engine-rust-sdk/src` and the engine's exported
symbols — not "we did not get to it".

| gap id | tool | what the SDK is missing | upstream issue candidate |
|---|---|---|---|
| `GAP_TIMELINE_MARKERS` | `timeline.add_marker` | no marker entry point at all (`oak_timeline_marker_*` does not exist; the undoable marker commands in `oak-timeline` are not exported) | "export the sequence marker add/remove/query commands over the C ABI" |
| `GAP_RENDER_AUDIO_SAMPLES` | `render.get_audio_levels` | no sequence audio-mix render (`oak_render::eval::render_audio_samples` has no C wrapper; see `docs/zh/plans/engine-sdk-capability-gaps.md` "音频采样渲染") | "C wrapper for render_audio_samples" |
| `GAP_NODE_INPUT_ENUM` | `node.list_types` with `node` (the `get_params` half) | no input enumeration: `standard_value` reads one *known* input id, but there is no "list the inputs of this node" call, hence no parameter name/min-max/choices inventory | "node input enumeration (id, display name, value type, range, choices)" |
| `GAP_TRANSITION_SEAM` | `timeline.add_transition` | not an ABI hole: the SDK has `create_transition`, `transition_set_offsets` and `connect`, but an engine transition is a block spanning a **seam between two blocks** with the duration split into in/out offsets, while the frozen tool face names one clip edge plus a total duration and never says which neighbour is on the other side | "specify the seam model (neighbour + offset split) in the tool face, or add a transition-with-neighbours helper" |
| `GAP_TRACK_INSERT_INDEX` | `timeline.add_track` with `index != append` | `oak_track_add(project, sequence, kind)` appends to the media-kind track list; no "insert track at index" command | "track insert-at-index command" |
| `GAP_NODE_STRING_PARAM` | `node.set_param` with a string value | `oak_node_set_standard_value` carries the POD `OakNodeValue`, whose bridge has no string payload (`to_node_value` has no STRING arm) | "string-carried parameter writes over the C ABI" |
| `GAP_CLIP_SOURCE_QUERY` | (informational) | no edge query: `oak_node_connect`/`oak_node_disconnect` exist but a block's connected input cannot be read back, so `clip → footage` is remembered when this session made the placement and reported `null` for a project loaded from disk | "node edge query (connected source of an input)" |

## End-to-end test (`tests/sdk_e2e.rs`, feature `sdk`)

No binary media fixture is committed. The test **generates** its media at run
time with the engine's own encoder (a 2 s, 10 fps, 64×64 solid-colour H.264
clip per colour, written to a temp dir), then runs the acceptance chain
through `dispatch::execute`:

```
project.open → media.probe → media.import_footage ×2 → timeline.add_track
→ timeline.place_clip ×2 → render.get_frame (before the edits)
→ timeline.split_clip → timeline.trim_clip → timeline.ripple_delete
→ project.get_overview (positions read back from the document)
→ render.get_frame ×2 (after the edits: red clip vs blue clip)
→ render.get_frame ×2 (determinism)
→ edit.undo … until the timeline serialisation equals the post-import snapshot
```

The pixel assertions are **invariants**, not a pinned golden hash — H.264 +
4:2:0 quantisation makes an exact hash meaningless across encoder/FFmpeg
versions:

* the rendered frame has the requested size and is fully covered
  (`non_black_ratio > 0.99`, a gap would be transparent black);
* the frame of the red clip has a dominant red channel, the frame of the blue
  clip a dominant blue channel, and the two frames differ by a wide margin
  (i.e. the picture follows the timeline after split/trim/ripple);
* rendering the same time twice yields the identical `fnv64` and
  `mean_rgb` (determinism).

If a fixed reference image is ever needed, the rule is: generate it with a
script that is committed next to it, and keep the PNG ≤ 64 KB.

A second test covers the transaction contract on the real engine: a rejected
(overlapping) placement returns `INVALID_PARAMS` with `txn: "aborted"`,
leaves the document unchanged and adds **no** engine history row, and a
schema-level rejection never opens an undo group at all.

## Verification record

Fill in from the actual command output; never write "green" from memory.

Both rows below are copied from the terminal on the dates shown; the engine
was rebuilt from scratch for the `sdk` row (full cdylib link, ~400 crates).

| command | result | date |
|---|---|---|
| `cargo test --offline` (default features, repo-local `target/`) | `DEFAULT_EXIT=0`, **90 passed / 0 failed**: lib 60, `hooks_policy` 11, `hooks_readback` 7, `hooks_replan` 4, `mock_e2e` 3, `registry_drift` 2, `schema_samples` 2, `sdk_e2e` 0 (feature-gated out), doctest 1 | 2026-11-08 |
| `PATH=<local cc shim>:$PATH CARGO_TARGET_DIR=/tmp/oak-ai-sdk-target cargo test --features sdk --offline` | `SDK_EXIT=0`, **96 passed / 0 failed**: lib 64, `hooks_policy` 11, `hooks_readback` 7, `hooks_replan` 4, `mock_e2e` 3, `registry_drift` 2, `schema_samples` 2, **`sdk_e2e` 2**, doctest 1; `liboak_engine.so` linked (457 MB debug cdylib) | 2026-11-08 |

The `sdk` row needed the local-only `cc` wrapper described in *Environment
requirements*; the repository itself carries no workaround, and the default row
needs none.

### Explicitly **untested** (未实测)

These paths are implemented but have not been executed against the engine in
this round; treat them as unverified until a run says otherwise:

* `timeline.add_transition` — unsupported by design (no combo `kind = 9`
  write is attempted at all, so nothing about transition combo encoding is
  verified here);
* `export.start`'s `preset` → `task::EncodingParams` mapping: the `preset`
  argument is echoed back but **not** mapped onto encoder settings (only the
  H.264/MP4 defaults derived from the sequence geometry and frame rate are
  used);
* the engine's own ripple-insert placement when the target area is occupied:
  the backend rejects overlaps *before* calling `block_place`, so
  `block_place`'s ripple/overwrite behaviour is not exercised;
* `node.set_param` on keyframed or `VEC3`/`COLOR` parameters (only scalar
  numeric/bool/rational/vector round-trips are exercised by the tests so
  far);
* `project.save` / `project.open` of an existing `.ove` file (the SDK save
  path is implemented; the E2E does not cover a save/reload round trip).
