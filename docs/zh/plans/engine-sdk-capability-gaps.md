# 引擎 SDK（oak-engine-rust-sdk）能力缺口分析

本文对照 **oak-app**（进程内直链 oak-\* 模块 crate 的旧 app）实际使用的 API 与
**oak-engine-rust-sdk**（`oak_engine` cdylib 的 C-ABI 客户端）现有封装，列出全部没有
C wrapper 的能力缺口。背景：`oak-app-new` 以「只用 gpui-kit + oak-engine-rust-sdk」为
目标重写，缺口决定各面板何时能落地。

## 方法

三层对照：

1. **oak-app 使用面**：`../../../crates/oak-app/src` 中对 `oak_node` / `oak_core` / `oak_undo` /
   `oak_timeline` / `oak_render` / `oak_codec` / `oak_task` / `oak_storage` /
   `oak_audio` / `oak_ofx_plugin` 的全部引用；
2. **引擎导出面**：`liboak_engine.so` 的真实符号表（`nm -D`，619 个 `oak_*` 符号，
   含宏生成的导出）；
3. **SDK 包装面**：`../../../crates/oak-engine-rust-sdk/src` 声明的 508 个 extern 函数。

## 一、SDK 包装层本身的漏洞（小）

**SDK 声明但 dylib 未导出**（2 个，已核实非缺陷）：

- `oak_core_displayicc_windows_acm_active`
- `oak_core_displayicc_windows_monitor_fingerprint`

两者是 Windows 专用导出。两侧的对称 cfg 门控保证了配对：引擎在
`corestate.rs` 用 `#[cfg(target_os = "windows")]` 导出，SDK 在 `state.rs` 用同样的
cfg 声明 extern 并包装安全函数 —— 每个平台只声明该平台存在的符号。符号表 diff 出现的
「缺失」只是 Linux 构建的自然结果。SDK 的 `displayicc_queries_never_crash` 测试会调用
当前平台的门控函数，平台配对由链接器在 CI 上强制校验。

**dylib 导出但 SDK 未包裹**：

- `oak_audio_params_rational_from_double` —— 唯一有实际用途的遗漏；
- `oak_ofx_message_impl` —— OFX 消息实现（可能刻意内部化）；
- `oak_vec_*` 全家（111 个原始 vector 操作）—— 引擎内部 plumbing，SDK 以安全
  包装替代，属合理不导。

结论：**SDK 对现有 C ABI 的覆盖接近完整**，真正的问题在 C ABI 本身。

## 二、C ABI 的缺口（oak-app 在用、dylib 完全没有导出）

### 效果系统（effect_library / inspector 面板卡在这里）

- **节点工厂枚举**：`oak_node::factory::Factory::global()` —— 内置效果的
  type id / 名称 / 分类表，效果库的数据源，无导出。
- **效果链全部操作**：效果链遍历、插入、移除、移动、启用/禁用
  （`clip_input::TEXTURE_INPUT`、`effect_input` 机制）。注意：退役的
  `oakengine.bk` facade 曾有一整套 `oakengine_node_effect_*`（chain walk、insert、
  remove、move、set_enabled），新的 `oak-engine` 未继承。
- **OFX 插件节点入图**：`PluginNode::new` / `PluginInstanceHandle` —— C 侧只能创建
  游离的 host instance（`oak_ofx_instance_create`），无法把插件实例挂进项目节点图。
- 节点元数据：`Category`、`DONT_SHOW_IN_CREATE_MENU`、`input::flags::HIDDEN`
  （建菜单与检查器过滤用）。

### 播放管线（program/source viewer 卡在这里）

- **音频采样渲染**：`oak_render::eval::render_audio_samples` —— 播放声音来源，无导出。
- **流式帧渲染**：`render_montage_frame_into` / `generate_frame` / `render_graph_frame`
  及 ticket/预取体系（`MontageClip` / `MontageEffect` / `ShmFrameRef`）—— C 侧只有
  单帧 `oak_render_frame_f32`（CPU F32 拷贝）。
- **RenderManager 播放控制**：播放/暂停/定位/循环 —— C 侧只有
  init / shutdown / is_initialized。
