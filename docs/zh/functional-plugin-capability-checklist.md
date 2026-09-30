# 功能性插件需导出的能力清单

本文列出 Oak 若要支持**功能性插件**（扩展编辑器功能的插件，如 AI 分析、自动粗剪、转写、批量导出、外部面板；区别于 OpenFX 特效插件）时，宿主需要向插件导出的能力清单。

每项能力标注：

- **OPP/1 对应**：`docs/zh/plans/completed/external-plugin-protocol.md` 中的协议章节（已冻结的方法目录）。
- **代码落点**：该能力在现有代码中的实现位置。
- **现状**：
  - ✅ 现成可用——已有公共接口，可直接包装导出；
  - 🔶 需封装——能力存在于 app 层内部函数，需要新建稳定的门面层（HostApi）导出；
  - ❌ 需新增——机制本身不存在，需要新建。

背景：功能性插件采用进程隔离 + JSON-RPC/NDJSON + 共享内存帧传输（总体设计见 `docs/zh/plans/completed/external-plugin-system.md`），与现有 `../../crates/oak-ofx-plugin`（OpenFX 特效宿主，dlopen 进主进程）完全正交。插件宿主 crate（`oak-ofx-plugin-host`）与 SDK（`oakxp`/`oakxp-c`）尚未实现。

---

## 1. 宿主基础设施（传输与生命周期）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 1.1 | 插件发现：`~/.oak/plugins/*/plugin.toml` manifest 解析、启用状态管理 | §4 | 无（OFX 的扫描在 `oak-ofx-plugin/src/host.rs:839`，不可复用） | ❌ 需新增 |
| 1.2 | 插件进程 spawn / 握手 / 心跳 / 崩溃检测与有界重启 | §4 | 范本是 `oak-render/src/procpool.rs` 的 `ProcessDispatcher`（start/is_alive/restarts_of） | ❌ 需新增（有现成模式可泛化） |
| 1.3 | NDJSON 分帧 + JSON-RPC 2.0 双向信封编解码 | §1–§2 | `oak-render/src/ipc.rs` 与 `oak-worker/src/ipc.rs` 有 NDJSON 协议常量，但消息类型是渲染专用，无 JSON-RPC 层 | ❌ 需新增 |
| 1.4 | 共享内存帧槽池（宿主→插件、插件→宿主双向） | §10 | `procpool.rs` 的 `ShmRegionView` / `ShmFrameRef` / `release_frame` 存在，但绑定 worker 语义、只有单向 | ❌ 需新增（可泛化） |
| 1.5 | 能力位协商与检查（插件声明、宿主授予） | §7 | 无 | ❌ 需新增 |
| 1.6 | 用户确认弹窗（敏感操作需用户授权）与限流/配额 | §7.3、§12 | 无 | ❌ 需新增 |
| 1.7 | 插件日志桥（插件输出汇聚到宿主日志） | — | app 侧 `logging.rs` 为 app 私有 | ❌ 需新增 |
| 1.8 | 插件配置命名空间（插件读写自己的设置，不与 app 键冲突） | — | `oak-core/src/configstore.rs` 的 `ConfigStore` 是全局单例、无命名空间 | 🔶 需封装（加前缀隔离） |
| 1.9 | 稳定不透明实体 ID（字符串形式，跨会话有效） | §5 | `oak-node/src/id.rs` 的 `NodeId::identity() -> u64`；app 侧 `graphops::id_of` 有映射雏形 | 🔶 需封装（新增字符串映射层） |

