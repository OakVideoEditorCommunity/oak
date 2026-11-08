# AI 剪辑路线状态台账（T0–T6）

> 性质：**只读审计快照**，供 T1 派发与后续里程碑验收使用。
> 审计对象：`docs/zh/plans/ai-editing-feature-plan.md`（下称"规划"）§3 的 T0–T6。
> 审计人：翰林院编修（`hanlin-audit`）；审计时间 **2026-11-05 11:03–11:25 CST**。
> 仓库状态：分支 `ai-feature`，HEAD `bf64bee6c`（`docs: plan AI editing feature branch …`）。
> 工作树：`git status` = ` M .gitignore`、`?? ai/`、`?? docs/zh/plans/resolve21-oak-applicability.md`
> ——**`ai/` 整棵目录尚未入仓**，本台账所有 `ai/*` 证据均指工作树内容，不是已提交状态。
> 方法：命令实测（记退出码与原文）+ 源码静态核查。未修改 `ai/`、`crates/` 任何文件；未构建引擎。
> 修订：**2026-11-05 11:40（task-9）**——T1 判定按 sdk 后端落地与 59 项测试复跑改写（§2）、T0 参数口径入账（§1）、新增「核验记录」（§0.1）与「入仓提交」（§10）。原始审计段落保留，过期处已在原处标注改写。
> 并发说明：审计期间 `ai/oak-ai-tools/src/sdk.rs` 于 **11:06:40** 出现（57 字节，仅一行注释占位），
> 非本审计产出；下述 T1 结论以该快照为准，若该文件随后被实现，请重跑 §6 命令复核。

## 0. 总览

| 里程碑 | 规划验收 | 实际 | 结论 |
|---|---|---|---|
| T0 schema 冻结 | schema 单测全绿 + 对照表防漂移 | 22 个工具 schema、27 合法 / 13 非法样本、三张对照表；`test_schema.py` 524 项全过，Rust 侧 4 个漂移/样本测试全过；参数范围**只回填 min/max（15 键）**，choices 经 `node.list_types` 回读（显式 open，已裁断，见 §9） | **已完成** |
| T1 工具执行层 | 真实引擎脚本 E2E：导入→铺轨→split/trim/ripple_delete→取帧→像素断言→整段 undo 还原 | 真机 E2E **2/2**（`tests/sdk_e2e.rs`，duration 兜底由 `:182` 断言钉住、消融删则红）；独立复跑默认 **90/0/0**、`--features sdk` **96/0/0**（task-14 `/tmp/oak-reviews/t1.md`）；22 方法全覆盖，缺能力一律 `unsupported` + 6 个 gap id | **已完成（已入仓，§10.3）** |
| T2 取证预筛 | 已知切点检出率达标 + 峰值表 golden 比对 | `prescan.py` 双通道独立进程；**32 项**测试全过（首轮 27 + 静态素材修复新增 5，含 golden 比对、2/2 已知切点）；静态素材产出空切点表 | **已完成** |
| T3 agent 核 + 记忆 | `pi --print` 无头冒烟、跨会话召回、密钥不落盘 | `ai/agent/` **已入仓（§10.5）**：pi 1.0.3 + 三层上下文链 + memlite 两档；stub 回放无头冒烟（未用真实 LLM）exit 0、live_tool_count=22；跨会话两进程召回复现；密钥 grep 零命中；消融 D 红吻合。验收 task-15 `/tmp/oak-reviews/t3-agent.md` pass | **已完成（已入仓，§10.5）** |
| T4 适配层 hooks | 三档确认门行为测试 + 失败回喂闭环 | `src/hooks.rs` + `tests/hooks_{policy,readback,replan}.rs`（11/7/4）；ProposalOnly 零副作用（Spy 15/15 全部变更方法）、分级表 fail-closed 双向守（删条目 + 改长度 → 恰 2 项红）、`execute()` 签名向后兼容；全量 **90/0/0** 独立复跑（task-13 `/tmp/oak-reviews/t4-hooks.md`） | **已完成（已入仓，§10.3）** |
| T5 MCP 封装 | 同一 schema 的 MCP server，零漂移 | `ai/oak-ai-mcp` **已入仓（§10.4）**：stdio NDJSON JSON-RPC 2.0，22 工具零漂移三方一致；协议级 **30/0/0**（含版本回声断言）独立复跑；`--backend sdk` 接线实测 7 passed（接线冒烟≠引擎功能可用）；验收 task-12 `/tmp/oak-reviews/mcp.md` + task-16 `/tmp/oak-reviews/t5-delta.md` 均 pass。`agent/mcp-oak` 仍是早期 Python 原型（不计入交付） | **已完成（已入仓，§10.4）** |
| T6 无头 CI | Mock LLM 录制回放 + 黄金帧 + 会话回放日志进 CI | `ai/ci/` + `.github/workflows/ai-headless.yml` **已入仓（§10.6）**：`run.sh` 一条命令七段；回放日志两次逐字节复现（帧 sha256 钉住）；消融坏链路 EXIT=101；黄金帧为运行时不变式、不入大图；workflow 默认不构建引擎。验收 task-17 `/tmp/oak-reviews/t6-ci.md` pass（七段实测 524/32/90/30/4+1ignored/6 工具 0 错/lint） | **已完成（已入仓，§10.6）** |

### 0.1 核验记录（独立验收）

- **T0 schema 冻结 + T2 取证预筛：通过**。验收人：大理寺评事（task-7），记录 `/tmp/oak-reviews/t0-t2.md`，日期 2026-11-05，被审快照 HEAD `bf64bee6c` 工作树。结论 pass：T0 双向一一对应、时码纪律（禁裸浮点）、参数范围回填、测试真实性（6 类样本消融后全部精确变红）均成立；T2 已知切点 27/27、golden 为**容差比对**（帧 ±5、分数 ±0.15，结构精确）、抽走 ffmpeg 后 `FAILED (errors=5)` exit 1（无静默 skip）。
- 3 项非阻塞缺陷及处置：
  1. **prescan 对静态素材报错、产不出空切点表**（`prescan.py:220-224`，假失败）→ 已另立 **task-8**（`gongbu-mcp`）修复；本次入仓的是已通过验收的行为，修复随 task-8 单独提交。
  2. **无切点素材存在 soft 候选误报**（`motion` 素材 2 条 soft，全靠 pHash 尖峰）→ **维持已知限制**（`ai/prescan/README.md:166-174` 已写明；`kind=soft` 定义为需人工复核的带）。
  3. **T0 效果参数 `choices` 未进 schema** → **口径已定**：min/max 回填（15 键，与引擎源码一致），choices 走 `node.list_types` 回读，记为显式 open；首辅已接受（§9）。
