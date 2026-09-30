# Oak 功能性插件系统实现计划

> 本文是 Oak **功能性插件系统**的完整实现计划，自包含：架构、协议、需导出的能力、
> 组件分解、里程碑、验收标准全部内联，无需阅读其他文档。
>
> **定位**：功能性插件扩展的是编辑器的 **功能与工作流**（AI 剪辑、转写、批量处理、
> 外部面板、自动化），与 `../../crates/oak-ofx-plugin`（OpenFX 特效宿主，管图像处理滤镜）
> **完全正交**。旗舰用例是 AI 剪辑插件：给多模态 AI 一组工具，让它自己"看"视频
> （取帧回喂）并执行剪辑——外部程序因此必须能完整操作 Oak。

---

## 1. 红线（不可违反）

1. 插件代码**永不进入 Oak 主进程**（不 dlopen、不链接任何 Rust 库）。一个插件
   一个独立进程，插件崩溃不得连带 Oak。
2. 插件可以是任何语言（C++/Python/Node…），协议必须是**语言无关的文本协议 +
   共享内存数据面**，不发明需要链接 Rust/C ABI 的绑定。
3. 插件的一切编辑动作**必须可撤销**（UndoStack 事务），默认"确认后执行"。
4. 复用既有基础设施，不新造轮子：渲染进程隔离（`crates/oak-render/src/procpool.rs`
   + `crates/oak-render/src/ipc.rs`，已落地）的 **NDJSON over stdio + shm 帧槽**
   模式就是本系统传输层的范本。
5. 不引入 tokio 到 app 路径：IO 线程 + `std::sync::mpsc` + gpui executor 足够。
6. 不为插件发明新的引擎内部机制：宿主 API 全部是现有
   `graphops` / `renderops` / `oak_task` 函数的组合。

---

## 2. 架构总览

### 2.1 进程模型：插件 = 独立可执行文件

每个插件是一个独立可执行文件（Python 插件则是 `python3 main.py` 这样的启动
命令），Oak 按 manifest spawn，经 stdio 说话（LSP / MCP 模型）。

与"插件即动态库 + 通用宿主进程"方案相比，本方案的优势：dlopen 进宿主进程后
插件崩溃同样杀掉宿主，隔离收益为零却要求按语言内嵌加载器；解释型语言插件本来
就是"一个命令"；且与仓库既有模式一致——`oak-worker` 就是"Oak spawn 一个可执行
文件 + NDJSON 握手 + shm 附加"，含崩溃检测与有界重启（`MAX_RESTARTS=5`）。

代价：每种语言需要一个薄 SDK；stdio 单通道对极高频事件有序列化开销——用事件
合并/降频缓解（见 §5.6），不另开 socket。

### 2.2 通信结构

```
Oak 主进程                         插件进程（每插件一个）
┌─────────────────────┐  stdio   ┌──────────────────────────┐
│ PluginHost (每插件)  │◄────────►│ 插件 SDK                   │
│  ├ 后台 IO 线程      │ NDJSON   │   └ 插件逻辑（任意语言）    │
│  ├ 崩溃检测/有界重启  │ JSON-RPC │                          │
│  └ 调用编排到引擎线程 │          │                          │
│ HostApi 实现 ────────┼─► 编排到 oak-app 引擎线程（mpsc/gpui）│
│ PluginPanel (gpui)   │          │                          │
└─────────┬───────────┘          └────────────┬─────────────┘
          │        shm（帧槽池，双向）            │
          └────────────────────────────────────┘
```

- **控制面**：严格 JSON-RPC 2.0，NDJSON 分帧（一行一个消息，与
  `oak-render/src/ipc.rs` 相同），双向：Oak→plugin 发请求（UI 事件、配置下发、
  shutdown），plugin→Oak 也发请求（调内部功能），notification 做事件推送。
- **stdio 纪律**：`stdout` 只走协议消息；插件日志一律写 `stderr`，Oak 捕获后
  进日志面板。SDK 提供 `redirect_stdout_to_stderr()` 防护，防止第三方库污染
  stdout。
- **数据面**：帧/缩略图/波形/插件 UI 位图走 POSIX shm（Windows 用
  `CreateFileMappingW`），消息体只带 shm 名 + 槽位元数据。泛化
  `oak-render/src/ipc.rs` 的 `SharedMemoryRegion` / `FrameSlotPool`，不新设计。
  小数据（≤ 64 KiB）允许内联 base64。

---

## 3. 需导出的能力清单

每项标注代码落点与现状：✅ 现成可用 / 🔶 需封装（能力在 app 内部函数里，需建
HostApi 门面）/ ❌ 需新增机制。

### 3.1 宿主基础设施

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 1 | 插件发现：`~/.oak/plugins/*/plugin.toml` manifest 解析、启用状态；发现路径另有应用内 `plugins/` 与 `oak_ofx_plugin_PATH` | 无（OFX 扫描 `oak-ofx-plugin/src/host.rs:839` 不可复用） | ❌ |
| 2 | 插件进程 spawn / 握手 / 心跳 / 崩溃检测 / 有界重启 | 范本：`oak-render/src/procpool.rs` 的 `ProcessDispatcher` | ❌（有模式可泛化） |
| 3 | NDJSON 分帧 + JSON-RPC 2.0 双向信封 | `oak-render/src/ipc.rs`、`oak-worker/src/ipc.rs` 有 NDJSON 常量，无 JSON-RPC 层 | ❌ |
| 4 | shm 帧槽池（双向：down=Oak→plugin，up=plugin→Oak） | `procpool.rs` 的 `ShmRegionView`/`ShmFrameRef`/`release_frame`，但只有单向、绑 worker 语义 | ❌（可泛化） |
| 5 | 能力位协商与检查；用户确认弹窗；限流/配额 | 无 | ❌ |
| 6 | 插件日志桥（stderr/会话日志汇聚到 Oak 日志面板） | app 侧 `logging.rs` 私有 | ❌ |
| 7 | 插件配置命名空间 | `oak-core/src/configstore.rs` 的 `ConfigStore` 全局单例、无命名空间 | 🔶（加 `plugin.<id>.` 前缀隔离） |
| 8 | 稳定不透明实体 ID 字符串（素材/序列/轨道/块/节点/事务/作业统一字符串 id，会话内稳定，工程重载后作废） | `oak-node/src/id.rs` 的 `NodeId::identity() -> u64`；`graphops::id_of` 有雏形 | 🔶（需映射层） |

