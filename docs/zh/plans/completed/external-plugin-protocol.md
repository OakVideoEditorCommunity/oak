# Oak 外部插件协议规范（OPP/1）

> 本文是 [`external-plugin-system.md`](external-plugin-system.md) 的**协议全文**，
> 冻结到可实现、可写 SDK 的粒度：传输分帧、消息信封、握手、全部 RPC 方法与事件、
> 错误码、shm 数据面布局、UI 协议。实现（`oak-plugin-host`、`oakxp-c`、`oakxp`
> Python 包）以本文为准；与设计文档冲突时**以本文为准**。
>
> **版本**：协议主版本 `1`（`"api": 1`）。同一主版本内只增不删（§13）。
>
> **面向**：`oak-plugin-host` 实现者、插件 SDK 作者、插件作者。

---

## 1. 传输层与分帧

- 通道：插件进程的 `stdin`（Oak→plugin）与 `stdout`（plugin→Oak），全双工。
- 分帧：**NDJSON**——每条消息是**一行** UTF-8 JSON，以 `\n` 结尾；消息内
  不得出现裸换行（JSON 序列化默认满足）。禁用 BOM。
- 消息大小上限 **16 MiB**；超限 Oak 直接判定协议错误并杀死插件。
- **`stdout` 纪律**：只允许协议消息。插件日志走 `stderr`（Oak 捕获进日志面板）
  或 `session.log`（§8.1）。SDK 必须在初始化时把第三方库的 stdout 输出重定向
  到 stderr。
- 关闭语义：Oak 关闭插件 stdin 写端 = 要求插件退出（等价于收到
  `session.shutdown` 后的超时强杀，见 §4.4）。

## 2. 消息信封（JSON-RPC 2.0）

严格遵循 JSON-RPC 2.0，**双向**：两方都可以发 Request 与 Notification。

```json
// Request（期望响应）
{"jsonrpc":"2.0","id":42,"method":"timeline.split_clip","params":{...}}
// Response 成功
{"jsonrpc":"2.0","id":42,"result":{...}}
// Response 失败
{"jsonrpc":"2.0","id":42,"error":{"code":-32001,"message":"not in edit transaction"}}
// Notification（无 id，无响应）
{"jsonrpc":"2.0","method":"playback.playhead_moved","params":{...}}
```

- `id`：字符串或整数，由**发送方**自定命名空间（同一连接上两方的 id 可能
  撞车，接收方配对时只看自己发出的 id——标准行为）。
- 允许任意数量的 in-flight 请求；**同一事务（§6）内的变更请求，Oak 严格按
  到达顺序串行执行**。其余请求不保证相对顺序。
- `params` 一律为对象（不用位置参数）。
- 需要用户确认的请求（§7.3），Oak 在用户裁决前**不返回响应**；插件不得假设
  超时，SDK 默认请求超时设为 120 s。

## 3. 错误码

标准码（-32700/-32600/-32601/-32602/-32603）按 JSON-RPC 规范。应用码占用
JSON-RPC 保留的 server-error 段：

| code | 常量 | 含义 |
|---|---|---|
| -32000 | `CAPABILITY_DENIED` | 插件无此方法所需能力位 |
| -32001 | `NOT_IN_TRANSACTION` | 变更方法缺少有效 `txn` |
| -32002 | `TRANSACTION_CONFLICT` | 事务被其他持有者占用 |
| -32003 | `ENTITY_NOT_FOUND` | id 失效（删除/工程重载后），`data.entity` 带原 id |
| -32004 | `RATE_LIMITED` | 触发限流，`data.retry_after_ms` 给重试间隔 |
| -32005 | `SHM_EXHAUSTED` | shm 池无空闲槽，先 `shm.release` |
| -32006 | `CONFIRMATION_DENIED` | 用户在确认弹窗中拒绝 |
| -32007 | `FRAME_TOO_LARGE` | 请求帧超过槽容量，调小 `max_size` |
| -32008 | `INVALID_STATE` | 当前状态不允许（如无打开的工程） |