## 2. 会话与工程（project.*）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 2.1 | 查询当前工程、当前序列、当前选择集 | §8.1–§8.2 | app 的 `RealEngine`（`oak-app/src/oakui/engine.rs`）；引擎侧无会话模型 | 🔶 需封装 |
| 2.2 | 读工程结构：文件夹树、素材列表、序列列表 | §8.2 | `oak-node/src/project.rs`、`folder.rs`、`sequence.rs`；`Graph` 全公开 | ✅ 现成（引擎层） |
| 2.3 | 新建 / 打开 / 保存 / 另存工程 | §8.2 | `oak-app/src/oakui/graphops.rs` 的 `create_project` / `load_ove` / `save_ove`；`RealEngine::new_project/open_project_path/save_project_as` | 🔶 需封装 |
| 2.4 | 工程元数据读写（设置项、插件私有数据挂载点） | §8.2 | `Project.settings: HashMap<String,String>`（`project.rs:58`） | 🔶 需封装（需约定插件键命名空间） |
| 2.5 | 工程库操作（列举 / 删除 / 复制 / 重命名 / 快照） | — | `oak-storage` 的 `DatabaseBackend`（`list_projects`、`snapshot` 等公开方法） | ✅ 现成 |
| 2.6 | 可插拔存储后端（第三方实现新存储 scheme） | — | `oak-storage/src/backend.rs:67` 的 `StorageBackend` trait + `registry::Registry::register` 公开 | ✅ 现成（唯一已有的可插拔后端接口） |

## 3. 素材与媒体（media.*）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 3.1 | 导入素材到工程（文件 → footage 节点） | §8.3 | `graphops.rs:879` 的 `import_footage` | 🔶 需封装 |
| 3.2 | 探测素材：时长、视频/音频/字幕流清单、时码 | §8.3 | `oak-codec/src/footagedescription.rs` 的 `FootageDescription` / `StreamEntry`；`FootageBehavior::probe`（`oak-node/src/footage.rs:52`） | ✅ 现成 |
| 3.3 | 代理（proxy）状态查询与生成触发 | §8.3 | `oak-codec/src/proxymanager.rs` 的 `ProxyManager`；`oak-codec/src/task.rs:144` 的 `set_task_submit_cb` | 🔶 需封装 |
| 3.4 | 波形数据：整文件提取与任意区间 min/max 摘要 | §8.6 | `oak-audio/src/waveform.rs` 的 `AudioVisualWaveform::get_summary_from_time`；`waveform::extract`（CStr + 硬编码 FFmpeg，需包一层） | ✅ 现成 |
| 3.5 | 音频电平 / 响度分析（峰值、RMS、LUFS） | — | `oak-audio/src/levelmeter.rs` 的 `analyze_sample_buffer` | ✅ 现成 |
| 3.6 | 波形对齐 / 同步（多机位、双系统声） | — | `oak-audio/src/waveformsync.rs` 的 `estimate_offset` / `extract_rms_envelope` | ✅ 现成 |

