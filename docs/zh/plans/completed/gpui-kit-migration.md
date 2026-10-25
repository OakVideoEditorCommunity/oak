# oak-app 的 gpui-kit 迁移

oak-app 从 oak-gpui fork（`gpui/` 子仓库的 `gpui` / `gpui_widgets` /
`gpui_elements` crate）迁移到 gpui-kit 0.7.3（registry 的
`gpui-pre` + `gpui-base` + `gpui-component`）。引擎侧保持直链进程内
oak-\* 模块 crate 不变。

## 结构决策

- **fork 的自定义 UI 模块提升为 app 侧模块**，构建在 gpui-kit 公共 API 上：
  - `gpui` crate 的 `effect_stack` / `node_graph` / `timeline` /
    `i18n`（字符串表挂钩）
  - `gpui_widgets` 的 `theme`（Olive 主题 → `GlobalColors`，
    `cx.default_colors()` 语义不变）、`menu`、`dialog`、`slider`（+`value`、
    `keyable`）、`tooltip`、`audio_meter`、`curve_editor`、`scopes`、
    `project_explorer`、`viewer`、`icons`
  - 迁移期间暂存于 `crates/oak-widgets`（独立可测），完成后折回
    `oak-app/src/widgets/`。
- **dock 不移植**：换成 gpui-kit 内置的 `component::dock`
  （`DockSkin` + `DockArea` + `BasePanel`/`Panel`）。
- **`gpui_elements::editable_text` 不移植**（4.9k 行编辑引擎）：换成
  gpui-kit 内置 `component::input::Input` / `InputState`（slider 直输编辑、
  文件对话框路径字段已切换）。

## 迁移中的 API 漂移（fork → gpui-pre 0.3.9）

- `Pixels` 的 `.0` 字段私有化 → `.as_f32()`（113 处，编译器诊断驱动批量修复）。
- `App::active_drag` 私有化 → 各拖拽 payload 自己记录抓点偏移
  （`on_drag` 构造器的第二参数正是该偏移）；timeline 的 clip 移动 ghost
  改由视图自身字段跟踪。
- `TextRun.letter_spacing`（fork 私增字段）→ 删除。
- `Window::draw(cx).clear()` → `.clear(cx)`；`cx.new(f)` 必须传闭包。
- `path_builder::PathBuilder` → 根导出 `PathBuilder`；const 上下文
  `Pixels(x)` → `px(x)`。
- gpui-kit 组件（如 `Input`）需要 `gpui_kit::init(cx)` 安装组件主题
  （`cx.theme()`），与 `init_colors`（`cx.default_colors()`）并存——
  两个全局都要初始化。
- 窗口事件分发 / render 期间无法重入 `cx.update_window`（窗口正被
  借用，调用静默失败）。`text_input::set_input_text` 因此把
  `set_value` 丢进 `App::defer`，等当前 effect cycle 结束、窗口回到
  app map 后再落地。另外 gpui-kit 的 `InputState::set_value` **不发**
  `InputEvent::Change`（只有真实键入/token 编辑发），程序化刷新不会
  触发订阅回调——取色器的 hex 字段利用这一点做「键入即实时解析进
  draft」，commit 以 draft 为准，不回读可能滞后的字段文本。
- dock 面板必须用 `panel_handle(entity)` 包一层再交给 dock
  （`DockArea::add_panel_view` / `DockLayout::panel_view`）。直接
  `add_panel(entity)` 也能停靠，但皮肤拿不到 presentation handle：
  标签页退化显示 `panel_name`（原始 id 而非本地化标题），标签栏尾部的
  面板按钮、zoom 控件全部消失。回归测试：
  `app::tests::docked_panels_carry_their_presentation_handle`。

## 10-bit GPU 纹理路径（已恢复）

**已按方案 A 恢复**（2026-10-13）：`[patch.crates-io]` 把三个 crate
vendor 到 `../../../../vendor` 并打上最小补丁 ——

- `../../../../vendor/gpui-pre`：`SurfaceSource::Texture` 变体（Linux/FreeBSD）、
  `Window::paint_surface` 的纹理形态、`Window::gpu_context` /
  `gpu_device_lost`、`PlatformWindow` 的两个默认 `None` 方法；
