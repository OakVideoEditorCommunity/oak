# OPP/1 实施偏差注记

> 本文记录实现 OPP/1（[`external-plugin-protocol.md`](../plans/completed/external-plugin-protocol.md)，**冻结，不改**）时的**有意偏差与实施口径**。协议本身不动；凡是与协议字面不一致的落地决定，逐条记在这里，附依据与复评条件。
>
> 配套规格：`/tmp/oak-reviews/opp-spec.md`（备份 `~/.oak-reviews/opp-spec.md`）。
> 首裁日期：2026-11-11（四条架构裁断 + 一条日志方案，首辅定）。

---

## 1. `project.open` 不进 undo 栈

**协议字面**：OPP §8.2 `project.open` 标注「事务：是、确认：是」（`protocol.md:239`）；§6 规则 2 说 `commit` = 一组 UndoCommand 压栈。

**偏差**：`project.open` 要求携带 `txn` 且过确认门，但**不进入 `oak_undo` 的 undo 分组**——事务令牌在该调用上仅承担两件事：串行化（全局单持的持有者身份）与确认门挂载点；打开工程不产生 undo 行，撤销栈在 open 成功后原样保留。

**依据**：「打开工程」是替换整个 `ProjectRef`（`graphops::load_ove`，`crates/oak-app/src/oakui/graphops.rs:145`），`group_begin/group_end` 包不住换树——把「换树」压进 undo 栈等于要求实现「撤销 = 换回旧树」，这既不在 `oak_undo` 现有语义内，也不是用户对 Ctrl-Z 的预期。C++ 老版的打开工程同样不入 undo 栈（**未验证**，待核 `cpp-legacy`）。

**影响面**：
- 插件视角无感：`edit.begin` → `project.open` → `edit.commit` 的报文流程完全按协议走。
- 例外记账：`edit.commit` 在「本事务内只做了 open/save 这类不入栈的操作」时，`group_begin/end` 实际不产生任何 undo 行，历史面板不出现空条目——实现需保证不留下空 undo 组。
- 崩溃路径（§6 规则 4）同样适用：未决事务 abort 时，open 已产生的效果**不回滚**（换树不可逆），只丢弃令牌并记日志。此为偏差中最需注意的一点，`A13` 验收项覆盖。

**复评条件**：若日后 `oak_undo` 引入「工程级快照栈」，可改回全事务入栈。

---

## 2. 宿主只依赖 `HostServices` trait

**问题**：规格首版提出三种依赖方向，(a) `oak-extensions` 直接依赖 `oak-app`（能直接复用全部 `graphops/renderops` 映射，但把 gpui 拖进宿主）、(b) app 侧定义 `HostServices` trait、(c) 下沉新建 crate。

**首辅裁断**：选 (b)——**宿主只依赖 `HostServices` trait，不依赖 `oak-app`**。

**实施口径**：
- `oak-extensions/Cargo.toml` 不出现 `oak-app` 依赖。
- `HostServices` 覆盖规格 §1 六域方法的全部落地调用（`graphops`/`renderops`/`effectchain`/`oak_undo`/`transport`/`ExportEvent`），形状参照 `EngineGateway`（`crates/oak-app/src/oakui/engine.rs:232`）。
- app 侧实现该 trait，把规格 §1 的映射包进去；事件埋点（`project.opened`、`playhead_moved` 等）也从这一层发。
- **待定**：trait 定义放哪个 crate。候选：新建 `oak-plugin-api`（最干净，多一个 crate）或并入既有低层 crate（`oak-core` 级别，少 crate 但语义混）。**此项未裁**，P1 开工前定。

**依据/收益**：与现有 `AppEngine` 泛型架构一致；上游 quercus 拆分把 15 crate 移出到 `quercus-engine` 子模块（见 2026-11-10 上游调研），宿主不碰 `oak-app` 即不随 GUI crate 迁移。

---

## 3. IO 用 mpsc 异步配对，UI 线程不阻塞

**协议字面**：OPP §2 允许任意数量 in-flight 请求；同一事务内变更按到达顺序串行（`protocol.md:43-44`）。协议不规定宿主内部线程模型。

**首辅裁断**：宿主 IO 走 **mpsc 异步配对**——插件请求进入 `mpsc` 通道 → 执行侧（引擎线程）处理 → 结果回 `mpsc` → IO 线程按 `id` 配对写回 stdout。**UI/gpui 线程永不阻塞等待插件**，不引入 tokio（`system.md:334`：「不引入 tokio 到 app 路径；IO 线程 + mpsc + gpui executor 足够」）。

**附带规则**：
- 事务内变更的串行（协议 §2）由**执行侧单消费者队列**保证，天然满足。
- 确认类请求（§7.3）在用户裁决前不返回响应（`protocol.md:46-47`，SDK 默认超时 120 s）→ 执行侧挂起该 `id` 的响应，不占用通道。

