# P2A：语义编辑与原生 ABI 实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P2A。  
> 状态：engine/SDK 编辑层完成；P2B 已由后续决策明确跳过。本文不把 P2A 当成 app
> 已完成接入。

## 1. 完成范围

- 原 `oakui/graphops.rs` 的编辑算法和测试已迁入 `oak-engine`。
- `EditorSession` 执行全部语义编辑：轨道、放置、转场、split/trim/roll/slide/slip、
  移动、链接、剪贴板、速度、重命名、嵌套、marker 和 workarea。
- SDK 提供强类型 `EditCommand` / `EditResult`，返回受影响 NodeId、数量、实际有理长度、
  revision 和拥有型剪贴板。
- `EditorSession` 保存当前序列、剪贴板、marker/workarea 和 write-through 绑定。

## 2. 原生 ABI，不使用 JSON

这里没有 IPC，只有同进程 C ABI 函数调用：

```text
oak_editor_edit(session, op, repr(C) params, result, error)
```

字符串使用指针和长度，ID 与剪贴板使用调用方缓冲区，结果使用 `repr(C)` 结构。
副作用命令只执行一次，不使用“先查询长度、再执行一次”的两段式调用。

模块根使用 `editor_commands.rs`、`editor_dispatch.rs`，子文件位于同名目录；不使用
`mod.rs`，也没有 `partxx.rs`。新增文件均少于 500 行。

## 3. 生命周期修复

全变体测试发现 marker 的临时 CHandle 路径会段错误。会话本来就持有
`Arc<Mutex<TimelineMarkerList>>` / `Arc<Mutex<TimelineWorkArea>>`，因此改为
`new_arc` / `new_with_old_arc` 原生构造并删除临时 make/release handle。最小复现和
完整变体现在均通过。

## 4. 验证

- 迁入的 48 个编辑测试通过，覆盖原有图编辑语义和 undo/redo。
- `semantic_edit_undo_redo_and_reload` 验证 revision、分割回滚/重做及保存重开。
- `all_semantic_operations_use_native_abi` 覆盖全部 `EditCommand` 变体，包括多 ID、
  剪贴板、转场实际 clamp 值和辅助状态。
- `marker_only_minimal_repro` 保留生命周期回归。
- engine / SDK 全量测试通过，工作区编译通过。

## 5. P2B 跳过决定

RealEngine 的渲染、效果链、多机位和代理仍直接读取旧图对象。仅切换时间线会迫使
app 同时维护两份工程或引入不安全的 Arc 桥接，二者都不接受。本轮据此明确跳过
P2B，不再把 RealEngine 采用、app 历史切换和 P2 依赖收口列为待执行步骤。

P2A 不改变 `oak-app` 的内部依赖，也不宣称 `oak-timeline` / `oak-undo` 已清零。
后续直接进入 P3 节点与效果，再按 P4、P5、P6 推进；P2B 的未完成事项不得计入
后续阶段的完成声明。