- **T2 修复回归验收：pass**（`/tmp/oak-reviews/t2-fix.md`，2026-11-05，同一验收人）：修复成立（零分数按 ffmpeg 退出码判别）、**双向回归锁成立**（改回旧逻辑 → 4 项红；过度放宽 → 1 项红）、既有 cuts 数值与首轮逐值相同、golden 与容差未动、全量 32 项 OK。非阻塞项：README 测试数字陈旧（本已更新）、台账数字陈旧（本已更新）、两条残留边界（已写入 README「Known limitations」）。
- **验收快照的边界（读台账必看）**：首轮 T2 验收针对 11:32 快照，**不含** task-8 的静态素材修复；该修复经上一条回归验收独立确认后随第二次提交入仓。
- 交付级前置（`ai/` 未入仓）按首辅裁断 B 分两步消解：**T0（`ai/schema/`）+ 本台账 + `.gitignore` 先行入仓**（§10 第一次提交）；**T2（`ai/prescan/`）等 task-8 完成后单独提交**，届时 §10 补第二行。

## 1. T0 schema 冻结：已完成

证据：

- 工具 schema 22 个：`ai/schema/tools/*.json` = 22（`ls | wc -l`）；样本 `samples/valid` 27、`samples/invalid` 13。
- 对照表 `ai/schema/method-map.md`：表① 22 工具↔OPP §8（:13-38）、表② §8 全量 38 条（:53-99）、表③ 公共类型约定（:101-113）。
- 运行 `python3 ai/schema/test_schema.py`（exit 0）：

  ```
  tool schemas loaded: 22
  OPP section 8 methods: 38 (mapped 23, deferred 15)
  [A] schema syntax  pass=246 fail=0     [B] method-map drift  pass=12 fail=0
  [C] samples  pass=139 fail=0           [D] hygiene  pass=127 fail=0
  TOTAL: pass=524 fail=0 / RESULT: OK
  ```

- **参数范围回填口径（首辅已裁断，见 §9 第 5 条）**：`node.set_param` 只回填 **min/max**（15 键，与 `crates/oak-node/src/nodes/*.rs` 逐条核对一致）；**choices 不回填**，改由 `node.list_types(node)` 回读（`method-map.md:113` 记为显式 open）。该口径不再视为 T0 缺口。
- Rust 侧同一契约：`ai/oak-ai-tools/tests/schema_samples.rs`（27 合法全过、13 非法全拒，`schema` 错误码）、`tests/registry_drift.rs`（注册表与 `tools/` 双向集合相等且计数 22）。二者均在 §6 的 `cargo test` 中通过。

缺口：无阻断项。两点已知口径需带进 T1：`project.open` 的"缺路径新建工程"是工具面扩展而非 OPP 方法（`method-map.md:17`）；§8.10 `ui.*` 与 §9 事件不入工具面（`method-map.md:96-99`）。

## 2. T1 工具执行层：已完成（已入仓，§10.3）

> 2026-11-09 修订：真机验收 pass（task-14，`/tmp/oak-reviews/t1.md`）——默认 `cargo test --offline` 90/0、`--features sdk` 96/0（`sdk_e2e` 2/2 真机跑通、duration 兜底消融成立），随第三次提交入仓（§10.3）。以下正文仍为 2026-11-05 审计快照及其后至 11-08 的进展记录，其中「59 项」「真机验证进行中」等表述已被本次验收结论取代。

已完成（Mock 通路，可独立构建/测试）：

- 22 工具注册表：`src/dispatch.rs:34-35`（`ToolDef` 22 项），schema 动态编译校验 `src/schema.rs`，`dispatch::execute` 流程 `src/dispatch.rs:247`。
- 后端契约 22 方法 + 事务基元：`src/backend.rs:24-91`；`MockBackend` 实现 `src/mock.rs`。
- "一次工具调用 = 一个 undo group"：`src/txn.rs`（`TxnOps` begin/commit/abort；失败 begin→invoke→abort 并保留错误码）。
- 取帧→PNG 与黄金帧断言工具：`src/render.rs:19`（`Frame`）、`:97`（`to_png`）、`:233`（`assert_frames_close`）。
- 结构化失败回喂模型：`src/error.rs`（11 个稳定 code，`unsupported(gap, …)` 在 `:74`；注释明确"缺能力不得报成功"）。
- E2E（Mock）：`tests/mock_e2e.rs:31` `acceptance_chain_render_and_full_undo`（导入→铺轨→split→trim→ripple_delete→取帧→PNG 解码像素断言→整段撤销后状态还原）、`:237` `one_call_is_one_undo_group_and_failure_rolls_back`。

现状更新（审计后于 2026-11-05 11:12–11:42 由 `gongbu-sdk` 推进，本节已据此改写）：

1. **`sdk` 后端已落地**：`ai/oak-ai-tools/src/sdk/{mod.rs, backend.rs, gaps.rs}` + `tests/sdk_e2e.rs`（首行 `#![cfg(feature = "sdk")]`，默认构建不含）。审计初记的 `src/sdk.rs` 57 字节占位已被替换；`src/lib.rs:33-34` 的 `pub mod sdk;` 现在有真实实现。
2. **默认 features 59 项测试通过**（2026-11-05 11:42 复跑 `cargo test --offline`，exit 0）：lib 52 + `mock_e2e` 3 + `registry_drift` 2 + `schema_samples` 2；`sdk_e2e` 在默认 features 下记 0 项。
3. **`--features sdk` 真机验证进行中 → 未验证**：`cargo test --features sdk` 自 11:12 起在 `/tmp/oak-ai-sdk-target` 构建（需连带引擎与 FFmpeg/ocio），至 11:41 仍在编译依赖，**尚无 test result**。任何"通过"结论必须附 `test result:` 原文与 target 目录，否则按未验证记录。
4. **README 仍缺**：`Cargo.toml:26` 与 `error.rs:72` 引用的 "Feature gates / gap 表" 尚无落点（task-2 在办）。`sdk` 门控测试已存在，不再是"零 sdk 测试"。

