# AI 剪辑开发规划（ai-feature 分支）

> 状态：待实施
> 基线核验：`upstream/main@11011eb3d`（2026-10-28）
> 本文是 #75（docs/ai-editing-integration）与 #77 讨论结论的**执行规划**：
> 与维护者已对齐的次序为 **tools → hooks → MCP**，外加 agent 核与
> memlite 记忆层；不等待 OPP 宿主，先用 Rust SDK 把闭环跑通，
> 工具 schema 冻结后 MCP 只作薄封装。

## 1. 对齐结论（为什么是这个次序）

#77 维护者回复确认两点：**SDK 接口长期稳定、可作为 AI 剪辑入口**；
功能性插件将拆为独立进程经 stdio 通信，AI 剪辑可采用 MCP 格式。
我方回应并已发布的次序：

1. **tools 先行**——模型真正消费的是工具 schema（OPP §8 方法名、参数
   类型、事务包裹、时码纪律）。schema 是契约，先冻结；先做 MCP 等于让
   传输层反向决定契约。in-process SDK 直连是 M1 底座，取帧回读的高频
   小调用不经 stdio 往返。
2. **hooks 补反方向**——MCP/工具是模型→app 的请求响应；AI 闭环还需要
   app→agent 的事件与拦截点（确认门、回读断言、失败回喂）。见 §4。
3. **MCP 后置薄封装**——schema 冻结且被闭环实测后，同一 schema 包一层
   MCP server（stdio），供 Claude Desktop 等外部客户端直连；引擎对 AI、
   对 MCP 保持无感知（继承 ai-agent-design.md §8）。

本分支纪律：以 `upstream/main` 为唯一基线并保持跟随（`git merge
upstream/main`，小步勤合），不改引擎进程，不绕过 SDK 公共面。

## 2. 总体架构

```
┌─ agent 核（仓内 agent/，CLI agent）────────────────────────────┐
│  AGENTS.md 三层上下文链 + 工具 schema 注入 + memlite 记忆召回/写入 │
└──────────────┬─────────────────────────────────────────────────┘
               │ 工具调用（先直调执行层，后 MCP，同一 schema）
┌─ 工具执行层（ai/oak-ai-tools，Rust）──────────────────────────┐
│  schema 校验（有理秒/帧号）→ 事务编排（一次动作一个 undo group） │
│  → oak-engine-rust-sdk（in-process）→ commit 后取帧回读断言     │
│  适配层 hooks：before_commit / render_done / edit_failed        │
└──────────────┬─────────────────────────────────────────────────┘
┌─ 取证层（ai/prescan，独立于引擎进程）─────────────────────────┐
│  直方图差 + pHash + 双阈值迟滞 → JSON 切点表（零 LLM 调用）      │
└─────────────────────────────────────────────────────────────────┘
记忆层：memlite 两档库（工程级仓外 data-dir / 用户级兜底），
        显式 --data-dir，敏感闸拒密钥正文——引擎、协议、.ove 全无感知。
```

铁律（继承 ai-agent-design.md §1.2 与 #75 文档）：

1. agent 只经冻结工具面访问引擎，LLM 永不接触事务令牌；
2. 一切编辑包在事务里，一次"动作"一个 undo group，失败即 abort 回喂；
3. 推理不进任何 Oak 进程；引擎对 agent、记忆、MCP 均无感知；
4. API key 与记忆数据都不写入 `.ove`、不入本仓。

## 3. 里程碑

| 里程碑 | 内容 | 依赖 | 验收 |
|---|---|---|---|
| **T0 schema 冻结** | 约 20 个工具的 JSON Schema 落 `ai/schema/`：方法名对齐 OPP §8、时码有理秒/帧号（禁裸浮点）、效果参数 min/max/choices 回填；附"工具 ↔ OPP 方法"对照表 | 无 | schema 单测（合法/非法样本）全绿；对照表测试防漂移 |
| **T1 工具执行层** | `ai/oak-ai-tools`：SDK 直连实现全表工具 + 事务编排（begin/commit/abort 自动包裹）+ 取帧→PNG 通路 + 回读断言（黄金帧 harness） | T0 | 脚本 E2E：导入→铺轨→split/trim/ripple_delete→render_frame→像素断言→整段 undo 后状态完全还原 |
| **T2 取证预筛** | `ai/prescan`：帧差+pHash 双通道、迟滞峰检、JSON 切点表；对素材文件独立运行，不经 SDK 亦可（FFmpeg fallback） | 无（可与 T1 并行） | 已知切点素材检出率达标；峰值表 golden 比对 |
| **T3 agent 核 + 记忆** | `ai/agent/`：pi CLI + AGENTS.md 三层上下文链；memlite 两档库接入（工程级仓外 data-dir + 用户级兜底）；意图→工具→验证→记忆写入全链 | T1 | 无头 `pi --print` 冒烟过；跨会话记忆召回命中；密钥不落盘（进程参数传入） |
| **T4 适配层 hooks** | before_commit 确认门（proposal-only / confirm / auto 三档按操作类分级）、render_done 回读断言触发、edit_failed 结构化回喂；app 侧事件（footage_imported 等）在 SDK 直连阶段用工具返回值近似，精确事件留待 OPP 事件面演进 | T1（可与 T3 并行） | 三档确认门行为测试；失败回喂→重规划闭环测试 |
| **T5 MCP 封装** | `ai/oak-ai-mcp`：同一 schema 的 MCP server（stdio），实现转发 T1 执行层；工具名/参数零漂移 | T0 冻结 + T1 实测 | 任意 MCP 客户端连通；直调与 MCP 双形态一致性测试 |
| **T6 无头 CI** | Mock LLM 录制/回放 tool_call 序列（无 key 无网络）+ 黄金帧 + 会话回放日志（tool_call+帧哈希落盘） | T3、T4 | CI 全绿；回放日志可复现会话 |