### 3.2 工程与会话（project.*）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 9 | 查询当前工程、当前序列、当前选择集 | `RealEngine`（`oak-app/src/oakui/engine.rs`）；引擎侧无会话模型 | 🔶 |
| 10 | 读工程结构：文件夹树、素材、序列 | `oak-node/src/project.rs`、`folder.rs`、`sequence.rs`；`Graph` 全公开 | ✅ |
| 11 | 新建/打开/保存/另存工程 | `graphops.rs` 的 `create_project`/`load_ove`/`save_ove`；`RealEngine::new_project/open_project_path/save_project_as` | 🔶 |
| 12 | 工程元数据读写（插件私有数据挂载点） | `Project.settings: HashMap<String,String>`（`project.rs:58`）；.ove 只存插件 id+版本，密钥绝不入工程文件 | 🔶 |
| 13 | 工程库操作（列举/删除/复制/重命名/快照） | `oak-storage` 的 `DatabaseBackend` 公开方法 | ✅ |
| 14 | 可插拔存储后端（第三方新 scheme） | `oak-storage/src/backend.rs:67` 的 `StorageBackend` trait + `registry::Registry::register` | ✅ |

### 3.3 素材与媒体（media.*）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 15 | 导入素材到工程 | `graphops.rs:879` 的 `import_footage` | 🔶 |
| 16 | 探测素材：时长、流清单、时码 | `oak-codec/src/footagedescription.rs`；`FootageBehavior::probe`（`oak-node/src/footage.rs:52`） | ✅ |
| 17 | 代理状态查询与生成触发 | `oak-codec/src/proxymanager.rs`；`oak-codec/src/task.rs:144` 的 `set_task_submit_cb` | 🔶 |
| 18 | 波形数据：整文件提取、任意区间 min/max 摘要 | `oak-audio/src/waveform.rs` 的 `AudioVisualWaveform::get_summary_from_time`；`waveform::extract` 需包一层（CStr + 硬编码 FFmpeg） | ✅ |
| 19 | 电平/响度分析（峰值、RMS、LUFS） | `oak-audio/src/levelmeter.rs` 的 `analyze_sample_buffer` | ✅ |
| 20 | 波形对齐/同步 | `oak-audio/src/waveformsync.rs` | ✅ |

### 3.4 时间线（timeline.*）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 21 | 读时间线结构（轨道/块/入出点/media_in/链接组/启用） | 数据在 `oak-node/src/{sequence,track,block}.rs`；marker/workarea 是 `oak-timeline` 的不透明 `CHandle`——结构横跨三个 crate，需新写聚合层 | 🔶 |
| 22 | 放置素材（insert/overwrite） | `graphops.rs:1605` 的 `place_footage_clip`；`TrackPlaceBlockCommand`/`TrackPrependBlockCommand`/`TrackInsertBlockAfterCommand` | 🔶 |
| 23 | 分割（含保留链接） | `graphops.rs:2488` 的 `split_clip`；`BlockSplitCommand`/`BlockSplitPreservingLinksCommand`/`TrackSplitAtTimeCommand` | 🔶 |
| 24 | 修剪/滚动/滑动/滑移（trim/roll/slide/slip） | `graphops.rs` 的 `trim_clip`/`roll_edit`/`slide_clip`/`slip_clip`；`BlockTrimCommand`/`TrackSlideCommand` | 🔶 |
| 25 | 删除/波纹删除/波纹删区间/删空隙 | `delete_clip`/`ripple_delete_clip`；`TrackRippleRemoveAreaCommand`/`TimelineRippleRemoveAreaCommand`/`TimelineRippleDeleteGapsAtRegionsCommand` | 🔶 |
| 26 | 移动/跨轨移动/启用禁用 | `TrackMoveBlockCommand`、`BlockEnableDisableCommand` | 🔶 |
| 27 | 轨道增删、插入空隙 | `add_track`/`remove_track`；`TimelineAddTrackCommand`/`TrackListInsertGaps` | 🔶 |
| 28 | 转场：添加（接缝/边缘/默认）/调偏移/移除 | `add_transition_at_seam`/`add_default_transition`；`TransitionSetOffsetsCommand`/`TransitionRemoveCommand` | 🔶 |
| 29 | 标记增删改（时间/名称/颜色） | `oak-timeline/src/marker.rs` + `MarkerAddCommand` 等 5 命令 | 🔶 |
| 30 | 工作区设置 | `oak-timeline/src/workarea.rs` + `WorkareaSetRangeCommand` | 🔶 |
| 31 | 多机位：启用/禁用/切换 | `oak-timeline/src/multicam.rs` 的 `MultiCamEnableCommand` 等 | 🔶 |
| 32 | 序列参数（分辨率/帧率/音频参数）读写 | `SequenceBehavior`（`oak-node/src/sequence.rs:35`）、`SequenceParameters` | 🔶 |

> 编辑底座（`oak-timeline` 命令集 + `oak-undo` 分组）完整且质量高，缺的是把
> `graphops` 这些 app 内部函数变成版本化 RPC 方法的门面。

