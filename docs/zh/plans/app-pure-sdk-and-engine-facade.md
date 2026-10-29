# oak-app 纯 SDK 化与引擎门面补全实施计划

> 状态：P0–P8 已完成
> 现状核验：`ed049df68f`（2026-10-25）；P1–P7 收口于 2026-10-30
> 目标：将 Oak 的领域能力完整收口到 `oak-engine`，由
> `oak-engine-rust-sdk` 提供安全、稳定的 Rust API，使第三方只依赖这两个
> Oak crate 就能编写一个功能完整的新视频剪辑软件。

## 1. 目标与边界

### 1.1 终态

依赖方向固定为：

```text
oak-app / 新剪辑软件 / 其他外部消费者
                  │
                  ▼
       oak-engine-rust-sdk
       安全 Rust 类型、RAII、错误、DTO、事件
                  │
                  ▼
            oak-engine
       C ABI 门面 + 领域用例编排
                  │
                  ▼
oak-core / oak-node / oak-timeline / oak-undo / oak-render
oak-codec / oak-audio / oak-task / oak-storage / oak-ofx-plugin / ...
```

必须同时满足：

1. `oak-app` 的 Cargo 直接 Oak 依赖只剩 `oak-engine-rust-sdk`；不再直接依赖
   `oak-audio`、`oak-codec`、`oak-core`、`oak-node`、`oak-ofx-plugin`、
   `oak-render`、`oak-storage`、`oak-task`、`oak-timeline`、`oak-undo`。
2. `oak-app` 源码和测试不再出现上述内部 crate 的类型、函数、常量或私有路径；
   不通过本地 `extern "C"`、动态找符号或复制引擎实现绕过 SDK。
3. 工程、时间线、节点效果、代理、多机位、播放、渲染、导出、历史记录等能力，
   均有真实的引擎实现和对应的 SDK 包装，而不是 app 私有能力。
4. 新增独立示例消费者，不复用 `oak-app` 的领域 helper，也不补调内部 crate，
   即可完成一条完整剪辑工作流。

这里的“只依赖两个 Oak crate”不禁止消费者使用 UI、窗口、图像、日志等第三方
库。`oak-engine` 由 SDK 传递引入和链接，应用不需要直接引用它的 Rust API。

### 1.2 留在 app 的内容

以下内容不是引擎能力，不应为追求“零依赖”而搬入引擎：

- gpui 窗口、面板、菜单、快捷键、拖放和对话框；
- `Frame`、`ClipData`、节点图布局等 widget 视图模型；
- 当前选择、悬停、拖拽预览、播放头显示等交互状态；
- `wgpu` 纹理上传、GPUI 图片创建和窗口合成；
- 时间码格式化、纯 transport 状态机等不访问内部 crate 的 UI 逻辑；
- MockEngine 的演示数据，但它必须消费与 RealEngine 相同的 app 本地 DTO。

### 1.3 非目标

- 不重写 UI，不改变现有交互和 `.ove` 文件格式。
- 不要求 `oak-worker` 改用 SDK；它是引擎渲染拓扑的内部实现进程。
- 不在本计划中迁移 `oak-cli`，但其现有直调不得成为新增 app 绕行的理由。
- 不把 SDK 变成 gpui 专用 API，不在引擎或 SDK 公共契约中暴露 gpui/wgpu 类型。
- **命名约定**：SDK 最终命名为 **Quercus**；待 P0–P8 全部完成后，再一次性批量修改 crate、包名、文档及示例，迁移期间沿用 `oak-engine-rust-sdk`。

## 2. 现状与主要缺口

（迁移前现状，记录用）`oak-app/Cargo.toml` 曾直接依赖 10 个 Oak 内部 crate；
P7 完成后普通依赖只剩 `oak-engine-rust-sdk`，边界守卫为 0 引用 / 0 内部依赖。
`RealEngine` 明确采用“直接驱动模块 crate、无 C ABI”的 M14 R3 路线。直接引用
分布在 25 个源码文件及 `waveform_e2e.rs`，不是只改 `real.rs` 就能完成。

