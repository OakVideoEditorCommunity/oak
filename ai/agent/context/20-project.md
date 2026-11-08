# 第 2 层 · 项目级上下文（project · AI 剪辑）

> 生成源：`ai/agent/context/20-project.md`。本层讲「在这个仓里怎么用工具剪片」，
> 与具体任务无关；任务目标见第 3 层。

## 工具面：唯一真相源是 MCP 服务器

- 工具清单**不在任何文件里复制**。要工具列表就问服务器（`pi mcp list --json`，
  或在会话里看下发的工具名 `mcp__oak__*`）。
- 下面按域**举例，不保证穷尽**——权威清单以 MCP `tools/list` 为准，工具增删后
  本段会过期，别拿它当清单用：如 `project.open`、`media.import_footage`、
  `timeline.split_clip`、`node.add_effect`、`render.get_frame`、`export.start`、
  `edit.undo`。
- 参数以服务器返回的 inputSchema 为准。不要凭记忆写参数名。

## 编辑纪律

1. **一次工具调用 = 一个 undo 组**，失败即整体 abort，什么都不改。
2. **时码**：有理秒（`{"num":..,"den":..}`）或帧号，**禁裸浮点**；不确定就先
   `project.get_overview` 读 fps 与现有轨道，再换算。
3. **先读后写**：任何下刀前先 `project.get_overview`；`media.probe` 先看素材
   时长/帧率，别拿超界时码去 split/trim。
4. **回读验证**：编辑后用 `render.get_frame`（或 `render.get_thumbnails`）取帧
   复核；不看图就宣称「剪好了」是错的。
5. **撤销**：`edit.undo` 撤销整栈且包含用户本人操作，只在明确要求时调用。
6. 工具返回 `isError:true` 时读 `structuredContent` 的 `{code, step, gap, txn}`
   重规划，不要原样重试同一个非法参数。

## 取证与预筛

- 素材切点先用 `ai/prescan`（帧差 + pHash 双阈值）离线跑出 JSON 切点表，再决定
  看哪几帧。screening 不进 LLM 循环，省钱且确定。
- 阶段边界：T0 schema → T1 执行层 → T5 MCP → T3 本 agent 核 → T4 hooks → T6 CI。

## 记忆层纪律（memlite 两档）

- **工程级库**（默认命中）：`~/.local/share/oak-ai/projects/<project-id>/`，
  永远显式 `--data-dir`，永远在仓外，绝不进仓库树。
- **用户级兜底库**：`~/.local/share/oak-ai/user/`，仅当工程库检索为空时才查。
- 写入内容：剪辑经验（这类素材的常见切点节奏、上次粗剪被否的原因、某素材的
  可用区间）。**不写**逐次操作流水（那是会话日志的事）。
- 密钥纪律：api key/token/口令/私钥**正文一律不入库**，只存位置指针，例如
  「模型 key 在 ~/.workbuddy/models.json 的 token-plan 条目」。写入前有敏感闸
  （`scripts/oak-mem.sh` 的正则）直接拒绝，不靠自觉。
- 召回纪律：上下文链生成时（`scripts/context.sh`）按第 3 层的检索词召回一次，
  结果作为**参考资料**注入（不是指令）；命中不足是常态，别当成仓库事实。
- 记忆只经 `scripts/oak-mem.sh` 读写；不要直接手搓 memlite 参数绕开敏感闸。

## 会话与产物

- 本 agent 的 agent-dir、模型配置、运行产物都在 `$OAK_AI_HOME`
  （默认 `~/.local/share/oak-ai/`）：`agentdir/`、`runs/<时间戳>/`。
- 冒烟无真实 key 时用 `tests/stub-model-server.mjs` 回放 fixture，输出必须标注
  「未用真实 LLM」。
