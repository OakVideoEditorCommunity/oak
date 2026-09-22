# 全链路 ACEScg + F32 色彩管线改造计划

> **⚠️ 已被实际架构取代（2026-08-28）**：本文档是 gpui 中心视角的旧方案
> （要求三套渲染器各自实现离屏 F32 场景 + 输出节点 pass）。实际实现采用
> **引擎侧色彩架构**：oak 引擎完成全部色彩工作（解码→ACEScg F32 工作空间
> →输出节点转项目输出色域），gpui 渲染器只做直通 blit + 窗口内容色域声明
> （Wayland color-management-v1 / macOS layer.colorspace / Windows
> SetColorSpace1），UI 保持 sRGB 不进 ACEScg。实际色彩数学在
> `crates/oak-common/src/colormath.rs`，显示策略在
> `../../../../crates/oak-app/src/oakui/displaycolor.rs`，内容色域声明 API 在 gpui 的
> `WindowContentColorspace`/`set_content_colorspace`。本文档仅作历史参考，
> 以代码为准。

> 面向实现者的任务书。配套审计：2026-08-27 色彩管理链路审计（会话记录，
> 结论摘要见本文"现状"一节）。本计划只描述改造方案与工作项，**不包含任何
> 已执行的代码修改**。
>
> 仓库边界说明：本仓库（oak-gpui）包含 gpui 核心、三个平台渲染器
> （`gpui_macos` / `gpui_windows` / `gpui_wgpu` / `gpui_linux`）与
> `oak_bridge` 视频桥。媒体解码、节点图求值在 oak 主仓库（引擎），
> 本计划为其定义**色彩契约**，并标注哪些工作项需要主仓库配合。
>
> 原则：
> 1. 输入节点把素材转换为 ACEScg + F32；
> 2. 输出节点转换回目标色域，目标色域由项目设置指定，未指定则为 sRGB；
> 3. 中间全过程使用 ACEScg + F32；
> 4. 三个平台（macOS / Windows / Linux-Wayland）都正确处理颜色：
>    应用只输出**色度学上明确的颜色空间**（如 sRGB），把"到显示器"的最后
>    一次映射交给操作系统，绝不自行施加显示器 ICC 变换，杜绝二次映射。

---

## 1. 目标与动机

Oak 是视频编辑器。现状管线是"全链路 gamma 编码 sRGB + UNORM 交换链"，
这对 UI 够用，但对视频编辑有三个根本缺陷：

1. **精度不足**：8-bit gamma 编码值做混合/插值/滤镜会产生色带与
   暗部误差；多次处理链（特效叠加）累积量化损失。
2. **色彩空间不可控**：素材可能是 BT.709 / Display P3 / BT.2020 HDR，
   现状要么被当 sRGB 直通（YUV 路径还硬编码了 BT.601 矩阵），要么由
   各平台着色器各算各的，三端结果不一致。
3. **输出目标不可配置**：交付色域（sRGB / P3 / BT.2020）应由项目设置
   决定，现状没有这个概念。

目标架构：**场景线性（scene-linear）工作空间 = ACEScg（AP1 基色，线性
传递），数据精度 = F32**。ACEScg 是影视工业标准工作空间（AP1 色域覆盖
BT.709/P3/BT.2020 大部分，线性光，负值可表示，矩阵运算友好）。

## 2. 现状摘要（审计结论）

- 颜色以 `Hsla` 存于场景（`crates/gpui/src/color.rs:697` 的
  `ColorSpace` 枚举只影响渐变插值，与显示无关）；三套着色器
  （`shaders.metal` / `shaders.hlsl` / `shaders.wgsl`）各自做
  HSL→RGB，输出 gamma 编码 sRGB 值。
- 交换链一律非 SRGB 的 UNORM：macOS `BGRA8Unorm`
  （`metal_renderer.rs:236`），Windows `DXGI_FORMAT_B8G8R8A8_UNORM`
  （`directx_renderer.rs:39`），wgpu 偏好 `Rgb10a2Unorm`/`Rgba16Float`
  （`wgpu_renderer.rs:125`）。OS 把内容当 sRGB，各映射一次——当前没有
  二次映射。