`error.data` 可选，结构化附加信息（见上表）。Oak 侧合成错误（插件进程已死、
握手失败）不进协议，直接体现在 Oak 的插件管理器 UI。

## 4. 生命周期

### 4.1 握手（细化设计文档 §2.2：由插件发起）

Oak spawn 插件后，插件必须在 **10 s** 内发出第一个消息——`session.hello`：

```json
// plugin → Oak
{"jsonrpc":"2.0","id":1,"method":"session.hello","params":{
  "api":1,
  "name":"ai-cut",
  "version":"0.1.0",
  "capabilities":["project.read","timeline.read","timeline.edit","render.frame","ui.panel"],
  "panels":[{"id":"chat","title":"AI 剪辑","ui":"declarative"}],
  "subscribe":["project.opened","timeline.structure_changed"]
}}
// Oak → plugin
{"jsonrpc":"2.0","id":1,"result":{
  "api":1,
  "oak_version":"0.4.0",
  "granted":["project.read","timeline.read","timeline.edit","render.frame","ui.panel"],
  "features":["ui.pixel"],
  "shm":{"down":{"name":"oakxp-d-1234","slots":8,"slot_bytes":16777216}}
}}
```

- `capabilities` 必须是 manifest 声明集的子集；`granted` 是 Oak 实际授予的
  子集（用户可能在安装时裁剪）。插件按 `granted` 工作。
- `features`：Oak 支持的可选特性清单，用于同主版本内的能力探测（§13）。
- `shm.down`：Oak→plugin 方向的帧槽池（§10），握手时已创建，插件自行 attach。
- 超时或首消息不是 `session.hello`：Oak 杀进程，标记插件启动失败。

### 4.2 心跳

握手成功后，Oak 每 **2 s** 发一次：

```json
{"jsonrpc":"2.0","id":"ping-317","method":"session.ping"}
```

插件应在 **5 s** 内响应（`"result":{}`）。**连续 3 次**超时或 stdout EOF/进程
退出 = 崩溃：有界重启（manifest `[restart]`，默认 `max=5`、退避 1 s 起倍增）。
重启后重新走 §4.1；所有 id 与事务令牌作废，插件须重新拉取状态。

### 4.3 事件订阅

`session.hello.subscribe` 是初始订阅；运行时用：

```json
{"method":"events.subscribe","params":{"events":["playback.playhead_moved"],"unsubscribe":["export.progress"]}}
```

事件目录见 §9。订阅需要对应的 read 类能力位（§7.2 各事件标注）。

### 4.4 关闭

Oak 退出或用户禁用插件：

```json
// Oak → plugin（notification）
{"jsonrpc":"2.0","method":"session.shutdown","params":{"reason":"app_quit"}}
```

插件应在 **2 s** 内自行退出（保存自己的状态）；超时 SIGTERM，再 2 s SIGKILL
（Windows：`TerminateProcess`）。插件主动崩溃/退出按 §4.2 崩溃路径处理。

## 5. 公共数据类型

| 类型 | JSON 表示 | 说明 |
|---|---|---|
| `Rational`（时间） | `{"num":3,"den":25}` | 秒为单位的有理数，`den>0`。全协议**唯一**时间表示 |
| `TimeRange` | `{"in":Rational,"out":Rational}` | 左闭右开 |
| `EntityId` | 不透明字符串 | 素材/序列/轨道/块/节点/事务/作业统一为字符串 id。**插件不得解析格式**；会话内稳定，工程重载后全部作废（靠 `project.opened` + 重新拉取恢复） |
| `Color` | `"#RRGGBB"` 或 `"#RRGGBBAA"` | |
| `FrameRef` | 见下 | 一帧位图的引用，两种形态 |

`FrameRef`：

```json
// shm 形态（默认）
{"shm":{"region":"down","slot":3},"format":"bgra8","width":1920,"height":1080,
 "stride":7680,"bytes":8294400,"time":{"num":3,"den":25}}
// inline 形态（bytes ≤ 64 KiB 时 Oak 可选用）
{"inline":"base64...","format":"png","width":320,"height":180,"time":{...}}
```

