# P9：oak-worker / oak-cli 收口与 SDK 文档

> 在原 P0–P8 之后的交付收口：所有第一方消费者统一只依赖
> `oak-engine-rust-sdk`，并把 SDK 文档补到 docs.rs 可发布水平。

## 1. oak-worker：运行时迁入 engine

原先 worker 把 NDJSON 控制循环、共享内存帧槽、图加载和渲染都编译进
`oak-worker` 二进制，直接依赖 `oak-core`/`oak-node`/`oak-render`/
`oak-ofx-plugin`。现在：

- 运行时迁入 `oak-engine/src/worker/`（`mod.rs`、`ipc.rs`、
  `framecache.rs`、`ofx_host.rs`），与引擎其他领域能力同属一个 cdylib。
- 新增 C ABI `oak_worker_run` / `oak_worker_ofx_host`；SDK 新增
  `worker` 模块（`run`、`ofx_host_main`、`parse_backend`、`main`）。
- `oak-worker/src/main.rs` 只剩一行 `worker::main()`；
  `oak-worker/Cargo.toml` 只依赖 `oak-engine-rust-sdk`。
- worker 的协议、进程池、崩溃隔离、OFX host 测试迁入 engine 测试模块；
  二进制契约测试保留在 `worker/process_tests.rs`（未构建 worker 时跳过）。

## 2. oak-cli：改为 SDK 适配

- `engine.rs` 重写为纯 SDK 适配：`EditorSession` 打开工程，`Project`
  查询时间线，`EditCommand` 做编辑，`task`/`render` 出帧/音频/导出。
- `probe` 用 `codec::decoder_probe`，`render`/`transcode` 用 SDK 帧与音频
  任务，`transcode --format mp4` 用统一导出任务；不再需要手写 montage、
  ticket 或 codec 会话。
- `oak-cli/Cargo.toml` 的 Oak 依赖只剩 SDK（其余为 `clap`）。
- 18 个 CLI 集成测试（info/probe/render/transcode）全绿。

## 3. 消费者守卫

新增 `tooling/check_sdk_consumers.py`：扫描 `oak-app`、`oak-cli`、
`oak-worker`、`examples/sdk_editor` 的 Cargo 依赖与源码，内部 Oak crate
或 `oak_*::` 引用一律失败；CI 已接入。

## 4. SDK 文档

- `crates/oak-engine-rust-sdk/README.md` 作为 crate 文档嵌入
  （生命周期、线程、错误、所有权、任务/事件、C ABI、发布）。
- `#![deny(missing_docs)]` + `#![deny(rustdoc::broken_intra_doc_links)]`：
  828 个缺失项（结构体字段、枚举变体、方法、常量）全部补齐；
  `EditCommand` 等核心枚举使用语义化说明，其余镜像类型使用字段级说明。
- `cargo doc -p oak-engine-rust-sdk --no-deps` 不再产生 SDK 自身警告；
  docs.rs 元数据（`all-features`）已就绪。
- `docs/zh/sdk-guide.md` 面向新前端，覆盖 Rust/C 两条入口。

## 5. 验证

```text
cargo test -p oak-cli                                 18 + 12 passed
cargo test -p oak-engine                              271 passed
cargo test -p oak-engine-rust-sdk                     117 passed
cargo check --workspace --all-targets                 OK
python3 tooling/check_sdk_consumers.py                 4 consumers OK
cargo doc -p oak-engine-rust-sdk --no-deps            no SDK warnings
cargo tree -p oak-worker --depth 1                    only SDK
cargo tree -p oak-cli --depth 1                       only SDK
```

## 6. 注意

- 引擎运行时现在包含 worker 与 OFX host；`oak-worker` 进程加载的是
  同一份 engine dylib，这符合“worker 是引擎渲染拓扑的一部分”。
- 发布顺序不变：仍先发布 engine 依赖链，再发布 SDK。