---

## 4. `oak-extensions` = lib + `opp-smoke` bin

**现状**：`crates/oak-extensions` 只有 `src/main.rs`（3 行 Hello World），`Cargo.toml` 零依赖。

**首辅裁断**：拆成 **lib + bin**——
- `src/lib.rs` 承载宿主全部逻辑（`manifest/host/state/capabilities/rpc/api/error/txn/logsink`），供 `oak-app` 引用；
- `src/bin/opp-smoke.rs` 作为独立冒烟入口（P1 验收 A1 载体）：起宿主、握手、跑 `project.get_info`、打印结果，不启 GUI；
- 原 `src/main.rs` 退役（并入 bin 或删除，**P1 首 commit 时一并处理**）。

**依据**：宿主逻辑必须是 lib 才能被 app 引用；bin 保证 P1 生命周期验收可 headless 跑（与 `ai/ci` 的 headless 思路一致）。

---

## 5. `session.log` P1 走 `$OAK_LOG_DIR` 文件方案

**协议字面**：OPP §8.1 `session.log`：「plugin→Oak，进 Oak 日志面板。高频日志请走 stderr」（`protocol.md:231`）；§1 也说 stderr 由 Oak 捕获「进日志面板」（`protocol.md:20-21`）。

**偏差**：**仓内没有日志面板**——`crates/oak-app/src/logging.rs:135` 的 `init()` 只装了 `StderrLogger`（`:120-127`，`eprintln!` 到 stderr），全 app 无 ring-buffer 日志 UI。

**首辅裁断**：P1 用 **`$OAK_LOG_DIR` 文件方案**——
- 每个插件进程的 `stderr` 转发线程写入 `$OAK_LOG_DIR/<plugin-id>.log`（按日/按大小轮转，上限可配）；
- `session.log` 的 `message` 同样追加进该文件，带级别前缀；
- `$OAK_LOG_DIR` 未设置时降级为丢弃（或接 `log` crate，**实现时定**），不 panic。

**影响面**：协议语义「进日志面板」在 P1 不满足——这是**功能性偏差**，非仅实现细节。复评条件：日后补日志面板时，把文件 sink 改成「文件 + 面板 ring buffer」双写，协议侧无需改。

**验收挂钩**：A1/A2 各跑完后应能在 `$OAK_LOG_DIR` 看到插件 stderr 与握手日志（文件存在且非空）。

---

## 6. 其他已定实施口径（非偏差，备忘）

| 项 | 口径 | 依据 |
|---|---|---|
| `inline` 阈值 | P1/P2 用 `inline`（base64 PNG），阈值从协议的 64 KiB 放宽到**可配 1 MiB**（`[limits] inline_max_bytes`） | 协议 §5 `protocol.md:151` 允许 Oak 选用 inline；放宽是**可配不改死**，P3 引入 shm 后回落 |
| `timeline.structure_changed` | `edit.commit` 成功后无条件发 `{"hint":"full"}`，v1 不做图变更 diff | 协议 §9 `protocol.md:358-359` 明确允许「always full」 |
| 像素面 UI | v1 不做（`ui.attach_surface/frame_ready/surface_ack` 延后） | `system.md:319` P5 与 P4 并列可并行；AI 插件用 `image` 控件够 |
| T1 六个 gap 的降级 | `GAP_TRANSITION_SEAM`（对侧块推断）、`GAP_TRACK_INSERT_INDEX`（忽略 index 追加尾部）等在 OPP 适配层**显式降级**，不假装支持 | `ai/oak-ai-tools/src/sdk/gaps.rs:22-81` 已记原由 |
| marker/workarea 序列化 | `CHandle` 句柄不随工程存（`graphops.rs:934/939`）→ **待定**：sidecar（上游 example-mcp 做法）还是补图节点 | P1 开工前定 |
| 版本号 | 宿主独立定义 `OPP_API_VERSION = 1`，**不复用** `oak-worker` 的 `PROTOCOL_VERSION`（`oak-worker/src/main.rs:42`） | 两者数值相同但命名空间不同（`protocol.md:8` 的 `api` vs worker 握手字段） |

---

## 7. 未裁事项（P1 开工前需首辅定）

1. **`HostServices` 所在 crate**（§2）：新建 `oak-plugin-api` vs 并入既有低层 crate。
2. **marker/workarea 存储**（§6 表）：sidecar vs 图节点。
3. **`$OAK_LOG_DIR` 未设置时的降级行为**（§5）：丢弃 vs 接 `log` crate。
4. **与上游 `example-mcp` 的对外协议边界**：P3 对内 OPP、对外是否走 MCP 生态（`system.md:300-304` 允许插件自起 MCP server）。

---

维护约定：本文件只增不改历史条目；某条偏差被推翻时，原条目保留并加「已废止（日期 + 去向）」标注。
