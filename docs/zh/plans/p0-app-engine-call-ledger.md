# P0：oak-app 引擎调用台账与行为基线

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P0。  
> 初始基线：`2a663868f`；本文是实现台账，不代表后续阶段已经完成。  
> 目标 SDK 名称是后续 API 的规划名；已有模块名表示应优先复用，而非新增重复封装。

## 1. P0 出口状态

- `oakui/engine.rs`、`oakui/mock.rs`、`widgets/` 的内部 crate 路径均为 **0**。
- `EffectParam` 改用 app 自有的 `EffectValue` / `EffectValueType`；RealEngine 在
  `effectchain.rs` 中双向转换，不支持的值只读，提交时返回错误而不是写成 `None`。
- 渲染使用 `render_params` 直接保留领域值，避免经 inspector DTO 丢失非 UI 类型。
- 节点库由 `nodegraph.rs`（真实 factory）和 MockEngine（本地表）分别提供。
- Mock 多机位只保存本地 enabled/source 状态，不再创建真实图或污染全局 undo。
- `settings.rs` 集中承接 P0 的默认配置和 proxy 参数转换，P1/P4 再迁到 SDK。
- `tooling/check_app_engine_boundary.py` 和 CI 守卫已启用，覆盖 normal/dev/build
  dependencies、target-specific dependencies、`src/` 和 `tests/`。

按守卫当前口径（包括注释中的内部路径和 `oak_engine`），调用数从初始约 1,297
降为 **1,217**，分布于 26 个文件；直接内部依赖仍为 10 个，留待 P1–P7 清除。
机器可读上限在 `tooling/app-engine-boundary-baseline.json`，更新基线本身也会拒绝增长。

## 2. AppEngine 方法 → 当前实现 → 目标能力

下表按方法族完整登记 `EngineGateway`/`AppEngine` 的公共操作。`select_item`、
时钟 Entity 和纯 eyedropper mailbox 等 UI 状态不强行迁入引擎。

