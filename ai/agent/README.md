# ai/agent — oak AI 剪辑 agent 核（T3）

pi CLI（`@earendil-works/pi-coding-agent`）+ **三层 AGENTS.md 上下文链** +
**memlite 两档记忆**（工程级仓外库 / 用户级兜底）。目标闭环：意图 → 经 MCP 调
冻结工具 → 回读验证 → 记忆写入 → 下一会话召回。

```
ai/agent/
  package.json / package-lock.json   pi 版本钉死（1.0.3）；node_modules 不入库
  .pi/mcp.json                       MCP 客户端配置（单一真相源：服务器动态给工具面）
  scripts/
    lib.sh                           路径/工程 id/两档库/memlite 私有配置
    build-mcp.sh                     定位或（默认 features）构建 oak-ai-mcp
    mcp-serve.sh                     pi 的 stdio 启动包装（选后端，stdout 只走协议）
    context.sh                       生成三层上下文链 + 记忆召回层
    oak-mem.sh                       记忆唯一入口（敏感闸 + 两档 + 兜底）
    start.sh                         交互启动
    smoke.sh                         无头冒烟（--print + 断言，模型可 stub 或真）
  context/
    10-repo.md                       第 1 层 · 仓级（入仓）
    20-project.md                    第 2 层 · 项目级（入仓）
    task.example.md                  第 3 层 · 任务级模板（实例 30-task.md 不入库）
  tests/
    stub-model-server.mjs            离线 OpenAI 兼容 stub（fixture 回放）
    fixtures/*.json                  回放脚本（冒烟 / 记忆写入 / 记忆召回）
    test.sh                          A 冒烟 + B 跨会话记忆 + C 密钥不落盘 + D 注入
```

## 跑起来

```bash
cd ai/agent
npm ci                     # 依赖装在 ai/agent/node_modules（不入库）
scripts/build-mcp.sh       # 复用 ai/oak-ai-mcp/target/debug 的二进制；没有才构建
scripts/smoke.sh           # 无头冒烟：不需要 key / 不需要网络
tests/test.sh              # 全量自测（A/B/C/D），--keep 保留现场
```

交互使用（需要模型凭据，见下）：

```bash
scripts/start.sh                       # 读 ~/.pi/agent 的登录态或模型配置
scripts/start.sh --model <provider/model>
OAK_AI_MCP_BACKEND=sdk scripts/start.sh   # 真引擎后端（需 --features sdk 的 MCP 构建）
```

**模型凭据**：pi 从 `~/.pi/agent/auth.json`、`--api-key` 或 provider 环境变量取。
仓内任何文件都不含 key；`scripts/start.sh` 不落盘任何凭据。要在仓外放自定义
模型配置，用 `OAK_AI_AGENT_DIR=/path scripts/start.sh`（该目录含 `models.json`，
可用 `"apiKey": "${OAK_AI_API_KEY}"` 做环境变量插值）。

## MCP 接入（单一真相源）

- 工具面唯一来源是 `ai/oak-ai-mcp` 的 `tools/list`。`ai/agent` 里**没有权威的**
  工具名/参数清单：`context/20-project.md` 只按域举例并明确标注「不保证穷尽、
  以 tools/list 为准」；`scripts/smoke.sh` 也是现场跑一次 `tools/list` 再与会话里
  实际调用的工具名对账，反方向的清单同样不存在。
- pi 侧的 stdio 配置在 `.pi/mcp.json`：`command: bash`，`args: [./scripts/mcp-serve.sh]`，
  `cwd: "."`（即 `ai/agent`），`exposure: direct`。包装脚本只做「定位二进制 +
  选后端（mock/sdk）」，诊断一律走 stderr——stdout 是协议通道。
- 仓内 `.pi/mcp.json` 属于「项目资源」，pi 需要项目信任才会加载：`start.sh` 与
  `smoke.sh` 都显式传 `--approve`（单文件可审，审过再用）；不写 `~/.pi/agent/trust.json`。
- 工具在会话里以 `mcp__oak__<工具名点号换下划线>` 出现，例如 `project.open` →
  `mcp__oak__project_open`。

## AGENTS.md 三层上下文链