- 已知缺陷（本计划顺带修复）：
  - 三端渐变插值不一致（wgpu 的 `linear_to_srgba`/`srgba_to_linear`
    双重编码，源于错误注释"`hsla_to_rgba` 返回 linear sRGB"）；
  - Windows HLSL 的 `linear_to_srgb`/`srgb_to_linear` 定义与名称互换，
    Oklab 渐变方向全反；
  - macOS YUV 直通路径硬编码 BT.601 full-range 矩阵
    （`shaders.metal:893`）；
  - Wayland 未用 `color-management-v1` 声明表面色彩空间（依赖已启用
    `staging` feature，见 `gpui_linux/Cargo.toml:104`，协议源码在依赖
    树中，未接线）；
  - `OAK_MACOS_LAYER_COLORSPACE=display` 直通标记只取主显示器
    （`display_colorspace.rs:32`），窗口跨屏/换 ICC 后过期
    （`window.rs:2362` 不更新）。

## 3. 目标架构总览

```
 素材(视频/图片, 任意源色域)          UI 颜色(Hsla)         文本/图标
        │ 输入节点                          │                    │
        │ 源色域→ACEScg(F32)                │ HSL→sRGB→linear→AP1 │ 覆盖率掩码,
        ▼                                  ▼                    ▼ 颜色同 UI
 ┌──────────────────────────────────────────────────────────────────┐
 │              场景合成：ACEScg + F32（混合/渐变/模糊/滤镜）          │
 │   scene texture(RGBA32F) → blur/group/path intermediates(F32)     │
 └──────────────────────────────────────────────────────────────────┘
        │ 输出节点（每个渲染器一个共享语义的最终 pass）
        │ ACEScg → 目标色域(项目设置, 默认 sRGB)：
        │   AP1→目标基色矩阵 → 色域映射/钳制 → 目标传递函数编码
        │   → (可选)抖动
        ▼
 交换链(8/10-bit UNORM, 携带"目标色域"语义) → 平台呈现
   macOS:   CAMetalLayer colorspace = 目标色域 → ColorSync 映射到显示器
   Windows: DXGI 默认 sRGB(显式 SetColorSpace1) → DWM/ACM 映射
   Wayland: color-management-v1 声明 image description → 合成器映射
```

关键不变量：**全仓库内不存在"显示器 ICC 变换"**。输出节点的目标永远是
色度学空间（sRGB / Display P3 / BT.2020+PQ…），最后一次到显示器的映射
由操作系统完成。macOS 的 `OAK_MACOS_LAYER_COLORSPACE=display` 直通模式
在本计划中退役（见 P2.4）。

## 4. 契约与配置类型（P0 产出）

新增（建议放 `crates/gpui/src/color.rs` 或新模块 `color_pipeline.rs`）：

```rust
/// 渲染管线工作模式。本 fork 默认 AcesCg；保留 SrgbLegacy 供
/// gpui-ce 上游用户与回退调试。
pub enum ColorPipeline { SrgbLegacy, AcesCg }

/// 输出节点的"目标色域"，由项目设置指定；未指定 = Srgb。
pub struct OutputColorSpec {
    pub gamut: OutputGamut,       // Srgb | DisplayP3 | Bt2020
    pub transfer: OutputTransfer, // Srgb | Gamma22 | Pq | Hlg
    pub peak_nits: Option<f32>,   // HDR 目标才需要
}
impl Default for OutputColorSpec { /* sRGB + sRGB 传递 */ }
```

- `WindowOptions` / `WgpuSurfaceConfig`（`wgpu_renderer.rs:155`）/
  macOS `MacWindow` 构造参数 / Windows 渲染器构造参数，各加
  `color_pipeline` 与 `output: OutputColorSpec` 字段，默认
  `AcesCg + OutputColorSpec::default()`。
- 运行时可变：`Window::set_output_color_space(spec)` → 重建输出
  pass uniform；若位深/格式需要变化则重配交换链（wgpu
  `surface.configure`；Windows 重建 swapchain；macOS 更新
  `layer.colorspace` 与像素格式）。Oak 主仓库的项目设置面板调用它。
- Surface 契约（输入节点交付面）：`paint_surface` 传入的纹理必须是
  **ACEScg 线性、F32（`Rgba32Float`）**；`oak_bridge::SurfaceFormat`
  （`crates/oak_bridge/src/surface.rs`）相应更新。纹理附带可选的
  元数据（源色域标签）仅用于引擎内部，交给 gpui 时一律已转换。
- 色彩数学集中在一个模块（矩阵 + 传递函数 + 单元测试），三套着色器
  的常量与之保持一致（矩阵同时写进 WGSL/Metal/HLSL，测试比对数值）。

## 5. 输入节点（素材 → ACEScg F32）

"输入节点"在本仓库内对应四个入口：

