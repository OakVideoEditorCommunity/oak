# P10：把私有 crate 合并进 `oak-engine`（可行性计划）

> 状态：**可行性方案，待评审/执行**。目标不是改功能，而是把发布面从
> “engine + 12 个内部 crate”缩成“engine + SDK”，让内部实现 crate 不再需要
> 单独发布，也不可能被第三方单独使用。

## 1. 结论

**可行。** 依赖图是严格 DAG，所有内部 crate 目前只被 engine 图内部使用，
没有外部消费者；合并不会引入循环依赖。主要成本不在“能不能编译”，而在：

1. 约 **160k 行源码、约 2,600 处跨 crate 路径**的机械迁移；
2. **74 个集成测试文件 / 约 2,500 个测试**的进程隔离与夹具路径；
3. 3 个 build script（FFmpeg 静态链接、OFX C shim、render 测试链接）合并；
4. 单 crate 的编译时间、增量编译和 docs.rs 构建需要实测。

建议在**功能冻结窗口**内一次性完成（一个分支、一个合并 PR），不采用
“逐个 crate 合并”的渐进方式，因为中间状态会出现两份同名类型（例如两个
`Rational`），无法通过编译。

## 2. 现状数据（`0.5.2`）

### 2.1 规模

| crate | 源码行数 | 文件 | 直接被谁依赖 |
|---|---:|---:|---|
| oak-core | 23,825 | 29 | codec/audio/node/render/ofx/otio/storage/task/timeline/engine |
| oak-ffmpeg-link | 24 | 1 | codec/audio/engine |
| oak-codec | 13,616 | 23 | audio/node/render/task/engine |
| oak-audio | 3,356 | 12 | engine |
| oak-undo | 1,119 | 5 | node/ofx/storage/task/timeline/engine |
| oak-node | 46,469 | 97 | render/ofx/storage/task/timeline/engine |
| oak-render | 25,323 | 19 | ofx/task/engine |
| oak-ofx-plugin | 23,003 | 30 | engine |
| oak-otio | 3,047 | 4 | storage/task/engine |
| oak-storage | 4,312 | 19 | engine |
| oak-task | 8,252 | 20 | engine |
| oak-timeline | 7,653 | 14 | engine |
| **小计** | **159,999** | **273** | |
| oak-engine（现有） | 37,825 | 87 | 合并后目标 197,824 行 |

### 2.2 依赖 DAG（内部边）

```
core ─┬─> codec ─> audio
      ├─> node ─> render ─> ofx
      ├─> otio ─> storage
      ├─> undo
      └─> timeline / task
ffmpeg-link（无代码，只有 links/build.rs）─> codec/audio
engine ─> 以上全部
```

没有外部 crate 依赖这些内部 crate：`oak-app` / `oak-cli` / `oak-worker` /
`examples/sdk_editor` 已全部只依赖 SDK（`tooling/check_sdk_consumers.py`
守卫）。因此合并不会破坏任何第一方消费者。

### 2.3 外部依赖

合并后 engine 的第三方依赖并集：

```text
thiserror, ocio-rs(bundled), wgpu, wgpu-types, log, half, toml,
quick-xml, image, ffmpeg-next, cpal, naga, cosmic-text, libc,
serde, serde_json, uuid, chrono, sea-orm, tokio
```

这些已经在 engine 的传递闭包中，合并只是把它们提升为直接依赖。

### 2.4 构建脚本与资源

- `oak-ffmpeg-link/build.rs`：静态 FFmpeg 的传递链接参数；crate 声明
  `links = "oak_ffmpeg_static_deps"`，链接参数才会传播到 engine。**不能简单删除**。
- `oak-ofx-plugin/build.rs`：编译 `cbits/ofx_*_shim.c` 与测试插件，依赖
  `cbits/`、`ofx/` 目录。
- `oak-render/build.rs`：macOS 测试二进制的 `-export_dynamic` 与
  IOKit/ColorSync/CoreGraphics framework。
- `oak-ofx-plugin/cbits` + `ofx/`（约 420 KB）需要迁入 engine 包。

### 2.5 测试

