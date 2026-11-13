# Quercus OFX 动态参数与动画补全计划

> 状态：待评审，尚未实施。  
> 范围：普通 OFX 参数关键帧、动态参数属性、曲线编辑、完整动画 Suite，
> 以及后续可选的音频驱动动画。mask、tracker、CUDA/Metal 和 DRX 互操作另立计划。

## 1. 当前底账

当前代码已经具有一条宿主侧关键帧渲染链路，不应再按“完全只支持静态参数”
重写一套动画模型：

- `quercus-node/src/keyframe.rs` 已实现 Hold、Linear、Bezier 和关键帧求值。
- `quercus-node/src/traverser.rs` 会将未连接输入按当前节点时间求值后放入参数行。
- `quercus-engine-rs/src/editor/workflow.rs` 已覆盖 OFX 关键帧渲染、保存和重开。
- `OfxParameterSuite` 仍是静态桩：`paramGetValueAtTime` 忽略时间，关键帧查询
  恒为空，删除和复制为空操作，导数和积分返回 `MissingHostFeature`。
- `OfxParamPropIsAnimating` 恒为 0；插件无法通过标准 Suite 查询或编辑普通参数动画。
- OFX worker 任务只携带逐帧参数值，插件在隔离进程中无法查询完整动画轨道。
- Inspector 只显示和写入标准值，尚未把现有关键帧快照、Keyable 和曲线编辑器
  接入普通 OFX 参数。

因此，本计划复用 `NodeCore::KeyframeTrack` 作为唯一事实源，主要工作是快照桥接、
OFX Suite、worker 协议、撤销事务和 UI 接线。

## 2. 目标与非目标

### 2.1 完成定义

1. 用户能为普通 OFX 数值、向量和颜色参数创建、编辑和删除关键帧。
2. 插件能查询任意时间的参数值、关键帧、导数和积分。
3. 插件能通过标准 OFX API 创建、删除和复制关键帧，且全部支持 Undo/Redo。
4. 插件动态修改的参数属性能同步到节点和 Inspector。
5. 主进程、隔离 worker、预览和导出在同一节点时间得到相同参数值。
6. Hold/Linear/Bezier 之外，后续支持 ease、loop、ping-pong、relative 和音频驱动。

### 2.2 非目标

- 不在 `ParamInstance` 中维护第二份可独立修改的关键帧轨道。
- 不把 OFX Parametric 曲线与普通时间关键帧混为同一种数据。
- 不让音频输出回调直接驱动渲染参数，避免预览与导出不确定。
- 不在本期承诺 mask、tracker 上下文、CUDA/Metal Suite 或 DRX 渲染一致性。

## 3. 总体设计

```text
NodeCore::KeyframeTrack（唯一事实源）
        ↓
不可变 ParamAnimationSnapshot（值、轨道、插值、手柄、revision）
        ↓
PluginJobPayload → JobSpec::Plugin → OFX IPC → RenderJob
        ↓
OfxParameterSuite 时间求值与关键帧查询

UI / 插件修改
        ↓
EditCommand + Undo/Redo
        ↓
NodeCore → 参数修订递增 → 缓存失效 → 重新渲染
```

关键约束：

- OFX 边界使用 `f64` 秒，节点内部继续使用 `Rational`；集中实现双向转换和
  关键帧精确查找，保证 `GetKeyTime → DeleteKey/SetValueAtTime` 往返稳定。
- 动画时间使用 `NodeTraverser` 传入的节点求值时间，UI 写入复用相同时间映射，
  不额外猜测 `media_in` 或播放时钟偏移。
- 每个 OFX 任务带稳定 `instance_key`；worker 以 `(type_id, instance_key)` 区分
  实例，防止相同插件类型的不同节点共享动画上下文。
- 动画快照随任务自包含，worker 崩溃并重发任务后不依赖先前的同步消息。
- 静态任务保留原快速路径，只有动画参数携带轨道快照。

## 4. 分阶段实施

### M0：冻结语义与补基线测试

- 制定参数类型能力矩阵：Double、Double2D/3D、Integer 族、RGB/RGBA、
  Bool/Choice、String 和 Parametric。
- 明确空轨道、单关键帧、首键前/末键后、精确关键帧时间、非法索引、NaN/Inf、
  反向积分和不支持类型的返回语义。
- 扩展现有 workflow 测试，在中间时间验证 Linear 和 Bezier，而不只验证关键帧端点。
- 新增 animated-probe OFX fixture，记录 Suite 查询、关键帧写入和 action 顺序。
- 保留现有静态参数测试，作为后续每阶段的回归门槛。

验收：能力矩阵和测试语义冻结，已有宿主动画与缺失的插件侧动画 API 被清楚区分。

### M1：只读动画快照与跨进程查询

- 新增拥有型 `ParamAnimationSnapshot`，包含标准值、关键帧、插值、Bezier 手柄和
  动画修订号。
- 将快照贯穿 `PluginJobPayload`、`JobSpec::Plugin`、OFX IPC 和 `RenderJob`。
- 为任务增加 `instance_key`，worker 按 `(type_id, instance_key)` 管理实例。
- 实现 `paramGetValueAtTime`、`paramGetNumKeys`、`paramGetKeyTime` 和
  `paramGetKeyIndex`。
- `IsAnimating` 根据轨道动态计算，`Animates` 控制参数是否允许动画。

验收：主进程和隔离 worker 对同一节点、同一时间的查询一致；两个同类 OFX
实例不会串值或串轨道。