### T1 剩余工作量（截至 2026-11-05 11:42）

| # | 文件/事项 | 状态 |
|---|---|---|
| 1 | `src/sdk/{mod,backend,gaps}.rs`（SdkBackend + TxnOps + EngineBackend 22 方法） | **已落地**；实现质量待 `--features sdk` 真机结果与独立验收，本台账不作通过判定 |
| 2 | 初始化时序（`render::manager_init(Threads)` → `node::set_active_project`，见 §5 第 18 行） | 需在 sdk 后端复核是否照序实现（未验证） |
| 3 | `tests/sdk_e2e.rs` 真机验收链 | **已落地**（11:38）；未跑通（真机构建未完成） |
| 4 | `ai/oak-ai-tools/README.md`（Feature gates + gap 表） | **仍缺** |
| 5 | 三处 SDK 缺口（`timeline.add_marker` / `render.get_audio_levels` / `node.list_types` 的 get_params） | **已定案**：一律 `unsupported` + gap id + README 记录，本分支不改引擎（§9 第 5 条）；实际实现以 `src/sdk/gaps.rs` 为准（未验证） |
| 6 | 待实测：转场 `type_in`、`export.start` 的 `preset`、`block_place` 重叠语义 | 仍待实测；不可行同样转 `unsupported` + gap id |

## 3. T2 取证预筛：已完成

证据：

- `ai/prescan/prescan.py`：独立进程、只用 `ffmpeg`/`ffprobe` 读素材，不触引擎/SDK（模块 docstring），双通道（scene 分数 + 32×32 灰度 DCT pHash 64 位）+ 双阈值迟滞，输出有理秒 + 帧号。
- `python3 ai/prescan/test_prescan.py`（exit 0）：`Ran 32 tests in 71.128s / OK`（2026-11-05 11:57 复跑；并发构建时曾测 140.8 s，耗时随机器负载浮动）。含 `TestKnownCuts.test_detection_rate`（`Both known cuts detected -> 2/2`）、`test_hard_cut_detected_within_5_frames`、`test_dissolve_band_yields_soft_candidate`、`TestGolden.test_golden_compare`（对比 `ai/prescan/golden/test_scenes.cutlist.json`）、`TestStaticMaterial` 3 项（静态素材 → `cuts: []` + exit 0）、`TestErrorPaths` 6 项、`TestUnits` 7 项。
- 测试素材由 ffmpeg lavfi 现场生成（`test_prescan.py:65` 起的固定配方）。

缺口：无阻断。风险提示：golden 与 ffmpeg 版本强绑定（`README.md`/`test_prescan.py:31-33` 已写"算法或 ffmpeg 变更后重新生成"）——进 CI（T6）前需固定 ffmpeg 版本，否则换机即红。已知边界（随修复写入 README「Known limitations」）：① faststart 截断素材在 `-f null` 下 ffmpeg 可能 rc=0，已解出部分帧时仍按正常素材出表（旧版同样如此，非本次引入）；② 若出现「零分数且 rc=0」的坏文件，会被当作静态素材（空切点表）——回归验收未构造出可复现反例，记为残留项，后续如需保护可在 `scan()` 加 `phash_samples == 0` 判断。

## 4. T3 / T4 / T5 / T6（2026-11-05 快照；2026-11-09 修订：T0–T6 全部入仓 §10.1–§10.6）

**T3 agent 核 + 记忆**

- `ai/agent/` 不存在（`ls ai/` = `oak-ai-tools`、`prescan`、`schema`）。
- 仓根 `agent/` 存在但被本地排除：`.git/info/exclude:9` = `/agent/`，`git ls-files agent` = 0 项。内含：`agent/pi`（仅 `package.json`，依赖 `@earendil-works/pi-coding-agent`，无 `node_modules`）、`agent/sdk-bridge`（934 行 NDJSON 引擎桥，**SDK 用法参考价值最高**）、`agent/mcp-oak`（Python MCP 原型，28 工具）。
- 上下文链断：`AGENTS.md` 是指向 `/home/fslong/AGENT.md` 的符号链接，目标**不存在**（`ls` 报 No such file；`AGENTS.override.md` 为普通文件）。规划 §2 的"AGENTS.md 三层上下文链"当前不成立。
- 记忆层基线：`/home/fslong/.local/bin/memlite` 可用；项目数据目录约定 `/home/fslong/.oak/memlite-data`（`AGENTS.override.md:25`）。

启动前置：① T1 SDK 后端可用（否则工具面为空壳）；② 修复/重建 `AGENTS.md` 链并解决"入仓 vs 本地"（`.git/info/exclude` 排除会让 T3 交付物永远不在 PR 里）；③ 安装 `pi` 并给出无头冒烟命令；④ 决定工程级记忆库的仓外 `--data-dir` 与 git 白名单（规划 §5 纪律 + CI 扫描）。

**T4 适配层 hooks**

- 全仓 grep `before_commit|render_done|edit_failed`：仅 `ai/oak-ai-tools/src/error.rs:6` 一处注释。`src/txn.rs` 无确认门、无 proposal-only 模式；`ai/oak-ai-tools/src/` 内无 `confirm`/`proposal` 标识符。

启动前置：① T1 后端；② 在 `txn::run` 前后插 `before_commit`（`src/txn.rs:39` 起）；`render_done` 挂在 `tools/render_tools.rs` 取帧回读之后；`edit_failed` 复用既有 `ToolError` 结构（已含 `tool/step/txn`）；③ 三档策略（proposal-only / confirm / auto）的配置载体与"按操作类分级"的映射表需先定。