1. **视频/引擎帧**（`paint_surface`，`window.rs:4192/4211`，
   `elements/surface.rs`）：
   - 转换在 oak 引擎侧完成（源色域从解码器元数据读取：
     BT.709/BT.2020/P3、transfer、full/limited range），经
     `oak_bridge` 交付 `Rgba32Float` ACEScg 纹理。本仓库只定义契约 +
     验证（格式不符时 `log::error` 并拒绝，沿用
     `metal_renderer.rs:1857` 的检查模式）。
   - **退役 macOS 的 YUV 直通路径**（`metal_renderer.rs:1857-1864`
     与 `shaders.metal:893` 的 BT.601 矩阵）：解码→转换放引擎侧后，
     gpui 不再需要 YUV 采样。过渡期保留但标记 deprecated。
2. **位图/图标**（polychrome sprites，atlas 上传）：
   atlas 插入时做一次性转换：sRGB 解码 → AP1 矩阵，存为
   F32（或 F16，见风险节）atlas 格式。涉及
   `metal_atlas.rs` / `wgpu_atlas.rs` / `directx_atlas.rs`。
3. **UI 颜色**（`Hsla`）：在着色器内转换——`hsla_to_rgba` 改为
   `hsla_to_acescg`：HSL→gamma-sRGB → sRGB EOTF → 线性 sRGB →
   AP1 矩阵（sRGB(D65)→ACEScg(D60 白点，矩阵含色适应)）：
   ```
   0.6131324224  0.3395230762  0.0473341514
   0.0701922769  0.9163536767  0.0134540464
   0.0206157712  0.1095697056  0.8698145232
   ```
   （数值以 ACES 官方规范核准版为准，P0 单测锁定。）
4. **截图/回读**（`render_to_image` / `render_scene_to_image`，
   `metal_renderer.rs:679/777`）：默认经过输出节点，得到与显示器
   一致的 sRGB 编码图；另留内部接口输出 ACEScg 原值供调试/测试。

## 6. 中间处理链（ACEScg + F32）

- 场景渲染目标、模糊乒乓、内容滤镜组纹理、path 中间纹理全部改为
  `Rgba32Float`（wgpu：`wgpu_renderer.rs:1290+` 的
  `ensure_blur_textures` 与 `create_path_intermediate`；Windows：
  `create_path_intermediate_texture` 等；macOS：新增离屏场景纹理，
  见 P2.1）。
- **混合/插值/模糊全部天然变为线性光**——顺带修复审计发现的三端
  渐变不一致与 gamma 混合偏暗问题。`ColorSpace::Srgb` 渐变语义改为
  "在线性 sRGB 中插值"（端点先 AP1→linear-sRGB），`Oklab` 改为
  "AP1→linear-sRGB→Oklab 插值→返回"，三端共用同一套函数，统一行为。
- 覆盖掩码类数据（字形 alpha、path 覆盖率）不属于颜色，保持低精度
  （R8 / F16）即可；只有**颜色**走 F32。
- 文本外观：现有 `ZED_FONTS_GAMMA` / enhanced-contrast 参数
  （`wgpu_renderer.rs:2667` 的 `RenderingParameters`，及三套
  `shaders_subpixel`/color_text_raster）是为 gamma 空间调的，线性化
  后需重新调参（P5.4）；覆盖率校正本身保留在覆盖率域。
- 抖动（现 `shaders.metal:1243` 等处的 ±2/255 渐变抖动）移到
  **输出节点编码之后**，线性域内抖动无意义。

## 7. 输出节点（ACEScg → 目标色域）

每个渲染器增加一个最终全屏 pass（三份实现、同一语义；建议先在
wgpu 端定型再移植）：

1. `AP1 → 目标基色`矩阵（目标为 sRGB 时即上面矩阵的逆）。
2. **色域映射**：P1 用简单钳制（UI 颜色几乎不越界）；越界严重的
   视频内容后续升级为色度压缩（列入开放问题）。
3. **传递函数编码**：`sRGB OETF` / `pow(1/2.2)` / `PQ` / `HLG`。
4. **HDR→SDR 目标时**需要色调映射（ACES RRT+ODT 或更简单的 roll-off）；
   列入 P6，首期只做同动态范围目标。
5. 编码后抖动（见上节）。
6. 输出到交换链（仍为 8/10-bit UNORM；`OAK_DISPLAY_BIT_DEPTH` 语义
   不变）。