| 领域 | 当前 app 实现 | 已有 SDK 基础 | 需要补齐的核心能力 |
|---|---|---|---|
| 工程与库 | `graphops.rs`、`projectbrowser.rs`、`real.rs` | `storage`、`node::Project` | write-through 绑定、库统计、复制/导入/导出、完整工程快照与当前序列管理 |
| 时间线与撤销 | `graphops.rs` 约 7,200 行，包含编辑组合与大量测试 | 基础 block/track/transition 与 undo 调用 | 链接编辑、剪贴板、嵌套、标记、工作区、调整层、转场、全部 trim/roll/slide/slip 组合语义 |
| 节点与效果 | `nodegraph.rs`、`effectchain.rs`、参数面板 | `node`、`effect`、`ofx` 基础 | 图快照、连接校验、效果链操作、完整参数元数据、关键帧/曲线、按钮和上下文 |
| 素材与代理 | `graphops.rs`、`real.rs`、`waveform.rs` | 解码、探测、部分 proxy 入口 | 导入/relink/替换、footage 参数、代理任务生命周期、波形数据与缓存状态 |
| 渲染与播放 | `renderops.rs` 约 3,800 行、`audio_thread.rs`、`real.rs` | 同步 `render_frame_f32`、render manager、音频设备 | montage、异步帧、帧所有权、音频块调度、双监视器播放、缓存、代理预览、示波器输入 |
| 任务与互操作 | app 后台线程和 channel | `task::export`、OTIO/FCPXML、任务轮询 | 与 app 一致的进度/取消/错误事件，任务与工程生命周期绑定 |
| 多机位与同步 | `multicam.rs`、`waveformsync.rs` | 音频分析和底层波形同步零件 | 多机位状态/建序列/切角度，按源时间和波形同步的组合用例 |
| 配置、色彩与文本 | `dialogs.rs`、`displaycolor.rs`、`gpu.rs`、`textengine.rs` | `state`、`color`、text 安装入口 | app 使用的全部配置项、显示 LUT/ICC 所需 DTO、字体枚举与统一初始化 |
| OFX 交互 | `ofx.rs`、OFX 参数与 overlay | 扫描、注册、渲染 executor、进度回调 | active viewer、参数交互、overlay 命令/绘制数据、进度取消和插件生命周期 |

现有 SDK 不是空壳，应优先复用已有 API。历史上的 C++ facade 迁移文档也不能
直接视为本计划已完成：当前 Rust app 的领域组合代码是在其后重新形成的。

## 3. 目标 API 与代码组织

### 3.1 两层门面，不做逐函数机械翻译

- **引擎 C ABI 层**负责稳定边界、所有权、错误和真实领域操作；建议新增
  `oak_editor_*`、`oak_preview_*`、`oak_media_*` 等族，已有 `oak_node_*`、
  `oak_storage_*`、`oak_task_*` 等继续复用。
- **Rust SDK 层**提供面向消费者的 `Engine`/`EditorSession`、查询快照、命令、
  `PreviewSession` 和任务句柄。SDK 内部组合 C 调用，应用不得重新实现这些组合。
- 底层 node/timeline API 可以保留给高级消费者，但完整编辑器不应被迫直接
  拼装 `Graph`、`NodeRef`、montage 或 undo command。

建议按领域新增或扩充：

```text
crates/oak-engine/src/
  editor.rs       # 工程会话、查询、时间线/节点组合命令
  preview.rs      # 预览帧、音频块、播放与渲染调度
  media.rs        # 素材、代理、波形、同步、多机位
crates/oak-engine-rust-sdk/src/
  editor.rs       # EditorSession、DTO、命令与事件的 RAII 包装
  preview.rs      # PreviewSession、Frame/AudioBuffer、订阅
  media.rs        # 上述媒体能力的安全包装
crates/oak-app/src/oakui/
  real.rs         # 只保留 SDK ↔ AppEngine/widget 的适配
  mock.rs         # 使用同一 app DTO，不含内部 crate 类型
```

文件名可按实际规模拆成目录，但不得把迁移后的领域实现继续留在 `oakui`。

### 3.2 契约规则

1. C ABI 只传不透明句柄、稳定整数 ID、`repr(C)` 数据、字节/数值缓冲和回调；
   Rust 内部类型、trait object、`Arc<Mutex<_>>` 不得越界。
