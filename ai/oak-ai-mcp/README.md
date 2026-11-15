# oak-ai-mcp（T5：MCP 薄封装）

把 Oak AI 剪辑执行层（`ai/oak-ai-tools`，22 个已冻结工具）以 **MCP
server（stdio）** 的形式暴露给外部客户端（Claude Desktop 等）。本 crate
是**传输适配器**：只做 JSON-RPC 2.0 / NDJSON 分帧、生命周期、`tools/list`
与 `tools/call` 转发，**不持有第二份工具表、不实现第二份校验、不实现第二份
事务**。

## 与 oak-ai-tools 的关系

```
MCP 客户端 ──stdio(NDJSON/JSON-RPC 2.0)──> oak-ai-mcp
                                              │  dispatch::registry()     读工具面
                                              │  dispatch::default_schemas() 读冻结 schema
                                              └─ dispatch::execute(backend, tool, args)
                                                   ├─ 未知工具 → UNKNOWN_TOOL / step=lookup
                                                   ├─ schema 校验 → SCHEMA_VALIDATION_FAILED / step=validate
                                                   ├─ 事务包裹（一调用一 undo 组；edit.undo 除外）
                                                   └─ 失败 → {code, message, tool, step, gap?, txn?}
```

- 工具名来源唯一：`oak_ai_tools::dispatch::registry()`；schema 来源唯一：
  `ai/schema/tools/*.json`（经 `dispatch::default_schemas()` 编译）。
- 本 crate **没有**工具名常量表、**没有** JSON Schema 校验代码、**没有**
  begin/commit/abort 代码；这些全部在 `oak-ai-tools` 里，改动只可能发生在
  那里，因此不存在两处维护导致的漂移。
- `tests/zero_drift.rs` 把「MCP `tools/list` ↔ registry ↔ schema 目录」
  三者钉成双向一一对应的集合相等，并断言每个 `inputSchema` 与 schema 文件
  文档**逐字段相等**、`description` 取自 schema 文件本身。

## 模块

| 文件 | 职责 |
|---|---|
| `src/rpc.rs` | JSON-RPC 2.0 信封 + NDJSON 分帧（读帧返回 `Message`/`Malformed`/`Eof`；写帧一行一对象并 flush） |
| `src/server.rs` | 生命周期、`tools/list`、`tools/call`、错误整形（`isError` + `structuredContent`） |
| `src/backend_select.rs` | `--backend mock|sdk` 解析与构造；`sdk` 缺失时**明确拒绝** |
| `src/main.rs` | 参数解析 + stdin/stdout 帧泵；stdout 只有协议帧，诊断走 stderr |

## 命令示例

```bash
# 构建（默认 features：不拉引擎 / FFmpeg；下文命令均以调试构建实测）
cd ai/oak-ai-mcp && cargo build

# 帮助与版本
./target/debug/oak-ai-mcp --help
./target/debug/oak-ai-mcp --version

# 手工会话：一行一个 JSON-RPC 消息，stdin EOF 即正常退出（exit 0）
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"demo","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"project.open","arguments":{"name":"demo","fps":30}}}' \
  '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"timeline.add_track","arguments":{"type":"video"}}}' \
  | ./target/debug/oak-ai-mcp --backend mock
```

Claude Desktop 一类的客户端配置（把 `$REPO` 换成本仓绝对路径）：

```json
{
  "mcpServers": {
    "oak": {
      "command": "$REPO/ai/oak-ai-mcp/target/debug/oak-ai-mcp",
      "args": ["--backend", "mock"]
    }
  }
}
```

## 协议方法覆盖

| 方法 | 类型 | 行为 | 错误 |
|---|---|---|---|
| `initialize` | request | 版本协商 + `capabilities.tools` + `serverInfo` + `instructions` | 缺 `protocolVersion` → `-32602` |
| `notifications/initialized` | notification | 不回包（生命周期 ack） | — |
| `tools/list` | request | 22 个工具，`inputSchema` 为冻结 schema 原文 | — |
| `tools/call` | request | 转发 `dispatch::execute`；成功 `isError:false`，失败 `isError:true` | 缺/非法 `name`、非对象 `arguments` → `-32602`；工具级失败在 `result` 里返回（见下） |
| `ping` | request | `{}` | — |
| 其他 request | — | `-32601`（`data.supported` 列出支持项） | — |
| 其他 notification | — | 静默忽略（JSON-RPC 不允许回包） | — |
| 批次数组 / 非对象帧 | — | `-32600`（MCP stdio 只承载单对象帧） | — |
| 非 JSON 行 | — | `-32700`，`id: null`，会话继续 | — |
| `initialize` 之前的任何 request | — | `-32002`（MCP `server not initialized`） | — |