随之而来的结构变化：**三个渲染器都必须"离屏场景 + 最终 blit"**。
Windows 已有离屏场景（`directx_renderer.rs:421-578` 的 blur 路径，
泛化为常开）；wgpu 已有 blit 基建（`fs_blur_downsample` 的 1:1 拷贝
分支）；**macOS 目前是直绘 drawable，需要新增离屏场景纹理**——这是
macOS 端最大的结构改动（P2.1），注意 `presents_with_transaction`
直显模式（`metal_renderer.rs:641`）与 offscreen 的相互作用。

## 8. 平台呈现（单次映射原则）

- **macOS**：layer 像素格式保持 `BGRA8Unorm`；`layer.colorspace`
  设为**输出目标色域**（默认 `CGColorSpaceCreateWithName(kCGColorSpaceSRGB)`，
  目标为 P3 时设为 P3），ColorSync 完成到显示器的唯一一次映射。
  替换 `OAK_MACOS_LAYER_COLORSPACE` 逻辑（`metal_renderer.rs:254-266`
  与 `display_colorspace.rs`）：**不再使用显示器色彩空间直通**。
  窗口跨屏时（`window_did_change_screen`，`window.rs:2362`）无需
  重打标记——标记的是内容色域而非显示器，ColorSync 自动按当前屏映射
  （这正是修复审计缺陷之处）。HDR 输出（P6）再启用
  `wantsExtendedDynamicRangeContent` + EDR headroom。
- **Windows**：交换链格式与现状一致（`B8G8R8A8_UNORM`，
  `directx_renderer.rs:39`）。增加显式声明：cast
  `IDXGISwapChain1 → IDXGISwapChain3`，调用 `SetColorSpace1`
  （默认 `DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709`；P3/BT.2020 目标
  在 P6 加对应值）。显式声明消除对"默认即 sRGB"的隐式依赖。
- **Linux/Wayland**：实现 `color-management-v1`（wayland-protocols
  `staging` feature 已在，`gpui_linux/Cargo.toml:104`）：
  - `client.rs` 绑定 `wp_color_manager_v1`（对齐方式参照现有
    `wp_fractional_scale_manager_v1` 的接法，`client.rs:66`）；
  - 每个窗口 `wl_surface`（`wayland/window.rs:539` 创建、rwh 句柄
    已在本仓库手里）取 `wp_color_management_surface_v1`，
    `set_image_description` = sRGB（BT.709 基色 + sRGB 传递，用
    params creator 构造；目标色域变化时更新）；
  - 合成器不支持该协议时静默降级（内容本来就是 sRGB，合成器默认假设
    也是 sRGB，行为不变）；
  - **绝不在应用侧做显示器映射**——维持审计结论：Wayland 的颜色管理
    不可关闭，程序只声明、不代劳。
- **X11**：无协议可用，维持 sRGB 直通，文档注明广色域屏过饱和属
  系统限制。

## 9. 分阶段工作项

### P0. 契约与色彩数学基础（无渲染行为变化）

- [ ] `OutputColorSpec` / `ColorPipeline` 类型与默认值（§4）。
- [ ] 色彩数学模块：sRGB↔ACEScg 矩阵、EOTF/OETF、PQ/HLG 占位；
      参考实现 + 单测（含与已知测试向量的比对，如 sRGB 红/绿/蓝
      原色在 ACEScg 下的坐标）。
- [ ] 三套着色器共用的矩阵/函数清单（哪些函数要改、改成什么），
      写成对照表放进实现 PR。
- [ ] `WindowOptions` / 各渲染器配置字段贯通（此阶段
      `SrgbLegacy` 行为与现状完全一致，`AcesCg` 先不启用）。
- 验收：`cargo test` 全绿；`SrgbLegacy` 下截图与改造前逐像素一致
  （现有 visual test 基线）。

### P1. wgpu 渲染器（Linux）先行试点

- [ ] `shaders.wgsl`：`hsla_to_rgba` → `hsla_to_acescg`；渐变/
      Oklab/over/blur 全部改为线性语义；删除双重编码路径
      （`shaders.wgsl:417-421, 473` 审计缺陷顺带消除）。
- [ ] 场景/模糊/组/路径中间纹理改 `Rgba32Float`
      （`wgpu_renderer.rs` 的 `ensure_blur_textures`、
      `create_path_intermediate`、`RenderingParameters` 的
      MSAA 采样数适配——部分后端不支持 32F MSAA，需降级策略）。
- [ ] 新增输出节点 pass（§7），交换链仍用
      `preferred_surface_formats()`（`wgpu_renderer.rs:125`）。