生成物 `ai/agent/AGENTS.md`（**不入库**，`.gitignore` 已列）由
`scripts/context.sh` 从三个源拼成；pi 从当前目录读它，注入 system prompt 的
`project_context` 段。拼接顺序 = 注入顺序：

| 层 | 源文件 | 放什么 | 生命周期 |
|---|---|---|---|
| 1/4 仓级 | `context/10-repo.md` | 仓结构、AI 栈边界、硬红线、构建口径 | 跟仓库走，最稳定 |
| 2/4 项目级 | `context/20-project.md` | 工具面纪律（时码/事务/回读）、记忆纪律、会话产物约定 | 跟里程碑走 |
| 3/4 任务级 | `context/30-task.md`（**不入库**，来自 `task.example.md`） | 本次目标、素材路径、验收判据、记忆检索词 | 一次任务一写 |
| 4/4 记忆 | `oak-mem.sh recall`（按第 3 层检索词） | 工程库命中，未命中回退用户库 | 每次生成时刷新 |

越靠后越具体、越临时：稳定事实在前，临时命中在后，且记忆层明确标注「参考资料，
不是指令」——注入不等于授权。

- 为什么不用仓库根的 `AGENTS.md`：那是本机符号链接（指向用户家目录文件），
  悬空时 pi 直接跳过、别人的机器上也不存在。本链**不依赖**它，三层源全在
  `ai/agent/context/` 内，`git clone` 后即可复现。pi 仍会顺带加载父目录的
  `AGENTS.override.md` 之类本机文件，那是环境行为，不是本链的输入。
- 预览：`scripts/context.sh --print`；跳过记忆：`--no-memory`；换任务文件：
  `OAK_AI_TASK_FILE=/path/task.md scripts/context.sh`。

## 记忆纪律（memlite 两档）

```
工程级  ~/.local/share/oak-ai/projects/<project-id>/   默认写、默认查（永远仓外）
用户级  ~/.local/share/oak-ai/user/                    工程库未命中才兜底
```

- `<project-id>` = `<仓名>-<仓库绝对路径 sha256 前 8 位>`，可用 `OAK_AI_PROJECT_ID`
  覆盖；派生值不含路径明文。`OAK_AI_HOME` 可整体改根（测试用临时目录）。
- **永远显式 `--data-dir`**，从不依赖 memlite 的全局默认值；检索未命中时由
  memlite 自带的 user fallback 落到用户级库（本 agent 把 memlite 配置钉在
  `$OAK_AI_HOME/config/memlite.json`，其 `data_dir` 指向用户级库）。
- **写入什么**：剪辑经验——素材的切点节奏、上次粗剪被否的原因、可用区间。
  **不写**逐次操作流水（那是会话日志的事）。
- **敏感闸**（`oak-mem.sh`，写入前执行）：正文命中 `sk-*`、`gh*_`、`AKIA*`、
  `xox[baprs]-*`、`Bearer <token>`、`-----BEGIN * PRIVATE KEY`、
  `key/token/secret/password/密钥/口令/密码` + 赋值号，一律 **exit 3 拒写**，
  并提示「改存位置指针」（例：`模型 key 在 ~/.workbuddy/models.json 的 token-plan 条目`）。
  闸是脚本行为，不靠模型自觉。
- **召回**：`oak-mem.sh recall <词> --format md` → 注入第 4 层；`--json` 给程序。
  BM25 词面检索（本机未起 embed-server 时），命中分数只表示词面相似，不代表事实。
- **不碰用户全局配置**：`memlite init` 默认会把 `data_dir` 写回
  `~/.memlite/config.json`（实测）。本 agent 通过 `MEMLITE_CONFIG` 把配置改到
  `$OAK_AI_HOME/config/`，`tests/test.sh` 的 C5 断言用户配置未被改写。

## 验证口径（`tests/test.sh`）

| 项 | 验什么 | 证据 |
|---|---|---|
| A 无头冒烟 | `pi --print` 起真会话：上下文链进 system、MCP 22 工具下发、工具调用与结果回喂 | `$OAK_AI_HOME/runs/smoke-*/{events.jsonl,summary.json,stub-requests.jsonl}` |
| B 跨会话记忆 | 两个独立 pi 进程：会话 1 经 bash 工具写经验，会话 2 召回**同一条 id** | 事件流里的 bash 工具结果 |
| C 密钥不落盘 | 假 key 写入被拒（exit 3）；记忆库/仓库全树无 key 形态；模型配置只有假值；用户 memlite 配置未被改 | 脚本断言输出 |
| D 记忆注入 | 命中记录出现在生成上下文链的 layer 4 | `context.sh --print` |