2. SDK DTO 自持数据并明确所有权。帧、音频、字符串和列表不得引用已释放的
   引擎内存；帧句柄必须定义释放、克隆及跨线程规则。
3. 时间统一使用有理秒或明确的时间基；跨轨索引沿用现有显示顺序，转换只在
   引擎边界完成。错误使用稳定错误码加可读消息，禁止以 `false` 掩盖所有原因。
4. 一个用户动作产生一个历史项。SDK 的组合命令在引擎内部建立 undo group；
   异常路径 abort，不能在 app 中逐条拼撤销记录。
5. 查询返回不可变快照或带 revision 的 DTO；通知只提示变化，不让 UI 在回调中
   重入引擎。订阅必须可注销，drop 后不得保留消费者裸指针。
6. 保留当前全局 undo 的既有行为作为迁移基线；是否改为按会话隔离另行设计，
   不与本次搬迁同时改变。
7. 引擎初始化、渲染器、OFX、文本后端和关闭顺序由 SDK 会话明确管理；无插件、
   无音频设备、无 worker 的环境要可诊断地降级，不能静默假成功。

### 3.3 app 适配层的允许职责

迁移后的 `RealEngine` 只做四件事：

1. 将 widget 事件转换为 SDK 命令参数；
2. 将 SDK DTO 转换为 widget DTO、`RenderImage` 或 wgpu 纹理；
3. 将 SDK 任务/预览事件 marshal 到 gpui 所在线程；
4. 保存纯 UI 状态并触发视图刷新。

它不得再做图遍历、命令构造、montage 生成、代理决策、波形同步计算或直接
管理内部 render ticket。纯 UI 的 scope 像素统计、颜色控件换算和 LUT 上传可以
留下，但其引擎输入必须来自 SDK DTO。

## 4. 分阶段实施

每个阶段按“引擎实现 + C ABI 测试 → SDK 包装 + SDK 集成测试 → app 切换 →
删除旧直调”的顺序提交。禁止先留下双实现长期并行；确需临时共存时，以同一组
行为测试约束并登记待删除文件。

### P0：建立契约、调用台账和行为基线（已完成）

实施台账与结果见 [p0-app-engine-call-ledger.md](p0-app-engine-call-ledger.md)。

- 按 `AppEngine` 的方法建立“UI 操作 → 当前 helper → 内部 crate → SDK API”台账，
  覆盖正常路径、默认实现、测试 helper 和 `dialogs.rs`/`actions.rs` 等外围直调。
- 从 `oakui/engine.rs` 和 widget data-source 中移除内部 crate 类型，先统一使用
  app 本地 DTO；MockEngine 同步切换，禁止用内部类型做空实现。
- 固定行为基线：示例工程、时间线编辑与 undo、工程库、代理、多机位、导出
  参数、预览黄金帧、音频采样数、OFX 参数往返。
- 增加依赖守卫和内部路径扫描。迁移期使用只允许减少的清单，终态改为零容忍；
  同时检查 `[dependencies]` 与 `[dev-dependencies]`，防止测试重新引入直调。

**出口**：所有直调均有归属阶段和目标 API；基线测试可重复运行；Mock/NullEngine
不再成为隐藏依赖来源。

### P1：工程会话、配置、存储和只读快照（已完成）

实施记录见 [p1-editor-session-implementation.md](p1-editor-session-implementation.md)。

> 顺序修正（2026-10-26）：RealEngine 在 P2 前仍直接持有并编辑
> `Arc<Mutex<Project>>`，write-through 又必须绑定同一实例。为避免双工程或
> 裸指针桥接，P1 先交付独立会话与可独立切换的能力；app 的实时工程采用、
> 剩余存储绑定和图只读清零与 P2 原子完成，最终边界不变。

- 在 engine 新增 `EditorSession`：工程 new/open/save/close、当前序列、
  write-through 绑定及 marker/workarea 辅助状态共享同一工程实例。
- 复用并扩充 SDK `storage`/`state`，补 write-through、库统计、创建、复制、
  导入导出和库内打开；不依赖当前 app 工程实例的库操作切到 SDK。
- 提供 project/sequence/track/clip/footage/marker/workarea 的拥有型 DTO 快照，
  带 revision；用保存前与重开后的结构对比验证。