- `format`：`"bgra8"`（shm 原始位图，行优先、顶左原点）或 `"png"`（已编码，
  inline 专用）。
- shm 形态的槽位**借用**自 `down` 池，插件用完必须 `shm.release`（§10.3），
  否则触发 `SHM_EXHAUSTED`；借用带 30 s 租约，超时 Oak 强制回收。

## 6. 编辑事务协议

一切变更方法（§8 各方法标注"事务：是"）必须携带 `txn` 参数；事务令牌由
`edit.begin` 签发：

```json
{"id":10,"method":"edit.begin","params":{"label":"AI: 粗剪访谈片段"}}
{"id":10,"result":{"txn":"t12"}}
{"id":11,"method":"timeline.split_clip","params":{"txn":"t12","clip":"...","time":{"num":3,"den":25}}}
{"id":12,"method":"edit.commit","params":{"txn":"t12"}}
```

规则（钉死）：

1. **全局单持**：同一时刻全 Oak 只有一个未决事务。`edit.begin` 冲突返回
   `TRANSACTION_CONFLICT`，`data.retry_after_ms` 提示重试。插件不得长持事务
   （建议 < 5 s）；Oak 对 60 s 未提交的事务强制 `abort`。
2. `commit` = 一组 UndoCommand 压栈，历史面板显示"插件名：label"，一次
   Ctrl-Z 整体撤销。`abort` = 已执行的变更逆序回滚，不留痕迹。
3. 事务内变更按到达顺序串行执行（§2）；任一变更失败，**前面已成功的保持
   有效**，由插件决定 `commit` 还是 `abort`——Oak 不自动回滚。
4. 崩溃时未决事务自动 `abort`：插件崩了也不会留下半截编辑。
5. `edit.undo`/`edit.redo` 不需要 `txn`，撤销的是整个 UndoStack（包括用户
   自己的操作）——插件应只在用户明确要求时调用。

## 7. 能力位

### 7.1 能力清单（v1 冻结）

| 能力 | 覆盖的方法/事件 |
|---|---|
| `project.read` | `project.get_info`；`project.opened/modified/closed` 事件 |
| `project.edit` | `project.open/save`（且需事务） |
| `media.read` | `media.probe/list_footage` |
| `media.import` | `media.import_footage`（且需事务） |
| `timeline.read` | `timeline.get_structure`；`timeline.structure_changed` 事件 |
| `timeline.edit` | `timeline.*` 全部变更方法（且需事务） |
| `node.read` | `node.list_types/get_params` |
| `node.edit` | `node.add_effect/set_param/set_keyframe/remove`（且需事务） |
| `render.frame` | `render.get_frame/get_thumbnails/get_audio_levels` |
| `playback` | `playback.*`；`playback.*` 事件 |
| `export` | `export.start/cancel`；`export.*` 事件 |
| `ui.panel` | `ui.*`（声明式）；`ui.event` 事件 |
| `ui.pixel` | `ui.attach_surface/frame_ready`（像素面，§11.2） |

### 7.2 检查时机

`HostApi` 在每个方法入口查 `granted`；越权返回 `CAPABILITY_DENIED` 并记
Oak 日志。事件订阅同理（订阅未授权事件返回 `CAPABILITY_DENIED`，
`data.event` 指明哪个）。

### 7.3 用户确认

`*.edit`、`media.import`、`export`、`edit.undo/redo` 属于**确认类**：Oak 弹窗
"插件 X 请求：split_clip n17:2 @ 3/25 [允许] [本会话内允许] [拒绝]"。拒绝返回
`CONFIRMATION_DENIED`；"本会话内允许"缓存到 Oak 会话结束。用户在插件设置里
可把某插件整设为"自动允许"。确认类方法清单与能力位一一对应，见 §8 各方法
"确认"列。

## 8. 宿主 API 方法（v1 全量）

通用列：**事务**=是否需要 `txn`；**确认**=是否触发 §7.3 弹窗。所有方法均可
返回 §3 通用错误，不再逐条列出。

### 8.1 会话

