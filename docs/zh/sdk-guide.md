# Quercus Engine SDK 指南（已迁移）

> 引擎与 Rust SDK 已从 Oak 仓库迁出，发布为 Quercus 项目：
>
> - 仓库：<https://github.com/OakVideoEditorCommunity/quercus-engine>
> - Engine：<https://crates.io/crates/quercus-engine>
> - Rust SDK：<https://crates.io/crates/quercus-engine-rs>
> - 文档：<https://docs.rs/quercus-engine-rs>

Oak 应用、CLI、worker 和 `examples/sdk_editor` 通过 **git submodule + 版本/路径双引用**
消费 SDK（`quercus-engine/` 子模块；本地走 path，发布消费者按 version 解析）：

```toml
quercus-engine-rs = { version = "=0.6.1", path = "../../quercus-engine/crates/quercus-engine-rs" }
```

```rust
use quercus_engine_rs::editor::{EditorSession, TrackKind};
use quercus_engine_rs::edit::EditCommand;
```

C/C++ 前端使用 Quercus 仓库中的 `include/quercus_engine.h` 和
`libquercus_engine`；该头文件由 Quercus 仓库的
`tooling/gen_c_header.py` 生成并校验。

SDK 的生命周期、线程、错误、所有权、任务事件、链接/rpath 和发布说明见
Quercus 仓库的 `crates/quercus-engine-rs/README.md`。

Oak 仓库不再包含 `oak-engine`、`oak-engine-rust-sdk` 或任何内部引擎 crate；
`tooling/check_sdk_consumers.py` 只允许 `quercus-engine-rs` 作为引擎入口。