- 将全局配置、快捷键配置目录、效果分组配置和文本初始化/字体枚举接到 SDK；
  工程实例内的 OCIO/cache/color 设置先提供 SDK API，app 切换随 P2 完成。

**出口**：新消费者能创建、打开、浏览、保存工程并读出完整时间线，项目库
全生命周期通过；app 的全局配置及非实时库操作走 SDK。实时工程仍保留的
`oak-storage` 绑定和 `oak-node` 只读路径明确转入 P2，不宣称提前清零。

### P2A：语义编辑与原生 ABI（已完成）

实施记录见 [p2a-semantic-editing.md](p2a-semantic-editing.md)。

- 将 `graphops.rs` 的编辑实现和测试迁入 engine，提供强类型 `EditCommand` /
  `EditResult` 和原生 `repr(C)` ABI，不使用 JSON。
- 覆盖轨道、放置、转场、全部修剪、移动、波纹删除、链接、调整层、标记、工作区、
  剪贴板、嵌套和速度；每个命令只执行一次并返回 revision。
- 以迁入测试、全变体 ABI 测试和 undo/redo/保存重开验证。

### P2B：RealEngine 采用、历史与依赖收口（已完成：历史统一 + P7B 依赖收口）

本轮先在 P3D 发现 engine dylib 与 app rlib 双 undo 栈的问题，随后统一为 engine
历史栈：graphops 的遗留 `UndoCommand` 通过迁移期 owned 转移入口写入同一栈，
RealEngine 的 undo/redo/history/jump/group 全部改走 SDK。P3D 必需的历史语义已验证，
`oak-timeline` / `oak-undo` 类型与其他内部依赖的完全清零由 P7B 完成：app 的
`ProjectRef` 改为 SDK `Project` 句柄，`graphops`/`real` 的图与时间线适配只走
SDK 查询与命令，10 个内部 crate 依赖全部删除。

- RealEngine 采用 P1 的 `EditorSession`，一次性切换工程实例、write-through
  绑定和项目级 OCIO/cache/color 设置；删除剩余 `oak-storage` 直调，并用会话
  快照替换实时工程只读图遍历，避免维护第二份工程。
- 将 `graphops.rs` 的编辑实现和测试迁入引擎：轨道、放置、分割、全部修剪、
  移动、波纹删除、链接、转场、调整层、标记、工作区、剪贴板、嵌套和速度。
- SDK 提供语义级命令；原子完成“计算目标 + 修改图 + undo 分组 + 失效预览”，
  不暴露 `NodeRef` 或内部 command。
- 接通历史条目、jump、change notification、参数拖动分组和异常回滚。
- 切换时间线、历史面板、actions 和 `RealEngine` 编辑方法，删除 app 旧实现。

**出口**：独立 SDK 测试可完成粗剪和精剪；每组编辑逐项验证 redo 后状态、undo
后完全还原、保存重开一致；app 不再直接依赖 `oak-timeline`/`oak-undo`。

### P3：节点图、效果、关键帧与参数体系（已完成）

实施记录见 [p3-node-graph-and-effects.md](p3-node-graph-and-effects.md)。P3A–P3C 完成
engine/SDK 能力，P3D 完成独立消费者验收和 app 的节点/效果/参数生产路径切换。
迁移会话与旧 `ProjectRef` 通过引用计数句柄共享同一 Arc，历史命令也统一到同一栈。

- 将 `nodegraph.rs` 的图查询/连接规则与 `effectchain.rs` 的链操作迁入引擎。
- 用 SDK DTO 表达节点、端口、连接、效果顺序、参数描述、值、枚举选项、
  分组、关键帧和曲线；支持按钮、上下文、嵌套图和受保护节点。
- 将一次参数修改、曲线编辑和批量效果操作纳入相同 undo/通知机制。
- 切换节点编辑器、效果库、效果栈和 inspector/OFX 参数面板。

**出口**：独立消费者不依赖 `oak-node` 即可建图、连图、加效果、编辑参数和
关键帧并渲染验证；app 的节点与效果领域 helper 已删除。

### P4：素材、代理、波形、同步与多机位（已完成）

实施记录见 [p4-media-and-multicam.md](p4-media-and-multicam.md)。

