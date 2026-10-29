# Oak SDK 与 C ABI 使用指南

本文面向**新前端开发者**：只依赖 Oak 的两个 crate（`oak-engine` 的 C ABI +
`oak-engine-rust-sdk`），即可实现一个完整的剪辑软件。应用层（`oak-app`）
是这套边界的第一个消费者；`examples/sdk_editor` 是第二个、独立的最小消费者。

## 1. 分层

```text
新前端（Rust / C / C++）
        │
        ├── Rust:  oak-engine-rust-sdk   （安全类型、RAII、任务、DTO）
        │             │  link(name = "oak_engine")
        └── C/C++: include/oak_engine.h ──┼──► liboak_engine（cdylib）
                                          │
                                   oak-engine 领域实现
```

- `oak-engine-rust-sdk` 只通过 `extern "C"` 调用引擎，不使用引擎的 Rust API。
- 引擎的所有权、错误码、线程与任务语义在 C ABI 层定义；SDK 与 C 头文件都从
  同一份声明生成/派生，不能各自漂移。

## 2. Rust SDK

```toml
[dependencies]
oak-engine-rust-sdk = { path = "crates/oak-engine-rust-sdk" }
```

完整用法见 [`crates/oak-engine-rust-sdk/README.md`](../crates/oak-engine-rust-sdk/README.md)，
可运行示例见 [`examples/sdk_editor`](../examples/sdk_editor)：

```bash
cargo run -p sdk_editor -- /tmp/oak-sdk-demo
cargo test -p sdk_editor
```

要点：

- `EditorSession` 持有工程、当前序列、剪贴板、marker/workarea 与写穿透绑定；
  `EditorSession::close`/`Drop` 释放会话状态。
- 所有跨边界返回值都是拥有型数据；帧/音频在边界复制，`take_*` 只能成功一次。
- `Task`、`TaskCancellation`、`PreviewFrameRequest` 可跨线程；渲染/混音/导出在
  引擎 worker 上执行，UI 线程只轮询状态。
- 保存重开后 **NodeId 会重新编号**，必须从 `ProjectSnapshot`/`GraphSnapshot`
  重新解析节点，不能缓存 id。

## 3. C ABI

### 头文件

- 路径：`crates/oak-engine/include/oak_engine.h`
- 覆盖：项目/时间线/节点/参数、编辑命令、预览与任务、渲染、音频、存储、OFX、undo。
- 生成：`python3 tooling/gen_c_header.py --write`
- CI 漂移检查：`python3 tooling/gen_c_header.py --check`
- 常用 POD 布局在头文件内用 `_Static_assert` 固化；头文件同时可用于 C11 与 C++17。

### 链接

```bash
cargo build -p oak-engine --release
cc app.c -I crates/oak-engine/include \
   -L target/release -loak_engine -Wl,-rpath,'$ORIGIN'
```

平台库名：

| 平台 | 产物 |
|---|---|
| macOS | `liboak_engine.dylib` |
| Linux | `liboak_engine.so` |
| Windows | `oak_engine.dll` + `oak_engine.dll.lib` |

### 规则

1. **句柄**：所有对象是不透明 `void*`；`*_add_ref`/`*_release` 成对导出， `NULL`
   安全。
2. **时间**：`OakRational { int64_t num, den; }`（秒，已约分），`0/0` 为无效值。
   `OakTimeRange { OakRational in_, out; }` 为半开区间。
3. **字符串**：snprintf 风格（`buf`/`len`，返回应有的长度，负数表示错误）。
4. **错误**：返回码 + 错误缓冲区；引擎不会跨 C 边界 unwind。
5. **回调**：可空函数指针 + `userdata`；回调在引擎线程触发，不得重入引擎。
6. **ABI 版本**：同一 major 内只追加；枚举判别值属于 ABI，由生成的头文件固化。

### 最小 C 示例

```c
#include "oak_engine.h"

int main(void) {
    void *project = oak_project_new();
    if (!project) return 1;
    /* 查询/编辑…… */
    oak_project_release(project);
    return 0;
}
```

### 编辑命令

C 侧的用户动作走 `oak_editor_edit(session, op, &params, &result, err, err_len)`：
`op` 是 `OakEditOp` 判别值（见头文件常量），`OakEditParams` 是共享参数块，
`OakEditResult` 返回主节点、ids、revision 等。一次调用只执行一次，不会为了
查询长度而重复产生副作用。

## 4. 示例消费者

`examples/sdk_editor` 的 Cargo.toml 中 Oak 依赖只有 `oak-engine-rust-sdk`：

- headless 工作流：初始化 → 新建工程/导入/浏览 → 建序列/轨道 → 放置/分割/修剪/
  移动/波纹删除 → 效果/关键帧/参数 → source/program 预览 + 音频 → undo/redo →
  保存重开 → 导出并轮询任务。
- `--gui`：零第三方依赖的终端前端，用同一 SDK 会话渲染 ANSI 帧并控制播放，
  证明“同一会话可驱动另一个前端”。
- `tests/workflow.rs` 是 P8 的自动验收入口。

## 5. 发布

两个 crate 的元数据（license、repository、readme、keywords、categories、
`docs.rs` 配置）已就绪，内部 path 依赖同时带 `version = "=0.5.2"`，可以直接
`cargo publish`。发布顺序由依赖关系决定：

```text
oak-core → oak-ffmpeg-link → oak-codec → oak-audio → oak-undo → oak-node
→ oak-render → oak-ofx-plugin → oak-otio → oak-storage → oak-task
→ oak-timeline → oak-engine → oak-engine-rust-sdk
```

```bash
# 一键按依赖序发布（脚本内联了上面的顺序）
tooling/publish_workspace.sh --dry-run   # 只检查
tooling/publish_workspace.sh             # 真正发布
```

注意：

- `oak-engine` 与 `oak-engine-rust-sdk` 声明了带版本号的路径依赖；如果前序 crate
  还没有出现在目标 registry，`cargo package`/`cargo publish` 会在解析依赖时失败，
  这是预期行为——先按上面的顺序发布。
- 只想检查包内容而不访问 registry：`cargo package -p oak-engine-rust-sdk --list`。
- `oak-engine` 是 cdylib，发布包里包含 `include/oak_engine.h`；C/C++ 消费者也可以
  直接使用仓库中的头文件。