### 3.5 节点、效果与参数（node.*）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 33 | 读效果栈：效果列表、参数值、关键帧 | `NodeBehavior`/`Input`/`KeyframeTrack`（`oak-node/src/{node,input,keyframe}.rs`）；`EffectStackDataSource`（engine trait） | 🔶 |
| 34 | 加/删效果、改参数、打关键帧 | `Graph`/`NodeCore` 全公开；参数回写撤销通道范本：`oak-ofx-plugin/src/instance.rs` 的 `submit_undo_command` | 🔶 |
| 35 | 注册自定义节点类型（v2 评估） | `oak-node/src/factory.rs:117` 的 `Factory::register_dynamic`——引擎侧唯一现成的一等运行时注册点（OFX 在用） | ✅ |
| 36 | 扩展值类型访问（22 种） | `oak-node/src/value.rs` 的 `ValueType` | ✅ |

### 3.6 渲染取帧与播放（render.* / playback.*）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 37 | 取合成帧：指定时间/分辨率/像素格式（AI 视觉闭环的关键路径） | `renderops.rs:954` 的 `render_sequence_frame`/`render_footage_frame`；`oak-render/src/eval.rs` 的 `render_graph_frame`；调用范例 `oak-cli/src/engine.rs:640` | 🔶（需加限流 + shm 槽 + FRAME_TOO_LARGE） |
| 38 | 批量缩略图 | `oak-render/src/cache.rs` 的 `PlaybackCache`；`frameio::load_cache_frame` | 🔶 |
| 39 | 取音频样本（区间渲染为 PCM） | `renderops.rs:1050` 的 `render_audio_range`；`eval.rs` 的 `render_audio_samples` | 🔶 |
| 40 | 播放控制：play/pause/seek/查询播放头 | `EngineGateway`（`engine.rs:232`）的 `play/pause/step/request_frame` | 🔶 |
| 41 | 渲染不阻塞 GUI（走 ticket/进程池） | `RenderManager` + `TicketArena`（`oak-render/src/{manager,ticket}.rs`） | ✅ |

### 3.7 导出与互操作（export.*）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 42 | 发起导出：序列 + 预设 + 输出路径；进度/完成/取消 | `renderops.rs:1368` 的 `spawn_export`；`oak-task` 的 `ExportTask` + `ExportSession`（mpsc 事件） | 🔶 |
| 43 | 导出预设枚举/读取/保存 | `oak-codec/src/encodingparams.rs` 的 `EncodingParams::load`/`save_to_string`（XML） | 🔶（容器/编解码是闭集枚举） |
| 44 | OTIO / FCPXML 导入导出 | `oak-otio`；`oak-task/src/project/` 的 loadotio/saveotio；`oak-storage` 的 `OtioBackend` | ✅ |
| 45 | 注册新导入/导出格式 | 无注册表；`oak-otio` 未知 schema 以 `Raw` 透传是唯一扩展点 | ❌ |

### 3.8 编解码扩展（v2 候选）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 46 | 注册新解码器/编码器 | `Decoder`/`Encoder` trait 公开，但生产注册表硬编码（`decoder.rs:509`），仅 `#[doc(hidden)]` 测试注入 | ❌ |
| 47 | 读取解码帧/原始 PCM（AI 分析、转写） | `Decoder::retrieve_video_frame`/`retrieve_audio`；`Frame::data()` | ✅ |
| 48 | 硬件帧零拷贝导入 | `oak-codec/src/gpuinterop.rs` 的 `try_import_hw_frame` | ✅ |

### 3.9 撤销、事务与后台任务

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 49 | 编辑事务分组（一次插件操作 = 一步 Ctrl-Z） | `oak-undo/src/global.rs` 的 `group_begin/group_end/group_abort`；缺事务令牌、超时强制 abort、跨线程归属 | 🔶 |
| 50 | 自定义撤销命令 | `oak-undo/src/undocommand.rs` 的 `UndoCommand::from_closures` | ✅ |
| 51 | 后台任务：启动/进度/取消/错误 | `oak-task/src/task.rs` 的 `Task`/`TaskBehavior`/`TaskEvent`/`CancelAtom`；`TaskManager` 单例 | ✅（缺"注册到 UI 进度条"公共入口） |
| 52 | 任务进度跨进程转发 | 范本：`oak-app/src/oakui/ofx.rs` 的 `PluginProgressEvent` + `set_progress_tx()` | 🔶 |

### 3.10 UI 扩展（ui.*）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 53 | **动态面板注册**（插件面板入 dock） | dock 容器支持任意 `Render` 视图，但 `AppPanelRegistry`（`app.rs:316`）、`ShellPanels`、9 路 `dispatch_to_focused_panel` 全部硬编码 | ❌（app 侧唯一公认的新机制） |
| 54 | 声明式 UI 协议（JSON 控件树 → gpui 渲染） | 无 | ❌ |
| 55 | 像素面 UI（shm 上送位图 + 输入事件转发） | 事件模型照抄 OFX interact 路径（`oak_ofx_plugin::gl_bridge` + `oakui/ofx.rs::forward_interact_pointer/key`） | ❌ |
| 56 | 动态命令/菜单/快捷键注册 | `actions.rs` 的 `define_actions!` 生成编译期 `ActionId`+`REGISTRY`；菜单项必须落到 `ActionId` | ❌ |

### 3.11 事件订阅（Oak → plugin）

| # | 能力 | 代码落点 | 现状 |
|---|------|---------|------|
| 57 | 事件总线：订阅/推送 notification | 无全局 pub/sub；gpui `EventEmitter`+`cx.subscribe` 无法出进程 | ❌ |
| 58 | 工程事件（opened/modified/closed） | 唯一近似信号：`oak_undo::global::add_observer`（无 payload、不可注销） | ❌ |
| 59 | 时间线结构变更事件 | `oak-node` 的 `ChangeRecord` 类型公开但引擎内无生产者；graph 无 observer | ❌（需引擎埋点） |
| 60 | 播放头/播放状态事件 | 现为外壳 16ms tick 轮询（`app.rs:860`），无推送 | ❌ |
| 61 | 导出/任务完成事件 | `ExportEvent` mpsc、`TaskEvent` 监听器为局部通道 | 🔶（接入事件总线） |