| AppEngine 方法 | 当前实现/内部依赖 | 阶段与目标 SDK 能力 |
|---|---|---|
| `project`、`current_sequence`、`source_media_name` | `real.rs` 工程/当前序列缓存 | P1 `EditorSession` 工程与序列快照 |
| `open_project`、`open_project_path`、`new_project`、`close_project` | `real.rs` + `graphops` + node/storage/undo | P1 `EditorSession::{new,open,close}` |
| `export_project_path`、`save_project_as` | `graphops`、storage/task | P1 `save`/`save_as`；互操作复用 `task` |
| `storage_bound`、`storage_last_error` | write-through 状态 | P1 存储状态 DTO |
| `library_*`（list/create/open/delete/rename/duplicate/import/export） | `graphops::library_*`、SQLite/write-through | P1 `ProjectLibrary` 完整方法族与统计 DTO |
| `request_frame`、`play`、`pause`、`step`、`tick` | `real.rs` transport + render/audio 调度 | P5 `PreviewSession` transport/tick |
| `source_clock`、`program_clock`、`clock_frame` | app `EngineClock`/gpui Entity | UI 保留；P5 只提供后端时钟值 |
| `cpu_frame`、`scope_data` | `real.rs` 帧缓存 + `renderops` + scopes | P5 拥有所有权的 `Frame`；scope 统计留 UI |
| `create`、`backend_name` | Real/Mock 构造与进程初始化 | P1/P5 `Engine` 生命周期；mock 留 app |
| `add_track`、`remove_track`、`delete_empty_tracks`、`set_track_height` | `graphops` + timeline/undo | P2 `EditorSession` track 命令与快照 |
| `add_adjustment_layer`、`add_default_transition`、`drop_transition_at` | `graphops` 转场/调整层组合命令 | P2 同名语义命令，单次 undo |
| `apply_timeline_event`、`split_clip`、`split_at_playhead` | `real.rs` 事件拆解 + `graphops` | P2 timeline 命令；widget 事件留 app |
| `delete_clip`、`delete_in_to_out`、`nudge_clips` | `graphops` 删除/移动/波纹组合 | P2 delete/move/ripple 命令 |
| `clipboard_copy/cut/paste/paste_insert`、`duplicate_clips` | `graphops` 剪贴板 DTO 与命令 | P2 会话剪贴板与粘贴命令 |
| `rename_clips`、`toggle_clips_enabled`、`set_clips_speed`、`nest_clips` | `graphops` block/嵌套组合 | P2 批量命令，原子 undo |
| `trim_selection_to_playhead`、`roll_selection_to_playhead` | `real.rs` 选择展开 + `graphops` | P2 trim/roll 复合命令 |
| `begin_param_drag`、`end_param_drag` | 全局 undo group | P2 SDK `EditTransaction` RAII |
| `can_undo`、`can_redo`、`undo`、`redo` | `oak_undo::global` | P2 复用并补齐 SDK undo |
| `history_entries`、`history_index`、`jump_history` | `oak_undo::global` 历史查询/jump | P2 history DTO 与 jump |
| `select_item`、`set_selected_clips` | Real/Mock 选择集 | UI 选择状态保留；命令只接收稳定 ID |
| `apply_effect_event`、`addable_effects`、`add_effect`、`add_effect_to_clip` | `effectchain` + node/undo | P3 effect library 与链命令 |
| `effect_params`、`set_effect_param` | P0 DTO + `effectchain` 转换 | P3 参数 DTO、值读写和 undo |
| `effect_push_button` | `effectchain`/plugin node | P3/P6 参数 action 命令 |
| `selected_graph_node`、`protected_graph_nodes` | `real.rs` 选择 + `nodegraph` | 选择留 UI；P3 图快照含 protected 标记 |
| `node_library`、`add_node_at`、`apply_node_graph_event` | `nodegraph` + factory/timeline/undo | P3 node library、图命令与 revision |
| `entry_path`、`project_entry_name`、`entry_is_sequence` | `graphops`/`projectbrowser` 图遍历 | P1 footage/sequence DTO |
| `replace_footage`、`rename_entry`、`delete_entry` | `graphops` + node/undo | P1/P4 素材写命令 |
| `import_footage`、`drop_footage` | `graphops` 解码/导入与放置 | P2/P4 `import_footage`、place 命令 |
| `create_folder`、`create_text_footage`、`drop_generator_clip` | `graphops` 节点创建/放置 | P2/P4 生成器与 bin 命令 |
| `footage_length_frames`、`footage_video_params` | footage behavior/probe | P4 footage metadata DTO |
| `workarea`、`set_workarea_preview`、`commit_workarea`、`clear_workarea` | `CHandle` + timeline/undo | P2 workarea DTO/命令；拖拽预览留 UI |
| `add_marker_at_playhead`、`remove_marker_at_playhead` | `CHandle` marker list + undo | P2 marker 命令 |
| `waveform_cache` | app `WaveformCache` | P4 波形快照/任务；缓存实现迁移 |
| `start_export`、`start_export_with`、`start_export_of` | `renderops` + task/channel | P5 复用 `task::export`，统一进度/取消 |
| `sequence_entries`、`current_sequence_id` | 工程树与当前序列 | P1 sequence 列表 DTO |
| `use_proxy_media`、`set_use_proxy_media` | P0 `settings` + config；real 另有失效处理 | P1/P4 配置与代理策略 |
| `playback_divider`、`set_playback_divider` | P0 `settings` + config | P1 配置；P5 预览参数 |
| `stop_on_last`、`set_stop_on_last`、`waveform_mode`、`set_waveform_mode` | P0 `settings` + config | P1 typed preferences |
| `save_frame` | app CPU image 写 PNG | UI 图片编码保留；帧来自 P5 |
| `project_ocio_config`、`set_project_ocio_config` | `real.rs` + core color/project settings | P1 复用 `state` + 工程设置 |
| `project_cache_location`、`set_project_cache_location` | project settings | P1/P5 cache policy DTO |
| `project_color_settings`、`set_project_color_settings` | core colormath + project settings | P1 复用 `state`/`color` |
| `open_sequence_id`、`sequence_parameters`、`create_sequence_with_params`、`update_sequence_parameters` | `graphops` + sequence/node/undo | P1 sequence 查询与写命令 |
| `proxy_rows/state/row`、`clip_footage_entries` | `real.rs` + codec/footage | P4 proxy DTO |
| `proxy_generate/delete/set_enabled/reveal` | codec proxy/task/render 失效 | P4 proxy 命令与任务 |
| `proxy_task_progress` | app proxy task channel | P4 统一任务 DTO |
| `proxy_set_custom_params/clear_custom_params/custom_params/effective_params` | P0 `settings` 转换 + codec/footage | P4 自有 `ProxyParams` DTO |
| `sync_eligibility`、`sync_clips_by_source_time`、`sync_clips_by_waveform` | `real.rs` + audio/timeline | P4 sync 查询与原子命令 |
| `toggle_clip_links` | `graphops` + timeline/undo | P2 link 命令 |
| `multicam_state/eligible/enabled_on_selection` | `multicam.rs` 图查询 | P4 multicam DTO/eligibility |
| `multicam_enable_selected`、`multicam_switch_to` | timeline multicam + undo | P4 enable/switch 复合命令 |
| `multicam_angle_frame` | `renderops` 异步角度帧 | P4/P5 angle frame 请求 |
| `multicam_wizard_footage/sync_offsets/create_sequence` | `multicam.rs` + audio/timeline | P4 wizard DTO、同步估算、建序列 |
| `ofx_interact_target` | `effectchain` + plugin registry | P6 plugin instance 句柄 |
| `set_eyedropper_armed`、`eyedropper_armed`、`eyedropper_picked`、`take_eyedropper_result` | gpui global mailbox | UI 保留；颜色值使用 app DTO |