- **GPU 上下文共享**：`oak_core::backend::GpuContext::{shared, adopt, install_shared}`
  —— 进程内设计里 UI 与引擎共享 GPU（viewer 零拷贝）；过 C ABI 帧只能以 CPU 缓冲
  回传，无 GPU 互通路径，viewer 性能属架构性受损。

### 时间线功能（timeline / multicam 面板缺口）

- **标记（markers）**：`MarkerAddCommand` / `MarkerRemoveCommand` /
  `TimelineMarkerList` —— 序列标记增删查，全部无导出。
- **多机位**：`multicam_enable` / `multicam_disable` / `multicam_switch` /
  `clip_find_multicam` / `MultiCamNode` —— 全部无导出。
- **工作区**：`TimelineWorkArea`（序列 in/out 工作区）无导出。
- 轨道高度换算 `internal_height_to_pixel_height` 等工具函数无导出。

### 音频工具（波形 / 同步）

- **波形提取**：`oak_audio::waveform::extract` —— 时间线/多机位波形显示的数据源，
  无导出。
- **波形同步对位**：`synchronizer::place_by_waveform_offset`、`waveformsync` ——
  多机位音频对齐，无导出。

### 任务系统（状态栏 / 任务列表缺口）

- **任务枚举**：列出运行中任务（TaskWidget/状态栏）无导出。
- **任务事件订阅**：`TaskEvent::Started/Progress/Finished` 推送 —— C 侧只能轮询
  `oak_task_state` / `oak_task_progress`。
- 代理并发配置（`proxy_max_concurrent` / `transcode_thread_budget`）无导出。

### 存储（write-through 库）

- `writethrough::{bind_project, unbind_project, is_bound, flush_all, last_error,
  library_uri, storage_enabled}` —— 项目自动落库绑定，全部无导出（C 侧只有一次性
  open/save 与库管理）。

### 撤销系统（架构性限制）

- **客户端自定义撤销命令不可能**：`oak_undo::global::push` +
  `UndoCommand::from_closures` / `multi` 无导出 —— oak-app 中标记、多机位标签等
  都是 app 侧构造撤销命令；过 C ABI 后每个可撤销操作必须是引擎内建调用（时间线
  编辑已覆盖；上述缺口操作则连操作带撤销一起缺）。

### 变更通知（跨领域架构缺口）

- 引擎状态变化（项目修改、节点值变化、代理完成、任务进度）**没有通用 observer /
  回调机制** —— 只有 `oak_undo_add_observer` 一个孤例。响应式 UI 只能轮询。

### 琐碎遗漏

- `project::SETTING_OUTPUT_GAMUT` 等设置 key 常量、`ENABLED_INPUT` 输入 id 常量；
- OFX `KEY_*` 键码常量（interact key_down 需要）；
- `param_curve::Curve` 编辑模型（C 侧只有 JSON 进出）；
- `testmedia::write_test_clip*`（测试工具，无妨）。

## 三、对 oak-app-new 重写路线的映射

| 面板 | 卡点 | 需要的 C ABI 扩展 |
|---|---|---|
| effect_library / inspector | 无工厂表、无效果链 | 移植 `oakengine.bk` 的 `node_effect_*` + `factory_*` |
| program / source viewer | 只能抓单帧、无声 | `render_audio_samples` + 播放控制 + 流式帧接口 |
| timeline | 块编辑已可用 | markers + workarea + 波形数据 |
| multicam | 整体缺失 | multicam 一组 + 波形同步 |
| history | SDK 已够用 | — |
| project_explorer | 节点树遍历已够 | — |
| status_bar / 任务 | 只能轮询 | 任务枚举 + 事件订阅 |

## 四、优先级建议

1. **效果链**：把 `oakengine.bk` 退役的导出移植回来，历史包袱最小；
2. **播放音频渲染**：`render_audio_samples` 的 C 包装；
3. **通用变更通知机制**；
4. **markers / 多机位**。

GPU 帧互通是独立的性能议题，可以先走 `render_frame_f32` 的 CPU 路径把 viewer 跑通。

---

数据快照：2026-10-13，`liboak_engine.so` 导出 619 个 `oak_*` 符号，SDK 声明 508 个。
