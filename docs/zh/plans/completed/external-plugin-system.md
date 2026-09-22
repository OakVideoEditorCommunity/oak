# 外部功能插件系统设计（进程隔离 + JSON-RPC/shm）

> 本文是 Oak **功能性插件系统**的总体设计，面向没有当前对话记忆的执行者，自包含。
>
> **定位**：与 `oak-plugin`（OpenFX 宿主）正交。OFX 管"效果/滤镜"这类图像处理插件；
> 本系统管"功能/工作流"插件——插件可以**调用 Oak 内部功能**（建工程、导入素材、
> 时间线编辑、加效果、取帧、导出）并**绘制自己的 UI 面板**。旗舰用例是 AI 剪辑插件：
> 给多模态 AI 一组工具，让它自己"看"视频（取帧回喂）并执行剪辑——外部程序因此
> 必须能完整操作 Oak。
>
> **红线**：
> 1. 插件代码**永不进入 Oak 主进程**（不 dlopen、不链接任何 Rust 库）。一个插件
>    一个独立进程，插件崩溃不得连带 Oak。
> 2. 插件可以是任何语言（C++/Python/Node…），协议必须是**语言无关的文本协议 +
>    共享内存数据面**，不发明需要链接 Rust/C ABI 的绑定。
> 3. 插件的一切编辑动作**必须可撤销**（UndoStack 事务），默认"确认后执行"。
> 4. 复用既有基础设施，不新造轮子：渲染进程隔离（`oak-render/src/procpool.rs` +
>    `oak-render/src/ipc.rs`，M15 已落地）的 **NDJSON over stdio + shm 帧槽** 模式
>    就是本系统传输层的范本。
>
> **协议全文**（消息信封、握手、方法/事件目录、错误码、shm 布局、UI 协议）
> 冻结在 [`external-plugin-protocol.md`](external-plugin-protocol.md)（OPP/1）；
> 实现以协议文档为准。

---

## 1. 关键决策

### 1.1 进程模型：插件 = 独立可执行文件（推荐），而非"库 + 宿主进程加载"

两种候选：

- **A. 插件即进程**：每个插件是一个独立可执行文件（Python 插件则是
  `python3 main.py` 这样的启动命令），Oak 按清单（manifest）spawn，经 stdio 说话。
  即 LSP / MCP 模型。
- **B. 插件即库 + 通用宿主进程**：插件编译成动态库，由一个 `oak-plugin-host`
  进程 dlopen 它，宿主进程再与 Oak 通信。

**定为 A**，理由：

1. **B 只是名义上更隔离**。dlopen 进宿主进程后，插件崩溃杀掉的是宿主进程，
   效果与 A 完全相同；但 B 要求宿主进程按语言分别内嵌加载器（C++ 用 dlopen，
   Python 得内嵌解释器或再起子进程），复杂度显著高于 A，没有换来任何隔离收益。
2. **A 对解释型语言天然成立**。Python/Node 插件本来就是"一个命令"，B 模型下
   反而要多包一层。
3. **A 与仓库既有模式一致**：`oak-worker` 就是"Oak spawn 一个可执行文件 +
   NDJSON 握手 + shm 附加"，含崩溃检测、有界重启（`MAX_RESTARTS=5`）、握手超时。
   插件宿主直接照搬这套生命周期管理。
4. **协议实现在 SDK，不在宿主**。担心"每个插件重写一遍协议"用 SDK 解决：
   官方提供 C/C++ 头文件库与 Python 包（各 ~200 行，见 §6），插件作者只写
   `on_request(method, params)` 回调。

代价（明说）：每种语言需要一个薄 SDK；stdio 单通道对极高频事件（如逐帧
playhead 推送）有序列化开销——用事件合并/降频缓解（§4.4），不另开 socket。

### 1.2 IPC：JSON-RPC 2.0 over stdin/stdout（控制面）+ shm（数据面）