## 4. 时间线读取与编辑（timeline.*）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 4.1 | 读时间线结构：轨道、块（clip/gap/transition/adjustment）、入出点、media_in、链接组、启用状态 | §8.4 `get_structure` | 数据在 `oak-node/src/{sequence,track,block}.rs`；marker/workarea 是 `oak-timeline` 的不透明 `CHandle`——**结构横跨三个 crate，需新写聚合层** | 🔶 需封装 |
| 4.2 | 放置素材到时间线（insert / overwrite） | §8.4 `place_clip` | `graphops.rs:1605` 的 `place_footage_clip`；底层 `oak-timeline` 的 `TrackPlaceBlockCommand` / `TrackPrependBlockCommand` / `TrackInsertBlockAfterCommand` | 🔶 需封装 |
| 4.3 | 分割（含保留链接的分割） | §8.4 `split_clip` | `graphops.rs:2488` 的 `split_clip`；`BlockSplitCommand` / `BlockSplitPreservingLinksCommand` / `TrackSplitAtTimeCommand` | 🔶 需封装 |
| 4.4 | 修剪 / 滚动编辑 / 滑动 / 滑移（trim / roll / slide / slip） | §8.4 `trim_clip` `move_clip` | `graphops.rs` 的 `trim_clip` / `roll_edit` / `slide_clip` / `slip_clip`；底层 `BlockTrimCommand` / `TrackSlideCommand` | 🔶 需封装 |
| 4.5 | 删除与波纹删除、波纹删区间、删空隙 | §8.4 `delete_clip` `ripple_delete` | `graphops.rs` 的 `delete_clip` / `ripple_delete_clip`；`TrackRippleRemoveAreaCommand` / `TimelineRippleRemoveAreaCommand` / `TimelineRippleDeleteGapsAtRegionsCommand` | 🔶 需封装 |
| 4.6 | 移动 / 跨轨移动 / 启用禁用块 | §8.4 `move_clip` | `TrackMoveBlockCommand`、`BlockEnableDisableCommand` | 🔶 需封装 |
| 4.7 | 轨道增删、插入空隙 | §8.4 `add_track` | `graphops.rs` 的 `add_track` / `remove_track`；`TimelineAddTrackCommand` / `TrackListInsertGaps` | 🔶 需封装 |
| 4.8 | 转场：添加（接缝/边缘/默认）、调偏移、移除 | §8.4 `add_transition` | `graphops.rs` 的 `add_transition_at_seam` / `add_default_transition`；`TransitionSetOffsetsCommand` / `TransitionRemoveCommand` | 🔶 需封装 |
| 4.9 | 标记（marker）增删改（时间、名称、颜色） | §8.4 `add_marker` | `oak-timeline/src/marker.rs` 的 `TimelineMarkerList` + `MarkerAddCommand` 等 5 个命令 | 🔶 需封装 |
| 4.10 | 工作区（workarea）设置 | §8.4 `set_workarea` | `oak-timeline/src/workarea.rs` + `WorkareaSetRangeCommand` | 🔶 需封装 |
| 4.11 | 多机位：启用 / 禁用 / 切换机位 | — | `oak-timeline/src/multicam.rs` 的 `MultiCamEnableCommand` 等 | 🔶 需封装 |
| 4.12 | 序列参数（分辨率、帧率、音频参数）读写 | §8.4 | `SequenceBehavior`（`oak-node/src/sequence.rs:35`）、`SequenceParameters`（engine） | 🔶 需封装 |

> 时间线编辑的底座（`oak-timeline` 的 `Command` 实现集 + `oak-undo` 分组）质量很高且完整，缺的是一层把 `graphops` 这些 app 内部函数变成版本化 RPC 方法的门面。

## 5. 节点、效果与参数（node.*）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 5.1 | 读效果栈：每个 clip 的效果列表、参数当前值、关键帧 | §8.5 | `NodeBehavior` / `Input` / `KeyframeTrack`（`oak-node/src/{node,input,keyframe}.rs`）；`EffectStackDataSource`（engine trait） | 🔶 需封装 |
| 5.2 | 添加 / 移除效果、改参数、打关键帧 | §8.5 | `Graph` / `NodeCore` 全公开；参数回写撤销通道范本是 `oak-ofx-plugin/src/instance.rs` 的 `submit_undo_command` | 🔶 需封装 |
| 5.3 | 注册自定义节点类型（v2 评估） | §8（v2） | `oak-node/src/factory.rs:117` 的 `Factory::register_dynamic`——**引擎侧唯一现成的一等运行时注册点**（OFX 正通过 `register_plugin_nodes` 使用） | ✅ 现成 |
| 5.4 | 参数化曲线 / PushButton 等扩展值类型访问 | §5 | `oak-node/src/value.rs` 的 22 种 `ValueType` | ✅ 现成（引擎层） |

## 6. 渲染取帧与播放（render.* / playback.*）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 6.1 | 取合成后的帧（AI 视觉分析闭环的取帧口）：指定时间、分辨率、像素格式 | §8.6 `get_frame` | `renderops.rs:954` 的 `render_sequence_frame` / `render_footage_frame`；引擎侧 `oak-render/src/eval.rs` 的 `render_graph_frame` / `render_footage_frame`；调用范例见 `oak-cli/src/engine.rs:640` | 🔶 需封装（加限流 + shm 槽 + `FRAME_TOO_LARGE`） |
| 6.2 | 批量缩略图 | §8.6 `get_thumbnails` | `oak-render/src/cache.rs` 的 `PlaybackCache`（`CacheKind` 含缩略图）；`frameio::load_cache_frame` | 🔶 需封装 |
| 6.3 | 取音频样本（区间渲染为 PCM） | §8.6 `get_audio_levels` | `renderops.rs:1050` 的 `render_audio_range`；`eval.rs` 的 `render_audio_samples`（交错 f32） | 🔶 需封装 |
| 6.4 | 播放控制：play / pause / step / seek、查询播放头 | §8.7 | `EngineGateway`（`engine.rs:232`）的 `play/pause/step/request_frame` | 🔶 需封装 |
| 6.5 | 后台预缓存 / 帧调度优先级 | — | `oak-render/src/autocacher.rs`、`scheduler.rs` | ❌ 需新增（插件侧接口不存在） |