| crate | unit tests | 集成测试文件 | 集成测试数 |
|---|---:|---:|---:|
| core | 457 | 1 | 12 |
| codec | 193 | 0 | 0 |
| audio | 5 | 7 | 43 |
| undo | 8 | 2 | 20 |
| node | 682 | 8 | 71 |
| render | 315 | 17 | 86 |
| ofx-plugin | 208 | 12 | 51 |
| otio | 30 | 4 | 23 |
| storage | 19 | 6 | 55 |
| task | 45 | 12 | 88 |
| timeline | 14 | 5 | 91 |
| **小计** | **1,976** | **74** | **540** |

合并后 `oak-engine` 的测试规模约为 2,787 个（含现有 271 个）。

## 3. 目标结构

```text
crates/oak-engine/
  Cargo.toml          # cdylib + rlib（rlib 仅用于测试，见 §5）
  build.rs            # 合并 ffmpeg-link + ofx cbits + render 链接参数
  links = "oak_ffmpeg_static_deps"
  cbits/              # 从 oak-ofx-plugin 迁入
  ofx/                # OFX headers
  include/oak_engine.h
  src/
    lib.rs            # 仅 C ABI 导出 + 模块声明
    node.rs, editor*.rs, ...   # 现有 ABI/领域代码保持原位
    internal/         # 合并进来的实现 crate（全部 pub(crate)）
      core/
      codec/
      audio/
      undo/
      node/
      render/
      ofx/
      otio/
      storage/
      task/
      timeline/
```

- 现有 engine 模块**不移动**，避免 1,106 处 `crate::` 互引再改一遍。
- 合并 crate 的根文件成为 `internal/<name>/mod.rs`；crate 级属性
  (`#![deny]`、`#![allow]`、crate docs) 改为模块级或上移到 engine 根后逐项评审。
- 所有内部项保持 `pub(crate)`/私有；engine 只导出 `extern "C"`。

## 4. 路径迁移规则

对每个被合并 crate 的源码：

1. 自身引用：`crate::` → `crate::internal::<self>::`
2. 跨 crate 引用：
   `oak_core::` → `crate::internal::core::`
   `oak_codec::` → `crate::internal::codec::`
   `oak_audio::` → `crate::internal::audio::`
   `oak_undo::` → `crate::internal::undo::`
   `oak_node::` → `crate::internal::node::`
   `oak_render::` → `crate::internal::render::`
   `oak_ofx_plugin::` → `crate::internal::ofx::`
   `oak_otio::` → `crate::internal::otio::`
   `oak_storage::` → `crate::internal::storage::`
   `oak_task::` → `crate::internal::task::`
   `oak_timeline::` → `crate::internal::timeline::`
3. engine 现有代码对内部 crate 的引用（1,106 处）同样替换。
4. 注意不要改字符串字面量/文档中的 crate 名（脚本应跳过字符串与注释，
   或改完人工检查 docs）。
5. `extern crate`、`#[path]`、`include!`、`env!("CARGO_MANIFEST_DIR")`、
   `#[doc(alias)]` 需要单独审计；测试夹具路径必须重新指向 engine 包内。

建议实现 `tooling/merge_internal_crates.py`：
- 读取 `cargo metadata` 的内部 crate 列表；
- 复制 `src/` → `src/internal/<name>/`；
- 基于 Rust 词法（至少跳过 `//`、`/* */`、字符串）替换路径；
- 生成 `internal/mod.rs` 的 `pub(crate) mod` 列表；
- 输出未处理的 `env!`/`include!`/`#[path]` 清单供人工处理。

## 5. 测试迁移策略（关键决策）

74 个集成测试文件不能简单变成 unit test：现有测试中至少有三类依赖
**独立进程**：

1. FFmpeg 解码 + 音频在同一进程退出时崩溃的已知问题（`waveform_e2e` 注释）；
2. `OAK_WORKER_*` 崩溃钩子、OFX host 进程生命周期；
3. `std::env::set_var`（Rust 2024 已 unsafe）与全局单例的串行假设。

**推荐方案：保留独立测试二进制。**

- `oak-engine` 改为 `crate-type = ["cdylib", "rlib"]`；rlib 只用于本包测试。
- 增加 `#[doc(hidden)] pub mod internal`（或 feature `test-internals`，
  CI 用 `--features test-internals` 打开）。
