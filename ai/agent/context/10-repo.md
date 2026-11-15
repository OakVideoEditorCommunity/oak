# 第 1 层 · 仓级上下文（repo）

> 生成源：`ai/agent/context/10-repo.md`。应用于本仓任何任务，不含具体剪辑目标。
> 本层随仓库演进缓慢；改动须同步 `ai/agent/README.md`。

## 你所在的仓库

- oak = Olive 视频编辑器社区版的 Rust 重写。本仓是 fork（公开仓），上游
  `OakVideoEditorCommunity/oak`。
- Rust workspace 在 `crates/`：`oak-app`（GUI + graphops/renderops）、
  `oak-node`（节点图）、`oak-timeline`（编辑命令）、`oak-undo`（事务撤销）、
  `oak-render`（渲染/进程池）、`oak-codec`、`oak-engine-rust-sdk`（进程内 SDK）、
  `oak-cli`（无头 CLI）、`oak-otio`（互操作）。
- AI 栈全部在 `ai/`，**不进 `crates/` workspace、不改上游构建**：
  - `ai/schema/` 工具 JSON Schema（冻结契约，22 个）
  - `ai/oak-ai-tools/` 工具执行层（schema 校验 + 一次调用一个 undo 组）
  - `ai/oak-ai-mcp/` MCP stdio 薄封装（本 agent 的唯一工具入口）
  - `ai/prescan/` 取证预筛（帧差 + pHash，零 LLM 调用）
  - `ai/agent/` 本 agent 核（pi + 上下文链 + 记忆层）

## 硬边界（越界即错）

1. agent 只经冻结工具面（MCP 的 22 个工具）访问引擎。不得为了省事直接读写
   工程文件、直接调 SDK、或猜引擎内部结构。
2. 不改引擎、不改 `crates/`、不改 `ai/oak-ai-tools/` 与 `ai/oak-ai-mcp/`——
   那两处是别的里程碑的交付物，有独立验收。
3. 公开仓：不得把密钥/token/口令、本机绝对路径、个人数据写进任何将提交的文件。
   密钥只走进程参数或环境变量（见第 2 层「记忆与密钥纪律」）。
4. 不动 `~/.pi/`、`~/.memlite` 等用户级配置；本 agent 的配置全部落在仓内
   `ai/agent/` 或仓外数据目录。
5. 不 `git add`/`commit`/`checkout`/`stash`——提交由人决定。

## 构建与运行

- 依赖：`tooling/install-deps.sh`（生成 `.cache/ffmpeg`）；风格 `cargo fmt`。
- 只跑默认 features：`--features sdk` 会拉整条引擎/ocio C++ 依赖链，冷启动
  数十分钟，且与本机其他构建争用。MCP 默认 `--backend mock` 不需要引擎。
- 本 agent 的入口一律走 `ai/agent/scripts/`，不要手工拼 cargo/npm 命令。