| 方法 | params | result | 说明 |
|---|---|---|---|
| `session.hello` | §4.1 | §4.1 | 首消息，仅此一次 |
| `session.ping` | – | `{}` | Oak→plugin 方向 |
| `session.shutdown` | `{reason}` | notification | Oak→plugin |
| `session.log` | `{level:"debug"\|"info"\|"warn"\|"error", message}` | notification | plugin→Oak，进 Oak 日志面板。高频日志请走 stderr |
| `events.subscribe` | `{events:[], unsubscribe:[]}` | `{subscribed:[]}` | §4.3 |

### 8.2 `project.*`

| 方法 | 事务 | 确认 | params | result |
|---|---|---|---|---|
| `project.get_info` | 否 | 否 | `{}` | `{path\|null, name, modified, sequences:[{id,name,fps:Rational,duration:Rational}]}` |
| `project.open` | 是 | 是 | `{txn, path}` | `{name}` |
| `project.save` | 是 | 是 | `{txn, path?}` | `{path}` |

无打开工程时读取方法返回 `INVALID_STATE`。

### 8.3 `media.*`

| 方法 | 事务 | 确认 | params | result |
|---|---|---|---|---|
| `media.probe` | 否 | 否 | `{path}` | `{duration:Rational, streams:[{type:"video"\|"audio"\|"subtitle", codec, width?, height?, fps?:Rational, sample_rate?, channels?}]}` |
| `media.list_footage` | 否 | 否 | `{}` | `{footage:[{id,name,path,duration:Rational}]}` |
| `media.import_footage` | 是 | 是 | `{txn, paths:[...]}` | `{footage:[{id,name,duration:Rational}]}`（跳过失败项，`errors:[{path,message}]` 单列） |

### 8.4 `timeline.*`

```json
// timeline.get_structure {sequence} →
{"result":{"sequence":{"id":"…","name":"访谈成片","fps":{"num":25,"den":1},
  "duration":{"num":183,"den":25},
  "tracks":[
    {"id":"…","type":"video","index":0,"clips":[
      {"id":"…","name":"A001.mp4","footage":"…",
       "in":{"num":0,"den":1},"out":{"num":72,"den":25},
       "media_in":{"num":10,"den":1},"enabled":true}
    ]},
    {"id":"…","type":"audio","index":0,"clips":[…]}
  ]}}}
```

| 方法 | 事务 | 确认 | params | result |
|---|---|---|---|---|
| `timeline.get_structure` | 否 | 否 | `{sequence}` | 见上 |
| `timeline.add_track` | 是 | 是 | `{txn, sequence, type:"video"\|"audio", index?}` | `{track}` |
| `timeline.place_clip` | 是 | 是 | `{txn, sequence, track, footage, in:Rational, media_in?}` | `{clip}` |
| `timeline.split_clip` | 是 | 是 | `{txn, clip, time:Rational}` | `{clips:[id,id]}` |
| `timeline.trim_clip` | 是 | 是 | `{txn, clip, side:"in"\|"out", time:Rational}` | `{clip}` |
| `timeline.move_clip` | 是 | 是 | `{txn, clip, in:Rational, track?}` | `{clip}` |
| `timeline.delete_clip` | 是 | 是 | `{txn, clip}` | `{}` |
| `timeline.ripple_delete` | 是 | 是 | `{txn, clip}` | `{}` |
| `timeline.add_transition` | 是 | 是 | `{txn, clip, side:"in"\|"out", type, duration:Rational}` | `{transition}` |
| `timeline.add_marker` | 是 | 是 | `{txn, sequence, time:Rational, name?, color?:Color}` | `{marker}` |
| `timeline.set_workarea` | 是 | 是 | `{txn, sequence, range:TimeRange}` | `{}` |

`time`/`in`/`out` 一律为序列时间轴上的有理秒。越界/重叠冲突返回
`INVALID_STATE`，`data.reason` 说明。

### 8.5 `node.*`（效果与参数）