**T5 MCP 封装**

- `ai/oak-ai-mcp` 不存在。`agent/mcp-oak` 不能算 T5 交付：它是独立 Python 实现（`server.py` + `registry.py` 自有 28 工具、`backend.py` 内建会话模型），未消费 `ai/schema/tools/*.json`，且被 git 排除（`.git/info/exclude:9`）。规划要求"同一 schema 的 MCP server，工具名/参数零漂移"，两者不等价。
- 唯一相关现状：`.pi/mcp.json` 已把 `agent/mcp-oak/server.py` 注册为 `oak-edit`（本地件）。

启动前置：① T0 冻结（已满足）+ T1 真实后端实测；② 形态决议：Rust 薄封装复用 T1 vs Python 直调 T1；③ `tools/*.json`（draft-07）→ MCP `inputSchema` 的转换保真（`oneOf`/`if-then`/`pattern` 在客户端侧的兼容性需实测）；④ 双形态一致性测试（直调 vs MCP）的基准。

**T6 无头 CI**

- `.github/workflows/ci.yml` 中 `ai/|oak-ai|prescan` 命中 0 条；`ai/` 未跟踪 → CI 无法运行任何 T0–T2 测试。

启动前置：① `ai/` 入仓（并解决上位 workspace 不受影响，`ai/oak-ai-tools/Cargo.toml` 的空 `[workspace]` 已处理）；② T3+T4 完成；③ Mock LLM 录制/回放格式（tool_call 序列）与"tool_call + 帧哈希"回放日志格式先定义；④ 引擎相关测试（sdk/黄金帧）需要重量级 CI runner 或标记为非必选，先定策略。

## 5. 22 工具 → SDK 现有能力 / 缺口（T1 实现输入）

SDK 侧锚点均在 `crates/oak-engine-rust-sdk/src/`。结论列"可行"= 有直接 API 组合可落地（**未经真实引擎实测**）；"部分"= 主路径可行但有语义待定或半边缺失；"缺口"= 现有 C ABI 无路径。

| # | 工具 | 需要的引擎语义 | SDK 现有 API（锚点） | 结论 |
|---|---|---|---|---|
| 1 | `project.open` | 打开既有工程 / 新建空工程并激活 | `storage::open`（`storage.rs:101`，返回 `OpenedProject.project`）；`Project::new`（`node.rs:300`）、`set_filename`（`:809`）、`set_active_project`（`:766`）、`Project::load`（`:799`） | 可行 |
| 2 | `project.save` | 存回原位/另存 | `Project::save(Option<&str>)`（`node.rs:790`）；`storage::save`（`storage.rs:123`，带 save 位掩码） | 可行 |
| 3 | `project.get_overview` | 工程元信息 + 序列/轨/块结构 + 素材表 | `sequence_track_list_count/at`（`:427/432`）、`tracklist_kind/track_count/track_at`（`:494/499/504`）、`track_block_count/at`（`:514/519`）、`block_in/out/length/media_in/media_out`（`:536-556`）、`footage_filename/counts/duration`（`:373-400`）、`folder_child_count/at`（`:360/366`）、`node_label`（`:482`）、`node_type_id`（`:353`） | 可行 |
| 4 | `media.probe` | 流清单（宽高/帧率/采样率/时长） | `codec::decoder_probe`（`codec.rs:910`）→ `FootageDescription::video_stream/audio_stream/duration/source_start_time`（`codec.rs:974-1020`） | 可行 |
| 5 | `media.import_footage` | 逐路径导入并报告失败项 | `Project::import_footage(filename)`（`node.rs:608`，单文件、返回 Option） | 可行（失败项由适配层按路径聚合 + `failed[]`） |
| 6 | `media.list_footage` | 素材枚举 | `folder_child_count/at`（`:360/366`）+ `NodeKind::Footage`（`:225`）+ `footage_*`（`:373-425`） | 可行 |
| 7 | `timeline.add_track` | 序列加轨（video/audio/subtitle） | `Project::add_track(sequence, kind)`（`node.rs:636`，kind `0=video,1=audio,2=subtitle`）；`timeline::add_track(track_list)`（`timeline.rs:195`） | 可行 |
| 8 | `timeline.place_clip` | 建块→设长/媒体入点→绑素材→落轨→回读 | `create_clip`（`:644`）、`block_set_length`（`:694`）、`block_set_media_in`（`:705`）、`connect(footage, clip, "tex_in", -1)`（`:732`）、`timeline::block_place`（`timeline.rs:154`）、`block_in` 回读（`:536`） | 可行（本地桥 `agent/sdk-bridge/src/main.rs:517-535` 用同序，未在本仓验证） |
| 9 | `timeline.split_clip` | 定点切块，后半 id 可查 | `timeline::block_split`（`timeline.rs:79`）、`split_preserving_links`（`:85`）、`track_split_at`（`:91`） | 可行（切后需按 `track_block_at` 重查后半，`node.rs:519`） |
| 10 | `timeline.trim_clip` | 按 side 裁 in/out（含邻接处理） | `timeline::block_trim(track, block, new_length, TrimMode)`（`timeline.rs:136`）+ `TrimMode{None,Move,TrimIn,TrimOut}`（`:128`）、`block_resize`（`:107`）、`block_resize_media_in`（`:112`） | 可行（`side`+`duration` → TrimMode 的映射与"是否外滑相邻块"需实测） |
| 11 | `timeline.move_clip` | 跨轨/同轨换位 | `timeline::block_move(timeline, track_index, block, in)`（`timeline.rs:174`） | 可行（schema 的 `track` id → `(track_list, index)`：`sequence_track_list_at`+`tracklist_track_at`） |
| 12 | `timeline.ripple_delete` | 波纹删除闭合空隙 | `Project::track_ripple_remove_block`（`node.rs:684`）；`timeline::ripple_remove_area(timeline, in, out)`（`timeline.rs:97`） | 可行 |
| 13 | `timeline.add_transition` | 建转场块 + 两侧 offset + **类型**（crossdissolve/fade/wipe/slide） | `create_transition`（`node.rs:656`）、`timeline::transition_set_offsets`（`timeline.rs:210`）、`connect(..., "out_block_in"/"in_block_in")`（`:732`）；类型=`type_in` combo，只能走 `set_standard_value`（`:710`）+ POD `kind=9`（`value.rs:57`） | **部分**：无专用"设转场类型"API；combo 写入路径待实测 |
| 14 | `timeline.add_marker` | 序列标记增删查 | **无任何标记 API**：SDK 全文 grep `marker` 只命中 `plugin.rs` 测试字符串；（`docs/zh/plans/engine-sdk-capability-gaps.md:70-71` 记 `MarkerAddCommand` 等无导出） | **缺口**（需引擎 C ABI 增补导出，或接受 `unsupported`） |
| 15 | `node.add_effect` | 按 type id 挂效果到链 | `effect::factory_entries`（`effect.rs:144`）、`Project::effect_chain_insert(host, index, type_id)`（`:196`，自带 undo）、`effect_chain/remove/move/set_enabled`（`:185-231`） | 可行 |
| 16 | `node.set_param` | 按 input id 写静态值 | `Project::set_standard_value(id, input_id, element, NodeValue)`（`node.rs:710`）+ `NodeValue` POD（`:279`，kind 语义见 `value.rs:35-60`） | 可行（键→input id、值→POD 全在适配层；**字符串型值不桥接**，`node.rs:276` 注释） |
| 17 | `node.list_types` | 类型表 + （可选）实例参数回读 min/max/choices | `factory_entries`（`effect.rs:144`，type_id/name/flags/category）、`dynamic_factory_entries`（`:162`，OFX）；实例参数只能 `standard_value`（`node.rs:564`）读已知 input id | **部分**：`get_params` 半边缺"输入枚举 + 参数元数据"API，无法给出 min/max/choices |
| 18 | `render.get_frame` | 任意时刻渲染单帧 | `render::render_frame_f32(project, viewer, time, size)`（`render.rs:45`，F32 RGBA）；需先 `manager_init`（`:111`） | 可行（线性→sRGB8 + PNG 在适配层，T1 已有 `render.rs` 工具） |
| 19 | `render.get_thumbnails` | 多条时间点批量取帧 | 同上循环 + `disk_cache_path/size/clear`（`render.rs:75/80/86`） | 可行（count ≤64 的限流在 T0 schema） |
| 20 | `render.get_audio_levels` | 时间线区间音频电平/静音检测 | **无时间线音频采样渲染**（`gaps.md:58` 记 `render_audio_samples` 无导出）；`audio::analyze_samples(&[&[f32]])`（`audio.rs:368`）只吃外部 planar 数据；`audio::output_levels`（`:197`）是播放设备电平 | **缺口**（需 C ABI 导出或 ffmpeg 解码 fallback，后者与 T2 同源） |
| 21 | `export.start` | 起导出任务 + 进度/取消 | `task::export(project, viewer, filename, &EncodingParams)`（`task.rs:159`）、`Task::state/progress/cancel/wait`（`:118/113/134/139`）、`codec::Format/Codec`（`codec.rs:310/248`） | **部分**：`preset`（OPP 开放集）无对应字段，需裁断映射或标 `unsupported` |
| 22 | `edit.undo` | 撤销（OPP §6：整栈、含用户操作） | `node::undo`（`:861`）、`undo_count`（`:885`）、`undo_jump(index)`（`:895`）、`undo_can_undo`（`:875`） | 可行（"整栈"= `undo_jump(0)` 语义待实测；`MockBackend` 已按协议实现） |