**汇总**：✅ 约 15 项可直接包装；🔶 约 25 项需建 HostApi 门面（占比最大，是
主体工作量）；❌ 约 15 项需新建机制（插件宿主、能力/确认/限流、动态面板与
命令注册、事件总线、声明式 UI、编解码注册表）。

---

## 4. 协议规范（OPP/1，实现以本节为准）

协议主版本 `1`（`"api": 1`）。同一主版本内只增不删（演进规则见 §4.10）。

### 4.1 传输层与分帧

- 通道：插件进程的 `stdin`（Oak→plugin）与 `stdout`（plugin→Oak），全双工。
- 分帧：NDJSON——每条消息一行 UTF-8 JSON，`\n` 结尾，消息内不得有裸换行，
  禁用 BOM。
- 消息大小上限 **16 MiB**；超限判协议错误并杀死插件。
- Oak 关闭插件 stdin 写端 = 要求插件退出。

### 4.2 消息信封（JSON-RPC 2.0，双向）

```json
// Request           {"jsonrpc":"2.0","id":42,"method":"timeline.split_clip","params":{...}}
// Response 成功     {"jsonrpc":"2.0","id":42,"result":{...}}
// Response 失败     {"jsonrpc":"2.0","id":42,"error":{"code":-32001,"message":"not in edit transaction"}}
// Notification      {"jsonrpc":"2.0","method":"playback.playhead_moved","params":{...}}
```

- `id` 由发送方自定命名空间，接收方配对时只看自己发出的 id。
- 允许任意数量 in-flight 请求；**同一事务内的变更请求严格按到达顺序串行执行**。
- `params` 一律为对象。
- 需用户确认的请求在用户裁决前不返回响应；SDK 默认请求超时 120 s。

### 4.3 错误码

标准码（-32700/-32600/-32601/-32602/-32603）按 JSON-RPC 规范。应用码：

| code | 常量 | 含义 |
|---|---|---|
| -32000 | `CAPABILITY_DENIED` | 插件无此方法所需能力位 |
| -32001 | `NOT_IN_TRANSACTION` | 变更方法缺少有效 `txn` |
| -32002 | `TRANSACTION_CONFLICT` | 事务被其他持有者占用 |
| -32003 | `ENTITY_NOT_FOUND` | id 失效，`data.entity` 带原 id |
| -32004 | `RATE_LIMITED` | `data.retry_after_ms` 给重试间隔 |
| -32005 | `SHM_EXHAUSTED` | shm 池无空闲槽，先 `shm.release` |
| -32006 | `CONFIRMATION_DENIED` | 用户在确认弹窗中拒绝 |
| -32007 | `FRAME_TOO_LARGE` | 请求帧超过槽容量，调小 `max_size` |
| -32008 | `INVALID_STATE` | 当前状态不允许（如无打开的工程） |

Oak 侧合成错误（插件进程已死、握手失败）不进协议，直接体现在插件管理器 UI。

### 4.4 生命周期

**握手**（由插件发起）：Oak spawn 插件后，插件须在 **10 s** 内发出首消息
`session.hello`：

```json
→ {"jsonrpc":"2.0","id":1,"method":"session.hello","params":{
    "api":1,"name":"ai-cut","version":"0.1.0",
    "capabilities":["project.read","timeline.read","timeline.edit","render.frame","ui.panel"],
    "panels":[{"id":"chat","title":"AI 剪辑","ui":"declarative"}],
    "subscribe":["project.opened","timeline.structure_changed"]}}
← {"jsonrpc":"2.0","id":1,"result":{
    "api":1,"oak_version":"0.5.0","granted":[...],"features":["ui.pixel"],
    "shm":{"down":{"name":"oakxp-d-1234","slots":8,"slot_bytes":16777216}}}}
```

- `capabilities` 必须是 manifest 声明集的子集；`granted` 是 Oak 实际授予的子集
  （用户可在安装时裁剪），插件按 `granted` 工作。
- `features`：同主版本内可选特性探测集（如 `"ui.pixel"`）。
- 超时或首消息非 `session.hello`：杀进程，标记启动失败。

**心跳**：握手后 Oak 每 **2 s** 发 `session.ping`，插件应在 **5 s** 内响应。
连续 3 次超时或 stdout EOF/进程退出 = 崩溃：该插件面板显示"已崩溃 [重启]"
徽标，未完成调用全部以插件死亡错误返回，按 manifest `[restart]` 有界自动重启
（默认 `max=5`，退避 1 s 起倍增）。重启后重新握手；所有 id 与事务令牌作废。

**关闭**：Oak 发 notification `session.shutdown {reason}`，插件 2 s 内自行退出；
超时 SIGTERM，再 2 s SIGKILL（Windows：`TerminateProcess`）。

**事件订阅**：`events.subscribe {events:[], unsubscribe:[]}`；订阅需要对应的
read 类能力位。

### 4.5 公共数据类型

| 类型 | JSON 表示 | 说明 |
|---|---|---|
| `Rational`（时间） | `{"num":3,"den":25}` | 秒为单位有理数，`den>0`。全协议唯一时间表示 |
| `TimeRange` | `{"in":Rational,"out":Rational}` | 左闭右开 |
| `EntityId` | 不透明字符串 | 插件不得解析格式；会话内稳定，工程重载后全部作废 |
| `Color` | `"#RRGGBB"` / `"#RRGGBBAA"` | |
| `FrameRef` | shm 形态 `{"shm":{"region":"down","slot":3},"format":"bgra8","width":…,"height":…,"stride":…,"bytes":…,"time":…}`；inline 形态（≤64 KiB）`{"inline":"base64…","format":"png",…}` | shm 槽位是借用，用完必须 `shm.release`，30 s 租约超时强制回收 |

### 4.6 编辑事务协议（铁律 3 的落地）