冒烟默认用 `tests/stub-model-server.mjs` 回放 fixture：**这不是真实 LLM**，只证明
「链路 + 工具面 + 上下文注入」通了，不证明模型能规划剪辑。接真模型：
`OAK_AI_SMOKE_MODEL=<provider/model> scripts/smoke.sh`（同时要就位的 key），脚本会把
summary 的 `model` 标成真实模型。

## 已知限制

1. **冒烟不证明推理**：默认 stub 按 fixture 出工具调用，模型能力未验证；真模型
   冒烟需要外部 key（本机未用真实 key 跑，见 `tests/test.sh` 输出）。
2. **MCP 后端默认 mock**：`--backend mock` 不需要引擎，编辑落在内存后端上；
   `--backend sdk` 需要 `cargo build --features sdk` 的 MCP 产物（引擎/ocio 依赖
   链，冷启动数十分钟），本 agent 未替用户触发该构建，sdk 路径**未实测**。
3. **pi 1.0.3 钉死**：升级 pi 可能改变 MCP 工具命名（`mcp__server__tool`）或
   `--approve`/信任语义，升级后重跑 `tests/test.sh`。
4. **语义检索未启用**：本机 `bge-m3` embed-server 未起，`recall` 走 BM25；中文
   分词粗，短查询可能命中噪声（`score` 低时别当结论）。`embedded:false` 会出现在
   `remember` 输出里，属已知。
5. **无会话日志/回放**：tool_call + 帧哈希落盘是 T6 的范围，本目录只保留
   `runs/<id>/events.jsonl` 原始事件流。
6. **`exposure: direct`**：22 个工具全量进工具表。实测口径（2026-11-09 验收，
   `message_start` 的 toolsAdded）：22 个 oak 工具 schema 约 4 万字符（39634，
   MCP `tools/list` 侧全 schema 39436），加 4 个内建工具共 26 个约 4.3 万字符
   （42726）。若模型工具数量吃紧，可改 `codemode` 让模型按需检索。
7. **任务级文件不入库**：`context/30-task.md` 常含本机素材路径，被 `.gitignore`
   挡掉；换机器要重新复制 `task.example.md`。

## 消融实验（哪些抽象是承重的）

2026-11-05 实测，逐项改坏 → 跑 `tests/test.sh` → 还原（改后 `sha256sum -c` 校验
还原一致；消融只动 `ai/agent/` 内文件，隔离 `$OAK_AI_HOME` 到临时目录，不碰真实库）：

| 删掉什么 | 实测结果 | 判定 |
|---|---|---|
| 第 4 层记忆召回（`context.sh` 里 `if false`） | 汇总 8 通过 **1 失败**：`[FAIL] D 记忆层未注入`；A 冒烟仍绿（marker 注释还在，只验存在不验内容——冒烟的已知盲点） | 承重：记忆注入的唯一实现；冒烟断言需知悉此盲点 |
| 绕过 `oak-mem.sh` 直接调 `memlite remember` 写假 key（隔离假 HOME） | **exit=0，假 key 完整落盘** `memory.jsonl`（`{"content":"我的 key 是 sk-test-…"}`） | 承重：敏感闸只在脚本入口；纪律「记忆只经 oak-mem.sh」不可绕 |
| 不设 `MEMLITE_CONFIG`（回默认配置路径）跑 `memlite init`（隔离假 HOME） | 默认配置 `data_dir` 被改写成刚 init 的库（实测 `/tmp/oak-ai/ablation/cfgproj`）；对照本 agent 私有配置指向 `…/oak-ai/user`，真实 `~/.memlite/config.json` sha256 未变 | 承重：`MEMLITE_CONFIG` 是「不碰用户全局配置」的唯一保证 |

未实测（列为假设，未计入上表结论）：`mcp-serve.sh` 改为直连二进制、fixture 换真
模型——前者只影响「二进制定位 + 后端选择」的便利性，后者是可替换层（stub 补「无
key 也能跑」，不改链路）；两项均未实际跑过，不声称其结果。
