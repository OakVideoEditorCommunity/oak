# oak-worker

Headless render-worker process (and the single OpenFX host mode) for the
Oak engine.

Since the SDK收口 the binary is a **thin launcher**: the NDJSON control
loop, shared-memory frame-slot transport, graph loading and per-process
rendering live in `oak-engine`; the worker links `oak-engine-rust-sdk` and
calls `oak_engine_rust_sdk::worker::main()`.

## Build and run

```sh
cargo build --release -p oak-worker      # target/release/oak-worker
./target/release/oak-worker --backend cpu
./target/release/oak-worker --ofx-host
```

`--backend` accepts the engine renderer backends (`opengl`, `vulkan`,
`cpu`, `none`, ...). `cpu` is headless but fully operational; `none`
skips the renderer and exits 1 like the C++ main.

## Where the runtime lives

- `oak-engine/src/worker/mod.rs` — backend selection, startup handshake,
  NDJSON control loop, `load_graph`, `render_frame`/`render_batch`,
  `render_audio_batch`, shutdown.
- `oak-engine/src/worker/ipc.rs` — re-export of `oak_render::ipc` (the
  shared-memory slots and NDJSON message types).
- `oak-engine/src/worker/ofx_host.rs` — the single OpenFX host loop.
- `oak-engine/src/worker/framecache.rs` — worker-side frame cache.

The worker tests (protocol, process-pool integration, crash isolation,
OFX host) live in the engine test modules; the binary-level contract
tests are in `oak-engine/src/worker/process_tests.rs`. Build the worker
before running them, or set `OAK_WORKER_BIN`:

```sh
cargo build -p oak-worker
cargo test -p oak-engine
```

## Dependency boundary

`crates/oak-worker/Cargo.toml` depends only on `oak-engine-rust-sdk`
(besides std). `python3 tooling/check_sdk_consumers.py` enforces this.

## License

GPL-3.0-or-later.
