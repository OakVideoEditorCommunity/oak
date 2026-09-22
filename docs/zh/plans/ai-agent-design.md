# AI Agent 插件设计（基于 OPP/1 外部插件系统）

> 本文是 Oak 引入 AI 能力的长期设计，**已按外部插件系统重写**（旧版假设
> RIIR 拆分后面对一堆 C ABI 小库、引擎内置 `oak-mcp-server`，已作废）。
> AI 能力现在是一个**外部功能插件**：独立进程、经 OPP/1 协议操作 Oak。
>
> 依赖文档（冲突时以它们为准）：
> - [`completed/external-plugin-system.md`](completed/external-plugin-system.md)——插件系统总体设计
>   （进程模型、能力位、确认模式、里程碑 P1–P6）
> - [`completed/external-plugin-protocol.md`](completed/external-plugin-protocol.md)——**OPP/1 协议全文**
>   （方法/事件/事务/shm/UI 的冻结定义，本文引用的 §n 均指该文档）
>
> **一句话**：多模态 LLM 跑在一个独立插件进程里，经 OPP/1 的策展宿主 API
> 操作 Oak（事务化编辑、确认后执行），经 `render.get_frame/get_thumbnails`
> 的 shm 取帧通路把画面回喂模型，形成"编辑 → 看图 → 再编辑"的视觉闭环。

---

## 1. 定位与前提

### 1.1 前提（未满足不动工）

- 插件系统 **P1–P3 完成**：传输与生命周期、宿主 API 核心（事务 +
  project/media/timeline/node 方法族）、取帧与导出的 shm 数据面可用。
- AI 面板需要 **P4**（声明式 UI）；无头模式（脚本/CI）只依赖 P1–P3。
- Python SDK `oakxp`（P6 的一部分）是本插件的载体，二者同期开发、互为验证。

### 1.2 设计铁律（继承旧版，按插件系统重述）

1. AI Agent **只经 OPP/1** 访问 Oak——插件进程内一行 Oak 代码都没有，
   不链接任何 Oak 产物（铁律：协议是唯一边界）。
2. 一切编辑动作**必须包在 `edit.begin/commit` 事务里**（协议 §6），
   历史面板一次 Ctrl-Z 整段撤销；UI 默认"确认后执行"。
3. 不为 AI 发明新的引擎内部机制；Agent 工具面是 OPP/1 方法的组合，
   OPP/1 方法又是现有 `graphops/renderops/oak_task` 的组合。
4. API key 绝不写入 **Oak 侧**的任何文件（`.ove` 工程、Oak 自身配置）；
   允许存**插件自己的**配置文件（§5.1），环境变量仅作 CI/无头场景的覆盖项。

## 2. 总体架构

### 2.1 插件进程内部分层

插件名 **`oak-plugin-ai`**（Python 3，实现语言与发行形态的论证见 §2.3；
参考实现即插件系统的 `examples/plugin-roughcut` 的完整版）：

```
┌─ Oak 主进程 ────────────────────────────────┐
│  oak-plugin-host（P1–P4 提供）               │
│   ├ OPP/1 控制面（JSON-RPC over stdio）      │
│   └ shm down/up 区域                         │
└───────┬─────────────────────────────────────┘
        │ stdio + shm
┌───────┴─── oak-plugin-ai（独立进程）────────┐
│ ⑤ LLMProvider：Claude/GPT │ llama.cpp 本地    │
│ ④ Agent 编排：对话 loop、工具调用、视觉闭环    │
│ ③ 工具适配层：OPP 方法 → LLM tool schema     │
│ ② 会话状态：事务令牌、帧缓存、操作日志         │
│ ① oakxp SDK：分帧/收发/shm attach/帧→PNG     │
└─────────────────────────────────────────────┘
```

- 插件可以**纯无头**跑（CI、批处理脚本：spawn 后不注册面板，只走工具面）；
  注册面板时才是用户可见的"AI 剪辑助手"。
- **MCP 的位置**：如要让外部 LLM 客户端（Claude Desktop、agent 框架）直连，
  由**插件自己**在进程内起一个 MCP server，把 §3 的工具面按 MCP 再暴露一次。
  Oak 内核始终对 AI、对 MCP 无感知——这是与旧版"引擎内置 oak-mcp-server"
  的根本区别。