工具级失败**不**用 JSON-RPC error 表达（否则模型拿不到可重规划的结构化
字段），而是 `result.isError = true`：

```json
{
  "jsonrpc": "2.0", "id": 4,
  "result": {
    "content": [{ "type": "text", "text": "SCHEMA_VALIDATION_FAILED: ... [tool=timeline.split_clip step=validate]" }],
    "isError": true,
    "structuredContent": {
      "code": "SCHEMA_VALIDATION_FAILED",
      "message": "arguments rejected by schema 'timeline.split_clip': ...",
      "tool": "timeline.split_clip",
      "step": "validate"
    }
  }
}
```

`structuredContent` 就是 `ToolError::to_json()` 原样（成功时是工具返回值
原样）。字段约定：`code`（稳定机器码）、`message`、`tool`、`step`
（`lookup`/`validate`/`begin`/`invoke`/`commit`）、`gap`（`code ==
UNSUPPORTED` 时的能力缺口 id）、`txn`（`aborted` 表示事务已回滚）。成功
结果没有 `txn`，schema 校验失败也没有 `txn`（未开事务）。

## MCP 版本基线与分帧

- 分帧：**NDJSON** —— 一行一个 JSON 文档，UTF-8，无 `Content-Length`
  头。这正是 MCP 对 stdio 传输的规定，因此不需要第三方传输库。
- 基线：**2024-11-05**。客户端请求 `2024-11-05` / `2025-03-26` /
  `2025-06-18` 任一版本时原样回声；请求其他版本时回 `2024-11-05`。
  本 crate 只用到三个版本间语义未变的部分（生命周期、`tools/list`、
  `tools/call`、`content`/`isError`），因此该回声是诚实的；未实现 2025
  版本的 `outputSchema`、resources、prompts、elicitation、sampling 等。
- `structuredContent` 自 2025-06-18 起是标准字段；在更早版本下它是附加
  字段，按 MCP 的兼容规则客户端会忽略未知字段。若客户端只读
  `content[0].text`，错误码仍在文本摘要里可见。
- stdout 只写协议帧，所有日志走 stderr（否则会污染帧流）。

## 后端选择

| 参数 | 需要 | 行为 |
|---|---|---|
| 省略 / `--backend mock` | 无 | `MockBackend`：内存时间线，无引擎、无 FFmpeg |
| `--backend sdk`（默认构建） | — | **拒绝启动**，exit 2，说明需 `--features sdk` 且不得静默降级 |
| `--backend sdk`（`--features sdk` 构建） | 引擎 | 构造 `oak_ai_tools::sdk::SdkBackend::new()` 并启动服务，工具调用走真实引擎（编译验证状态见「已知限制」） |

拒绝时 stdout 不输出任何字节，退出码 2，错误在 stderr：

```
oak-ai-mcp: --backend sdk is not available: this binary was built without the
'sdk' feature. Rebuild with `cargo build --release --features sdk` (it links
oak-engine-rust-sdk and the project FFmpeg/engine). Refusing to fall back to
the mock backend; pass --backend mock to run against the in-memory mock.
```

## 依赖

只新增 `serde_json`（`oak-ai-tools` 已依赖它，未引入任何新的第三方 crate）
与 path 依赖 `../oak-ai-tools`。JSON-RPC 信封与 NDJSON 分帧手写，理由：
协议面极小（5 个方法 + 3 类错误），引第三方 MCP SDK 反而会把工具面复制
进第二处。`[patch.crates-io] ocio-rs` 与 `../oak-ai-tools/Cargo.toml` 的
补丁保持同一 fork/rev，仅为让 `--features sdk` 用同一份引擎源码；默认
features 不构建它。

冷启动 `--release` 构建会连带编译 `jsonschema` 传递引入的
`reqwest` / `aws-lc-sys`（含 C 代码），耗时明显长于 debug 构建——这是
`oak-ai-tools` 的既有依赖树，不是本 crate 引入的；日常跑协议用 debug
构建即可。

本 crate 是**独立 workspace 根**（`[workspace]` 空表），与
`../oak-ai-tools/Cargo.toml` 同理：仓库根 workspace 只列 `crates/*` 与
`examples/*`，`ai/*` 不进入上游构建。