> 读表注意：`MockBackend` 对 22 个工具全部返回成功，它**不是**真实能力判据（`backend.rs:13-15` 明确"缺能力必须 `unsupported` 带 gap id，禁止报成功"）。上表只对 SDK 侧成立。

## 6. 实测记录（命令 / 退出码 / 关键输出）

| 命令 | 退出码 | 关键输出 |
|---|---|---|
| `python3 ai/schema/test_schema.py` | 0 | `TOTAL: pass=524 fail=0` / `RESULT: OK`；22 schemas、38 方法（mapped 23, deferred 15） |
| `python3 ai/prescan/test_prescan.py` | 0 | 11:57 复跑（含静态素材修复）：`Ran 32 tests in 71.128s` / `OK`；并发构建下曾测 140.8 s；`Both known cuts detected -> 2/2` |
| `cargo test`（`ai/oak-ai-tools`，默认 features，无 `sdk`） | 0 | 11:04 首跑与 **11:42 复跑**（`--offline`）一致：`52 passed`（lib）+ `3 passed`（`mock_e2e`）+ `2 passed`（`registry_drift`）+ `2 passed`（`schema_samples`）= **59 项**，全部 `0 failed`；`sdk_e2e` 记 0 项（feature 门控） |
| `cargo test --features sdk` | **进行中/未验证** | 11:12 起在 `/tmp/oak-ai-sdk-target` 构建（连带引擎 + FFmpeg/ocio）；至 11:41 仍编译依赖，无 `test result` 可引用。见 §2 第 3 条 |
| `git commit`（T0/T2 入仓） | 见 §10 | 仅 `ai/schema/`、`ai/prescan/`、本台账、`.gitignore` 四类路径 |
| `grep -rn 'before_commit\|render_done\|edit_failed' ai/` | 1（无匹配代码） | 仅 `error.rs:6` 注释 |
| `grep -n 'ai/\|oak-ai\|prescan' .github/workflows/ci.yml` | 1（无匹配） | CI 未纳入 AI 目录 |
| `git ls-files agent \| wc -l` | 0 | `agent/` 全部未入仓（`.git/info/exclude:9`） |

构建环境说明：`cargo test` 首次运行在 `ai/oak-ai-tools/target` 生成约 1.4 GB 产物（审计产生，属构建缓存，非源码改动，未入仓）；审计当次无并发 cargo（`ps` 无 cargo/rustc、`target/` 初始为空）；11:42 复跑时另有成员在用 `/tmp/oak-ai-sdk-target` 构建 sdk 特性（不同 target 目录，互不取锁）。