### 2.2 视觉闭环（本设计的核心）

```
多模态 LLM ──► Agent 编排 ──► OPP 事务化编辑 ──► render.get_frame ──► PNG ──► 回喂 LLM
   ▲                                                                          │
   └────────────────── 看图判断（效果/切点/内容定位） ◄─────────────────────────┘
```

- **验证式**：`edit.commit` 后立即 `render.get_frame` 取切口/效果帧，LLM 判断
  "效果对不对"，不对则 `edit.undo` 或追加修正事务。
- **内容感知式**：`render.get_thumbnails(range, count≤64)` 等间隔采样拼
  contact sheet，LLM 扫图定位（"人何时进画面""哪里该切"），据此下刀——
  自动粗剪/打点的雏形。
- **连续回放**：`playback.play` + `playback.playhead_moved` 事件（≤30 Hz）+
  按间隔 `get_frame` 采样。注意受 §12 限流约束（默认 8 帧/s），采样间隔
  不得小于限流周期。
- 完整报文流水示例见协议文档 §14（握手→缩略图→事务下刀→验证帧→事件），
  可直接作为本插件的集成测试夹具。

### 2.3 实现语言与跨宿主复用（决策已定）

**定为 Python，解释器嵌入式发行**：插件包内含私有 CPython（PyInstaller 或
python-build-standalone），`.oakplugin` 单包交付，用户无需自行安装 Python。
插件以 **GPLv3** 开源发布；作为独立进程经 stdio/JSON-RPC 与 Oak 通信，
许可证选择不影响 Oak 本体。

选 Python 而非 Rust 的理由：

1. **性能无关**：插件只做协议编解码、LLM 编排、PNG 编码（Pillow 为 C 实现）；
   延迟大头是 LLM 往返（秒级）与 Oak 侧渲染（与插件语言无关），插件进程
   没有任何重计算。
2. **跨宿主复用**：同一套 AI 能力规划覆盖 Oak / DaVinci Resolve / Premiere，
   但三家宿主的扩展 API 语言各异——Resolve 是 Python/Lua；**Premiere 没有
   Python 入口**（UXP/JavaScript 面板 + C++ SDK）。因此正确结构不是
   "一种语言通吃"，而是**一份与宿主无关的 AI core + 各宿主薄适配层**：

```
┌─ AI core（宿主无关，一份代码）───────────────┐
│  Agent 编排 / LLMProvider / tool schema      │
│  提示词策略 / 会话状态                        │
└──────┬───────────┬──────────────┬───────────┘
   Oak 适配层    Resolve 适配层   Premiere 适配层
   (OPP/1,       (Resolve         (UXP/JS 面板 →
    oakxp SDK)    Python API,      localhost 调
                  直接 import)     core 本地服务)
```

   - Oak 适配层 = oakxp SDK（OPP/1），即本文的 `oak-plugin-ai`；
   - Resolve 适配层直接 import core（Python 母语，零成本复用）；
   - Premiere 用 UXP 面板做壳，经 localhost 与本机 core 进程通信。
3. **生态**：主流 LLM SDK（anthropic / openai / llama.cpp 绑定）均为 Python
   一等公民，Agent 框架与评测工具链也最全。

## 3. 工具面（LLM tool schema → OPP/1 方法映射）

Agent 暴露给 LLM 的是约 20 个**策展工具**，每个是 OPP/1 方法的薄组合
（协议 §8 是方法全文，下表"OPP 方法"列即最终调用）：