- 完成素材导入、替换/relink、参数覆盖、素材与 sequence 互转及项目浏览器写操作。
- 将代理生成/删除/启停/自定义参数与任务进度封装为 SDK 会话能力，不返回
  `oak-codec` 参数类型。
- 提供缩略图和波形 DTO/异步任务；将同步 eligibility、偏移估算、多机位建序列、
  角度切换及关联撤销迁入引擎。纯波形窗口聚合可留 UI，但不得调用内部算法 crate。
- 用真实媒体覆盖缺帧、非整数帧率、链接音视频、代理切换和波形对齐。

**出口**：独立消费者能从媒体导入走到代理、多机位和同步；app 不再直接依赖
`oak-codec`/`oak-audio` 的媒体与同步 API。

### P5：渲染、异步预览、播放与导出（已完成）

- 将 montage、render ticket、进程池帧句柄和音频块调度迁入引擎；保留现有
  M15/M16 线程与 worker 拓扑，不能为了封装改成 UI 同步渲染。
- SDK 提供 source/program 两个 `PreviewSession`：seek/play/pause/step/rate、
  异步帧、音频块、时钟、电平、缓存失效和任务取消。
- 明确 CPU frame、共享内存 frame 和拥有所有权的 buffer 三种路径；第一版可以
  在 SDK 边界复制为拥有所有权的帧，后续再优化零拷贝，但语义与生命周期必须先正确。
- 将导出进度、取消、错误和 OTIO/FCPXML 接到统一任务 API；保留格式、色彩、
  自定义范围和码率参数。
- 切换 `real.rs`、`renderops.rs`、`audio_thread.rs`、viewer、导出对话框和 scopes
  输入；`gpu.rs` 仅保留 SDK 像素/LUT 到 wgpu 的显示代码。

**出口**：独立消费者能连续播放、暂停、逐帧、渲染音频和导出；现有黄金帧、
音频长度、取消、worker 故障隔离和非 UI 线程渲染测试保持通过。

### P6：OFX 与高级交互收尾（已完成）

- 把 OFX 初始化、active viewer、project extent、进度与取消统一到 SDK 生命周期。
- 为 interact 定义与 UI 无关的指针/键盘/视口命令及 overlay 绘制数据；确需原生
  GPU/窗口句柄时，使用单独、可空、平台明确的 SDK display 接口，不能泄漏
  `oak-ofx-plugin` 类型。
- 迁移 `ofx.rs`、OFX 参数控件和 overlay；验证无插件环境、重复初始化、插件
  崩溃/取消、进度事件注销与进程关闭顺序。

**出口**：不直接依赖 `oak-ofx-plugin` 的消费者可发现和使用插件效果；交互插件
能力有明确降级，不再有 app 私有注册或回调。

### P7：oak-app 依赖清零与文档收口（已完成）

- 删除已经失去调用者的 app 领域模块与重复 RAII helper；保留必要的 UI 单测，
  将领域测试迁到真正拥有实现的 engine/SDK。
- 从 `oak-app/Cargo.toml` 删除全部内部 crate 依赖，加入
  `oak-engine-rust-sdk`；普通依赖和开发依赖均清零。
- 更新 `oak-app`、`oakui` 中仍宣称“直调模块、无 C ABI”的文档与注释。
- 启用零容忍 CI：内部 crate 路径扫描、Cargo 直接依赖检查、workspace 全量构建
  与测试、三平台动态链接/启动冒烟。

**出口**：`oak-app` 是纯粹的 SDK 消费者；删除所有允许清单和临时兼容层后仍全绿。

### P8：用独立新消费者证明“可重建”（已完成）

新增 `examples/sdk_editor`（可使用任意 UI 工具包，但 Oak 依赖只有
`oak-engine-rust-sdk`），禁止依赖 `oak-app`。至少完成：

1. 初始化引擎、色彩、文本、音频和插件能力；
2. 新建/打开工程，导入素材并浏览工程树；
3. 创建序列与轨道，放置、分割、修剪、移动、波纹删除片段；
4. 添加效果和关键帧，修改参数；
5. 预览 source/program、播放音频、显示时间线和电平；
6. undo/redo、保存并重新打开，验证状态恢复；
7. 导出视频，轮询进度并取消/完成任务。