## 3. 剩余直调文件与阶段归属

计数以 P0 最终 baseline 为准，包含代码、测试和注释；因此它是防增长口径，
不是运行时调用次数。

| 文件 | 数 | 主要 crate | 去向 |
|---|---:|---|---|
| `oakui/graphops.rs` | 373 | node/timeline/undo/storage/task | P1 工程/库；P2 编辑；P4 素材 |
| `oakui/real.rs` | 288 | core/node/undo/codec/task/audio | P1–P6 逐方法切换，最终只留 SDK 适配 |
| `oakui/renderops.rs` | 213 | core/node/render/codec/task | P5 montage/render/export/音频 |
| `oakui/effectchain.rs` | 90 | node/undo | P3 效果与参数；P0 DTO 转换随真实调用迁移 |
| `oakui/ofx.rs` | 53 | ofx-plugin/render | P6 插件生命周期与交互 |
| `oakui/multicam.rs` | 52 | node/timeline/core/audio | P4 多机位与同步 |
| `oakui/nodegraph.rs` | 39 | node/undo/timeline | P3 图快照与命令 |
| `oakui/gpu.rs` | 24 | core | P1/P5 色彩 DTO；wgpu 上传留 UI |
| `dialogs.rs` | 19 | codec/core | P1/P5 typed config 与 export metadata |
| `panels/ofx_params.rs` | 19 | ofx-plugin | P6 曲线 DTO/codec；控件留 UI |
| `oakui/displaycolor.rs` | 10 | core | P1 显示色彩与 ICC |
| `oakui/audio_thread.rs` | 5 | render/core | P5 音频调度 |
| `oakui/waveform.rs` | 4 | audio/codec | P4 波形提取与缓存 |
| `oakui/projectbrowser.rs` | 4 | node | P1/P4 工程树 DTO |
| `app.rs` | 3 | core | P1 配置/显示位深 |
| `oakui/settings.rs` | 3 | core/codec | P1 配置；P4 proxy DTO |
| `tests/waveform_e2e.rs` | 3 | audio/codec | P4 SDK 测试媒体/波形 |
| `handle.rs` | 2 | engine（文档引用） | P7 删除重复 RAII helper |
| `actions.rs` | 1 | core | P1 配置 |
| `i18n.rs` | 1 | node | P3 category metadata |
| `oakui/waveformsync.rs` | 1 | audio | P4 波形同步 |
| `oakui/textengine.rs` | 2 | render | P1/P5 文本初始化/字体枚举 |
| `panels/effect_library.rs` | 2 | core | P1 配置 |
| `panels/multicam.rs` | 2 | node/timeline | P4 常量/命令语义 |
| `panels/project_explorer.rs` | 2 | undo | P2 测试改走 SDK history |
| `panels/timeline.rs` | 2 | node | P3 generator/category metadata |

