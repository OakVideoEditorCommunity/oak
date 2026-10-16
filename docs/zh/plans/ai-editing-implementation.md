# AI 剪辑实现文档（取证预筛 → 视觉复核 → 事务化下刀）

> 本文是 [ai-agent-design.md](ai-agent-design.md) 的**实现篇**：那份文档定义了
> "多模态 LLM 作为 OPP/1 外部插件"的长期形态与协议细节；本文给出**不等
> P1–P3 的可先行实现路径**——用独立取证工具与引擎 Rust SDK 先把
> "读画面 → 出刀 → 回看"闭环跑通，工具面按 OPP 方法表命名，后续迁移零改语义。
> 行号与状态核验于 `071396241`（2026-10-14）。

## 1. 目标与范围

做四件事：

1. **本地画面变化分析**：帧间相似度预筛，稠密计算找出"哪里变了"，产出切点候选。
2. **多模态视觉复核**：对候选点与区间抽帧回喂 LLM，稀疏定性"为什么变、该不该切"。
3. **自然语言剪辑**：意图 → 工具调用 → 事务化编辑 → 取帧验证 → 修正/撤销。
4. **agent 记忆层**：剪辑经验与工程上下文落本地记忆引擎，跨会话可召回。

明确不做（边界）：不改引擎内核；不在任何引擎进程内跑推理；不实现 OPP/1 宿主
（那是 [external-plugin-system.md](completed/external-plugin-system.md) 的 P1–P6）；
台词级 ASR 转写不在本文（仓库暂无转写能力，见 §8）。

## 2. 现状底座（已逐项核验）

| 能力 | 代码落点 | 状态 |
|---|---|---|
| 编辑纯函数全集（放置/分割/修剪/滚动/滑动/滑移/移动/波纹删/转场/轨道） | `crates/oak-app/src/oakui/graphops.rs`：`place_footage_clip:1605`、`split_clip:2488`、`trim_clip:2590`、`roll_edit:2704`、`slide_clip:2755`、`slip_clip:2835`、`move_clip:2901`、`ripple_delete_clip:3139`、`add_transition_at_seam:1848`、`add_track:1392` | ✅ 在库 |
| 编辑事务与撤销分组 | `crates/oak-undo/src/global.rs`：`group_begin:110` / `group_end:125` / `group_abort:145`，journal 持久化见 `docs/zh/project-storage.md` | ✅ 在库 |
| 取合成帧 / 音频区间 | `crates/oak-app/src/oakui/renderops.rs`：`render_sequence_frame:954`、`render_audio_range:1050` | ✅ 在库（🔶 封门面） |
| 取素材原始帧（CPU，免渲染后端） | `crates/oak-engine-rust-sdk/src/codec.rs`：`retrieve_video_frame:1194` | ✅ 在库 |
| 进程内编辑闭环（SDK 面） | `crates/oak-engine-rust-sdk`：`Project::load/save/import_footage`、`block_split/block_place/block_move/block_trim/ripple_remove_area/add_track/transition_set_offsets`、`undo_group_begin/end/abort`、`render_frame_f32` | ✅ 在库 |
| 时间线互操作 | `crates/oak-otio`（OTIO/FCPXML）；`.ove` 格式文档 `docs/zh/project-file-reference.md` 足以独立读写 | ✅ 在库 |
| 取帧缩略图 / 音频电平（扫描用） | OPP 方法 `render.get_thumbnails`、`render.get_audio_levels` 已在协议冻结（`completed/external-plugin-protocol.md` §8.6），引擎侧对应 `PlaybackCache` 与 `levelmeter` | 🔶 需门面 |
| 场景/镜头检测 | 全仓 `scene/shot detect` 零命中 | ❌ 本文 §4.1 补 |

**结论**：引擎侧"编辑、事务、取帧、存取、互操作"五要素齐备；缺的只是把
app 内部函数变成版本化门面（OPP 宿主层，非本文范围）与取证工具（本文 M0）。

## 3. 总体架构