- 原 `tests/*.rs` 迁到 `oak-engine/tests/<name>.rs`，路径替换为
  `oak_engine::internal::...`；每个文件仍是独立进程，保留隔离。
- 共享 `tests/common` 合并为 `engine/tests/common/mod.rs`。
- 测试夹具（`tests/ofx/`、OTIO golden、`demo.mp4`/`tone48k.mp4`）迁入
  `engine/tests/fixtures/`，并把 `CARGO_MANIFEST_DIR` 相对路径改为
  engine 包内路径。
- CI 分片运行（`--test` 按 crate 对应文件），避免单个测试进程过长。

备选方案（不推荐）：全部改为 `#[cfg(test)] mod`。会失去进程隔离，且
FFmpeg/OFX 崩溃会污染整个测试进程。

## 6. 构建脚本合并

`oak-engine/build.rs` 合并三份逻辑：

1. ffmpeg-link：读 `FFMPEG_DIR`、`PKG_CONFIG_PATH`，向 cargo 输出
   `rustc-link-search`/`rustc-link-lib`；engine 声明
   `links = "oak_ffmpeg_static_deps"`。
2. ofx cbits：`cc::Build` 编译 `cbits/ofx_*_shim.c`，并用系统 cc 生成
   `oak_test_plugin` 动态库；`build-dependencies = ["cc"]`。
3. render：macOS 的 `rustc-link-arg-tests` 与 framework 参数。

注意：

- 同一包只能有一个 build script；`cargo:rustc-link-arg-tests` 只对
  engine 包自己的测试二进制生效，迁移后的 `tests/` 正好覆盖。
- `links` 值不能与 `ffmpeg-sys-next` 的 `links = "ffmpeg"` 冲突
  （`oak_ffmpeg_static_deps` 唯一，保持）。
- `Cargo.toml` 需要 `build = "build.rs"`、`cc` build-dependency，
  并把 `cbits/`、`ofx/` 加进 `include`。
- 三平台验证：macOS frameworks、Linux lld、Windows 静态 FFmpeg/DLL。

## 7. 分阶段执行计划

### P10.0 准备（0.5–1 天）

- 冻结窗口；新建分支 `engine-consolidation`。
- 记录基线：`cargo build -p oak-engine` 时间、`cargo test --workspace`
  时间、engine dylib 大小、`cargo package --list` 内容。
- 生成依赖/路径/测试/夹具清单（§2）。
- 评审本计划与测试策略。

**出口**：基线数据与冻结确认。

### P10.1 库代码合并（1–2 天）

- 用脚本复制 11 个有代码的 crate 到 `src/internal/`（`oak-ffmpeg-link`
  没有 Rust 代码，只迁 build.rs。
- 执行路径替换，修 `internal/mod.rs`。
- `cargo check -p oak-engine --lib` 到零错误。
- 逐 crate 处理 crate 属性、宏、`cfg`、`env!`/`include!`。
- 此时**不删除**旧 crate，engine Cargo 暂时仍依赖它们会导致重复类型：
  为通过编译，必须在同一提交里移除 engine 对内部 crate 的依赖，旧 crate
  保留在 workspace 但不再被 engine 使用（可暂时不参与构建）。

**出口**：engine lib 单独编译；无重复类型；ABI 符号表与基线一致
（`nm` 比对）。

### P10.2 测试迁移（1–2 天）

- unit test 随模块迁入，`cargo test -p oak-engine --lib` 全绿。
- 集成测试按 §5 迁到 `engine/tests/`；rlib + `internal` 暴露。
- 夹具路径修正；`ofx_roundtrip` 等 CI 显式测试改为 `-p oak-engine --test ...`。
- 串行/环境变量测试保持独立二进制；CI 分片。

**出口**：engine 测试总数 ≈ 2,787，全部通过；无跳过项被误当通过。

### P10.3 构建脚本与资源（0.5–1 天）

- 合并 build.rs、迁 `cbits/`/`ofx/`、声明 `links`、加 `cc`。
- 三平台编译与链接冒烟；确认 FFmpeg 静态链接、OFX shim、macOS framework。
- `cargo package -p oak-engine --list` 包含全部资源。

**出口**：三平台构建通过；dylib 可被 SDK 加载；示例/CLI/worker 正常。

### P10.4 删除旧 crate 与发布收口（0.5 天）