## 4. 冻结的行为基线

以下现有测试构成迁移对照。每个后续阶段至少运行相关行及
`cargo test -p oak-app`；完整迁移再跑 workspace。黄金帧是确定的像素/通道断言，
不是无像素内容的截图。

| 基线 | 固定测试 | 关键断言 |
|---|---|---|
| 示例工程与保存重开 | `real_project_save_load_round_trip` | 工程图、素材与序列保存后重开一致 |
| 时间线与 undo | `undo_redo_cycles_converge_to_the_same_state`、`edit_menu_operations_roundtrip`、`real_engine_multicam_switch_round_trips_through_undo` | redo 收敛、undo 还原、复合操作只产生正确历史语义 |
| 工程库 | `engine_library_lifecycle_with_sqlite`、`library_argument_validation_and_disabled_list` | create/open/rename/duplicate/delete/import/export 与参数错误 |
| 代理 | `engine_proxy_rows_and_params`、`engine_proxy_generate_and_delete`、`proxy_lifecycle_and_autostart_policy` | 参数往返、生成/删除、启停和自动生成策略 |
| 多机位 | `multicam_state_resolves_node_sequence_and_count`、`engine_multicam_wizard_sync_offsets_with_angles`、`engine_multicam_angle_frames_render` | 角度数/当前角度、同步偏移、实际角度帧 |
| 导出参数 | `encoding_params_cover_ranges_and_rejections`、`encoding_params_with_settings_cover_overrides_and_rejections` | 范围、码率、色彩、10-bit 与无效组合 |
| 预览黄金帧 | `montage_effect_stack_reaches_the_rendered_pixels`、`real_render_frame_e2e`、`graph_render_switches_multicam_source` | 效果确实改变像素、真实帧非空、切角度改变来源 |
| 音频采样与调度 | `render_entry_points_through_the_manager`、`audio_montage_overlaps_the_range`、`audio_prefetch_orders_and_serves_chunks` | 48 kHz、采样数/区间、异步顺序与去重 |
| OFX/参数往返 | `plugin_color_param_set_read_round_trip`、`color_param_set_read_round_trip`、`control_events_route_through_the_engine_callbacks` | 默认值、提交后读回、渲染侧值、undo/redo、UI 回调 |
| P0 DTO 与 Null/Mock | `app_engine_defaults_degrade_gracefully`、`app_engine_remaining_defaults_are_safe`、`proxy_params_round_trip_through_the_codec_type`、`build_control_builds_a_widget_for_every_param_kind` | 默认值不触引擎、参数值不丢、每类控件可构造 |

## 5. 守卫使用

```bash
# 每个 PR 必跑；CI 在 Build 前执行同一命令。
python3 tooling/check_app_engine_boundary.py

# 仅在计数减少后收紧基线；若有任何增长，该命令同样失败。
python3 tooling/check_app_engine_boundary.py --update-baseline
```

P0 对 `engine.rs`、`mock.rs` 和 `widgets/` 要求零容忍；后续修改这些目录时，
不得把它们重新加入 baseline。P7 完成后再把全 app 的 baseline 清零并移除迁移期
台账，只保留依赖和源码检查。