```
┌─ 取证层（稠密在本地，稀疏在 LLM）───────────────────────────┐
│ 相似度预筛(全片帧差/感知哈希) → 变化曲线 → 峰值候选(时间码)      │
│ → 分层抽帧(粗扫 contact sheet → 疑问区间加密) → LLM 视觉复核    │
└──────────────────────┬──────────────────────────────────┘
                       │ 候选切点表（有理秒）
┌─ 编辑层 ──────────────┴──────────────────────────────────┐
│ 自然语言意图 → LLM tool_calls（schema 对齐 OPP §8 方法名）    │
│ → 适配层事务编排（begin/commit/abort，LLM 不见 txn）          │
│ → 引擎编辑面（现：Rust SDK；将来：OPP/1 同名方法）             │
│ → commit 后取帧回读 → 判定通过/undo/修正事务                   │
└──────────────────────┬──────────────────────────────────┘
┌─ 记忆层（agent 侧，引擎无感知）─────────────────────────────┐
│ 本地记忆引擎（明文 JSONL + 本地向量，两档：工程级随工作区 /      │
│ 用户级兜底；事实句标题、敏感闸拒明文密钥）                       │
│ agent 核 = 轻量 CLI agent（AGENTS.md 常驻指令 + TS 扩展 + MCP）│
└─────────────────────────────────────────────────────────────┘
```

三条铁律（继承 ai-agent-design.md §1.2）：agent 只经版本化工具面访问引擎；
一切编辑必须包在事务里；引擎对 agent、对记忆、对 MCP 均无感知。

## 4. 取证层实现

### 4.1 M0：本地相似度预筛（独立先行，不依赖引擎进程）

- **抽帧**：素材文件直接走解码器逐点取帧（SDK `retrieve_video_frame`，任意时间戳
  CPU 解码），或外部 FFmpeg（`scdet` / `select=scene` 语义对齐）。
- **相似度双通道**：帧间直方图差（便宜、对硬切敏感）＋ 感知哈希 pHash（对缓慢
  运镜、光照漂移稳），输出逐采样点变化曲线。
- **双阈值迟滞**：高阈值峰 = 硬切候选；中阈值峰 = 动作/转场候选；峰带时间码落
  JSON 切点表。这一层是稠密计算（每秒可采多点），**成本近零，永不调用 LLM**。
- 仓库现状无场景检测实现（上表末行），M0 正是补此空白；对素材文件即可运行，
  是整条链路里唯一可以完全脱离 oak 进程的部件。

### 4.2 分层抽帧与视觉复核（LLM 只看关键少数）

1. **粗扫**：候选区间等间隔抽 16–64 图拼 contact sheet（标时间码），一次调用让
   LLM 定位"人何时进画面、哪段该切"；回报值一律为**时间区间（有理秒）**。
2. **加密**：对可疑区间二分加密抽帧——等间隔抽帧会漏短事件（闪切、口误），
   峰值候选（§4.1）天然给出加密锚点。
3. **单帧复核**：对切点峰顶帧逐张定性——硬切 / 动作 / 转场 / 闪烁，输入进编辑层
   的"该不该切"判据。
4. **自律限流**：即便不经 OPP，也按协议约束设计（`get_thumbnails` count≤64、
  短边 ≤1080、8 帧/s 令牌桶），将来过门面无需重写。

## 5. 编辑层：自然语言 → 事务工具

- **工具面约 20 个，方法名逐一对齐** ai-agent-design.md §3 的 OPP 映射表
  （`open_project` / `get_project_overview` / `import_footage` / `place_clip` /
  `split_clip` / `trim_clip` / `move_clip` / `ripple_delete` / `add_transition` /
  `add_marker` / `add_effect` / `set_param` / `set_keyframe` / `get_frame` /
  `scan_timeline` / `get_audio_levels` / `play` / `export_render` /
  `undo_last_action`）。M1 由 Rust SDK 直连实现，M2 换 OPP/1 适配层——
  **同一 agent 换适配层即跑**。
- **事务编排在适配层**：LLM 的一次"动作"可能含多条工具调用，统一包成一个事务；
  任一失败 `abort` 并把错误回喂 LLM 重规划；LLM 全程不接触事务令牌。
- **出参约束前移**：效果参数 `min/max/choices` 回填 tool schema，模型出参即合法。
- **验证闭环**：`commit` 后立即 `render_frame` 取切口/效果帧——像素级断言
  （复用黄金帧 harness）＋ LLM 看图判定"效果是否符合意图"，不过则 `undo` 或追加
  修正事务。