| 方法 | 事务 | 确认 | params | result |
|---|---|---|---|---|
| `node.list_types` | 否 | 否 | `{category?:"effect"\|"transition"\|"all"}` | `{types:[{id,name,category}]}`（含 OFX 动态类型，id 即 OFX identifier） |
| `node.add_effect` | 是 | 是 | `{txn, clip, effect, index?}` | `{node}` |
| `node.get_params` | 否 | 否 | `{node}` | `{params:[{key,name,type:"float"\|"int"\|"bool"\|"string"\|"color"\|"vec2"\|"choice", value, default, min?, max?, choices?:[]}]}` |
| `node.set_param` | 是 | 是 | `{txn, node, key, value}` | `{}` |
| `node.set_keyframe` | 是 | 是 | `{txn, node, key, time:Rational, value}` | `{}` |
| `node.remove` | 是 | 是 | `{txn, node}` | `{}` |

`value` 的 JSON 类型随 `type`：`float/int`→number，`bool`→boolean，
`string/choice`→string，`color`→Color，`vec2`→`[x,y]`。

### 8.6 `render.*`（AI 视觉闭环的取帧口）

| 方法 | 事务 | 确认 | params | result |
|---|---|---|---|---|
| `render.get_frame` | 否 | 否 | `{sequence?, footage?, time:Rational, max_size?:{width,height}, format?:"bgra8"\|"png"}` | `{frame:FrameRef}` |
| `render.get_thumbnails` | 否 | 否 | `{sequence?, footage?, range:TimeRange, count, height?:180}` | `{frames:[FrameRef,…]}`（等间隔采样，`count` ≤ 64） |
| `render.get_audio_levels` | 否 | 否 | `{sequence, range:TimeRange, resolution?:100}` | `{channels, peaks:inline base64 float32le 数组（channels×resolution）}` |

- `sequence` 与 `footage` 二选一，都缺省返回 `INVALID_PARAMS(-32602)`。
- `max_size` 超槽容量（§4.1 `slot_bytes`）返回 `FRAME_TOO_LARGE`。
- 限流（§12）：默认每插件 8 帧/s、短边 ≤ 1080，超限 `RATE_LIMITED`。
- 渲染走引擎 ticket/进程池路径，**不阻塞 GUI 线程**；典型延迟 50–500 ms，
  插件侧应并发流水线化而不是串行等帧。

### 8.7 `playback.*`

| 方法 | 事务 | 确认 | params | result |
|---|---|---|---|---|
| `playback.play` | 否 | 否 | `{sequence?}` | `{}` |
| `playback.pause` | 否 | 否 | `{}` | `{}` |
| `playback.seek` | 否 | 否 | `{time:Rational}` | `{}` |
| `playback.get_state` | 否 | 否 | `{}` | `{playing, time:Rational, sequence\|null}` |

### 8.8 `export.*`

| 方法 | 事务 | 确认 | params | result |
|---|---|---|---|---|
| `export.start` | 否 | 是 | `{sequence, output_path, preset?:string}` | `{job}` |
| `export.cancel` | 否 | 否 | `{job}` | `{}` |

`preset` 引用 Oak 导出预设名；自定义编码参数（分辨率/码率/封装）v1 不开放，
需要时按 §13 加 `encoding` 对象。进度经 `export.progress` 事件推送（§9）。

### 8.9 `shm.*`

| 方法 | params | result |
|---|---|---|
| `shm.release` | `{slots:[{region:"down", slot:3}, …]}` | `{}`（notification 亦可） |

详见 §10。

### 8.10 `ui.*`

见 §11（声明式与像素面两条路径共用 `ui.*` 命名空间）。

## 9. 事件（Oak→plugin notification）

| 事件 | 所需能力 | params | 频率 |
|---|---|---|---|
| `project.opened` | `project.read` | `{path, name}` | – |
| `project.modified` | `project.read` | `{modified}` | 状态翻转时 |
| `project.closed` | `project.read` | `{}` | – |
| `timeline.structure_changed` | `timeline.read` | `{sequence, hint:"full"\|{"clips_added":[],"clips_removed":[],"clips_moved":[]}}` | 变更合并后发，≤ 10 Hz |
| `playback.playhead_moved` | `playback` | `{sequence, time:Rational}` | ≤ 30 Hz，只发最新值 |
| `playback.state_changed` | `playback` | `{playing}` | – |
| `export.progress` | `export` | `{job, fraction:0..1, eta_ms?\|null}` | ≤ 4 Hz |
| `export.done` | `export` | `{job, ok, output_path?, error?}` | – |
| `ui.event` | `ui.panel` | §11 | 输入事件实时；pointer_move ≤ 60 Hz 合并 |

