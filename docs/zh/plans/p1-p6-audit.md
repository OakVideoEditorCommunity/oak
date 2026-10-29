# P1–P6 完成度静态审计

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P1–P6。
> 审计时间：2026-10-30；代码快照：`42dafeec6` + 未提交的 P7 工作树。
> 结论用于决定 P7 还需要做什么，不替代各阶段的实施记录。
> **后续状态（2026-10-30）**：本文列出的 P7 收口项已在 P7B/P7C 完成——app 的 10 个
> 内部 crate 依赖与全部源码引用已清零，边界守卫为 0；保留本文作为审计快照。

## 1. 审计口径

1. **能力口径**：engine 是否有真实实现、C ABI 是否暴露、SDK 是否有安全包装与测试。
2. **出口口径**：逐条核对主计划每个阶段的“出口”文字，包括“app 不再直接依赖……”一类条件。
3. **防绕行口径**：主计划第 5.3 节列出的伪完成形态（app 内同名 wrapper、测试直调、复制领域逻辑等）。
4. 本次以静态检查为主，并实跑了 engine / SDK / app 三包测试；P7 正在改造中，部分结论只对当前工作树成立。

## 2. 结论摘要

**按出口口径，P1–P6 不能判为全部完成。**

- engine / SDK 能力侧：六类能力全部落地，`oak-engine` 169 项、`oak-engine-rust-sdk`
  115 项测试通过。
- app 切换侧：P1、P6 达成；P3/P4/P5 生产路径基本切换但出口有残留；**P2B 明确未达成**。
- 当前工作树 `oak-app` 编译失败（P7 半成品），因此“此刻全量回归通过”不成立。

| 阶段 | engine/SDK 能力 | app 生产路径 | 计划出口 | 判定 |
|---|---|---|---|---|
| P1 工程会话/配置/存储/只读快照 | 完成 | 完成（双 store shim 仍在） | 满足 | **完成** |
| P2A 语义编辑与原生 ABI | 完成 | 完成 | 满足 | **完成** |
| P2B RealEngine 采用/历史/依赖收口 | 历史栈完成，依赖收口未做 | 未完成 | 未满足 | **未完成（转 P7）** |
| P3 节点图/效果/关键帧/参数 | 完成 | 完成 | helper 物理删除到 P7A 才做 | **基本完成** |
| P4 素材/代理/波形/同步/多机位 | 完成 | 完成 | dev-test 仍直调 codec/audio | **基本完成** |
| P5 渲染/异步预览/播放/导出 | 完成 | 音频设备、RenderManager 仍直调；未采用 PreviewSession | 当前树不可验证 | **能力完成、收口未完成** |
| P6 OFX 与高级交互 | 完成 | 完成 | 满足（Cargo 依赖留给 P7） | **完成（范围内）** |

## 3. 本次实跑结果

```text
cargo test -p oak-engine -- --test-threads=1
  169 passed; 0 failed
cargo test -p oak-engine-rust-sdk -- --test-threads=1
  115 passed; 0 failed；doctests 2 ignored
cargo test -p oak-app -- --test-threads=1
  error[E0308] ×2，编译失败（APP_EXIT=101）
python3 tooling/check_app_engine_boundary.py
  207 internal references in 7 files, 10 direct internal dependencies（baseline 254）
git diff --check
  通过
```

`oak-app` 的两个错误都来自未提交的 P7 改动：

```text
graphops.rs:139 / graphops.rs:150
  project_from_storage(session.project().into_storage_project_handle())
  期望 CHandle，实得 StorageProjectHandle
```

同一转换在 `real.rs` 的 OTIO 加载路径已手工逐字段构造 `CHandle`，属漏改而非设计冲突。
修复前 `cargo test -p oak-app` 与 `cargo test --workspace` 均无法执行。

## 4. 逐阶段审计

### 4.1 P1：完成

- Engine：`EditorSessionHandle` 持有唯一 `Arc<Mutex<Project>>`、当前序列、
  marker/workarea 与 write-through 绑定。
- SDK：`EditorSession::{new,open,save,close,bind_project_to_library,...}`；
  `ProjectSnapshot` 带 `revision`；`Project` 提供 uuid/setting/cache/sequence/track/block 查询。
- 测试：`file_round_trip_preserves_the_owned_snapshot`、
  `library_lifecycle_and_live_session`、`idiomatic_config_uses_full_keys_and_native_rust_types`
  均通过。
- app：项目库 create/delete/rename/duplicate/import/export 走 `ProjectLibrary`；全局配置读取
  走 `Config::global()`。