## 测试

```bash
cd ai/oak-ai-mcp
cargo test                    # 默认 features，无需引擎
cargo test --test protocol    # 只跑协议级（会 spawn 真实二进制）
cargo test --test zero_drift  # 只跑零漂移
```

- 单元测试（`src/*.rs`，20 项）：信封、分帧、生命周期、参数校验、错误整形、
  后端拒绝。
- `tests/protocol.rs`（7 项；含回声断言 `protocol_version_is_echoed_or_baselined`；
  其中 sdk 一项按构建二选一：默认构建断言拒绝
  语义，`--features sdk` 构建断言能启动并服务）：启动真实二进制经管道跑
  `initialize → notifications/initialized → ping → tools/list →
  tools/call(project.open/timeline.add_track) → 非法参数 → 未知工具 →
  事务内失败 → EOF`；另测「initialize 前请求 `-32002`」「坏帧不杀会话」
  「sdk 拒绝」「未知 backend / --help 退出码」「版本回声与未知版本回落基线」。
  帧读取有 30s 超时，服务端
  卡住会失败而不是挂住 CI。
- `tests/zero_drift.rs`（3 项）：三方集合相等（双向）、每个
  `inputSchema` 与冻结 schema 文件逐字段相等、`tools/list` 两次调用一致。

## 已知限制

1. **`--features sdk` 已接线，2026-11-08 实测通过（仅接线冒烟，见条末）**：
   `src/backend_select.rs::sdk_backend()` 直接构造
   `oak_ai_tools::sdk::SdkBackend::new()`（该构造函数在 `ai/oak-ai-tools/src/sdk/mod.rs`
   落地，无参数、只建空状态）。默认构建不编译该分支，故默认 `cargo test`
   不受影响。
   **实测记录（2026-11-08，复用 T1 线 `--features sdk` 构建产物）**：
   `cd ai/oak-ai-mcp && PATH=/tmp/oak-link-shim:$PATH CARGO_TARGET_DIR=/tmp/oak-ai-sdk-target cargo test --features sdk --test protocol --offline`
   → **exit code 0；`test result: ok. 6 passed; 0 failed; 0 ignored`
   （回声断言加入前）**；回声断言加入后同命令复跑（2026-11-09 00:21）→
   **exit code 0；`test result: ok. 7 passed; 0 failed; 0 ignored`**，其中
   `sdk_backend_starts_and_serves_when_built_with_the_feature ... ok`（该会话
   启动横幅为 `backend sdk`：initialize 应答 → tools/list 22 项 → EOF exit 0；
   引擎 `liboak_engine.so` 由 cargo 注入的 LD_LIBRARY_PATH 正常加载，无 exit 127）。
   **环境事实：验证环境曾随 /tmp 清理丢失（2026-11-08 实测
   `/tmp/oak-link-shim/`、`/tmp/oak-ai-sdk-target/` 与 `liboak_engine.so`
   全部不存在），后由 T1 线重建（.so 于当日 23:38 重新链成）**。复验前置：
   - 前置：`/tmp/oak-link-shim/cc` 存在（本机链接 shim，把
     `.cache/ffmpeg/lib` 提到系统 `/usr/lib` 之前以避开非 PIC 归档，仅本机
     验证手段、不入仓）；`/tmp/oak-ai-sdk-target/debug/deps/liboak_engine.so`
     已链成（由 T1 线的 `--features sdk` 构建产出）。
   - 复验命令即上文实测命令（经 cargo 跑，LD_LIBRARY_PATH 自动覆盖，见下条
     前置说明）；`cargo check --features sdk --offline` 只做类型检查，可作更快
     的先行信号，但不能替代运行验证。
   **该断言的运行时前置（机制用最小复刻实测，非推测）**：
   - `oak-engine` 是 cdylib，SDK 以 `#[link(name = "oak_engine")]` 动态链接，
     最终二进制带 `DT_NEEDED liboak_engine.so` 且**没有 RUNPATH**（
     `crates/oak-engine-rust-sdk/build.rs` 只把 `target/<profile>` 与
     `target/<profile>/deps` 作为**链接期**搜索路径）。所以运行时只能靠加载器
     搜索路径找到该 `.so`。复刻实验：同样写法的 cdylib 依赖 → 产物为未加哈希的
     `deps/lib<name>.so`，`readelf -d` 有 NEEDED、0 条 RUNPATH，
     `env -u LD_LIBRARY_PATH` 直接执行 → `error while loading shared libraries`，
     exit 127。
   - `cargo test` / `cargo run` 会为子进程注入
     `LD_LIBRARY_PATH=<target>/<profile>:<target>/<profile>/deps:…`；而复刻实验中
     `cargo test` 的测试进程用 `Command` 启动被测二进制后，子进程打印到的
     `LD_LIBRARY_PATH` 正含 `<target>/debug` 与 `<target>/debug/deps`，且调用
     成功。`tests/protocol.rs` 的 `Session` 同样继承环境（未 `env_clear`），
     故**经 cargo 跑测试时自动可加载**，无需手工设置。
   - 反之是**假红**：直接执行 `target/*/deps/protocol-*`，或用不继承 cargo 环境的
     IDE/CI runner 跑，二进制在进入协议前就 exit 127。此时本 crate 的断言会在
     `recv_timeout` 上以「空且已断开的通道」立刻失败（`tests/protocol.rs:101-106`
     的 panic 文案会说 “within 30s”，其实立即返回），真实原因只在子进程
     stderr（`Stdio::inherit`）里，不在 panic 文案里。
   - `--features sdk` 构建下**所有**协议测试（包括 `--backend mock` 的那些）都要求
     该 `.so` 可加载：protocol 测试二进制与被测二进制都经 `oak-ai-tools/sdk`
     链接引擎（依赖图事实，非运行实测）。手工运行 server 时需
     `LD_LIBRARY_PATH=<target>/<profile>/deps`。
   - `cargo check --features sdk` 只做类型检查，不链接、不产出可执行文件，
     **不能**证明运行时加载；运行验证按本条开头的实测命令执行（2026-11-08
     首跑 6 passed、回声断言加入后 2026-11-09 复跑 7 passed，均 exit 0，
     cargo 自动带上加载路径，链接或加载失败都会当场报）。
   - 该用例只是**接线冒烟**：它不调用任何引擎操作（`SdkBackend::new()` 只建空状态），
     所以它绿**不等于**引擎功能可用——真实引擎行为由 T1 的 sdk E2E 覆盖；
     默认构建下它被 `cfg` 排除，默认套件的「绿」不包含这条断言。