一切变更方法必须携带 `txn` 参数；事务令牌由 `edit.begin` 签发：

```json
{"id":10,"method":"edit.begin","params":{"label":"AI: 粗剪访谈片段"}}
{"id":10,"result":{"txn":"t12"}}
{"id":11,"method":"timeline.split_clip","params":{"txn":"t12","clip":"…","time":{"num":3,"den":25}}}
{"id":12,"method":"edit.commit","params":{"txn":"t12"}}
```

规则（钉死）：

1. **全局单持**：同一时刻全 Oak 只有一个未决事务，冲突返回
   `TRANSACTION_CONFLICT`。Oak 对 60 s 未提交的事务强制 `abort`。
2. `commit` = 一组 UndoCommand 压栈（映射 `oak_undo::global::group_begin/group_end`），
   历史面板显示"插件名：label"，一次 Ctrl-Z 整体撤销；`abort` = 逆序回滚不留痕。
3. 事务内变更串行执行；任一失败，已成功的保持有效，由插件决定 commit/abort。
4. 插件崩溃时未决事务自动 `abort`。
5. `edit.undo`/`edit.redo` 不需要 `txn`，撤销整个 UndoStack（含用户操作）——
   插件应只在用户明确要求时调用。
6. **未在事务内的变更调用直接返回 `NOT_IN_TRANSACTION`**，从协议上杜绝不可
   撤销的编辑。

### 4.7 能力位（v1 冻结）

| 能力 | 覆盖的方法/事件 |
|---|---|
| `project.read` | `project.get_info`；`project.opened/modified/closed` 事件 |
| `project.edit` | `project.open/save`（需事务） |
| `media.read` | `media.probe/list_footage` |
| `media.import` | `media.import_footage`（需事务） |
| `timeline.read` | `timeline.get_structure`；`timeline.structure_changed` 事件 |
| `timeline.edit` | `timeline.*` 全部变更方法（需事务） |
| `node.read` | `node.list_types/get_params` |
| `node.edit` | `node.add_effect/set_param/set_keyframe/remove`（需事务） |
| `render.frame` | `render.get_frame/get_thumbnails/get_audio_levels` |
| `playback` | `playback.*` 方法与事件 |
| `export` | `export.start/cancel`；`export.*` 事件 |
| `ui.panel` | `ui.*`（声明式）；`ui.event` 事件 |
| `ui.pixel` | `ui.attach_surface/frame_ready`（像素面） |

**检查时机**：HostApi 在每个方法入口查 `granted`，越权返回
`CAPABILITY_DENIED` 并记日志；事件订阅同理。

**用户确认**：`*.edit`、`media.import`、`export`、`edit.undo/redo` 为确认类，
Oak 弹窗"插件 X 请求：split_clip n17 @ 3/25 [允许] [本会话内允许] [拒绝]"。
拒绝返回 `CONFIRMATION_DENIED`；"本会话内允许"缓存到会话结束；插件设置里
可整设为"自动允许"。

### 4.8 宿主 API 方法（v1 全量）

通用列：**事务** = 需要 `txn`；**确认** = 触发确认弹窗。

**会话**：`session.hello` / `session.ping`（Oak→plugin）/
`session.shutdown`（Oak→plugin notification）/
`session.log {level, message}`（plugin→Oak notification，进日志面板）/
`events.subscribe`。

**project.\***：

| 方法 | 事务 | 确认 | params → result |
|---|---|---|---|
| `project.get_info` | 否 | 否 | `{}` → `{path\|null, name, modified, sequences:[{id,name,fps,duration}]}` |
| `project.open` | 是 | 是 | `{txn, path}` → `{name}` |
| `project.save` | 是 | 是 | `{txn, path?}` → `{path}` |

无打开工程时读取方法返回 `INVALID_STATE`。

**media.\***：

| 方法 | 事务 | 确认 | params → result |
|---|---|---|---|
| `media.probe` | 否 | 否 | `{path}` → `{duration, streams:[{type, codec, width?, height?, fps?, sample_rate?, channels?}]}` |
| `media.list_footage` | 否 | 否 | `{}` → `{footage:[{id,name,path,duration}]}` |
| `media.import_footage` | 是 | 是 | `{txn, paths:[…]}` → `{footage:[…], errors:[{path,message}]}` |

**timeline.\***：`get_structure {sequence}` 返回
`{sequence:{id,name,fps,duration,tracks:[{id,type,index,clips:[{id,name,footage,in,out,media_in,enabled}]}]}}`。
变更方法（全部 事务=是、确认=是）：`add_track {sequence,type,index?}`、
`place_clip {sequence,track,footage,in,media_in?}`、`split_clip {clip,time}` →
`{clips:[id,id]}`、`trim_clip {clip,side:"in"|"out",time}`、
`move_clip {clip,in,track?}`、`delete_clip {clip}`、`ripple_delete {clip}`、
`add_transition {clip,side,type,duration}`、
`add_marker {sequence,time,name?,color?}`、`set_workarea {sequence,range}`。
越界/重叠冲突返回 `INVALID_STATE`，`data.reason` 说明。

**node.\***：`list_types {category?}` → 含 OFX 动态类型；`add_effect`（事务/确认）；
`get_params {node}` → `{params:[{key,name,type,value,default,min?,max?,choices?}]}`，
`type ∈ float|int|bool|string|color|vec2|choice`；`set_param`/`set_keyframe`/`remove`
（均事务/确认）。`value` 的 JSON 类型随 `type`。

**render.\***（AI 视觉闭环取帧口，均非事务非确认）：

| 方法 | params → result |
|---|---|
| `render.get_frame` | `{sequence?\|footage?, time, max_size?, format?:"bgra8"\|"png"}` → `{frame:FrameRef}` |
| `render.get_thumbnails` | `{sequence?\|footage?, range, count≤64, height?:180}` → `{frames:[FrameRef,…]}`（等间隔采样） |
| `render.get_audio_levels` | `{sequence, range, resolution?:100}` → `{channels, peaks: inline base64 float32le}` |