## 7. 未验证 / 存疑（明确标注）

1. **`--features sdk` 是否能编译/跑通**：截至 11:42 **未验证**（真机构建进行中，见 §2 第 3 条）。sdk 模块与门控测试已落地，但「能编译」与「E2E 通过」都还没有 `test result` 证据。
2. **SDK 侧 22 工具的真实行为**：§5 仍为源码阅读结论，**未经真实引擎实测**；`src/sdk/backend.rs` 的实现同样未经真机验证，尤其 place/trim/transition 的邻接与冲突语义、`render_frame_f32` 的像素值域。
3. **`agent/sdk-bridge`、`agent/mcp-oak` 是否能跑通**：本地件（git 排除），本次未运行；仅作为 API 用法参考引用。
4. **golden 与 ffmpeg 版本绑定程度**：未在第二台机器/第二版本验证。
5. **`.gitignore` 的改动（`__pycache__/`、`*.pyc`）** 是否已获 lead 认可：未验证（非本次审计写入）。
6. **T0 与规划文案差**：规划写"约 20 个工具"，实际 22 个；以 `ai/schema/` 与 `method-map.md` 为准。

## 8. 偏差与风险（需 lead 裁断）

1. **`ai/` 未入仓（部分消解）**：T0/T2 + 本台账 + `.gitignore` 已按首辅裁断随本轮提交入仓（见 §10）；**`ai/oak-ai-tools/`、`ai/oak-ai-mcp/`、`ai/agent/` 仍为工作树未跟踪状态**，待各自里程碑验收后单独提交。T6 CI 需等这些目录入仓后才能跑。
2. **目录布局偏差**：规划写 T3 落 `ai/agent/`，实际本地探索放仓根 `agent/`，且被 `.git/info/exclude` 排除（`/agent/`、`/AGENTS.md`、`/AGENTS.override.md`、`/.pi/`）。T3 开工前必须决定"入仓还是继续本地"，否则 T3 交付物同样进不了 PR。
3. **`AGENTS.md` 悬空符号链接**（→ `/home/fslong/AGENT.md` 不存在）：T3 三层上下文链当前不可用；仓内任何 agent 读到的上下文为空。
4. **T1 验收依赖重型构建**：真实后端要求本机/CI 具备引擎构建链（FFmpeg、ocio git fork、`oak_engine` cdylib）；T6 CI 需先决定 runner 形态。
5. **SDK 文档小瑕疵**（不影响使用，但会误导 T1 实现者）：`node.rs:280` 注释写 `NodeValueKind` "0 = none … 9 = string"，实际 `COMBO = 9`、`STRING = 10`（`crates/oak-node/src/value.rs:39-59`）。
6. **能力缺口的处置顺序**：markers / audio levels / effect params 三项若不先定 (a)/(b)/(c) 策略，T1 会卡在"22/22 完成"的定义上；按契约，带 gap id 的 `unsupported` 是合法交付，但需 lead 明确验收口径。

## 9. 首辅裁断（2026-11-05，收讫 task-1 后补记）

1. `ai/` 入仓：按里程碑小步提交；T1 验收通过后由工部 `git add ai/` 并提交（提交信息写明里程碑），T3/T5 完成后同样提交。审计方不动 git。
2. `AGENTS.md` 悬空：**不修**用户家目录全局链；T3 一律在 `ai/agent/` 内自建上下文链文件，不复用仓根符号链接。
3. 目录布局：以规划为准——T3 落 `ai/agent/`；仓根 `agent/` 只读参考，不进交付。
4. T0（524 断言）与 T2（27 测试）结论已采信。
5. SDK 缺口处置（§5 第 13/14/17/20/21 行）：**一律走 `unsupported` + gap id + README 记录**；本分支不改引擎、不扩 C ABI（规划 §6"不触碰 crates/"）。`timeline.add_marker`、`render.get_audio_levels`、`node.list_types` 的 `get_params` 半边按此处理，并列为上游 issue 候选。
6. `crates/oak-engine-rust-sdk/src/node.rs:280` 注释错（COMBO=9 / STRING=10）记入遗留问题，不改引擎，交后续上游沟通。
7. T3/T4/T5/T6 启动前置已写入任务板（task-4 依赖 task-2，task-5 依赖 task-3）。

> 影响：§2 剩余工作量第 5 项（三选一）已定案为 (a) 方案；第 6 项（transition `type_in`、`export.start` 的 `preset`）仍按"待实测"处理，若实测不可行同样转 `unsupported` + gap id。

## 10. 入仓提交

### 10.1 第一次提交：T0 工具面 + 本台账（2026-11-05 11:44）

- commit：`e94d3707b2da60d75891e25873a6d8ab1932110f`，标题 `add(ai): freeze the T0 tool schema face`（作者 `fslong`），`66 files changed, 2329 insertions(+)`。
- 范围：`ai/schema/`（22 tools + 40 samples + `method-map.md` + `test_schema.py` = 64 文件）、本台账、`.gitignore`（增补 `__pycache__/`、`*.pyc`）。
- 提交前复验：`python3 ai/schema/test_schema.py` exit 0、`pass=524 fail=0`；`ai/schema` 当日 0 次改动（无并发污染）。
- 未纳入：`ai/prescan/`（task-8 在办，见 10.2）、`ai/oak-ai-tools/`、`ai/oak-ai-mcp/`、`ai/agent/`、`docs/zh/plans/resolve21-oak-applicability.md`。
- `git show --stat HEAD` 尾部原文：

  ```
   ai/schema/tools/timeline.split_clip.json           |  49 +++
   ai/schema/tools/timeline.trim_clip.json            |  58 +++
   docs/zh/plans/ai-feature-status.md                 | 210 +++++++++++
   66 files changed, 2329 insertions(+)
  ```

### 10.2 第二次提交：T2 prescan 入仓 + task-8 修复（2026-11-05 11:57）

