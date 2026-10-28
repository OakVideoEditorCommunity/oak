# P3：节点图、效果与参数体系实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P3。  
> 状态：P3A–P3D 已完成，P3 整体完成。P2B 已明确跳过；本文不把 P2A 或
> P3 子阶段完成解释为 app 已完成切换。

## 1. 子阶段

- P3A：效果库 SDK 枚举、`EditorSession` 效果链命令、静态参数写入及 undo/ABI 测试。
- P3B：会话图快照、端口/连接 DTO、受保护节点、连接校验和图命令。
- P3C1：参数元数据、枚举/分组、静态值、关键帧与曲线编辑。
- P3C2：按钮动作和独立通知订阅。
- P3D：切换节点编辑器、效果库、效果栈与 inspector，删除 app 领域 helper。

P3A 复用 engine 已有的效果链实现，不在 SDK 复制图算法。所有会话命令继续走
原生 `repr(C)` ABI，保持一次执行和统一 undo revision。

## 2. P2B 跳过的影响

RealEngine 仍持有旧工程路径，因此 P3A 的会话能力先由独立 SDK 消费者和测试验收。
app 中不依赖实时工程实例的效果/节点库枚举可以先切 SDK；效果链和参数面板只有在
同一工程实例能够安全接入时才切换，不引入第二份工程或裸 Arc 桥接。

## 3. 完成边界

只有 P3A–P3D 全部满足，且独立消费者不依赖 `oak-node` 完成建图、连图、效果、参数、
关键帧与渲染验证后，才能把 P3 标为完成。P3D 的 app 切换和独立验收现已完成。

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

## 6. P3B 实施结果

- 新增一次性 `EditorGraphHandle`：engine 在一次调用中捕获完整图并记录 undo revision，
  SDK 再将不可变快照复制为拥有型 `GraphSnapshot`，避免逐字段查询时读到不同 revision。
- 完整序列视图包含输出卡、虚拟输入/输出端点、真实边和 clip→sequence 合成边；单片段
  视图仅包含 `footage → effects → clip`，不包含序列输出卡、端点或合成边。
- `GraphNode` 提供角色、类型、标签、enabled、受保护标记和上下文物件位置；`GraphPort`
  只包含 Texture/Samples 流输入及隐式主输出，参数输入不会误显示为图端口。
- `can_connect` 对当前实时图校验自连接、失效 ID、已占用输入、未知输入、不可连接输入和
  非流类型输入；查询不修改图、不创建 undo entry。既有连接、断开和位置命令与新快照
  共同验证。
- 新增两个端到端 SDK 测试，覆盖完整/单片段快照、不可变旧 revision、受保护节点、真实与
  合成边、位置更新以及连接校验的实时状态变化。

## 7. P3B 验证

- 新增图快照与连接校验测试：2 个通过。
- engine / SDK 全量回归：engine 168 个、SDK 90 个测试通过。
- 本阶段不改变 app 实时工程实例，也不宣称节点编辑器已经切换；app 切换属于 P3D。

## 8. P3C1 实施结果

- 新增一次性 `EditorParamHandle` 与拥有型 `ParameterSnapshot`：参数声明、标签、值类型、
  flags、静态元素值、属性和关键帧轨道来自同一 revision。
- DTO 保留 POD 与字符串双表示，支持 Text/StrCombo、Combo 选项、UI 属性和任意元素索引；
  值与轨道元素通过显式枚举查询，不存在固定数组上限。
- 新增 `UpsertNodeKeyframe`、`RemoveNodeKeyframe`、`SetNodeValueAtTime`。前两者完整保存
  插值与 Bezier 控制点并逐轨道恢复 undo；后者复用 oak-node 的标准值/关键帧时间写入规则。
- ABI 使用 `NativeMirror` 统一内部 DTO 与 `repr(C)` POD 转换，字符串继续走有界 UTF-8
  缓冲区，不使用 JSON 作为参数或曲线 ABI。
- 新增参数/文本/动画测试，覆盖 Float 静态值、Text/StrCombo、线性插值、Bezier 控制点、
  upsert/remove、时间值插入、旧快照不变性及 undo/redo。

## 9. P3C1 验证与剩余范围

- 新增参数测试：2 个通过；新增命令同时纳入全变体 ABI 覆盖。
- engine / SDK 全量回归：engine 168 个、SDK 92 个测试通过。
- 按钮动作与独立通知订阅由 P3C2 接续；app 参数面板切换属于 P3D，因此 P3C1
  单独不构成完整 P3。

