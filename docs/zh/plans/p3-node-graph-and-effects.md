# P3：节点图、效果与参数体系实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P3。  
> 状态：P3A 已完成，P3B 待启动；P3 整体进行中。P2B 已明确跳过；本文不把 P2A 或
> P3 子阶段完成解释为 app 已完成切换。

## 1. 子阶段

- P3A：效果库 SDK 枚举、`EditorSession` 效果链命令、静态参数写入及 undo/ABI 测试。
- P3B：会话图快照、端口/连接 DTO、受保护节点、连接校验和图命令。
- P3C：完整参数元数据、枚举/分组、关键帧与曲线编辑、按钮和通知。
- P3D：切换节点编辑器、效果库、效果栈与 inspector，删除 app 领域 helper。

P3A 复用 engine 已有的效果链实现，不在 SDK 复制图算法。所有会话命令继续走
原生 `repr(C)` ABI，保持一次执行和统一 undo revision。

## 2. P2B 跳过的影响

RealEngine 仍持有旧工程路径，因此 P3A 的会话能力先由独立 SDK 消费者和测试验收。
app 中不依赖实时工程实例的效果/节点库枚举可以先切 SDK；效果链和参数面板只有在
同一工程实例能够安全接入时才切换，不引入第二份工程或裸 Arc 桥接。

## 3. 完成边界

只有 P3A–P3D 全部满足，且独立消费者不依赖 `oak-node` 完成建图、连图、效果、参数、
关键帧与渲染验证后，才能把 P3 标为完成。当前处于 P3B 待启动状态。

## 4. P3A 实施结果

- `EditCommand` 追加 `EffectInsert`、`EffectRemove`、`EffectMove`、`EffectSetEnabled`
  和 `SetNodeStandardValue`；opcode 只向后追加，ABI 使用原生节点 ID、字符串缓冲区、
  元素索引和 `repr(C)` POD 参数值。
- 会话分发直接复用 engine 已有效果链算法；静态参数写入与旧 `oak_node_set_standard_value`
  共用同一个实现，保留声明类型转换、逐输入旧值捕获和单次 undo。
- SDK 参数打包与 engine ABI 解码分别拆到子文件，保持每个新增文件少于 500 行；
  不在 SDK 复制效果链或参数领域逻辑。
- app 的效果库和节点库枚举改走 SDK factory DTO；动态插件节点按 OpenFX 类别进入节点库，
  效果分组、i18n 和排序保持原行为。
- 新增 `effect_commands_undo_redo_and_static_parameter_roundtrip`，并把五个变体纳入
  全命令 ABI 覆盖；效果插入、重排、删除、启停和静态参数均验证 undo/redo。

## 5. P3A 验证

- engine：168 个测试通过。
- SDK：88 个测试通过。
- app：effectchain 20 个、nodegraph 8 个定向测试通过。
- app 全量：720 个通过、1 个忽略、15 个失败。失败均为渲染 worker/共享内存环境路径，
  包括 `oak-worker` 未构建或 `OAK_WORKER_BIN` 未设置，以及沙箱中的
  `shm_open: Operation not permitted`；P3A 相关效果、图和参数测试没有失败。
- `python3 tooling/check_app_engine_boundary.py --update-baseline`：内部引用基线由 1193
  收紧至 1182，直接内部依赖数量未增加。