- commit：`b19afced7dd32fd156fcbaa2f0d11735a654f2fc`，标题 `fix(ai): land the T2 forensics prescan with the static-shot empty cut list fix`（作者 `fslong`），`5 files changed, 1342 insertions(+), 8 deletions(-)`。
- 范围：`ai/prescan/`（`prescan.py`、`test_prescan.py`、`README.md`、`golden/test_scenes.cutlist.json`）+ 本台账；不含 `ai/oak-ai-tools/`、`ai/oak-ai-mcp/`、`ai/agent/`、`__pycache__`。
- 提交前复验：`python3 ai/prescan/test_prescan.py` exit 0，`Ran 32 tests in 71.128s / OK`（11:56 跑，代码文件 mtime 11:42，即修复后冻结版本）；`prescan.py`/`test_prescan.py` 自 11:42 起无并发改动。
- `git show --stat HEAD` 原文：

  ```
   ai/prescan/README.md                       | 217 +++++++++++
   ai/prescan/golden/test_scenes.cutlist.json |  46 +++
   ai/prescan/prescan.py                      | 569 +++++++++++++++++++++++++++++
   ai/prescan/test_prescan.py                 | 483 ++++++++++++++++++++++++
   docs/zh/plans/ai-feature-status.md         |  35 +-
   5 files changed, 1342 insertions(+), 8 deletions(-)
  ```

### 10.3 第三次提交：T1 SDK 后端 + T4 执行层 hooks 入仓（2026-11-09 00:31）

- commit：`18c1bffda064a7b5fc918624269ba282cc655f6a`，标题 `feat(ai): land the T1 SDK backend and T4 execution-layer hooks`（作者 `fslong`），`34 files changed, 16324 insertions(+), 9 deletions(-)`。
- 范围：`ai/oak-ai-tools/**` 33 文件（`Cargo.toml`、`Cargo.lock`、`README.md`、`src/{dispatch,txn,backend,mock,render,error,hooks,lib,schema,timecode,util}.rs`、`src/tools/{common,edit,export,media,mod,node,project,render_tools,timeline}.rs`、`src/sdk/{mod,backend,gaps}.rs`、`tests/{hooks_policy,hooks_readback,hooks_replan,mock_e2e,registry_drift,schema_samples,sdk_e2e}.rs`）+ 本台账；不含 `target/`（根 `.gitignore:119` `**/target/`）、`ai/oak-ai-mcp/`、`ai/agent/`、`node_modules`。
- 验收依据（四项门全过）：
  - task-13（T4 抽查）`/tmp/oak-reviews/t4-hooks.md` **pass**：全量 `cargo test --offline` 90/0/0；ProposalOnly 零副作用主测有效（Spy 15/15）；分级表 fail-closed 反向实验恰好 2 项变红 + 运行时探针拒 `UNCLASSIFIED_TOOL`；README「Hooks」节与实现逐项吻合。
  - task-14（T1 独立验收）`/tmp/oak-reviews/t1.md` **pass**：默认 90/0、`--features sdk` 96/0（`sdk_e2e` 2/2 真机，libx264 实编码日志佐证）；`duration` 兜底消融 → 精确红在 `tests/sdk_e2e.rs:182`；22/22 方法覆盖由编译器背书。
  - 整改：D1（README 计数行口径）、O1（ripple 矩阵行只写首步）已改；D2（测试改名 `unknown_tool_fails_closed_without_touching_the_backend`，`tests/hooks_policy.rs:515`）已完成，改后 90/0/0。
- 状态改写：§0 表 T1「部分」、T4「未开始」→ **已完成（已入仓）**；§2、§4 标题加 2026-11-09 修订注记。
- `git show --stat HEAD` 尾部原文：

  ```
   ai/oak-ai-tools/tests/sdk_e2e.rs          |  525 +++
   docs/zh/plans/ai-feature-status.md        |   45 +-
   34 files changed, 16324 insertions(+), 9 deletions(-)
  ```

- 口径：本条哈希与 stat 回填发生在提交之后，属工作树改动，随下一批提交入库。提交说明正文把第 6 个 gap 写成了 `GAP_AUDIO_LEVELS`，源码常量实为 `GAP_RENDER_AUDIO_SAMPLES`，且 `gaps.rs` 另有 `GAP_CLIP_SOURCE_QUERY`（共 7 个常量）；代码未改，不 amend。

### 10.4 第四次提交：T5 MCP 薄封装入仓（2026-11-09 00:35）

- commit：`dfd42eafaf1450e5af5e61043dffa53daf01764b`，标题 `feat(ai): add the T5 MCP thin wrapper with zero schema drift`（作者 `fslong`），`12 files changed, 7679 insertions(+), 6 deletions(-)`。
- 范围：`ai/oak-ai-mcp/**` 11 文件（`Cargo.toml`、`Cargo.lock`、`.gitignore`、`README.md`、`src/{backend_select,lib,main,rpc,server}.rs`、`tests/{protocol,zero_drift}.rs`）+ 本台账；不含 `target/`（`ai/oak-ai-mcp/.gitignore:1` `/target/`）、`ai/agent/`、`docs/zh/plans/resolve21-oak-applicability.md`。
- 验收依据（两项门全过）：
  - task-12 独立验收 `/tmp/oak-reviews/mcp.md` **pass**：默认 features 干净构建 lib 20 + protocol 6 + zero_drift 3 = 29 全绿；协议链（initialize → tools/list → tools/call → 非法参数 → 未知工具）实测；删 `structuredContent` 消融 → 3 项红；`--backend sdk` 未构建时明确拒绝（exit 2，不静默降级）。
  - task-16 增量抽查 `/tmp/oak-reviews/t5-delta.md` **pass**：全量 **30/0/0**（回声断言加入后）；删版本回声 → 恰 `protocol_version_is_echoed_or_baselined` 红（protocol.rs:443）；O1 schema 目录缺失 → `tools/list` panic exit 101，README 段落逐字复现。
  - README 整改：F1（sdk 计数 6→双口径 6/7）与 F2（测试清单）已改；`/tmp/oak-mcp-sdk-verify2.log` 实测 sdk 配置 **7 passed exit 0**（含 `protocol_version_is_echoed_or_baselined` ok）。