2. **运行时依赖仓库布局：schema 目录缺失时 `tools/list` 直接 panic（exit 101）**。
   `oak-ai-tools` 把 `env!("CARGO_MANIFEST_DIR")` 在**编译期**固化进二进制
   （`ai/oak-ai-tools/src/schema.rs:21-23`），运行时回推 `<repo>/ai/schema/tools`；
   目录不可读时 `SchemaRegistry::load_dir` 抛 panic（`schema.rs:76`，`cannot read
   the frozen tool schema directory ... No such file or directory`），进程 exit 101，
   **不产生任何 JSON-RPC 错误帧**。副本实测（2026-11-08，/tmp 拷贝整仓、仅移走副本
   的 `ai/schema/`）：基线 exit 0、22 工具；移除后 `initialize` 仍正常应答，
   随后的 `tools/list` 即 panic——客户端看到的是连接断开而非 `-32603`。
   影响：**二进制不能脱离源码检出树单独分发**；构建后整体搬移仓库目录同样会断
   （panic 信息里的路径是编译期固化的绝对路径）。修复属 T1 层的 schema 打包/embed
   方案，超出本 crate 范围，此处只记事实与影响。
3. 不实现 MCP 的 resources / prompts / logging / sampling / completion；
   `notifications/cancelled` 是 no-op（请求逐帧同步处理，没有第二个在飞
   请求可取消）。
4. 同步单请求模型：一次只处理一帧，长耗时调用（`export.start`、
   `render.get_frame`）期间不响应 `ping`。
5. `render.get_frame` 返回 base64 PNG：会同时出现在 `content[0].text`
   （给只读文本的客户端）与 `structuredContent` 里，帧大时响应体翻倍。
6. `initialize` 可重复发送（幂等，返回同样的结果）；MCP 规范要求客户端只
   发一次，这里选择容忍而不报错。
7. 客户端发来的 response 形态帧被静默忽略——本 server 从不主动发请求。
8. 无鉴权、无 TLS：面向本机 stdio，非网络传输。
9. 非法 UTF-8 输入按 I/O 错误终止进程（exit 1），不是逐帧错误。
