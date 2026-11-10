# oak-extensions — OPP/1 宿主核心（T7.1）

外部插件协议 **OPP/1** 的宿主侧核心库：NDJSON 分帧、JSON-RPC 信封、
`plugin.toml` 清单、11 态进程生命周期（有界重启 + 心跳 + stdout 纪律）、
双向会话（in-flight 配对 + 派发串行化）。

冻结线上参考：[`docs/zh/plans/completed/external-plugin-protocol.md`](../../../docs/zh/plans/completed/external-plugin-protocol.md)
（§1 分帧 / §2 信封 / §3 错误码 / §4 生命周期 / §7 能力位 / §8 方法目录）。

**本 crate 不含**（后续里程碑）：
- 引擎路由 —— `HostServices` trait 只在此声明（纯数据类型、不依赖
  `oak-app`），由 app 侧实现（P1.2，首辅裁断）；
- UI/面板壳 —— `ui.*` 只有类型占位（`protocol::ui`），渲染逻辑 P2；
- shm 帧池（P3）；日志面板（P1 只把插件 stderr 捕获进内存 ring，
  文件落盘 `$OAK_LOG_DIR` 由 P1.2 接线）。

## 模块

| 模块 | 职责 |
|---|---|
| `protocol` | 信封（Request/Response/Notification/Error）、五标准码 + 9 应用码、NDJSON 分帧（16 MiB 上限 / 禁 BOM / 禁裸 CR）、`session.hello` 握手类型、`ui.*` 占位 |
| `manifest` | `plugin.toml` 解析、校验（api=1、能力位⊆冻结集合、panel id 唯一）、`{plugin_dir}` 替换、环境白名单 |
| `lifecycle` | 11 态状态机 + supervisor 线程 + `PluginHost` 句柄 |
| `session` | in-flight 配对（发送方 id 命名空间）、120 s 请求超时、派发门、`events.*`/`session.log` 处理 |
| `host_services` | `HostServices` trait（project/media/timeline/node/render/export/undo 六域 + 事件订阅）与纯数据类型 |

## manifest 字段

```toml
id = "com.example.chat"        # 必填：反向域名，全局唯一（panel id 前缀）
name = "Example Chat"          # 必填：用户可见名
version = "0.1.0"              # 必填：semver，仅展示
api = 1                        # 必填：协议主版本，v1 = 1（非 1 即拒）

[process]
command = ["/usr/bin/python3", "{plugin_dir}/plugin.py"]  # 必填：argv，{plugin_dir} 由宿主替换
# env_passthrough = ["PATH", "HOME"]   # 可选：环境白名单（默认 PATH/HOME，绝不整体继承）

capabilities = ["project.read", "timeline.edit"]  # 可选：必须是 §7.1 冻结 13 项的子集
# panels = [{ id = "chat", title = "AI 剪辑", ui = "declarative" }]  # 必须与 session.hello.panels 一致
# subscribe = ["project.opened"]         # 初始订阅（session.hello.subscribe）

[restart]                      # 可选
max = 5                        # 默认 5（对齐 procpool MAX_RESTARTS）
backoff_ms = 1000              # 默认 1000，指数退避 1s→2s→4s→8s→16s

[limits]                       # 可选：覆盖协议 §12 默认值
# max_message_bytes = 16777216
# request_timeout_s = 120
```

校验失败（`Manifest::from_toml` / `PluginHost::start` 返回
`ManifestError`/`StartError::Manifest`）：空 id/name/version、`api != 1`、
空 argv、未知能力位、重复 panel id、`backoff_ms == 0`。

## 11 态生命周期

```
Discovered → Enabling → Spawning ──10s 无首消息──► Failed(HandshakeTimeout)
                │            └──收到 session.hello──► Handshaking
                │                                        │ 校验通过
                │                                        ▼
                │                                      Ready ◄──────────┐
                │                                        │              │ 退避
                │            ┌───────────┬───────────────┤              │ backoff<<n
                │            │           │               │              │
                │       ping×3 超时   stdout EOF/违规   shutdown       │
                │            ▼           ▼               ▼              │
                │        Crashed     Failed(违规不重启) Stopping        │
                │            │                                    │      │
                │      restarts < max                            │      │
                │            ▼                                   │      │
                │        Restarting ─────────────────────────────┘      │
                │            │ restarts ≥ max → Failed(MaxRestarts)      │
                └────────────┘（Failed/Stopped/Disabled 等用户 enable → 回 Spawning）
                                          Stopping → 2s 自退 → SIGTERM → 2s → SIGKILL → Stopped
```

- **心跳**：握手后每 2 s 发 `session.ping`，5 s 内须回，连续 3 次判死；
- **stdout 纪律**：协议外输出 / 超 16 MiB 单行 → 判违规、杀进程、**不重启**；
- **崩溃重启**：`restart.max` 次内指数退避重启，重启后所有 id/事务作废、
  订阅清空（插件须重新 `session.hello` + 重订阅）；
- **关闭阶梯**：`session.shutdown` 通知 → 2 s 自行退出 → SIGTERM → 2 s → SIGKILL。

## 可跑的 smoke 示例

```bash
# 1. 造一个假插件目录
mkdir -p /tmp/demo-plugin && cat > /tmp/demo-plugin/plugin.toml <<'EOF'
id = "demo.echo"
name = "Echo"
version = "0.0.1"
api = 1
capabilities = ["project.read"]
[process]
command = ["/usr/bin/python3", "{plugin_dir}/plugin.py"]
EOF
cat > /tmp/demo-plugin/plugin.py <<'EOF'
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
send({"jsonrpc":"2.0","id":1,"method":"session.hello","params":{
  "api":1,"name":"echo","version":"0.0.1",
  "capabilities":["project.read"],"panels":[],"subscribe":[]}})
for line in sys.stdin:
    m = json.loads(line)
    if m.get("method") == "session.shutdown":
        sys.exit(0)
    if "method" in m and "id" in m:
        send({"jsonrpc":"2.0","id":m["id"],"result":{}})
EOF

# 2. 跑（无 GUI 验收）
cargo run --offline -p oak-extensions --bin opp-smoke -- /tmp/demo-plugin --duration-ms 5000
# 期望输出：state 序列 … Ready … 收到 duration 到点的 shutdown → Stopped，
# spawns=1、failure=None、kill=None、exit="exit status: 0"，退出码 0
```

`opp-smoke` 退出码：`0` 到过 Ready 且无失败 · `2` 终态 Failed ·
`3` 时长内没到 Ready · `64` 用法错 · `65` manifest 错 · `66` 启动错。

## 测试

```bash
CARGO_TARGET_DIR=/tmp/opp-target cargo test --offline -p oak-extensions
```

五条必测（`tests/lifecycle.rs`，全部起真子进程、无网络）：
① 握手超时杀进程 ② 协议外 stdout 判违规 ③ 17 MiB 超限拒绝
④ 崩溃重启到 5 次停手 ⑤ 正常 shutdown 不动刀。另有协议分帧、
manifest、session 配对、HostServices 错误映射的单元测试。

## 已知边界（P1）

- 插件请求只服务 `events.subscribe`/`events.unsubscribe`/`session.log`，
  其余回 `METHOD_NOT_FOUND` —— `HostServices` 路由在 P1.2；
- 心跳 miss 计数、握手超时等旋钮由 `LifecycleConfig` 覆盖（测试用），
  manifest `[limits]` 的覆盖接线留给 P1.2；
- `project.open` 的 `txn` 只做串行化 + 确认门，不进 undo 栈（首辅裁断，
  偏差记录在 `docs/zh/plans/opp-implementation-notes.md`，P1.2 建档）。