`hint` 是优化提示：插件可永远按 `"full"` 处理（重新 `get_structure`），
`hint` 对象仅当下发增量安全时出现。

## 10. shm 数据面

### 10.1 区域与方向

- `down`：Oak→plugin（渲染帧）。Oak 在握手前创建，握手响应携带
  `{name, slots, slot_bytes}`；插件 `shm_open`+`mmap` 只读 attach。
- `up`：plugin→Oak（像素面 UI 位图）。Oak 在 `ui.attach_surface` 时按需创建
  （每个像素面板一个区域），result 携带同名结构；插件可写 attach。
- POSIX：`shm_open`/`mmap`；Windows：`CreateFileMappingW`/`MapViewOfFile`。
  名称不带前导 `/` 的语义差异由 SDK 抹平。

### 10.2 无头部、无锁（钉死）

**shm 内不放任何元数据、不放锁**。槽位布局：槽 `i` 的字节区间
`[i*slot_bytes, (i+1)*slot_bytes)`，位图从偏移 0 开始，格式/宽/高/步长全部
由控制面消息携带（`FrameRef` / `ui.frame_ready`）。槽位有效性由 RPC 配对界定：

- `down`：从携带该槽的 Response/事件到达，到插件 `shm.release`（或 30 s 租约
  到期）为止，Oak 保证不写该槽。
- `up`：从 `ui.frame_ready` 发出，到 Oak 回 `ui.surface_ack` 为止，插件保证
  不写该槽。

因为控制面与数据面一一配对，不需要 seqlock/环形缓冲那套（render-worker 的
SPSC ring 是高频流式场景，本协议是请求-响应场景，刻意简化）。

### 10.3 流控

- `down` 池 `slots` 个槽（默认 8）。插件未释放的借用数达到 `slots` 后，
  `render.*` 一律 `SHM_EXHAUSTED`。批量取帧的插件必须流水线化 release。
- `up` 区域固定 3 槽（三缓冲）。`ui.frame_ready` 未收到 `surface_ack` 的槽
  不得复用；3 槽全在飞行中时插件应丢弃新帧（UI 丢帧安全）。
- 租约：`down` 借用 30 s 未 release，Oak 强制回收并记日志（视为插件 bug）。

## 11. UI 协议

### 11.1 声明式 UI

面板在 `session.hello.panels` 声明 `"ui":"declarative"`。Oak 为其创建
`PluginPanel`（可关闭/可停靠的 DockPanel），初始为空。

**控件树下发**（全量替换）：

```json
{"id":31,"method":"ui.set_tree","params":{"panel":"chat","root":
  {"type":"column","gap":8,"children":[
    {"type":"chat_log","id":"log","grow":true},
    {"type":"row","gap":4,"children":[
      {"type":"text_input","id":"prompt","placeholder":"描述你的剪辑意图…","grow":true},
      {"type":"button","id":"send","text":"执行"}]},
    {"type":"progress","id":"job","visible":false}
  ]}}}
```

**增量更新**：`ui.set_props {panel, id, props:{…}}`，只改给出的属性；
不存在的 `id` 返回 `ENTITY_NOT_FOUND`。结构性增删用全量 `set_tree`
（树规模小，不做 diff 协议）。

**控件目录（v1 冻结）**：