- **控制面**：严格 [JSON-RPC 2.0](https://www.jsonrpc.org/specification)，
  NDJSON 分帧（一行一个消息，与 `oak-render/src/ipc.rs` 相同）。**双向**：
  Oak→plugin 发请求（UI 事件、配置下发、shutdown），plugin→Oak 也发请求
  （调内部功能，即 §3 宿主 API），靠 `id` 配对，notification 做事件推送。
  选 JSON-RPC 而非自定义协议：所有语言都有现成实现，且规范本身解决了
  双向请求/通知/错误码问题。
- **stdio 纪律**：`stdout` 只走协议消息；插件日志一律写 `stderr`，Oak 捕获后
  进日志面板（LSP 惯例）。绝不允许第三方库污染 stdout——SDK 提供
  `redirect_stdout_to_stderr()` 之类的防护。
- **数据面**：帧/缩略图/波形/插件 UI 位图走 POSIX shm（Windows 用
  `CreateFileMappingW`），消息体只带 `shm 名 + 槽位元数据`（宽/高/格式/步长）。
  直接泛化 `oak-render/src/ipc.rs` 的 `SharedMemoryRegion` / `FrameSlotPool`，
  不新设计。小数据（几 KB 的缩略图）允许内联 base64，阈值建议 64 KiB。

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

---

## 2. 生命周期与进程管理

### 2.1 清单与发现

插件是一个目录（或 `.oakplugin` 包），内含 `plugin.toml`：

```toml
id      = "com.example.ai-cut"
name    = "AI 剪辑助手"
version = "0.1.0"
api     = 1                      # 协议主版本，见 §2.2

[process]
# {plugin_dir} 由 Oak 替换；Python 插件就写解释器命令
command = ["python3", "{plugin_dir}/main.py"]
env_passthrough = ["PATH", "HOME"]

# 能力声明（§5），安装时向用户展示
capabilities = ["project.read", "media.read", "timeline.edit",
                "render.frame", "export", "ui.panel"]

[restart]
max = 5          # 对齐 procpool 的 MAX_RESTARTS
backoff_ms = 1000
```

发现路径（对齐 OFX 的发现习惯）：`~/.oak/plugins/`、应用内 `plugins/`、
环境变量 `OAK_PLUGIN_PATH`。Oak 启动时扫描 → 展示在"插件管理器"面板 →
用户启用后才 spawn（不自动启动未启用插件）。

### 2.2 握手与心跳

```
Oak ──► {"method":"handshake","params":{"protocol":1,"oak_version":"...",
        "shm":{"region":"oakxp-1234","slots":8,"slot_bytes":16777216}}}
Oak ◄── {"result":{"name":"ai-cut","api":1,"capabilities":[...],
         "panels":[{"id":"chat","title":"AI 剪辑"}]}}
```

- 握手超时（对齐 procpool 的实现）→ 判定启动失败，标记插件不可用。
- 之后 Oak 每 2s 发 `ping`，连续 3 次未响应或 stdout EOF → 判定崩溃：
  该插件的面板显示"已崩溃 [重启]"徽标，未完成的宿主 API 调用全部以
  `PLUGIN_DEAD` 错误返回，按 `restart.max` 有界自动重启。
- **重启无状态恢复**：协议设计为"注册式"——插件重连后重新走握手、重新注册
  面板。Oak 侧不丢数据：已提交的编辑早已进 UndoStack，与插件存亡无关。

### 2.3 Oak 侧组件

新增叶子 crate **`oak-plugin-host`**（与 `oak-worker` 平级的消费者角色，
不动引擎模块）：

- `PluginHost`：spawn/管道/NDJSON 读写（独立 IO 线程，`std::sync::mpsc` 与
  gpui `cx.spawn` 编排回引擎线程——沿用 app 现有 `set_progress_tx` 模式，
  不引入 tokio）。
- `HostApi`：把插件请求翻译成内部调用（§3），执行前查能力位（§5）。
- `PluginPanel`：实现 gpui `DockPanel` 的通用面板壳，注册进
  `AppPanelRegistry`（`../../../../crates/oak-app/src/panels/mod.rs` 目前是硬编码
  panel ids——需加一处"动态 panel 注册"扩展点，这是 app 侧唯一的新机制）。
- `ShmPool`：泛化自 `oak-render/src/ipc.rs`。

---

## 3. 宿主 API（插件调用 Oak 内部功能）

策展而非全量。插件看不到"内部函数"，看到的是一组**版本化的 RPC 方法**，
每个方法是现有 `graphops`/`renderops`/`oak_task` API 的组合（下表"落到哪里"
均为现有代码位置）。协议主版本 `api` 保证：同一主版本内只增不删。

### 3.1 编辑事务（铁律 3 的落地）

所有变更类方法必须包在事务里：

```json
{"id":10,"method":"edit.begin","params":{"label":"AI: 粗剪访谈片段"}}
{"id":11,"method":"timeline.split_clip","params":{"clip":"n17","time":"3/25"}}
{"id":12,"method":"timeline.ripple_delete","params":{"clip":"n18"}}
{"id":13,"method":"edit.commit"}
```

`edit.begin/commit` 映射到 `oak_undo::undostack` 的 UndoCommand 分组：一次
事务 = 一次 Ctrl-Z。`edit.abort` 回滚整组。**未在事务内的变更调用直接报错**，
从协议上杜绝不可撤销的编辑。

### 3.2 方法面（v1）

| 方法族 | 方法（摘要） | 落到哪里 | 所需能力 |
|---|---|---|---|
| `project.*` | `open` / `save` / `get_info` / 事件 `project.modified` | `oak_storage::Session`、`oak_node::serializer` | `project.read` / `project.edit` |
| `media.*` | `probe` / `import_footage` / `list_footage` / `get_streams` | `oak_app::oakui::graphops::import_footage`、`oak_codec` 探测 | `media.read` / `media.import` |
| `timeline.*` | `get_structure`（序列/轨道/块树）、`place_clip`、`split_clip`、`trim`、`move`、`ripple_delete`、`add_transition`、`add_marker`、`set_workarea` | `graphops::place_footage_clip` / `split_clip` / …、`oak-timeline` 命令族 | `timeline.read` / `timeline.edit` |
| `node.*` | `list_types`（含 OFX 动态类型）、`add_effect`、`set_param`、`set_keyframe`、`get_params` | `Factory::global()`、`engine.rs::add_effect/set_effect_param`、`set_value_at_time_command` | `node.read` / `node.edit` |
| `render.*` | `get_frame(time)`→shm、`get_thumbnails(range,n)`、`get_audio_levels(range)` | `renderops::render_sequence_frame` / `render_audio_range`、缩略图缓存 | `render.frame` |
| `playback.*` | `play` / `pause` / `seek` / 事件 `playhead_moved` | `EngineGateway`（`request_frame/play/pause/seek`） | `playback` |
| `export.*` | `start(params)` / `cancel` / 事件 `export.progress` | `renderops::spawn_export`、`oak_task::export::EncodingParams` | `export` |
| `ui.*` | 见 §4 | `PluginPanel` + gpui_widgets | `ui.panel` |
| `edit.*` | `begin` / `commit` / `abort` / `undo` / `redo` | `oak_undo` | 随变更方法 |

**取帧→AI 通路**（旗舰用例的关键路径，对齐 `../ai-agent-design.md` §2.2）：
`render.get_frame {sequence, time, max_size}` → 引擎经 ticket/进程池渲染 →
BGRA 进 shm 槽 → 返回 `{shm_slot, width, height, format}`；插件侧 SDK 一行
`frame.to_png_bytes()`（OIIO/stb_image_write 或 Pillow）即可回喂多模态模型。
`get_thumbnails` 一次取 N 帧拼 contact sheet，供"扫时间线定位内容"。
**限流**：取帧调用带每插件速率与分辨率上限（默认 8 fps / 1920 宽），防止批量
取帧拖垮渲染进程池。

### 3.3 事件（Oak→插件 notification）

`project.opened/modified`、`timeline.structure_changed`（增量，非全量）、
`playhead_moved`（§4.4 降频）、`export.progress/done`、`ui.*` 输入事件（§4.2）、
`shutdown`（Oak 退出前发，插件应在 2s 内退出，否则 SIGTERM→SIGKILL）。

---

## 4. 插件 UI

gpui 没有 webview，也不可能让 Python 插件直接调 gpui。提供**两条路径**，
插件按需在握手时声明（可同时用）：

### 4.1 声明式 UI（v1 基线，推荐大多数插件用）

插件用 JSON 描述控件树，Oak 用 gpui_widgets 渲染成 `PluginPanel` 内容：

```json
{"method":"ui.set_tree","params":{"panel":"chat","root":
  {"type":"column","children":[
    {"type":"chat_log","id":"log"},
    {"type":"row","children":[
      {"type":"text_input","id":"prompt","placeholder":"描述你的剪辑意图…"},
      {"type":"button","id":"send","text":"执行"}]},
    {"type":"progress","id":"job"}
  ]}}}
```

控件集 v1 保持小：`column/row/label/button/text_input/list/chat_log/image/
progress/slider/checkbox`。用户在面板里的交互以 `ui.event {id, kind, value}`
推给插件；插件用 `ui.set_props {id, props}` 增量更新（不做全量重绘 diff，
控件树很小，全量 `set_tree` 也行）。

收益：**零崩溃面**（插件不画一个像素）、风格与 Oak 一致、实现量最小。
AI 剪辑插件的聊天面板、操作日志、确认按钮，这套完全够。

### 4.2 像素面 UI（完整能力路径）

插件自己用任意工具包（Qt/imgui/web 引擎——在**自己的进程**里）离屏渲染，
把 BGRA 位图经 shm 推给 Oak，Oak 在 `PluginPanel` 里原样贴图：

```
插件 ──shm 写帧──► ui.frame_ready {panel, slot, dirty_rect} ──► Oak 贴图
插件 ◄── ui.event {kind:"pointer_down|pointer_move|key|scroll|focus",
                   x,y,button,modifiers,dpi_scale} ◄── gpui 事件转发
```

- 这是既有 **OFX Interact GL-overlay 路径**（`oak_plugin::gl_bridge` +
  `oakui/ofx.rs::forward_interact_pointer/key` + `program_viewer` 合成）的
  进程外泛化：把"插件 GL 离屏 + readback 合成 + 事件转发"换成
  "插件进程离屏 + shm + 事件经 IPC 转发"，事件模型照抄 interact 的。
- resize 时 Oak 发 `ui.resize {width,height,dpi}`，插件按新尺寸重渲染；
  帧槽数 ≥2 做双缓冲，`frame_ready` 携带脏矩形减少合成开销。
- v1 明确不做：IME 合成串转发、剪贴板互通、跨进程拖拽（需要时另立文档）。

### 4.3 其他 UI 形态

- **独立窗口**：插件进程自己开 OS 窗口，Oak 不管——始终允许，无需协议支持，
  集成度差，适合调试工具类插件。
- **监看器叠加层**：OFX Interact 那种画在节目监视器上的 overlay，属于
  "效果交互"范畴，继续归 OFX；功能插件如需 viewer overlay（如 AI 打点预览），
  列为 v2 候选，复用 `program_viewer` 的合成点。

### 4.4 事件降频

`playhead_moved`、`pointer_move` 这类高频事件：Oak 侧合并到 30 Hz 上限、
只发最新值（对齐 viewer 的刷新语义），避免 stdio 被事件洪水淹没。

---

## 5. 能力、确认与安全

- **能力位**：manifest `capabilities` 声明，安装/升级时向用户展示差异；
  `HostApi` 在每次调用入口检查，越权调用返回 `CAPABILITY_DENIED` 并记日志。
  v1 能力集合即 §3.2 表右列。
- **确认模式**（继承 `../ai-agent-design.md` §6）：`*.edit` 与 `export` 类调用
  默认弹"插件 X 请求执行：split_clip n17 @ 3/25 [允许] [允许本会话] [拒绝]"；
  用户可在插件设置里改为自动。
- **可撤销**：事务分组进 UndoStack，历史面板里显示为"插件名：事务标签"，
  用户可整段撤销（§3.1）。
- **限流与配额**：取帧速率/分辨率上限（§3.2）；单插件 shm 池有上限；
  单请求参数大小上限（防内存炸弹）。
- **密钥**：插件需要 API key 走自己的环境变量/自己的配置文件，
  **绝不写入 Oak 工程文件**（.ove 里只允许存插件 id + 版本，对齐 OFX
  `<plugins>` 段的语义）。
- **不做沙箱**：本系统隔离的是"崩溃"，不是"恶意"——插件进程与 Oak 同用户
  权限。恶意插件防护（seccomp/签名/商店审核）明确出范围。

---

## 6. 插件 SDK 与参考插件

- **`oakxp-c`**（头文件-only C/C++ SDK，放 `shared/include/oakxp/`）：
  NDJSON 分帧、JSON-RPC 收发、shm 附加、回调注册。无第三方依赖
  （JSON 用内置极简 parser，或允许作者自选）。这是 C ABI 纪律下唯一
  允许插件 #include 的东西——**纯协议，不含任何 Oak 内部类型**。
- **`oakxp`（Python 包）**：`pip install oakxp` 或随 Oak 分发；
  `asyncio` 友好但非强制；`frame.to_png()` 依赖 Pillow（可选 extra）。
- **参考插件**（验收的一部分）：
  1. `examples/plugin-echo`：C++，注册一个声明式面板，按钮触发
     `project.get_info` 并显示——验证协议与 UI 基线。
  2. `examples/plugin-roughcut`：Python，接多模态 LLM，实现
     "聊天指令 → get_thumbnails 扫时间线 → 事务化 split/ripple_delete →
     get_frame 验证"的 AI 粗剪闭环——**它就是 ai-agent-design.md 的落地形态**。

### 与 `../ai-agent-design.md` 的关系

该文档写于 RIIR 拆分前，假设"C ABI 小库 + 引擎内置 MCP server"。RIIR 与
M15（渲染进程隔离）完成后，更优路径是：**AI 能力不进引擎，作为一个外部
插件**跑在本系统上；MCP 仍可作为该插件对外的协议（插件自己起 MCP server
连 LLM 客户端），Oak 内核始终对 AI 无感知。本文落地后，`../ai-agent-design.md`
的 M1/M2（工具面、取帧通路）由 §3.2 取代，M3（AI 面板）由 §4.1 取代。

---

## 7. 里程碑

| 里程碑 | 内容 | 验收 |
|---|---|---|
| **P1 传输与生命周期** | `oak-plugin-host`：spawn/握手/心跳/崩溃检测/有界重启；JSON-RPC 双向收发；`oakxp-c` 最小 SDK；echo 插件跑通 `project.get_info` | 杀掉插件进程：Oak 不崩、面板显示崩溃徽标、可重启；握手超时路径有测试 |
| **P2 宿主 API 核心** | `edit.*` 事务 + `project/media/timeline/node` 方法族 + 能力检查 | 插件完成"导入素材→铺轨→切开→波纹删除→加效果→改参数"，逐步可在历史面板撤销；越权调用被拒 |
| **P3 取帧与导出** | `render.*` shm 数据面、`export.*` 事件、限流 | 黄金帧校验（复用 render-worker 端到端 harness）：插件取到的帧与 viewer 一致；连续取帧不拖垮进程池 |
| **P4 声明式 UI** | `PluginPanel` + 动态 panel 注册 + `ui.*` 控件集 | echo 插件面板交互全通；控件树快照测试 |
| **P5 像素面 UI** | shm 贴图 + 输入转发 + resize/DPI | 参考 imgui 插件 60fps 交互无撕裂；事件转发对齐 interact 语义 |
| **P6 Python SDK 与 AI 粗剪** | `oakxp` 包 + `plugin-roughcut` | Mock LLM 录制/回放（无网络 CI）跑通"看图→下刀→验证"闭环 |

P1–P3 是系统地基，任何插件都依赖；P4/P5 可并行；P6 随时可开始（SDK 与
宿主 API 稳定后）。

---

## 8. 明确不做（边界）

- **不**取代 OFX：图像处理节点仍走 `oak-plugin`（渲染在 worker 进程内已有
  隔离）。功能插件如需注册新节点类型，v2 再评估（机制上是现成的
  `Factory::register_dynamic`）。
- **不**做插件沙箱、签名、商店（§5）。
- **不**做跨机器/网络插件（stdio only；socket 传输变体留作以后，协议本身
  不绑定 stdio）。
- **不**为插件发明新的引擎内部机制：宿主 API 全部是现有
  `graphops/renderops/oak_task` 的组合（铁律 3 同源于 ai-agent-design）。
- **不**引入 tokio 到 app 路径；IO 线程 + mpsc + gpui executor 足够。