## 10. P3C2 实施结果

- 新增 `PushNodeButton`，复用 OFX 实例的 push-button action。内建节点、未知参数和非按钮
  参数返回错误；按钮动作不伪造 undo，成功后通过会话通知提示消费者刷新参数快照。
- 新增会话级 `subscribe_edit` / `EditSubscription`：每个成功语义命令通知一次并携带
  opcode/revision；领域错误不通知，结果缓冲区失败但命令已执行时仍通知。
- 通知在会话锁外执行，回调数据使用 Arc 延迟到 in-flight 调用结束后释放；Rust 回调互斥
  执行，panic 被隔离。订阅 Drop、显式 unsubscribe 和会话销毁均具有确定的清理语义。
- 新增通知测试，覆盖一次通知、失败抑制、Drop 注销和 panic 隔离；按钮变体覆盖原生 ABI
  与非插件错误路径，实际 OFX action 继续由现有插件测试覆盖。

## 11. P3C2 验证与 P3D 边界

- 新增通知测试：2 个通过。
- engine / SDK 全量回归：engine 168 个、SDK 94 个测试通过。
- P3A–P3C 的 engine/SDK 能力至此完成；P3 整体仍需 P3D 的 app 切换与 helper 删除，
  以及独立消费者的完整图/效果/参数/关键帧渲染验收。

## 12. P3D 独立消费者验收

新增 `editor::workflow` 集成测试，只通过公开 SDK 完成完整工作流：真实媒体导入、片段
放置、OFX 效果插入、参数关键帧、图/参数快照、按钮 action、CPU 渲染以及保存重开。

- 使用 CPU `org.oak.test-plugin`，避免把需要 GPU context 的 `colorcorrect` 误当作
  headless 渲染能力。
- 插件输出逐像素验证为 `(0.5, 0.5, 0.5, 1.0)`；`gain` 关键帧在 0s/1s 分别读回 0/1。
- `PushNodeButton` 成功路径通过；保存重开后关键帧轨道和渲染结果保持一致。
- engine / SDK 全量回归：engine 168 个、SDK 95 个测试通过。

独立验收证明新消费者无需 `oak-node` 即可组合 P3 能力。RealEngine/app 面板切换仍是
P3D 的剩余工作，必须继续保持同一工程实例，不以第二份工程或裸 Arc 桥接替代。

## 13. P3D app 切换与完成结果

- 新增 `oak_editor_adopt_storage_project_handle` 和显式 `repr(C)` `StorageProjectHandle`：迁移会话
  与旧 `ProjectRef` 持有同一个 Arc，句柄引用计数完整，不重开工程、不裸还原指针。
  app 通过既有 `graphops::storage_bind` 取得句柄，没有新增 `oak-storage` 直调。
- RealEngine 的效果插入、删除、重排、启停、按钮和文本/POD 参数写入全部走
  `EditorSession::execute`；inspector 使用 `ParameterSnapshot` 并保留 app i18n、Combo、
  字体和 multiline 展示规则。
- 节点编辑器使用 `GraphSnapshot` 和会话连接/断开/位置/删除命令。GPUI 卡片、颜色、端口
  打包与角色网格只留在 app；空 ID 流端口在 engine 快照中同步过滤。
- 旧效果插入/删除/移动/启停/参数写入 helper 已限制为测试兼容代码，不再构成生产路径；
  渲染描述和布局回退所需的只读链信息仍由对应 P5 路径接续。
- 独立 SDK 工作流继续验证真实 CPU 效果、关键帧、按钮和保存重开；app 的
  `engine_effect_and_node_graph_events` 证明新旧调用作用于同一工程图。

## 14. P3 完成验证

- engine：168 个测试通过；SDK：95 个测试通过。
- app：effectchain 20 个、nodegraph 8 个定向测试通过，`engine_effect_and_node_graph_events`
  通过。
- app 全量：720 个通过、1 个忽略、15 个失败。失败仍是既有 worker/共享内存环境路径，
  包括 `OAK_WORKER_BIN` 缺失和 `shm_open: Operation not permitted`；没有新增节点图、
  效果或参数失败。
- `python3 tooling/check_app_engine_boundary.py --update-baseline`：内部引用基线由 1182
  收紧至 1177，直接内部依赖数量未增加。

P3A–P3D 的出口均已满足：独立消费者不依赖 `oak-node` 即可建图、连图、加效果、编辑
参数和关键帧并渲染；app 的节点、效果与参数生产路径已经切换。