`max_size` 超槽容量返回 `FRAME_TOO_LARGE`。渲染走引擎 ticket/进程池路径，不
阻塞 GUI；典型延迟 50–500 ms，插件应并发流水线化。

**playback.\***（均非事务非确认）：`play {sequence?}`、`pause`、`seek {time}`、
`get_state` → `{playing, time, sequence|null}`。

**export.\***：`export.start {sequence, output_path, preset?}` → `{job}`（确认=是）；
`export.cancel {job}`。自定义编码参数 v1 不开放。进度经 `export.progress` 事件。

**shm.\***：`shm.release {slots:[{region,slot},…]}`（request 或 notification）。

### 4.9 事件（Oak→plugin notification）

| 事件 | 所需能力 | params | 频率 |
|---|---|---|---|
| `project.opened` / `project.closed` | `project.read` | `{path,name}` / `{}` | – |
| `project.modified` | `project.read` | `{modified}` | 状态翻转时 |
| `timeline.structure_changed` | `timeline.read` | `{sequence, hint:"full"\|{clips_added,clips_removed,clips_moved}}` | 合并后发，≤ 10 Hz |
| `playback.playhead_moved` | `playback` | `{sequence, time}` | ≤ 30 Hz，只发最新值 |
| `playback.state_changed` | `playback` | `{playing}` | – |
| `export.progress` | `export` | `{job, fraction, eta_ms?\|null}` | ≤ 4 Hz |
| `export.done` | `export` | `{job, ok, output_path?, error?}` | – |
| `ui.event` | `ui.panel` | 见 §5.5 | 输入实时；pointer_move ≤ 60 Hz 合并 |

`hint` 是优化提示：插件可永远按 `"full"` 处理（重新 `get_structure`）。

### 4.10 shm 数据面

- **区域与方向**：`down`（Oak→plugin 渲染帧）握手前创建，握手响应携带
  `{name, slots, slot_bytes}`，插件只读 attach；`up`（plugin→Oak 像素面 UI 位图）
  在 `ui.attach_surface` 时按需创建，插件可写 attach。POSIX 用
  `shm_open`/`mmap`，Windows 用 `CreateFileMappingW`/`MapViewOfFile`。
- **无头部、无锁（钉死）**：shm 内不放元数据、不放锁。槽 `i` 占字节区间
  `[i*slot_bytes, (i+1)*slot_bytes)`，格式/宽/高/步长全部由控制面消息携带。
  槽位有效性由 RPC 配对界定：`down` 从携带该槽的响应到达到插件 `shm.release`
  （或 30 s 租约到期）Oak 保证不写；`up` 从 `ui.frame_ready` 发出到 Oak 回
  `ui.surface_ack` 插件保证不写。
- **流控**：`down` 默认 8 槽，借用满后 `render.*` 一律 `SHM_EXHAUSTED`，插件
  必须流水线化 release；`up` 固定 3 槽（三缓冲），全在飞行中时插件丢帧。

### 4.11 限流与配额（v1 默认值，随握手 `limits` 字段下发）

| 资源 | 默认 | 超限行为 |
|---|---|---|
| `render.get_frame` | 8 帧/s（令牌桶，burst 4） | `RATE_LIMITED` + `retry_after_ms` |
| 渲染帧短边 | ≤ 1080 px | `RATE_LIMITED` |
| `get_thumbnails` | count ≤ 64/次 | `INVALID_PARAMS` |
| 单条消息 | ≤ 16 MiB | 协议错误，杀进程 |
| `down` 借用 | ≤ 8 槽 | `SHM_EXHAUSTED` |
| 未决事务时长 | ≤ 60 s | 强制 `abort` |

### 4.12 版本演进规则

1. `api` 主版本只在破坏性变更时 +1；Oak 同时支持的旧主版本数 ≥ 1。
2. 同主版本内只准新增方法/事件/可选参数/能力位；不改语义、不删、可选不变必填。
3. 可选能力经握手 `features` 字符串集探测，插件用前必查。
4. 插件声明的 `api` 高于 Oak 支持：握手返回 `INVALID_PARAMS`，
   `data.supported_api` 给出 Oak 侧主版本，插件应降级或退出。

---

## 5. 插件 UI（两条路径，握手时声明，可同时用）

### 5.1 为什么不做 webview / 直接调 gpui

gpui 没有 webview，也不可能让 Python 插件直接调 gpui。提供声明式与像素面两条
路径。

### 5.2 声明式 UI（v1 基线，推荐大多数插件）

插件用 JSON 描述控件树，Oak 用 gpui_widgets 渲染成 `PluginPanel` 内容：

```json
{"method":"ui.set_tree","params":{"panel":"chat","root":
  {"type":"column","gap":8,"children":[
    {"type":"chat_log","id":"log","grow":true},
    {"type":"row","gap":4,"children":[
      {"type":"text_input","id":"prompt","placeholder":"描述你的剪辑意图…","grow":true},
      {"type":"button","id":"send","text":"执行"}]},
    {"type":"progress","id":"job","visible":false}]}}}
```

- 增量更新：`ui.set_props {panel, id, props}`；不存在的 id 返回
  `ENTITY_NOT_FOUND`。结构性增删用全量 `set_tree`（树规模小，不做 diff）。
- 控件目录（v1 冻结）：`column/row`（布局）、`label`、`button`（click）、
  `text_input`（change/submit）、`text_area`、`list`（select）、`chat_log`、
  `image`（inline base64 或 shm source）、`progress`、`slider`、`checkbox`、
  `separator/spacer`。
- 事件上行：`ui.event {panel, id, kind, …}`。面板被关闭发
  `ui.event {kind:"panel_closed"}`；重新打开发 `panel_shown`，插件应重发
  `ui.set_tree`。