| Agent 工具 | OPP 方法 | 说明 |
|---|---|---|
| `open_project` / `save_project` | `project.open/save` | 工程生命周期（事务 + 确认类） |
| `get_project_overview` | `project.get_info` + `timeline.get_structure` | 一次返回序列/轨道/块树，供 LLM 建立上下文 |
| `probe_media` / `import_footage` / `list_footage` | `media.probe/import_footage/list_footage` | 媒体探测与导入 |
| `add_track` / `place_clip` / `split_clip` / `trim_clip` / `move_clip` / `ripple_delete` / `add_transition` / `add_marker` | `timeline.*` | 时间线编辑，全部在事务内 |
| `add_effect` / `set_param` / `set_keyframe` / `list_effects` | `node.add_effect/set_param/set_keyframe/list_types` + `get_params` | 效果与关键帧；`get_params` 的 min/max/choices 回填进 tool schema，约束 LLM 出参 |
| `get_frame(time)` / `scan_timeline(range,n)` / `get_audio_levels` | `render.get_frame/get_thumbnails/get_audio_levels` | **视觉闭环取帧口** |
| `play` / `pause` / `seek` | `playback.*` | 回放控制 |
| `export_render(preset)` | `export.start` + `export.progress/done` 事件 | 导出（确认类） |
| `undo_last_action` | `edit.undo` | 仅用户明确要求时调用（协议 §6 规则 5） |

**事务编排是适配层的职责，不暴露给 LLM**：LLM 的一次"动作"（可能含多个
`timeline.*` 调用）由适配层包成一个事务——先 `edit.begin(label=LLM 动作摘要)`，
串行执行（协议保证同事务内按到达顺序），任一失败则 `edit.abort` 并把错误
回喂 LLM，全成功才 `commit`。LLM 看不到 `txn` 令牌，从根上避免"忘记 commit"
"嵌套事务"这类误用。

**取帧→PNG 通路**（关键路径）：`render.get_frame` 返回 `FrameRef`（shm 形态：
`bgra8` + 槽位号），SDK `frame.to_png()`（Pillow）编码 → base64 → 作为图片
消息发给 LLM；随后**立即 `shm.release`**——批量扫描时必须流水线化释放，
否则 8 槽耗尽触发 `SHM_EXHAUSTED`（协议 §10.3）。小图（≤64 KiB）Oak 可能
直接 inline PNG，SDK 对两种形态透明。

## 4. AI 面板（声明式 UI，协议 §11.1）

面板在 `session.hello.panels` 声明 `"ui":"declarative"`，控件树：

```
column
├── chat_log    #log      对话与操作日志（用户/助手/系统三角色）
├── list        #pending  待确认动作清单（确认模式，见 §5）
├── row
│   ├── text_input #prompt  剪辑意图输入
│   └── button     #send    执行
└── progress    #job      扫描/导出进度
```

- 交互经 `ui.event` 上行（`submit`/`click`/`select`），插件用
  `ui.set_props` 增量追加 `chat_log` 条目、更新进度。
- **"撤销整段会话"**：面板放一个按钮，逐个 `edit.undo` 回滚本会话提交的
  事务（插件在自己的会话状态里记事务顺序）。
- 面板被关闭会收到 `panel_closed`，重开收到 `panel_shown` 时重发
  `ui.set_tree` 恢复（协议 §11.1）。
- 像素面 UI（§11.2）本插件**不用**——聊天面板声明式足够。

## 5. 模型层与安全

### 5.1 LLMProvider

抽象接口（输入：消息 + 图片；输出：文本 + tool_calls），后端：

- **云端 BYOK**：Claude / GPT 多模态，用户自带 key（效果优先）。
- **本地**：llama.cpp 跑 Qwen-VL / LLaVA 类多模态模型（隐私、离线优先）。
- **自定义 endpoint**（OpenAI 兼容接口）：把 provider 指向任何兼容
  `/v1/chat/completions` 的服务（自建代理、企业网关等）——为后续接入
  托管服务预留通用通路，不绑定特定厂商。

API key 存**插件自己的配置文件**（如 `~/.oak/plugins/oak-plugin-ai/config.toml`），
面板提供密钥输入框（`text_input`），用户无需手配环境变量；环境变量只作
CI/无头场景的覆盖项（优先级：环境变量 > 配置文件）。配置文件权限 0600、
不进版本库。存插件自己的配置是插件的内部事务，Oak 不感知——铁律 4 约束的
只是 **Oak 侧**的文件。无 key 时优雅降级：面板仍可用，但只做"工具说明 +
手动执行"，不做对话编排。

### 5.2 能力位与确认（协议 §7 的具体化）

manifest 声明**最小必要集**：

```toml
capabilities = ["project.read", "media.read", "media.import",
                "timeline.read", "timeline.edit", "node.read", "node.edit",
                "render.frame", "playback", "export", "ui.panel"]
```