- 保留项：写配置仍同步旧 `oak_core::configstore`（`real.rs` 的 `config_load/set_*` 三处），
  属 P1 记录声明的迁移 shim；P1 出口允许实时工程绑定转入 P2。

### 4.2 P2A：完成

- 71 个 `EditCommand` 变体、`repr(C)` 参数面、无 JSON；副作用命令单次执行。
- 迁入 48 个编辑测试；`semantic_edit_undo_redo_and_reload`、
  `all_semantic_operations_use_native_abi` 通过。
- 全变体 sweep 实际覆盖 62/71 变体：未覆盖的 11 个是 P4 的媒体/多机位命令与
  `SetNodeTextValue`，它们有各自的专项测试；文档“全变体覆盖”的表述偏宽，但不是能力缺口。

### 4.3 P2B：未完成（已明确转入 P7）

能力与历史部分：

- engine 历史栈统一，app 的 undo/redo/history/jump/group 已走 SDK。
- 旧 `UndoCommand` 通过 `migration::undo_push` 的 owned 转移入口进入同一栈。

未满足的出口：

- `oak-app/Cargo.toml` 仍有 `oak-timeline`、`oak-undo`（连同其余 8 个内部 crate）。
- 当前边界引用中，`graphops.rs` 有 `oak-timeline 25`、`oak-undo 24`、`oak-storage 17`。
- 同步剪辑仍由 app 组装 `TrackReplaceBlockWithGapCommand` +
  `TrackPlaceBlockCommand` 并 `push_multi_command`（`real.rs` 的两条 sync 路径）。
- 时间线视图仍直接遍历实时 `oak_node::graph::Graph`（`real.rs::snapshot_track`）。
- write-through 仍由 app 通过 `oak_storage::writethrough` 绑定，再用
  `StorageProjectHandle` 交给 SDK 共享同一 Arc。

因此 P2B 的“app 不再直接依赖 `oak-timeline`/`oak-undo`”出口不成立；主计划已把其余收口
转入 P7，但 P2 作为阶段不能标记为完整完成。

### 4.4 P3：基本完成

已完成：

- 会话图快照、连接校验、效果链命令、参数元数据/枚举/分组、关键帧/曲线、按钮与通知订阅。
- app 的效果库、节点编辑器、inspector 已切换；独立 SDK 工作流测试覆盖建图、效果插入、
  关键帧、按钮、CPU 渲染与保存重开。
- 当前树中 `effectchain`/`nodegraph`/`renderops` 各只剩 1 个 `NodeId` 类型导入。

保留：

- P3 完成时点“app 的节点与效果领域 helper 已删除”并不成立：HEAD 的 `effectchain` 仍有
  51 处、`nodegraph` 17 处内部引用，P3 记录自己写的是“限制为测试兼容代码”。
- 这些 helper 是在 P7A 才被删除的。因此 P3 的出口是“今天基本达成”，不是“P3 当时达成”。

### 4.5 P4：基本完成

已完成：

- `ReplaceFootage`、`DeleteEntry`、代理参数自定义/effective、`CreateSequenceFromFootage`、
  `PlaceNestedSequenceClip`、代理生成/完成/删除/启停、波形任务、缩略图任务、
  `ClipSyncInfo`、包络/变速偏移估算、多机位建序列/切换/启停。
- app 的代理、波形、缩略图、多机位向导与面板生产路径已走 SDK。

未满足/缺口：

1. `crates/oak-app/tests/waveform_e2e.rs` 仍直接调用 `oak_codec::testmedia` 与
   `oak_audio::waveform`。这违反 §5.3“normal 依赖迁完、tests 保留内部调用”，也直接违反
   P4 出口“app 不再直接依赖 `oak-codec`/`oak-audio` 的媒体 API”。
2. 按源时间/波形同步的“放置 + 单 undo 组”仍由 app 用 `oak_timeline` 原始命令拼装；
   引擎只提供偏移估算，同步用例没有整体迁入引擎。
3. 序列拖入时间线后的多机位重连在 app 内直接 `graph.connect`，既绕过 SDK，也不进 undo 栈；
   P4E 的“drop 均改走 SDK、关联撤销”不成立。

### 4.6 P5：能力完成，app 收口未完成

已完成：

- engine montage/ticket/worker 帧、`PreviewSession` source/program 传输、异步帧、
  音频块、时钟、电平、generation 失效、任务取消。
