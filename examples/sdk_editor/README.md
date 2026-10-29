# sdk_editor — an independent Oak SDK consumer

This example is the P8 acceptance proof: a new editor that depends on exactly
one Oak crate, `oak-engine-rust-sdk`, and completes the full workflow through
the public SDK only.

```bash
cargo run -p sdk_editor -- /tmp/oak-sdk-demo   # headless workflow
cargo run -p sdk_editor -- --gui /tmp/oak-sdk-demo  # terminal frontend
cargo test -p sdk_editor                        # automated acceptance
```

What the headless run does:

1. init color/text/audio/OFX/render backends;
2. new project, import `assets/test_clip.mp4`, browse the project tree;
3. create a sequence, place video/audio clips, split/trim/move/ripple-delete;
4. insert an effect, add two keyframes, read the parameter snapshot;
5. render source and program frames plus an audio block;
6. undo/redo one edit and check the restored timeline state;
7. save and reopen the project (effect and keyframes included);
8. export H.264 MP4 through the unified task API and poll it to success.

The `--gui` shell renders program frames from the same `EditorSession` as ANSI
art and accepts `p`/`+`/`-`/`s <sec>`/`l`/`q` on stdin. It has no third-party
UI dependency on purpose: the point is that the SDK session, not the Oak app
shell, drives the frontend.

Dependency proof:

```bash
cargo tree -p sdk_editor --depth 1
# oak-engine-rust-sdk is the only Oak entry
```