## 7. 导出与转码（export.*）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 7.1 | 发起导出：指定序列、编码参数、输出路径；进度 / 完成 / 取消 | §8.8 | `renderops.rs:1368` 的 `spawn_export`；`oak-task` 的 `ExportTask` + `ExportSession`（mpsc 事件通道） | 🔶 需封装 |
| 7.2 | 导出预设：枚举 / 读取 / 保存 | §8.8 | `oak-codec/src/encodingparams.rs` 的 `EncodingParams::load` / `save_to_string`（XML） | 🔶 需封装（容器/编解码是闭集枚举 `Format`/`Codec`） |
| 7.3 | 工程互操作导入导出（OTIO / FCPXML） | — | `oak-otio`（`from_json_string`、`fcpxml.rs`）；`oak-task/src/project/` 的 loadotio/saveotio 任务；`oak-storage` 的 `OtioBackend` | ✅ 现成 |
| 7.4 | 注册新导入 / 导出格式 | — | 无注册表；`oak-otio` 未知 schema 以 `Raw` 透传是唯一高保真扩展点，新格式需新写映射层 | ❌ 需新增 |

## 8. 编解码扩展（codec）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 8.1 | 注册新解码器 | — | `oak-codec/src/decoder.rs:262` 的 `Decoder` trait 公开，但生产注册表 `receive_list_of_all_decoders()` 硬编码，仅 `set_test_decoders()`（`#[doc(hidden)]` 测试注入） | ❌ 需新增公共注册表 |
| 8.2 | 注册新编码器 | — | 同上：`Encoder` trait 公开，`encoder_type_from_format()` 私有，仅 `set_test_encoders()` | ❌ 需新增公共注册表 |
| 8.3 | 读取解码帧 / 原始 PCM（供 AI 分析、转写） | §8.6 | `Decoder::retrieve_video_frame` / `retrieve_audio`；`Frame::data()` | ✅ 现成 |
| 8.4 | 硬件帧零拷贝导入 | — | `oak-codec/src/gpuinterop.rs` 的 `try_import_hw_frame` | ✅ 现成（引擎层） |

## 9. 撤销、事务与后台任务

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 9.1 | 编辑事务：`edit.begin / commit / abort` 映射为撤销分组（一次插件操作 = 一步撤销） | §6 | `oak-undo/src/global.rs` 的 `group_begin/group_end/group_abort` 已有；缺事务令牌、超时强制 abort、跨线程归属 | 🔶 需封装 |
| 9.2 | 自定义撤销命令（插件私有状态参与撤销） | §6 | `oak-undo/src/undocommand.rs` 的 `UndoCommand::from_closures(redo, undo)` | ✅ 现成 |
| 9.3 | 后台任务：启动、进度回报、取消、错误上报 | §8（任务类方法） | `oak-task/src/task.rs` 的 `Task` + `TaskBehavior` + `TaskEvent::Progress` + `CancelAtom`；`TaskManager` 单例 | ✅ 现成（缺一个"注册到 UI 进度条"的公共入口，现为外壳特判 `poll_export`） |
| 9.4 | 任务进度跨进程转发到插件 | §4 | 范本是 `oak-app/src/oakui/ofx.rs` 的 `PluginProgressEvent` + `set_progress_tx()` 通道 | 🔶 需封装 |