- [ ] atlas 改线性（polychrome）；字形覆盖率保持。
- [ ] Surface 元素按契约采样 `Rgba32Float`
      （`wgpu_renderer.rs:1880` 的 `draw_surfaces`）；格式校验。
- [ ] Wayland `color-management-v1` 接线（§8）。
- 验收：`examples/legacy/gradient.rs` 三端一致（本阶段与 macOS 对照
  用截图比对）；KWin/启用色彩管理的合成器下声明生效（协议日志或
  合成器调试工具确认），不支持的合成器无回归；模糊/滤镜视觉测试通过。

### P2. macOS Metal 渲染器

- [ ] **结构改造：离屏场景纹理（F32）+ 输出节点 pass → drawable**
      （`metal_renderer.rs` 的 `draw`/`draw_primitives` 重构，
      注意 `presents_with_transaction`、`next_drawable` 超时处理与
      `render_to_image`/`render_scene_to_image` 测试路径）。
- [ ] `shaders.metal` 与 `shaders.wgsl` 对齐（线性语义、
      统一的渐变/Oklab 实现、pow(2.2) 近似换精确 sRGB TF）。
- [ ] atlas 线性化（`metal_atlas.rs`）。
- [ ] `layer.colorspace` = 输出目标色域；移除
      `OAK_MACOS_LAYER_COLORSPACE`/`display_colorspace.rs` 直通逻辑
      （与 oak 主仓库协调：主仓库停止设置该 env）。
- [ ] 退役 YUV 直通路径（§5.1），`oak_bridge` 交付格式契约更新
      （`Rgba32Float`；`Bgra8Unorm` 过渡期保留并打警告）。
- 验收：广色域显示器上 UI 颜色与"系统设置-显示器-P3/sRGB 切换"的
  行为一致（ColorSync 单次映射）；跨屏移动窗口颜色不变；
  visual test 基线更新并通过。

### P3. Windows D3D11 渲染器

- [ ] 离屏场景泛化为常开（现有 `scene_rtv/scene_srv` 机制，
      `directx_renderer.rs:421-578`）+ F32 中间纹理。
- [ ] `shaders.hlsl` 对齐：修复 `linear_to_srgb`/`srgb_to_linear`
      名称/定义互换（审计缺陷），统一线性语义。
- [ ] 输出节点 pass 替换 `dx_blit`（`directx_renderer.rs:1116`）。
- [ ] `IDXGISwapChain3::SetColorSpace1` 显式声明（§8）。
- [ ] atlas 线性化（`directx_atlas.rs`）。
- 验收：与 Linux/macOS 的截图逐像素近似比对（容差来自抖动/驱动）；
  Win11 ACM 显示器上行为正确；透明窗口（DComposition，
  premultiplied）无回归。

### P4. 三端一致性与视频链路收口

- [ ] 三端渐变/混合/文本外观交叉比对（用 `gradient` example +
      新增 color-checker example：24 色卡 + 灰阶 + 色域边界色）。
- [ ] 引擎侧输入节点联调（oak 主仓库）：解码元数据→ACEScg 转换、
      `oak_bridge` F32 交付、Windows/Linux 的
      `paint_surface(wgpu::Texture)` 直连（oak-app-rewrite.md W3
      遗留项一并完成）。
- [ ] 项目设置→`OutputColorSpec` 的运行时切换联调（改项目设置后
      不重启窗口即生效）。
- 验收：同一项目在三平台导出的检视器画面一致；切换目标色域
  （默认 sRGB ↔ P3）立即可见且与外部参考（如系统色彩管理应用）
  观感一致。

### P5. 文本与外观回归

- [ ] 线性空间下的文本参数重调（`ZED_FONTS_GAMMA` 等，
      `RenderingParameters`），subpixel 覆盖率校正在覆盖率域重推；
      提供 A/B 对比工具。
- [ ] 主题/调色板审视：UI 颜色在 ACEScg 管线下的最终呈现与旧管线
      应逐像素等价（sRGB→ACEScg→sRGB 往返），若有偏差定位到具体
      着色器路径。
- 验收：现有 visual tests 全绿；文本在明/暗背景下的可读性评审通过。

### P6. HDR 与广色域输出（二期，可与主仓库排期解耦）

- [ ] `OutputGamut::Bt2020` + `PQ/HLG`：输出节点色调映射选型
      （候选：ACES RRT+ODT / Khronos PBR Neutral / 简单 roll-off），
      先在 wgpu 端原型。