- 从 workspace `members`/`default-members` 删除 11 个 crate 目录
  （`oak-ofx-plugin` 等），删除各自的 `Cargo.lock`/`COVERAGE.md`/README
  （内容归档到 engine 文档）。
- `tooling/publish_workspace.sh` 缩减为 `oak-engine` + `oak-engine-rust-sdk`。
- 更新 CI（测试目标、coverage、边界守卫、examples）、文档、
  `docs/zh/sdk-guide.md` 发布顺序。
- `oak-engine/Cargo.toml` 去掉全部内部 path deps；SDK 仍只依赖 engine。

**出口**：workspace 只剩 app/cli/worker/engine/sdk/extensions/examples；
`cargo tree -p oak-engine --depth 1` 无 Oak 内部依赖。

### P10.5 验证与度量（1 天）

```text
cargo check --workspace --all-targets
cargo test --workspace -- --test-threads=1
cargo test -p oak-engine --lib
cargo test -p oak-engine --test <each>   # CI 分片
cargo doc -p oak-engine --no-deps
cargo package -p oak-engine --list
cargo package -p oak-engine-rust-sdk --list
python3 tooling/check_sdk_consumers.py
python3 tooling/check_app_engine_boundary.py
```

度量项：全量构建/测试时间、engine dylib 大小、增量编译时间、docs.rs 构建
时间、`cargo publish --dry-run`（依赖已在 registry 时）。

## 8. 风险与控制

| 风险 | 影响 | 控制 |
|---|---|---|
| 测试进程隔离丢失 | FFmpeg/OFX 崩溃污染测试 | 保留 `tests/` 独立二进制（§5） |
| 重复类型/两条实现 | 编译失败或行为分叉 | 一次性合并；同一提交移除 engine 的内部依赖；`nm` 对比符号 |
| 路径误改字符串/文档 | 文档错误、测试夹具路径失效 | 词法替换 + `env!`/`include!` 清单 + 人工 review |
| build.rs 链接参数丢失 | 静态 FFmpeg/OFX/macOS 框架链接失败 | 三平台冒烟；保留 `links` 键；对比 `otool -L`/`ldd` |
| 单 crate 编译变慢 | 开发迭代变慢 | 基线对比；必要时 `codegen-units`/`-Z` 或 CI 分片；仍可接受再合并 |
| docs.rs 构建超时 | 文档发布失败 | 限制 docs.rs 构建（`no-default-features`/`--no-deps`）；实测 |
| 合并期间上游改动 | 大量冲突 | 冻结窗口、短分支、一次合并 |
| rlib 暴露内部 API | 第三方可用私有 API | `#[doc(hidden)] pub mod internal` + 仅测试 feature；crate 不发布 rlib 使用说明 |
| 包体积 | crates.io 包变大（约 200k 行 + 420KB 资源） | 检查 `cargo package --list` 体积；可接受 |

## 9. 工作量与建议

- 机械迁移：2–3 人日
- 测试迁移与夹具：1.5–2 人日
- build script/三平台：1–1.5 人日
- 验证与发布收口：1 人日

**合计约 5.5–7.5 人日**，风险中等偏高，主要集中在测试隔离与三平台链接。

**建议**：如果“只发布两个 crate”是硬需求，执行 P10；否则维持现状
（14 个 crate 的发布顺序已脚本化，成本主要在版本同步而非技术风险）。
如果执行，建议在 0.5.x 稳定分支上先做 P10.0–P10.2 的 PoC，再决定是否
进入主分支。

## 10. 验收标准

- [ ] `oak-engine` 不再依赖任何 Oak 内部 crate（Cargo 与源码均无）。
- [ ] 原 2,516 个内部测试全部迁入并通过（或明确记录等价替代）。
- [ ] `oak-app`/`oak-cli`/`oak-worker`/`examples/sdk_editor` 行为不变，
      测试全绿。
- [ ] `cargo package -p oak-engine --list` 只含 engine 源码/资源；
      `cargo package -p oak-engine-rust-sdk --list` 只含 SDK。
- [ ] 三平台构建/链接/冒烟通过。
- [ ] 构建与测试时间相对基线无不可接受回退（阈值由 P10.0 确定）。
- [ ] 文档与发布脚本只剩两个包。