### M2：可撤销的动画写入与 change 通知

- `paramSetValueAtTime` 映射到 `UpsertNodeKeyframe`。
- `paramDeleteKey`、`paramDeleteAllKeys`、`paramCopy` 使用原子 EditCommand，
  完整保存旧轨道以支持 Undo/Redo。
- 插件调用 `paramSetValue` 时，动画参数写当前时间关键帧，静态参数写标准值；
  统一复用 `SetNodeValueAtTime` 语义。
- 复用 `paramEditBegin/End`，一次交互只产生一条撤销记录。
- 参数同步后，在 clip preferences、RoI、isIdentity 和 render 前发送正确的
  `UserEdited`/`TimeChanged` 通知。
- 增加重入保护，防止插件写回和 `instanceChanged` 相互递归。
- 不可编辑的 worker 上下文收到非法写入时返回明确 OFX 错误，不伪装成功。

验收：插件可创建、删除和复制动画，所有操作可撤销，保存重开后保持一致。

### M3：Inspector 关键帧与曲线 UX

- 扩展 `EffectParam`，携带当前时间求值结果、关键帧轨道、`can_animate`、
  `is_animating` 和 `enabled`。
- 新增批量 `parameters_at_time(node, time)` 快照，避免每个控件分别锁工程求值。
- 将现有 Keyable 菱形接入 scalar、vector 和 color 控件。
- 首次启用动画时在当前时间记录当前值；动画参数后续编辑始终走
  `SetNodeValueAtTime`。
- AutoKey 只负责首次编辑自动启用动画，避免关闭 AutoKey 后修改不可见的标准值。
- 复用曲线编辑器编辑普通时间关键帧，支持添加/删除、前后关键帧跳转、
  Hold/Linear/Bezier、ease-in/out 和手柄编辑。
- vector/color 初版仍是一整条值轨道；分量曲线只是投影，编辑时原子写回整个值。
- 参数拖动继续合并为一条 Undo，播放时控件显示当前时间的求值结果。

验收：用户无需 SDK 即可完成 OFX 动画制作，UI、预览和导出显示相同动画。

### M4：动态参数属性与完整 Suite

- 插件修改 Secret、Enabled、Label、Hint、Min/Max 和 Choice 选项后，增量同步
  NodeCore 元数据，不重建整个节点，不丢失值、轨道、连接和节点身份。
- 元数据和参数值变化合并为一次事务，通知 Inspector 刷新但不破坏进行中的拖动。
- 实现 Double/Color 的 `paramGetDerivative` 和 `paramGetIntegral`：
  Hold/Linear 使用解析结果，Bezier 使用确定性的曲线导数和固定精度积分。
- 完整实现 `paramCopy` 的类型检查、时间偏移、范围筛选和目标轨道替换。

验收：动态隐藏、启用和选项变化及时反映到 UI；Suite 不再以静态桩或静默
空操作返回成功。

### M5：高级动画与可选音频驱动

- ease 作为 Bezier 预设，不新增插值类型。
- 为轨道增加可序列化循环行为：loop、ping-pong、relative，并定义首键前和
  末键后的独立外推规则。
- 多片段同调使用批量命令，按各节点自己的时间映射写入，不直接复制绝对时间。
- 可选增加 `AudioDriveBinding`：音频源、声道、RMS/Peak、增益、映射范围、
  平滑、时间偏移及与基础关键帧的合成方式。
- 音频包络离线解码并按时间索引缓存，保证随机 seek 和导出可重复。

验收：循环动画无需烘焙大量关键帧；同一音频驱动配置在预览和导出中生成相同
参数序列。

### M6：兼容性与性能收尾

- 覆盖主进程/worker、CPU/可用 GL、顺序播放/随机 seek/导出和 worker 崩溃重试。
- 覆盖 23.976/29.97/59.94 fps、子帧、负时间、单关键帧、空轨道和非法索引。
- 至少验证两个带动画或动态 UI 的第三方 OFX；DRX 仅作独立探针，不作验收依赖。
- 动画快照按 revision 复用，渲染线程不长期持项目锁，静态任务不增加快照开销。
- 更新 OFX 能力文档及旧报告中的“静态层”结论。

## 5. 音频播放设备的边界

`quercus-audio/src/previewdevice.rs` 负责输出缓冲、播放时钟和 underrun 重同步。
它只应间接决定当前显示哪一帧，不应成为 OFX 参数的数据源或求值驱动器。

OFX 参数必须保持 `parameter = f(node_time)` 的纯函数语义。音频驱动动画从离线
音频数据计算，不能读取会被实时消费、清空或 zero-fill 的播放缓冲，否则预览、
随机 seek 和导出会产生不同结果。

## 6. 总体验收矩阵

- 功能：UI 和插件双向创建/编辑/删除/复制关键帧，Undo/Redo、保存和重开一致。
- 渲染：主进程与 worker、预览与导出在相同时间逐像素一致。
- 隔离：两个相同类型、不同动画轨道的 OFX 节点互不影响。
- 回归：所有原静态 OFX 测试保持通过，非法调用返回明确错误。
- 性能：静态任务零额外轨道负载，动画快照不重复序列化，拖动无全节点重建。
- 实机：至少两个第三方动画 OFX 通过兼容性验证，失败项形成独立能力缺口记录。

推荐先交付 M0–M3 的最小可用闭环，再推进 M4–M6，避免同时改动 Suite、UI、
高级动画和第三方插件兼容而难以定位问题。