| type | 关键 props | 事件（`kind`） |
|---|---|---|
| `column` / `row` | `gap, grow, children[]` | – |
| `label` | `text, color?` | – |
| `button` | `text, enabled?` | `click` |
| `text_input` | `text, placeholder?, enabled?` | `change{text}`, `submit{text}` |
| `text_area` | `text, readonly?` | `change{text}` |
| `list` | `items:[{id,text}], selected?` | `select{id}` |
| `chat_log` | `entries:[{role:"user"\|"assistant"\|"system", text}]`（set_props 追加） | – |
| `image` | `source:{inline_base64}` 或 `{shm:{region,slot},width,height,stride,format}` | – |
| `progress` | `fraction:0..1, indeterminate?, text?` | – |
| `slider` | `value, min, max, step?` | `change{value}` |
| `checkbox` | `checked, text` | `change{checked}` |
| `separator` / `spacer` | – | – |

**事件上行**：

```json
{"jsonrpc":"2.0","method":"ui.event","params":
  {"panel":"chat","id":"send","kind":"click"}}
```

面板被用户关闭：`ui.event {panel, kind:"panel_closed"}`；Oak 重新打开时插件
会收到 `ui.event {kind:"panel_shown"}`，插件应重发 `ui.set_tree`。

**通知**：`ui.notify {level:"info"\|"warn"\|"error", text}` → Oak 状态栏 toast。

### 11.2 像素面 UI

面板声明 `"ui":"pixel"`（需 `ui.pixel` 能力，握手 `features` 里有才可用）。

```json
// 1) 建表面：Oak 创建 up 区域
{"id":40,"method":"ui.attach_surface","params":{"panel":"paint","width":960,"height":540,"dpi":2.0}}
{"id":40,"result":{"shm":{"region":"up","name":"oakxp-u-1234-paint","slots":3,
                         "slot_bytes":8294400},"format":"bgra8"}}
// 2) 插件画好一帧 → 通知（notification）
{"jsonrpc":"2.0","method":"ui.frame_ready","params":
  {"panel":"paint","slot":1,"width":960,"height":540,"stride":7680,
   "dirty":[0,0,960,540]}}
// 3) Oak 合成完毕 →  ack（notification），槽位可复用
{"jsonrpc":"2.0","method":"ui.surface_ack","params":{"panel":"paint","slot":1}}
```

**输入事件下行**（`ui.event`，`id` 固定为 `"surface"`）：

| kind | params 增量 |
|---|---|
| `resize` | `{width, height, dpi}`（插件用新尺寸重画并 `frame_ready`） |
| `pointer_move` / `pointer_down` / `pointer_up` | `{x, y, button?, modifiers:["shift","ctrl",…]}`（逻辑坐标，已除 dpi） |
| `scroll` | `{x, y, dx, dy, modifiers}` |
| `key_down` / `key_up` | `{key, text?, modifiers}`（`key` 为 USB HID usage name 字符串，如 `"A"`/`"Enter"`） |
| `focus` / `blur` | `{}` |

pointer_move 合并到 ≤ 60 Hz。IME 合成串、剪贴板、拖拽：v1 不做（设计文档
§4.2 已声明边界）。

## 12. 限流与配额（v1 默认值）

| 资源 | 默认 | 超限行为 |
|---|---|---|
| `render.get_frame` | 8 帧/s（令牌桶，burst 4） | `RATE_LIMITED` + `retry_after_ms` |
| 渲染帧短边 | ≤ 1080 px | `RATE_LIMITED`（插件调小 `max_size`） |
| `get_thumbnails` | `count` ≤ 64/次 | `INVALID_PARAMS` |
| 单条消息 | ≤ 16 MiB | 协议错误，杀进程 |
| `down` 借用 | ≤ `slots`（8） | `SHM_EXHAUSTED` |
| 未决事务时长 | ≤ 60 s | 强制 `abort` |

配额随握手响应的 `limits` 字段下发（v1 可缺省 = 上表默认）；插件以 `limits`
为准，不要硬编码。

## 13. 版本演进规则

1. `api` 主版本只在**破坏性变更**时 +1；Oak 同时支持的旧主版本数 ≥ 1。
2. 同主版本内：只准**新增**方法/事件/可选参数/能力位；不得改语义、不得删、
   不得把可选参数变必填。