## 10. UI 扩展（ui.*）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 10.1 | 动态面板注册（插件面板入 dock 系统） | §8.10、§11 | dock 容器支持任意 `Render` 视图，但 `AppPanelRegistry`（`app.rs:316`）、`ShellPanels`、9 路 `dispatch_to_focused_panel` 全部硬编码 | ❌ 需新增（app 侧唯一公认的新机制，设计文档 §2.3 已点名） |
| 10.2 | 声明式 UI 协议（插件描述表单/列表，宿主渲染） | §11.1 | 无 | ❌ 需新增 |
| 10.3 | 像素面 UI（插件经 shm 上送帧，宿主贴进面板） | §11.2 | shm 基础设施同 1.4 | ❌ 需新增 |
| 10.4 | 动态命令注册（菜单项 / 命令面板条目） | — | `actions.rs` 的 `define_actions!` 宏生成编译期 `ActionId` + `REGISTRY`；`handle_global_action` 是巨型 match | ❌ 需新增 |
| 10.5 | 动态菜单项注入 | — | `oakui/component/menu.rs` 的 `Menu`/`MenuItem` 是纯数据可动态拼，但菜单项必须落到 `ActionId`，故依赖 10.4 | ❌ 需新增 |
| 10.6 | 插件快捷键注册与用户覆盖 | — | `cx.bind_keys` + `<config>/shortcuts` 覆盖表存在，但绑定编译期 action | ❌ 需新增（依赖 10.4） |

## 11. 事件订阅（Oak → plugin notification）

| # | 能力 | OPP/1 | 代码落点 | 现状 |
|---|------|-------|---------|------|
| 11.1 | 事件总线：插件订阅 / 宿主推送 notification | §4.3、§9 | 无全局 pub/sub。面板间靠 gpui `EventEmitter` + `cx.subscribe`，引擎状态靠 `cx.observe` + `cx.notify`——均无法给进程外推 | ❌ 需新增 |
| 11.2 | 工程事件：`project.opened / modified / closed` | §9 | "工程已修改"唯一近似信号是 `oak_undo::global::add_observer`（无 payload、不可注销） | ❌ 需新增 |
| 11.3 | 时间线结构变更事件：`timeline.structure_changed` | §9 | `oak-node` 的 `ChangeRecord` 类型公开但**引擎内无生产者**；graph 无 observer API | ❌ 需新增（需先在引擎侧埋点） |
| 11.4 | 播放事件：`playback.playhead_moved` 等 | §9 | 播放状态为外壳 16ms tick 轮询（`app.rs:860`），无推送 | ❌ 需新增 |
| 11.5 | 导出 / 任务完成事件 | §9 | `ExportEvent` mpsc 通道、`TaskEvent` 监听器存在，为局部通道 | 🔶 需封装（接到事件总线） |

---

## 汇总

- **现成可用（✅）**：工程图读写、探测/波形/电平/同步等音频分析、OTIO/FCPXML 互操作、存储后端注册、节点类型动态注册（`Factory::register_dynamic`）、撤销命令基座、后台任务基座、解码帧/PCM 读取。
- **需封装门面（🔶，占比最大）**：时间线全部编辑操作、工程会话操作、渲染取帧/取音、导出驱动、事务令牌、工程元数据、插件配置命名空间、实体 ID 字符串化。能力都在 `oak-app/src/oakui/graphops.rs` / `renderops.rs` 等 app 内部函数里，需要一层版本化的 HostApi 把它们组合导出。
- **需新增机制（❌）**：插件宿主本身（发现/进程管理/JSON-RPC/shm）、能力位与确认模型、动态面板注册、动态命令/菜单/快捷键、事件总线与引擎变更埋点、解码器/编码器公共注册表、声明式 UI 协议。

## 参考

- 总体设计：`docs/zh/plans/completed/external-plugin-system.md`
- 协议全文（方法/事件/能力位目录）：`docs/zh/plans/completed/external-plugin-protocol.md`
- OFX 特效宿主（正交，勿混淆）：`../../crates/oak-ofx-plugin`
