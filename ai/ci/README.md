# T6 无头 CI（oak-ai）

一条命令跑完 `ai/` 全链的无引擎、无 key、无网络测试：

```bash
ai/ci/run.sh
```

覆盖七段（每段独立失败即红，日志落 `$OAK_CI_LOG_DIR`（缺省仓外
`~/.local/share/oak-ai/runs/ci-<时间戳>/`），最后给总计）：

| 段 | 内容 | 命令 |
|---|---|---|
| 1 | T0 schema 测试 | `python3 ai/schema/test_schema.py` |
| 2 | T2 prescan 测试（要 ffmpeg） | `python3 ai/prescan/test_prescan.py` |
| 3 | T1 执行层测试（默认 features，无引擎） | `cargo test --offline --locked --manifest-path ai/oak-ai-tools/Cargo.toml` |
| 4 | T5 MCP 协议测试（无引擎） | `cargo test --offline --locked --manifest-path ai/oak-ai-mcp/Cargo.toml` |
| 5 | T6 回放库 + 黄金帧 + 零漂移 | `cargo test --offline --locked --manifest-path ai/ci/Cargo.toml` |
| 6 | 真实 pi 进程 agent 回放（stub 模型） | `bash ai/ci/scripts/agent-replay.sh` |
| 7 | workflow 静态检查 | `bash ai/ci/scripts/lint-workflows.sh` |

`OAK_CI_SKIP_AGENT=1` 跳过第 6 段的 pi 回放（缺 node_modules 时）；
`OAK_CI_SKIP_PY=1` 跳过第 1–2 段（无 ffmpeg 的机器上 prescan 会红）。

Rust 三段实测计数（2026-11-09 `cargo test --offline` 实跑）：`oak-ai-tools`
90 passed / 0 failed（9 个 test 二进制）、`oak-ai-mcp` 30 passed / 0 failed、
`ai/ci` 4 passed / 0 failed / 1 ignored（lib 2 + replay 2；ignored 即消融
测试 `ablation_black_frame_is_rejected`，`#[ignore]`，需 `OAK_CI_BAD_FIXTURE=1`
显式跑）——三段合计 124 passed / 0 failed / 1 ignored。

## 回放夹具（Mock LLM）

格式定义在 [`src/lib.rs`](src/lib.rs) 顶部注释：NDJSON，一行一个
`{"call": "冻结工具名", "args": {...}}`；`OAK_CI_MEDIA` 是媒体路径 token，
回放时替换成本次运行的临时文件路径（mock 后端按**路径哈希**上色，路径必须
两次运行一致才能复现 PNG sha256）。

夹具：

- [`fixtures/edit_chain.ndjson`](fixtures/edit_chain.ndjson) — 建工程 → 导素材 →
  铺轨 → 取帧 → 读总览，正向链路。
- [`fixtures/bad_black.ndjson`](fixtures/bad_black.ndjson) — 消融用：无素材的
  黑帧链路，坏链路必须红（当前由 `tests/replay.rs:444` 的 `assert!(bad != "1")`
  守卫承担：该链 0 帧进入不变式；`non_black_ratio > 0.5` 断言由正向测试覆盖）。
- [`fixtures/agent-replay.json`](fixtures/agent-replay.json) — 真实 pi 进程的
  模型 stub 脚本（工具名带 `mcp__oak__` 前缀），复用
  `ai/agent/tests/stub-model-server.mjs`，**未用真实 LLM**。

会话回放日志（NDJSON）：每次调用记 `call` / `args` / `ok` / `error.code` /
`txn`（committed/aborted/read）/ `undo_depth` / `result_sha256` / 每帧
`sha256+width+height+non_black_ratio`。媒体路径回写成 `OAK_CI_MEDIA` token，
日志本身机器无关；两次运行逐字节相同（`replay_is_reproducible_and_frames_are_golden`）。

## 黄金帧策略（运行时不变式，不入仓大图）

1. 尺寸与请求一致（`width/height > 0`，与 `max_size` 同参）；
2. `png_base64` 能解码，解码几何与声明一致（T4 `PixelReadback`）；
3. 非黑比例 > 0.5（T4 `render_done` 钩子在管线内强制，测试再断言一遍）；
4. 同一夹具两次回放的 PNG sha256 相同；MCP 子进程与进程内直调的 sha256
   相同（零漂移）。

## CI 集成

`.github/workflows/ai-headless.yml`：独立 job，默认**不构建引擎**（`--backend
mock` + 默认 features），只在 `ai/**` 变更时触发；引擎（`--features sdk`）
的构建/测试仍归主 CI `ci.yml` 的 build-test，本 workflow 不碰。workflow
语法静态检查：`scripts/lint-workflows.sh`——本机没有 actionlint，退化为
PyYAML 解析 + 结构断言（on/jobs/runs-on/steps/uses 钉版本/无密钥片段），
首次推到 GitHub 后以 Actions 运行结果为准。

## agent 回放的两个 pi 前提（实测）

1. **项目信任**：pi 只信任 `agentDir/trust.json` 里按 cwd 记的决定（`--approve`
   只覆盖会话级）。`agent-replay.sh` 只写本次 run 的 agentdir，不碰
   `~/.pi/agent/trust.json`。
2. **上下文链位置**：pi 只沿 cwd 向上找 `AGENTS.md`（`resource-loader.js`
   `loadProjectContextFiles`），所以 `context.sh` 支持 `OAK_AI_CWD` 把生成链
   复制进会话 cwd（本回放里是 `ai/ci/AGENTS.md`，已 gitignore）。

## 本地等价复跑

```bash
ai/ci/run.sh                 # 全绿基线
OAK_CI_BAD_FIXTURE=1 cargo test --offline --manifest-path ai/ci/Cargo.toml \
  --test replay ablation -- --ignored   # 消融：坏链路必须红（replay.rs:444 守卫）
```

## 已知限制

- agent 回放用 stub 模型，**未用真实 LLM**；只证明链路（工具面/上下文/回放）通。
- 黄金帧哈希依赖 mock 后端按路径上色：同一 fixture + 同一 crate 版本才可比，
  换 mock 上色算法会全红（属预期，需重录基线）。
- `cargo fmt` 门禁暂未进本 workflow：`ai/oak-ai-tools`/`oak-ai-mcp` 现状有
  130/17 处 diff（fmt 门禁按既定裁断维持不加），先跑会把已验收基线
  打红；`ai/ci` 自身 `cargo fmt --check` 已绿。