双层确认，职责分清：

1. **Oak 协议层**（§7.3）：`timeline.edit` 等确认类方法默认弹窗
   "插件 oak-plugin-ai 请求：split_clip …"。用户可选"本会话内允许"。
2. **插件会话层**：Agent 把 LLM 规划出的整段动作先列入 `#pending` 清单，
   用户点"执行"才发事务——这是体验层确认，与协议层弹窗不冲突：
   建议插件引导用户在 Oak 侧对本插件设"本会话允许"，确认交互集中在面板内。

### 5.3 其他安全约束

- **限流遵守**：扫描采样按握手 `limits` 下发的配额规划（默认 8 帧/s、
  短边 ≤1080），收到 `RATE_LIMITED` 按 `retry_after_ms` 退避，不得重试轰炸。
- **沙箱会话**（可选增强）：对破坏性大改，Agent 可先 `project.save` 副本到
  临时路径操作，用户接受后再回真实工程。有了事务 + 整段撤销后，此项降为
  可选，默认不启用。
- 插件崩溃不丢编辑：已 commit 的事务都在 UndoStack 里，未决事务 Oak 自动
  abort（协议 §6 规则 4）——AI 死在哪都不会留下半截剪辑。

## 6. 可测试（与项目风格一致）

1. **Mock LLM server**：录制/回放 tool_call 序列与固定回复，Agent loop 在
   CI 无 key 无网络跑通。
2. **Mock Oak（协议级）**：`oakxp` SDK 自带回放 harness——把协议文档 §14 的
   报文流水当夹具，插件不连真 Oak 也能单测工具适配层与事务编排。
3. **黄金帧校验**：连真 Oak 的端到端测试复用 render-worker harness
   （真实渲染 ≥2 帧 + 像素非全黑 + 一致性断言），验证"Agent 的编辑确实
   改变了画面"。
4. **会话回放**：tool_call + 帧哈希落盘日志，可回放复现、可作测试夹具。

## 7. 里程碑（对齐插件系统 P1–P6）

| 里程碑 | 内容 | 依赖 | 验收 |
|---|---|---|---|
| **A1 骨架** | `oakxp` SDK + 握手/心跳/重连；插件注册空面板 | P1、P4 | 杀掉插件 Oak 不崩、面板徽标与重启正常 |
| **A2 工具适配层** | §3 全表映射 + 事务编排（自动 begin/commit/abort）+ 取帧→PNG 通路 | P2、P3 | Mock Oak 夹具全绿；真 Oak 上"导入→铺轨→切开→删除→加效果"可整段撤销 |
| **A3 无头闭环** | Agent 编排 + LLMProvider + Mock LLM | A2 | CI 无网络跑通"LLM→工具→取帧→回喂"；黄金帧校验过 |
| **A4 AI 面板** | §4 面板 + 待确认清单 + 撤销会话 | A3 | 真机对话粗剪一段素材，确认/撤销交互完整 |
| **A5 内容感知** | contact sheet 扫描打点、自动粗剪策略、本地模型 provider | A4 | 对 10 分钟素材自动出粗剪版，人工抽检切点可用 |

## 8. 风险与边界（明确不做）

- **不**把 LLM/推理放进任何 Oak 进程或引擎模块（引擎对 AI 无感知）；
  MCP server 如需存在，只在插件进程内。
- **不**绕过 OPP/1 访问 Oak（协议是唯一边界）；LLM 不直接接触 `txn`
  令牌（适配层封装事务）。
- **不**把 API key 写入 Oak 工程文件或 Oak 自身配置（插件自己的配置文件
  除外，见 §5.1）。
- **不**让取帧回路阻塞 Oak GUI 线程——`render.*` 本来就走引擎 ticket/进程池
  路径（协议 §8.6），插件侧并发流水线化而不是串行等帧。
- **不**突破协议限流；需要更高帧率的"连续回放分析"场景，先按 §13 版本
  演进规则给协议加配额项，不在插件侧硬挤。
- 第三方大模型客户端的接入细节（OAuth、计费、配额）**超出本文范围**，
  按需另立文档。