与上游计划的映射：T1/T3 是 ai-agent-design.md A2/A3 的前置可交付物；
T5 对应 #77 回帖承诺的"MCP 薄封装"；A4 面板与 OPP P4 事件面等宿主
能力就绪后再并入，不在本分支先行实现面板。

## 4. hooks 设计（两类落点）

**适配层内 hook（本分支实现，零上游依赖）**——执行层在工具调用链上的
拦截点，不是协议事件：

- `before_commit`：事务提交前触发，按操作类分级——读类 auto、编辑类
  默认 confirm、工程/导出类 confirm；策略可配（proposal-only 模式下执行
  层只产提案清单不落编辑）。这是 #77 问题 5（自动提交 vs 仅提案）的落点。
- `render_done`：取帧返回后触发回读断言管线（像素检查 + LLM 判定）。
- `edit_failed`：任一工具失败 → abort → 错误结构化回喂 LLM 重规划。

**引擎/app 侧事件 hook（不先行，等 OPP 事件面）**——`footage_imported`
触发 prescan、`render.done` 等属 OPP §4.9 事件表；同主版本内只增不改
（§4.12 演进规则），T1 阶段用工具返回值（import_footage 结果）触发
prescan 即可，不为此提前改引擎。

## 5. memlite 记忆层

- **两档库**：工程级库放**仓外**独立数据目录（显式 `--data-dir`，
  绝不放本仓内，防泄漏进公开树）；检索未命中回退用户级兜底库。
- **写入纪律**：剪辑经验（"这类素材常见切点节奏"、"上次粗剪被否原因"）
  入工程级库；敏感闸拒密钥/token/口令正文，只存位置指针；`sync` 走
  git 白名单数据文件。
- **召回纪律**：agent 经 CLI 在上下文内召回（inject 注入 AGENTS.md
  链）；引擎、协议层、`.ove` 对记忆零感知。
- **基线**：memlite CLI 在本机可用（init/remember/recall/show/update/
  forget/sync/inject，语义检索可配本地 embed-server，BM25 兜底）。

## 6. 目录布局（全部新增，不触碰 crates/）

```
ai/
  schema/            # T0：工具 JSON Schema + 对照表 + 样本
  oak-ai-tools/      # T1/T4：执行层 crate（独立 workspace，path 依赖
                     #   crates/oak-engine-rust-sdk，不入上游 workspace）
  oak-ai-mcp/        # T5：MCP server 薄封装
  prescan/           # T2：取证预筛（独立运行，只吃素材文件）
  agent/             # T3：pi CLI + AGENTS.md 链（node_modules 不入库）
```

`ai/` 不入上游 `crates/` workspace、不改上游构建——上游 diff 面最小化，
facade 迁移（app-pure-sdk-and-engine-facade.md P0–P8）演进 SDK 公共面时
本分支只跟版本、不跟内部。

## 7. 风险与边界

| 风险 | 控制 |
|---|---|
| upstream/main 快进（规划当日即领先 fork main 3 个提交） | 开工前 `merge upstream/main`；里程碑小步提交，每步可独立合入 |
| SDK 直连（in-process）与将来 OPP 进程边界形态不同 | T0 schema 冻结保证换适配层不换语义（M2 形态，#77 已述） |
| 取帧回读的 LLM 判定成本 | 稀疏复核（只看峰值帧）；像素断言为主、LLM 判定为辅 |
| 记忆库误入仓 | 数据目录永远显式、永远仓外；`.gitignore` 双保险；CI 扫描 |
| 时码漂移 | schema 校验禁裸浮点；有理秒/帧号统一走 T0 公共类型 |

明确不做：AI 面板（等 OPP P4）；ASR 台词级剪辑（另立文档）；
引擎内任何推理；沙箱与插件商店（上游范围）。

## 8. 与 #77 的衔接

T0 动工即回帖 #77 报告 schema 草案（英文）；T1 E2E 通过后报告闭环
录像/日志；本规划与 #75 文档（docs/ai-editing-integration 分支）保持
一致，若上游要求拆分或改名，以维护者意见为准。