- **双层确认与整段撤销**：待确认动作清单（用户点"执行"才提交）＋ 会话级逐事务
  回滚，语义照搬 ai-agent-design.md §5.2/§4。

## 6. 记忆层与 agent 核

- **记忆引擎**（参考实现：memlite，明文 JSONL＋本地向量检索）：两档存储——
  工程级随工作区、用户级兜底，检索自动回退；`remember/recall/show/update/forget`
  CLI 五件套；写前敏感闸（明文口令/token 拒入，只存位置指针）；同步走白名单
  数据文件。剪辑经验（"这类素材常见切点节奏""上次粗剪被判不合格的原因"）落
  工程级库，随工程走。
- **agent 核**：轻量 CLI agent（AGENTS.md 常驻指令＋TS 扩展＋MCP 接入均可），
  记忆经 CLI 在 agent 内部完成召回与写入——引擎、插件、协议层对记忆零感知。
- **密钥边界**：LLM API key 存 agent/插件自己的配置，绝不写入 `.ove` 工程与
  引擎配置（ai-agent-design.md §1.2 铁律 4）。

## 7. 里程碑

| 里程碑 | 内容 | 依赖 | 验收 |
|---|---|---|---|
| **M0 取证工具** | 相似度预筛＋分层抽帧＋contact sheet，输出 JSON 切点表 | 无（对素材文件独立运行） | 已知切点素材检出率达标；峰值表人工抽检可用 |
| **M1 进程内闭环原型** | Rust SDK：load → import → 事务编辑 → `render_frame_f32` → PNG 回喂 LLM → 修正；记忆引擎接上 | M0 | 真素材粗剪一段，整段 Ctrl-Z 可回滚；黄金帧断言过 |
| **M2 工具面正式化** | 工具 schema 与 OPP §8 全表对齐，适配层可切换 SDK / OPP/1 双后端 | OPP P1–P3（M2 的 OPP 半边） | 同一 agent 换适配层零改语义跑通 |
| **M3 面板与无头** | AI 面板（对话＋待确认清单＋撤销会话）与 CI 无头模式 | M2＋OPP P4 | 对话粗剪一段素材；CI 无网络（Mock LLM）回放全绿 |

与 ai-agent-design.md 里程碑 A1–A5、插件系统 P1–P6 完全兼容：M0/M1 是其**前置
可交付物**，M2/M3 分别并入 A2/A4。

## 8. 风险与边界

- **时间精度**：LLM 一律输出有理秒/帧号，结构化输出＋schema 校验，禁止裸浮点。
- **内容判断上限**：粗剪打点可用，精剪卡点必人审；本地小模态模型更弱，BYOK
  云端优先、离线可降级为"仅工具说明＋手动执行"。
- **台词级剪辑（剪口误、卡语速）**：需 ASR，仓库暂无——音频电平/LUFS 已有
  （`levelmeter`），可先做静音/停顿的粗剪，转写另立文档。
- **相似度漏检**：等间隔方案漏短事件——§4.1 峰检＋§4.2 分层双保险；残余漏检
  由人工抽检兜底。
- **时延形态**：LLM 秒级往返＋取帧渲染 ⇒ 交互是"提案 → 确认 → 执行"，非实时
  操控；扫描按限流配额规划，收到 `RATE_LIMITED` 按 `retry_after_ms` 退避。
- **不突破既有铁律**：推理不进引擎进程；不绕过版本化工具面；事件推送、声明式
  面板等待办依赖 OPP 宿主层，本文不重复定义。

## 9. 测试与验收（对齐项目既有风格）

1. **M0**：已知切点标注素材上的检出率/误检率；峰值表 golden 文件比对。
2. **Mock LLM 回放**：录制/回放 tool_call 序列，CI 无 key 无网络跑通全链路。
3. **黄金帧校验**：复用 render-worker 端到端 harness（真实渲染 ≥2 帧＋像素断言），
   证明"agent 的编辑确实改变了画面"。
4. **会话回放**：tool_call＋帧哈希落盘日志，可回放复现、可作回归夹具
   （与 ai-agent-design.md §6 同源）。