- 通知：`ui.notify {level, text}` → Oak 状态栏 toast。
- 收益：零崩溃面（插件不画一个像素）、风格与 Oak 一致、实现量最小。

### 5.3 像素面 UI（完整能力路径）

插件在自己进程里用任意工具包（Qt/imgui/web 引擎）离屏渲染，BGRA 位图经 shm
推给 Oak，Oak 在 `PluginPanel` 里原样贴图：

```json
// 1) 建表面（Oak 创建 up 区域）
→ {"id":40,"method":"ui.attach_surface","params":{"panel":"paint","width":960,"height":540,"dpi":2.0}}
← {"id":40,"result":{"shm":{"region":"up","name":"oakxp-u-1234-paint","slots":3,"slot_bytes":8294400},"format":"bgra8"}}
// 2) 插件画好一帧（notification）
→ {"method":"ui.frame_ready","params":{"panel":"paint","slot":1,"width":960,"height":540,"stride":7680,"dirty":[0,0,960,540]}}
// 3) Oak 合成完毕 ack（notification），槽位可复用
← {"method":"ui.surface_ack","params":{"panel":"paint","slot":1}}
```

输入事件下行（`ui.event`，`id` 固定 `"surface"`）：`resize {width,height,dpi}`、
`pointer_move/down/up {x,y,button?,modifiers}`（逻辑坐标，已除 dpi）、
`scroll {x,y,dx,dy,modifiers}`、`key_down/key_up {key,text?,modifiers}`（key 为
USB HID usage name）、`focus/blur`。pointer_move 合并到 ≤ 60 Hz。事件模型照抄
OFX interact 的语义。v1 不做：IME 合成串、剪贴板互通、跨进程拖拽。

### 5.4 其他 UI 形态

- 独立窗口：插件进程自己开 OS 窗口，Oak 不管，始终允许，适合调试工具。
- 监看器叠加层（viewer overlay）：归 OFX 范畴；功能插件的 viewer overlay
  （如 AI 打点预览）列为 v2 候选。

### 5.5 事件降频

`playhead_moved`、`pointer_move` 等高频事件：Oak 侧合并到 30/60 Hz 上限、只发
最新值，避免 stdio 被事件洪水淹没。

### 5.6 动态面板注册（app 侧唯一公认的新机制）

`PluginPanel` 实现 gpui `DockPanel`，注册进 dock。当前
`AppPanelRegistry`（`oak-app/src/app.rs:316`）的 `panel_key`/`build_panel` 是
硬编码 match，`ShellPanels` 9 个面板字段固定，`dispatch_to_focused_panel` 9 路
match——需改为"静态内置 + 动态注册"双层：内置面板走原 match，插件面板进
`HashMap<String, PanelFactory>`。动态命令/菜单注册（§3.10 #56）同此模式：
`ActionId` 编译期集合保持不变，新增运行时 `DynamicActionRegistry`，菜单构建时
合并。

---

## 6. 组件分解与工作项

### 6.1 新增 crate：`oak-ofx-plugin-host`（与 `oak-worker` 平级的消费者角色）

| 模块 | 职责 | 关键实现要点 |
|---|---|---|
| `manifest.rs` | `plugin.toml` 解析、发现路径扫描（`~/.oak/plugins/`、应用内 `plugins/`、`oak_ofx_plugin_PATH`）、启用状态持久化 | manifest 含 `id/name/version/api/[process]command/env_passthrough/capabilities/[restart]` |
| `process.rs` | spawn/管道/心跳/崩溃检测/有界重启 | 照搬 `procpool.rs` 的 `ProcessDispatcher` 生命周期（MAX_RESTARTS、退避、握手超时） |
| `transport.rs` | NDJSON 分帧 + JSON-RPC 2.0 双向信封、id 配对 | 独立 IO 线程，`mpsc` 与 gpui `cx.spawn` 编排回引擎线程；不引入 tokio |
| `host_api.rs` | 方法分发：能力检查 → 确认弹窗 → 编排到 `graphops`/`renderops`/`oak_task` | 每方法入口查 `granted`；确认类先弹窗；错误码按 §4.3 |
| `transaction.rs` | 事务令牌签发/校验/超时强制 abort/崩溃自动 abort | 映射 `oak_undo::global::group_begin/group_end/group_abort`；全局单持锁 |
| `shm_pool.rs` | 双向 shm 区域与槽位池 | 泛化 `oak-render/src/ipc.rs` 的 `SharedMemoryRegion`/`FrameSlotPool`；30 s 租约回收 |
| `entity.rs` | `NodeId::identity()` ↔ 不透明字符串 id 映射，工程重载失效 | 会话级 `HashMap` 双向映射 |
| `events.rs` | 事件总线：引擎埋点 → 订阅匹配 → 降频合并 → notification | 见 §6.3 |
| `panel.rs` | `PluginPanel`（gpui DockPanel 壳）+ 声明式控件树渲染 + 像素面贴图与输入转发 | 控件树 → gpui_widgets；像素面照抄 OFX interact 事件语义 |

### 6.2 插件 SDK

- **`oakxp-c`**（头文件-only C/C++ SDK，放 `shared/include/oakxp/`）：NDJSON
  分帧、JSON-RPC 收发、shm 附加、回调注册、stdout→stderr 防护。无第三方依赖。
  纯协议，不含任何 Oak 内部类型——这是 C ABI 纪律下唯一允许插件 #include 的东西。
- **`oakxp`（Python 包）**：`pip install oakxp` 或随 Oak 分发；asyncio 友好但
  非强制；`frame.to_png()` 依赖 Pillow（可选 extra）。
- 各约 200 行，插件作者只写 `on_request(method, params)` 回调。

### 6.3 app 侧新增扩展点（`oak-app`）