示例先以可测试的 headless 主流程验收，再提供最小 GUI shell。GUI 只需证明相同
SDK 会话能驱动另一个前端，不要求复制 Oak 的全部视觉设计。

**出口**：该示例在干净构建目录中独立构建和运行；其 Cargo 元数据与源码扫描
证明没有其他 Oak 内部依赖，工作流测试与黄金帧通过。

## 5. 测试与防绕行要求

### 5.1 每批必跑

```bash
cargo test -p oak-engine
cargo test -p oak-engine-rust-sdk
cargo test -p oak-app
cargo test --workspace
```

按改动追加真实 worker 渲染、导出、音频设备可用性降级、OFX 和三平台冒烟。
环境不具备某项设备时允许跳过设备实测，但初始化和错误路径仍须执行并明确报告。

### 5.2 终态自动检查

```bash
# 结果应为空；文档注释中的历史名称也应清理或改为明确说明。
rg 'oak_(audio|codec|core|node|ofx_plugin|render|storage|task|timeline|undo|otio)::' \
  crates/oak-app

# oak-app 的第一级依赖中，Oak 内部项只能有 oak-engine-rust-sdk。
cargo tree -p oak-app --depth 1

# 新消费者也不得借用 oak-app 或其他内部 crate。
cargo tree -p sdk_editor --depth 1
cargo test -p sdk_editor
```

依赖检查以 Cargo 元数据为准，文本扫描用于防止类型/路径绕行。还要检查符号：
app 和独立消费者不得未定义引用内部模块符号，所有 Oak 引擎符号只从
`oak-engine` 的公开 ABI 进入。

### 5.3 禁止的伪完成

- 在 app 内增加同名 wrapper，但 wrapper 仍直调内部 crate；
- 为通过扫描把 `use oak_node` 改成相对路径、宏拼接或动态符号解析；
- 把 `Graph`、command、montage 等实现复制到 SDK，使引擎成为空转发；
- 暴露原始指针让 app 自己还原内部对象，或新增只返回成功的空操作；
- 只迁移 normal dependencies，却在 dev-dependencies/tests 保留内部引擎调用；
- 用只覆盖 load/import/split/render 的小 demo 宣称具备完整重建能力。

## 6. 风险与控制

| 风险 | 控制方式 |
|---|---|
| 搬迁时破坏精细编辑和撤销语义 | 连实现与测试一起迁；命令级快照断言、undo/redo 和保存重开三重验证 |
| 同步 SDK 封装造成播放卡顿 | 预览/音频/导出从设计上就是异步；保留 render worker 和专用音频调度 |
| C ABI 所有权或回调泄漏 | SDK RAII、可注销订阅、drop/关闭顺序测试；禁止消费者裸指针常驻 |
| 为迁就 gpui 污染通用 API | SDK 只暴露 DTO、句柄和通用事件；GPUI 类型只允许出现在 oak-app |
| 查询快照带来过多复制 | 先保证正确，再按 profiling 引入 revision、增量快照和帧句柄，不牺牲边界 |
| 阶段推进后旧路径反复回潮 | 每阶段删除旧实现；依赖清单只减不增，P7 后零容忍 |
| 只做 app 所需的最小包装，第三方仍需补领域逻辑 | 以 `EditorSession` 组合用例和 P8 独立消费者验收，而非以 app 编译通过验收 |

## 7. 完成定义

只有以下条件全部满足，本计划才可归档到 `completed/`：

- [x] `oak-app` 普通及开发依赖中的 Oak 引擎项只剩 `oak-engine-rust-sdk`。
- [x] app 源码/测试无内部 crate 引用、私有 FFI、动态符号绕行和领域实现副本。
- [x] P1–P6 的每类能力都有 engine 实现、C ABI 测试和 SDK 集成测试。
- [x] 现有 UI、编辑、历史、预览、音频、导出及工程兼容性测试无回归。
- [x] `examples/sdk_editor` 不依赖 `oak-app` 或其他 Oak 内部 crate，完整工作流通过。
- [x] SDK 有面向新前端的生命周期、线程、错误、所有权和任务/事件使用文档。
- [ ] 三平台 CI 构建、链接和冒烟通过，依赖与源码守卫持续启用。
