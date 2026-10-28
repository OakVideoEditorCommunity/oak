# P2A：语义编辑与原生 ABI 实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P2A。  
> 状态：engine/SDK 编辑层完成；RealEngine 采用留待 P2B。本文不把 P2A 当成完整 P2。

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

## 5. P2B 尚未完成的原因

RealEngine 的渲染、效果链、多机位和代理仍直接读取旧图对象。仅切换时间线会迫使
app 同时维护两份工程或引入不安全的 Arc 桥接，二者都不接受。因此 RealEngine 采用
必须在 P3–P6 的查询和会话能力齐备后原子完成。

P2A 不改变 `oak-app` 的内部依赖，也不宣称 `oak-timeline` / `oak-undo` 已清零。
后续顺序为 P3 节点与效果、P4 素材/多机位、P5 渲染/播放、P6 OFX，最后执行 P2B
和 P7 的依赖收口。