- 状态改写：§0 表 T5「部分（待独立验收）」→ **已完成（已入仓，§10.4）**。
- `git show --stat HEAD` 尾部原文：

  ```
   ai/oak-ai-mcp/tests/protocol.rs           |  495 +++
   ai/oak-ai-mcp/tests/zero_drift.rs         |  180 ++
   docs/zh/plans/ai-feature-status.md        |   31 +-
   12 files changed, 7679 insertions(+), 6 deletions(-)
  ```

- 口径：本条哈希与 stat 回填发生在提交之后，属工作树改动，随下一批提交入库。

### 10.5 第五次提交：T3 agent 核 + 两档记忆入仓（2026-11-09 00:38）

- commit：`b8401b75bbb7d89511c4baf6292d2d6cb65d9ffe`，标题 `feat(ai): add the agent core with the three-layer context chain and memlite memory`（作者 `fslong`），`21 files changed, 3420 insertions(+), 7 deletions(-)`。
- 范围：`ai/agent/**` 20 文件（`.gitignore`、`.pi/mcp.json`、`README.md`、`package.json`、`package-lock.json`、`context/{10-repo,20-project,task.example}.md`、`scripts/{build-mcp,context,lib,mcp-serve,oak-mem,smoke,start}.sh`、`tests/{stub-model-server.mjs,test.sh}`、`tests/fixtures/{memory-recall,memory-write,smoke}.json`）+ 本台账；**不含**被 `ai/agent/.gitignore` 挡下的生成物 `AGENTS.md`、`context/30-task.md`、`node_modules/`、`.agentdir/`，不含 `runs/` 产物，不含 `docs/zh/plans/resolve21-oak-applicability.md`。
- 验收依据：task-15 独立验收 `/tmp/oak-reviews/t3-agent.md` **pass**（八项必查全绿）：smoke exit 0（51 events / 3 tool_calls / live 22 / fails=[]）；`tests/test.sh` 9 通过 0 失败；隔离三进程记忆复现（写 `4de460d66a43` → 新进程同 id 召回 → layer 4 注入）；密钥 grep 零命中；`~/.memlite/config.json` sha 六时点不变；消融摘记忆层 → 恰 D 红 8/1。
- 整改：F1（`context/20-project.md` 枚举 22 工具改为「举例，不保证穷尽」）、F2（`README.md:142` 工具表字符数改实测 39634/42726）已完成。
- 状态改写：§0 表 T3「未开始」→ **已完成（已入仓，§10.5）**。
- `git show --stat HEAD` 尾部原文：

  ```
   ai/agent/tests/stub-model-server.mjs     |  254 ++++
   ai/agent/tests/test.sh                   |  206 ++++
   docs/zh/plans/ai-feature-status.md       |   32 +-
   21 files changed, 3420 insertions(+), 7 deletions(-)
  ```

- 口径：本条哈希与 stat 回填发生在提交之后，属工作树改动，随下一批提交入库。

### 10.6 第六次提交：T6 无头 CI 入仓（2026-11-09 02:0x）

- commit：`<提交后回填>`，标题 `feat(ai): add the headless CI with mock-LLM replay and golden-frame invariants`（作者 `fslong`）。
- 范围（四块）：`ai/ci/**` 15 文件（`.gitignore`、`.pi/mcp.json`、`Cargo.toml`、`Cargo.lock`、`README.md`、`package.json`、`run.sh`、`fixtures/{agent-replay,bad_black,edit_chain}.ndjson`、`fixtures/task.example.md`、`scripts/{agent-replay,lint-workflows}.sh`、`src/lib.rs`、`tests/replay.rs`）+ `.github/workflows/ai-headless.yml` + `ai/agent/scripts/context.sh`（`OAK_AI_CWD` 增量 +11 行）+ 本台账；**不含** `ai/ci/AGENTS.md`、`ai/ci/target/`、`node_modules/`（`ai/ci/.gitignore` 挡下）、不含 `crates/**`/`examples/**` 格式化改动（首辅调查中的外部事件）。
- 验收依据：task-17 独立验收 `/tmp/oak-reviews/t6-ci.md` **pass / T6 可入仓**：`bash ai/ci/run.sh` 两次独立复跑七段全 PASS exit 0（schema 524/0、prescan 32 OK、oak-ai-tools 90/0、oak-ai-mcp 30/0、ci-replay 4/0/1ignored、agent-replay `tool_call_count=6, tool_errors=0`、workflow lint OK）；回放 session log 两次 diff=0、帧 sha256 前缀吻合；消融 `OAK_CI_BAD_FIXTURE=1` EXIT=101（坏链路守卫 `replay.rs:444`）+ 正向对照 0；`context.sh` 跨范围增量 9/9 不回归、缺省 md5 等价；密钥与本机绝对路径入库零命中。
- README 整改（T6 线落盘，翰林三查复核通过）：D1 对账段（`:24-29` 三段合计 124 passed / 0 failed / 1 ignored，与 run1 九行求和 90+30+4 一致）；D2 措辞改「坏链路必须红」并注明 `tests/replay.rs:444` 守卫；删除 `5f5c1487`（记忆层 ID，非 git 对象，不得入仓）。
- 状态改写：§0 表 T6「未开始」→ **已完成（已入仓，§10.6）**；§4 注记改为 T0–T6 全部入仓。
- `git show --stat HEAD`：**提交后回填**（本条为最后一个里程碑提交，回填后不再有后续里程碑提交，留作工作树注记）。

### 10.7 口径注记

- §10.2–§10.6 的哈希/统计回填发生在提交之后，属工作树改动；T6 之后无里程碑提交承接，回填行保留在工作树中由用户决定是否提交。
- `ai/ci/README.md` 的 `fmt` 门禁未加（实测 130/17/0）为已知限制，见该 README「已知限制」节。

---

证据口径：本文所有 `文件:行号` 均对应审计时工作树内容（HEAD `bf64bee6c` + 未跟踪 `ai/`）；命令输出为 2026-11-05 11:03–11:25 CST 实跑原文摘录。