- SDK 统一导出/OTIO/FCPXML 任务、错误与取消。
- app 的视频帧、播放预取、缩略图、多机位角度帧、导出、OTIO 均走 SDK 任务；
  `audio_thread.rs`、旧 montage/ticket 运行路径已移除。

未收口：

1. `oak-app` 完全不使用 `PreviewSession`（全仓库 0 处引用），仍是 app 自己的 transport +
   任务轮询。
2. 音频设备仍直接调 `oak_audio::manager`（underrun 自愈、丢帧、推 PCM、时钟复位），
   而 SDK `audio` 已完整包装这些调用。
3. 直接访问 `oak_render::manager::RenderManager` 的 `set_inline_project` 与
   `set_workspace_size`；SDK 的 `oak_render_*` ABI 没有对应入口，属 facade 缺口。
4. 当前树编译失败，P5D 声称的 app 全量回归无法在本轮复现。

### 4.7 P6：完成（范围内）

- SDK `ofx` 统一扫描/注册、render executor、进度/取消、active viewer、project extent、
  `shutdown`；interact 的 draw/pen/key/idle/focus/redraw/swap 与 `Curve` DTO 齐全。
- app `ofx.rs` 只保留视口/键盘映射与 GL 合成，初始化与关闭顺序走 SDK；
  源码中 `oak-ofx-plugin` 引用为 0。
- 测试覆盖 interact 生命周期、shutdown 后降级、进度 handler 注销、重复初始化与
  app 的 GL 不可用降级路径。
- 尾巴：`oak-ofx-plugin` 仍在 Cargo 直接依赖中，属 P7 清零项。

## 5. 跨阶段问题

1. **计划文档状态不一致**：主计划头部仍写“状态：待实施”；P5/P6 标题没有“已完成”标记；
   §7 完成定义的 7 个复选框全部未勾；而 P5/P6 实施记录已写“已完成”。
2. **边界数字**：HEAD 664 处 / 12 文件 / 10 直接依赖；当前 207 处 / 7 文件 / 10 依赖。
   `tooling/app-engine-boundary-baseline.json` 仍写 254，尚未收紧到 207。
3. **剩余直调集中**：`graphops` 106、`real` 93；`effectchain`/`nodegraph`/`renderops`
   各 1；`waveform_e2e` 3。
4. **死代码/双实现**：`transition_commands`、`edge_transition_commands`、`trim_command`、
   `paste_clips_commands` 均无调用者，且与 SDK 的 `AddTransition*`/`TrimClip`/
   `PasteClipsInsert` 重复。
5. **SDK 依赖瑕疵**：`oak-engine-rust-sdk` 依赖 `oak-undo`，仅用于 `migration.rs` 的
   owned 转移入口；P7 删除 app 侧桥接后应一并移除，否则 app 会传递链接 `oak-undo` rlib。
6. **“独立消费者”口径偏弱**：P3/P5 的独立验收实为 SDK crate 内 unit test，且使用了
   `crate::test_support` 与 `crate::node::UNDO_SERIAL`；真正的外部消费者
   `examples/sdk_editor` 属 P8，尚未创建。

## 6. P7 剩余工作清单

1. 修复 `graphops.rs:139/150` 的 `StorageProjectHandle → CHandle` 转换，恢复编译与回归。
2. 同步放置做成引擎语义命令（或在 SDK 内组合），移除 app 的
   `TrackReplaceBlockWithGapCommand`/`TrackPlaceBlockCommand`/`push_multi_command`。
3. 多机位 drop 的直连改图移入引擎放置命令，保证进 undo 栈。
4. `waveform_e2e` 改为只用 SDK，或把覆盖迁到 SDK 测试。
5. 音频设备/时钟调用改走 `oak_engine_rust_sdk::audio`；`RenderManager` 的
   workspace size / inline project 补 SDK 入口或移除。
6. 删除 `transition_commands`/`edge_transition_commands`/`trim_command`/
   `paste_clips_commands` 等无调用者实现。
7. 移除 `oak-ofx-plugin` 等已无源码引用的 Cargo 依赖；`oak-engine-rust-sdk` 去掉
   `oak-undo` 依赖。
8. 更新主计划状态行、P5/P6 标题、§7 复选框，并把本文审计结论并入 P7 记录。

## 7. 复现命令

```bash
cargo test -p oak-engine -- --test-threads=1
cargo test -p oak-engine-rust-sdk -- --test-threads=1
cargo test -p oak-app -- --test-threads=1
python3 tooling/check_app_engine_boundary.py
cargo tree -p oak-app --depth 1
git diff --check
```
