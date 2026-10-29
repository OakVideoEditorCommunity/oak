# P7：oak-app 依赖清零实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P7。
> 状态：P7A–P7C 已完成，P7 整体完成；下一阶段为 P8。
> 审计基线见 [p1-p6-audit.md](p1-p6-audit.md)。

## 1. P7A：删除失去调用者的实现与重复测试

- 删除 `audio_thread` 及旧 montage、ticket、共享内存、旧 GPU context 和 effect 编辑
  兼容实现。`renderops` 只保留 SDK 音视频/导出适配与像素转换；`effectchain` 只保留
  效果展示和 SDK 参数 DTO。
- 移除 app 私有领域测试模块，行为覆盖归还给 engine/SDK；UI 与面板测试保留。
- `nodegraph` 的连接、位置回退和编辑统一使用 `GraphSnapshot`；项目树读取使用
  `ProjectSnapshot`。
- 显示位深、输出色彩、display ICC、codec 元数据、多机位布局和 GPU 上传改走 SDK，
  不再使用 engine 内部类型。
- 新增 display-ICC XYZ/BGRA8 processor、OpenFX instance id 查询和多机位布局 SDK
  入口。

## 2. P7B：graphops / real 的图与时间线适配收口

- `ProjectRef` 从 `Arc<Mutex<Project>>` 改为 SDK 的 `node::Project` 句柄；工程会话
  统一由 `EditorSession` 持有，app 发布一个线程本地 live session，编辑、剪贴板、
  marker/workarea 与当前序列都走同一会话。
- 时间线、工程树、素材、轨道与片段的只读查询全部换成 SDK Project 查询；
  `real.rs::snapshot_track` 不再遍历 `oak_node::Graph`。
- 同步剪辑（按源时间、按波形）改为 SDK undo group + `MoveClip`/`SetClipsSpeed`，
  删除 app 内的 `TrackReplaceBlockWithGapCommand`/`TrackPlaceBlockCommand` 组合。
- 多机位素材重新连接、生成器创建、序列 interlaced 写入等新增 SDK 入口；
  删除 `storage_bind`/`project_from_storage`/`StorageProjectHandle` 迁移桥。
- 删除无调用者的 `transition_commands`、`edge_transition_commands`、`trim_command`、
  `paste_clips_commands`、`move_clip_to_track_commands`、`shift_blocks_after` 等实现。
- `handle.rs` 的未使用 RAII 宏与 `waveform_e2e` 的 codec/audio 直调测试删除；
  app 的波形覆盖保留在 SDK 与 app 自己的 `waveform` 单测中。

## 3. P7C：依赖清零与文档

- `oak-app/Cargo.toml` 的普通依赖只剩 `oak-engine-rust-sdk`；`oak-audio`、`oak-codec`、
  `oak-core`、`oak-node`、`oak-ofx-plugin`、`oak-render`、`oak-storage`、`oak-task`、
  `oak-timeline`、`oak-undo` 全部删除。
- `oak-engine-rust-sdk` 删除对 `oak-undo` 的依赖与 `migration` 模块（app 已无调用者）。
- 边界守卫基线从 664 收紧到 **0 处引用、0 个内部依赖**，之后任何新增直调都会失败。
- `cargo tree -p oak-app --depth 1` 中 Oak 项只剩 `oak-engine-rust-sdk`。

## 4. 验证

- `cargo test -p oak-engine -- --test-threads=1`：169 通过。
- `cargo test -p oak-engine-rust-sdk -- --test-threads=1`：115 通过；doctests 2 忽略。
- `cargo test -p oak-app -- --test-threads=1`：518 通过（领域测试迁回 engine/SDK 后，
  新增 `real_engine_edits_undo_and_auxiliary_state` 覆盖真实引擎的工程/导入/undo/
  marker/workarea 往返）。
- `python3 tooling/check_app_engine_boundary.py`：0 引用 / 0 依赖。
- `cargo tree -p oak-app --depth 1`：Oak 依赖只剩 `oak-engine-rust-sdk`。
- `git diff --check` 通过。

## 5. 剩余边界与风险

- app 的 UI/视图模型仍保留 `oak-engine-rust-sdk` 的 DTO 与句柄，这是预期终态。
- 时间线重建现在逐片段调用 SDK 查询（每片段约 8 次），大型工程的重建成本高于
  旧的图内遍历；P8 前可按 profiling 增加批量时间线快照/查询。
- `StorageProjectHandle` / `adopt_storage_project_handle` 仍作为 SDK 的迁移期公共 API
  保留，app 已无调用者；可在后续清理或明确标注为兼容入口。
- P8 的 `examples/sdk_editor` 尚未创建；三平台 CI 冒烟与 SDK 使用文档仍待补齐。