| 扩展点 | 现状 | 工作项 |
|---|---|---|
| 动态面板注册 | `AppPanelRegistry`/`ShellPanels`/9 路 dispatch 硬编码 | 加动态注册层（§5.6） |
| 动态命令/菜单/快捷键 | `ActionId`/`REGISTRY` 编译期常量 | 运行时 `DynamicActionRegistry`，菜单构建合并 |
| 事件源埋点 | graph 无 observer；`ChangeRecord` 无生产者 | 在 HostApi 变更路径与引擎关键点发事件；`oak_undo::global::add_observer` 作为 modified 信号源（需加 payload 或可注销变体） |
| 播放头推送 | 16ms tick 轮询 | 在 tick 里取 `program_clock.current_frame()` 按 30 Hz 降频推给订阅者 |
| HostApi 接线 | `oakui/ofx.rs:206 ofx::init()` 是现成模板 | 照抄其形状：扫描 → 注册 → 装钩子 |
| 插件管理器面板 | 无 | 内置面板：插件列表/启用/能力展示/崩溃徽标与重启/日志查看 |
| 确认弹窗 | 无 | 确认对话框 + "本会话内允许"缓存 + 插件设置"自动允许" |

### 6.4 参考插件（验收的一部分）

1. `examples/plugin-echo`（C++）：注册声明式面板，按钮触发 `project.get_info`
   并显示——验证协议与 UI 基线。
2. `examples/plugin-roughcut`（Python）：接多模态 LLM，实现"聊天指令 →
   `get_thumbnails` 扫时间线 → 事务化 `split`/`ripple_delete` → `get_frame`
   验证"的 AI 粗剪闭环。

---

## 7. 里程碑

| 里程碑 | 内容 | 验收 |
|---|---|---|
| **P1 传输与生命周期** | `oak-ofx-plugin-host`：manifest 解析、spawn/握手/心跳/崩溃检测/有界重启；JSON-RPC 双向收发；`oakxp-c` 最小 SDK；echo 插件跑通 `project.get_info` | 杀掉插件进程：Oak 不崩、面板显示崩溃徽标、可重启；握手超时路径有测试 |
| **P2 宿主 API 核心** | `edit.*` 事务 + `project/media/timeline/node` 方法族 + 能力检查 + 实体 id 映射 + 确认弹窗 | 插件完成"导入素材→铺轨→切开→波纹删除→加效果→改参数"，逐步可在历史面板撤销；越权调用被拒；未带 txn 的变更被拒 |
| **P3 取帧与导出** | `render.*` shm 数据面、`export.*` 事件、限流配额 | 黄金帧校验（复用 render-worker 端到端 harness）：插件取到的帧与 viewer 一致；连续取帧不拖垮进程池；超限正确返回 RATE_LIMITED |
| **P4 声明式 UI** | `PluginPanel` + 动态 panel 注册 + `ui.*` 控件集 + 事件总线（`project.*`/`timeline.structure_changed`/`playback.*`/`export.*`） | echo 插件面板交互全通；控件树快照测试；事件降频符合 §4.9 上限 |
| **P5 像素面 UI** | shm 贴图 + 输入转发 + resize/DPI | 参考 imgui 插件 60fps 交互无撕裂；事件转发对齐 interact 语义 |
| **P6 Python SDK 与 AI 粗剪** | `oakxp` 包 + `plugin-roughcut` + 动态命令/菜单注册 | Mock LLM 录制/回放（无网络 CI）跑通"看图→下刀→验证"闭环 |

依赖关系：P1–P3 是系统地基，严格串行；P4 依赖 P1、可与 P2/P3 并行；P5 依赖
P4；P6 依赖 P2–P4（SDK 与宿主 API 稳定后随时可开始）。

---

## 8. 安全与边界

**安全模型**：

- 能力位在 manifest 声明、安装/升级时向用户展示差异、每次调用入口检查。
- 确认类操作默认弹窗，可缓存到会话或整插件设为自动。
- 一切编辑可撤销，历史面板显示"插件名：事务标签"。
- 限流与配额防批量取帧拖垮渲染进程池、防内存炸弹（消息 16 MiB 上限）。
- 插件所需 API key 走插件自己的环境变量/配置文件，**绝不写入 .ove 工程文件**
  （.ove 只存插件 id + 版本）。
- **不做沙箱**：本系统隔离的是"崩溃"，不是"恶意"——插件进程与 Oak 同用户
  权限。恶意插件防护（seccomp/签名/商店审核）明确出范围。

**明确不做**：

- 不取代 OFX：图像处理节点仍走 `oak-ofx-plugin`。功能插件注册新节点类型 v2 再
  评估（机制上 `Factory::register_dynamic` 现成）。
- 不做插件沙箱、签名、商店。
- 不做跨机器/网络插件（stdio only；协议本身不绑定 stdio，socket 变体留作以后）。
- 不为插件发明新的引擎内部机制。
- 不引入 tokio 到 app 路径。
- v1 不做：IME 合成串转发、剪贴板互通、跨进程拖拽、自定义导出编码参数
  （只开放预设名引用）、编解码器插件注册表。

---

## 9. 风险与对策

| 风险 | 对策 |
|---|---|
| 高频事件（playhead、pointer_move）淹没 stdio | 合并 + 降频（§5.5）；只发最新值 |
| 批量取帧拖垮渲染进程池，影响用户交互 | 限流（8 fps/短边 1080）+ shm 槽有界 + `SHM_EXHAUSTED` 背压 |
| 插件长持事务锁死其他编辑 | 全局单持 + 60 s 强制 abort + 崩溃自动 abort |
| Python 第三方库污染 stdout 破坏协议 | SDK 初始化时 stdout→stderr 重定向；Oak 侧对非 JSON 行容错记日志 |
| 实体 id 在工程重载后悬空 | id 会话级生效，重载全部作废；`ENTITY_NOT_FOUND` + `project.opened` 重拉 |
| app 侧动态注册改动波及现有面板/命令 | 双层设计：内置走原静态路径不变，动态层只增不改 |