3. 可选能力经握手 `features` 字符串集探测（如 `"ui.pixel"`、`"thumbs.contact_sheet"`），
   插件用前必查。
4. 插件声明的 `api` 高于 Oak 支持：握手返回 `INVALID_PARAMS`，
   `data.supported_api` 给出 Oak 侧主版本，插件应降级或退出。

## 14. 附录：AI 粗剪会话示例（完整报文流水）

```jsonc
// ── 握手
→ {"jsonrpc":"2.0","id":1,"method":"session.hello","params":{
    "api":1,"name":"roughcut","version":"0.2.0",
    "capabilities":["project.read","timeline.read","timeline.edit","render.frame","ui.panel"],
    "panels":[{"id":"chat","title":"AI 粗剪","ui":"declarative"}],
    "subscribe":["timeline.structure_changed"]}}
← {"jsonrpc":"2.0","id":1,"result":{
    "api":1,"oak_version":"0.4.0","granted":["project.read","timeline.read","timeline.edit","render.frame","ui.panel"],
    "features":["ui.pixel"],
    "shm":{"down":{"name":"oakxp-d-7812","slots":8,"slot_bytes":16777216}}}}

// ── 扫时间线：取 12 张缩略图拼 contact sheet 回喂 LLM
→ {"jsonrpc":"2.0","id":2,"method":"render.get_thumbnails","params":{
    "sequence":"sq1","range":{"in":{"num":0,"den":1},"out":{"num":600,"den":1}},
    "count":12,"height":180}}
← {"jsonrpc":"2.0","id":2,"result":{"frames":[
    {"shm":{"region":"down","slot":0},"format":"bgra8","width":320,"height":180,"stride":1280,"bytes":230400,"time":{"num":0,"den":1}},
    … ]}}
→ {"jsonrpc":"2.0","id":3,"method":"shm.release","params":{"slots":[
    {"region":"down","slot":0}, …]}}

// ── LLM 判定 83.2s–141.6s 为废片 → 事务化下刀（用户确认后执行）
→ {"jsonrpc":"2.0","id":4,"method":"edit.begin","params":{"label":"AI 粗剪：删除 83.2–141.6s 废片"}}
← {"jsonrpc":"2.0","id":4,"result":{"txn":"t7"}}
→ {"jsonrpc":"2.0","id":5,"method":"timeline.split_clip","params":{"txn":"t7","clip":"blk9","time":{"num":416,"den":5}}}
← {"jsonrpc":"2.0","id":5,"result":{"clips":["blk9","blk9b"]}}
→ {"jsonrpc":"2.0","id":6,"method":"timeline.split_clip","params":{"txn":"t7","clip":"blk9b","time":{"num":708,"den":5}}}
← {"jsonrpc":"2.0","id":6,"result":{"clips":["blk9b","blk9c"]}}
→ {"jsonrpc":"2.0","id":7,"method":"timeline.ripple_delete","params":{"txn":"t7","clip":"blk9b"}}
← {"jsonrpc":"2.0","id":7,"result":{}}
→ {"jsonrpc":"2.0","id":8,"method":"edit.commit","params":{"txn":"t7"}}
← {"jsonrpc":"2.0","id":8,"result":{}}

// ── 视觉验证：取切口后一帧确认画面正确
→ {"jsonrpc":"2.0","id":9,"method":"render.get_frame","params":{
    "sequence":"sq1","time":{"num":416,"den":5},"max_size":{"width":960,"height":540}}}
← {"jsonrpc":"2.0","id":9,"result":{"frame":{"shm":{"region":"down","slot":0},
    "format":"bgra8","width":960,"height":540,"stride":3840,"bytes":2073600,
    "time":{"num":416,"den":5}}}}
→ {"jsonrpc":"2.0","id":10,"method":"shm.release","params":{"slots":[{"region":"down","slot":0}]}}

// ── 结构变化推送（Oak 合并后下发）
← {"jsonrpc":"2.0","method":"timeline.structure_changed","params":{
    "sequence":"sq1","hint":{"clips_added":["blk9b","blk9c"],"clips_removed":[],"clips_moved":[]}}}
```