- [ ] macOS：`wantsExtendedDynamicRangeContent` + EDR headroom 监听；
      `Rgba16Float` 交换链（EAC 模式）。
- [ ] Windows：HDR swapchain（`DXGI_FORMAT_R16G16B16A16_FLOAT` +
      `SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020)`），
      查询 `DXGI_OUTPUT_DESC1` 的 HDR 状态。
- [ ] Wayland：image description 声明 BT.2020+PQ；跟随
      `preferred` 反馈。
- [ ] 输入侧：HDR 素材（PQ/HLG 源）在引擎侧转 ACEScg 的场景参考
      语义定义（与色调映射策略联动）。
- 验收：HDR 显示器上高光细节保留、SDR 内容不炸白；三端行为对齐。

## 10. 测试策略

- **单测**：色彩数学（矩阵往返误差 < 1e-6、传递函数锚点值、
  色域边界钳制行为）。
- **headless 截图**：`render_scene_to_image` 走输出节点，锁定
  golden image；`SrgbLegacy` 模式保留旧基线用于回归。
- **跨端比对**：color-checker example 三端截图自动比对
  （容差需显式定义，抖动用固定种子）。
- **真实显示器**：P2/P3/P6 验收需要广色域/EDR/HDR 显示器 + 目视或
  色度计；CI 无 GPU 环境跳过（沿用 `oak_bridge` demo 的做法）。

## 11. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| F32 目标带宽/显存 ~4×（模糊乒乓最明显） | 低端 GPU 掉帧 | 提供 `Rgba16Float` 降级开关（视觉差异对 8-bit 交付可忽略）；模糊半分辨率已存在；先测量再优化 |
| 32F MSAA 部分后端不支持 | path 抗锯齿退化 | `RenderingParameters::path_sample_count` 已有降级逻辑，F32 下按需降到 1× 或用 F16 中间层做 MSAA |
| macOS 离屏化破坏直显模式性能 | 帧延迟/掉帧 | 保留 `presents_with_transaction` 语义，输出 pass 与 present 同 command buffer 提交；基准对比改造前后 |
| 文本外观变化（线性混合显细） | 可读性回归 | P5 专项；覆盖率校正留覆盖率域；参数可调 |
| Wayland 协议可用性参差 | 声明不生效 | 降级路径 = 现状（合成器按 sRGB 处理，内容恰为 sRGB，无损） |
| 与上游 zed 分叉进一步扩大 | 合并成本 | 改动集中在渲染器/着色器（上游也在快速变动），核心场景结构不动；`SrgbLegacy` 保持与上游行为一致 |
| 引擎侧输入节点未就绪（主仓库依赖） | P4 联调阻塞 | gpui 侧先用合成测试纹理验证契约；YUV 旧路径过渡期保留 |

## 12. 兼容性说明

- `gpui-ce` 的其他使用者：`ColorPipeline::SrgbLegacy` 与现状
  逐像素一致，可作默认逃生口；本 fork（oak）默认 `AcesCg`。
- `OAK_DISPLAY_BIT_DEPTH` 语义不变（只影响交换链位深）。
- `OAK_MACOS_LAYER_COLORSPACE` 在 P2 移除，需同步通知主仓库
  （该 env 由主仓库设置，见审计）。
- 截图/视觉测试的像素基线在 P1-P3 各平台切换时一次性更新，
  更新前后用 `SrgbLegacy` 双跑确认差异全部来自预期语义变化。

## 13. 开放问题（实施前需拍板）

1. 中间链 F32 是否允许按设备能力降级 F16（Apple Silicon/现代独显
   上两者带宽差异显著）？建议：默认 F32，配置项允许 F16。
2. 色域映射算法：首期钳制是否可接受（视频内容可能越界）？
   还是 P1 就上色度压缩？
3. HDR→SDR 色调映射选型（P6）——影响输入侧"场景参考"语义定义，
   建议 P6 启动时单独评审。
4. Wayland 下目标色域为非 sRGB（P3/BT.2020）时，是否要求合成器
   支持对应 image description，还是回退 sRGB 输出（合成器能力查询
   `wp_color_manager_v1` 的 render intent/primaries 反馈）？
5. `ColorSpace::Oklab` 渐变在线性管线下的语义：Oklab 本为感知
   均匀空间，输入应使用线性 sRGB——与 CSS `oklab` 一致，三端统一后
   无歧义，但需确认与现有设计稿的视觉差异可接受。