- `../../../../vendor/gpui-pre-linux`：wayland/x11 窗口把渲染器的 device/queue 透出；
- `../../../../vendor/gpui-pre-wgpu`：`OAK_DISPLAY_BIT_DEPTH` 下优先
  `Rgb10a2Unorm`/`Rgba16Float` swapchain 格式、`draw_surfaces` 纹理
  管线（fork 的 YCbCr 视频 surface shader 换成单纹理 RGBA 采样）、
  `WgpuRenderer::gpu_context` 访问器。

viewer 的 `SurfaceSource::Texture` 快速路径、`gpu.rs` 的全部入口、
app 启动时的 `gpu_context` 捕获均已恢复；fork 的
`registered_gpu_frame_switches_to_surface` 测试随移植带回并通过。

**OS 层色彩空间声明也已恢复**（同日）：`LayerColorManagement` /
`WindowContentColorspace` / `ContentPrimaries` / `ContentTransfer` 类型与
`Window::set_layer_color_management` / `set_content_colorspace` 进了
`../../../../vendor/gpui-pre`；Wayland 的 color-management-v1 声明（图像描述创建 +
`ready` 派发 + `apply_color_description`）进了 `vendor/gpui-pre-linux`。
`displaycolor::apply_to_window` 恢复原实现。

- Wayland：合成器按声明的 primaries/transfer 映射（PQ/HLG 附带
  10000/80 cd/m² 亮度范围），与 fork 一致；
- X11：无合成器映射，app 自管（不变）；
- macOS/Windows 平台层 crate 未打补丁（registry 原版）——trait 的默认
  no-op 保持兼容，但那两个平台的 layer 颜色标记暂不生效，需要时按同一
  模式补 `vendor/gpui-pre-apple` / `vendor/gpui-pre-windows`。

## 进度

- [x] oak-widgets：fork 模块提升（dock 除外），155 单测 + 5 doctest 全过
- [x] oak-app 本体：依赖切换完成（oak-app 不再链接 fork 的任何 crate），
      dock 换 gpui-kit 内置（component::dock + DockSkin），
      `cargo test -p oak-app` 560 测试全过
- [ ] oak-widgets 折回 `oak-app/src/widgets/`（可选清理）
- [ ] 删除 gpui/ 子仓库引用（待迁移观察期结束）

### 其他回退（随 dock 迁移）

- **面板浮动/撕出窗口**：gpui-kit dock 没有 floating 面板；窗口 菜单的
  开关=移除/重新落位（不记忆上次位置，重新打开回设计默认位）。

### 时间线不再停靠（设计恢复）

gpui-kit dock 的底部 dock 只夹在左右 dock 之间，而设计要时间线顶满
窗口全宽（fork 的做法是根级 Bottom 拆分）。时间线面板因此移出 dock，
成为 shell 的固定底部条带：窗口 → 时间线 开关显隐，顶部 4px 拖柄调高。
它不再是 dock 面板（不可拖走到别的区域）——这正是设计本意。

顺带修正：ruler 的点击定位原来用「窗口 x=0 + HEADER_WIDTH」的硬编码
假设，在 dock 居中布局下偏一个左 dock 宽；改为用 canvas 捕获的
clip_area_origin 换算，任何布局下都正确。

### 滑杆拖拽与引擎回读打架（「不跟手」）

`ofx_params` 的 `sync_values` 每帧把引擎快照写回控件，拖拽中也写 ——
引擎值落后指针一帧时滑杆拇指被顶回，手感就是「不跟手」。现在拖拽中
的滑杆（`is_dragging`）和编辑中的 spin box（`is_editing`）跳过回写。
控件本身（controls.rs 的 Slider）与 fork 逐行一致且测试覆盖点击跳转/
拖拽跟踪/键盘/双击编辑，不换成 gpui-kit 内置 slider：内置组件没有
ValueKind（int/float 步进）、数值直输框、关键帧菱形和
DragStarted/Finished 的撤销分组事件，切换等于重写 ofx_params 的
参数提交链路。
- **文件选择器的扩展名过滤**：gpui-pre 的 `PathPromptOptions` 没有
  `allowed_extensions`；打开/导入对话框不再按扩展名过滤。
- **布局持久化**：fork 的 registry 从未接线保存，随迁移移除；gpui-kit
  的 `DockArea::dump`/`load` 是将来做布局持久化的入口。
