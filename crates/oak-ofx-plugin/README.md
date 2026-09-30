# oakplugin Rust crate（M11 第 1+2 期实现状态）

> 状态：**M11 第 1、2 期已实现**（第 2 期：GL 渲染路径 +
> ofxColour + pluginrenderer 渲染驱动收编）。原声明底稿（类型与
> 函数签名 + 文档注释即规约）已由实现方填充；`src/` 内已无
> `todo!()`。
> 计划：docs/zh/plans/riir/M11-ofx-host.md。

## 结构

```
src/
  lib.rs        crate 文档、模块图、FFI 纪律
  error.rs      错误码（与 include/plugin/error.h 一一对应）
  handle.rs     RefBox 边界容器（Host::create_instance 返回值类型）
  property.rs   PropertySet：OFX 属性集的存储与类型化读写
  suites/       插件调进来的 C 函数表（unsafe trampoline 层）
    mod.rs      fetchSuite 注册表 + 渲染/GL 上下文 TLS
    property.rs memory.rs image_effect.rs param.rs
    message.rs  progress.rs timeline.rs multithread.rs
    gl_render.rs OfxImageEffectOpenGLRenderSuiteV1（M11 §4 新增）
    draw.rs     OfxDrawSuiteV1（OFX 1.5，interact 绘制；真实 GL 渲染）
    interact.rs OfxInteractSuiteV1 + Interact 宿主对象（interact 宿主）
  host.rs       Host 单例：bundle 扫描、插件缓存、action 分发
  descriptor.rs EffectDescriptor/ClipDescriptor（describe 的产物）
  instance.rs   Instance：action 调用面（render/协商/RoD/RoI/isIdentity/
                render_gl/GetOutputColourspace）
  render_driver.rs pluginrenderer.cpp 渲染流程收编（M11 §4 新增）
  clip.rs       ClipInstance：clip↔oakrender 纹理桥（render.rs）
  image.rs      Image：帧缓冲/纹理的 OFX 视图
  param.rs      12 种参数实例 + param↔oaknode 桥（node.rs/undo）
  progress.rs   PluginProgressReporter
  node.rs       oaknode 桥：节点值 POD、身份注册表、undoable 回写
  render.rs     oakrender 桥：Texture/Frame/Renderer 值类型调用面
```

## 插件搜索路径

`host::PluginCache::scan` 按以下顺序扫描（重复路径去重、不存在的目录
跳过；bundle 递归扫描，深度 3）：

| 层级 | 路径 |
| --- | --- |
| 用户级（`$HOME`） | `$HOME/.OFX/Plugins`、`$HOME/.local/share/OFX/Plugins`、`$HOME/.local/share/olive/ofx/Plugins`；macOS 另含 `$HOME/Library/OFX/Plugins` |
| 系统级（OFX 规范） | macOS `/Library/OFX/Plugins`；Linux `/usr/OFX/Plugins`、`/usr/local/OFX/Plugins`；Windows `%ProgramFiles%\Common Files\OFX\Plugins`（`%ProgramFiles%` 未设置时回退 `C:\Program Files\Common Files\OFX\Plugins`） |
| app-relative（Olive 对齐） | `../OFX/Plugins`、`../share/olive/ofx/Plugins`、`../lib/olive/ofx/Plugins` |
| 环境变量 | `OFX_PLUGIN_PATH`（OFX 官方）、`OLIVE_OFX_PLUGIN_PATH`、`OLIVE_PLUGIN_PATH`（Olive 扩展）；平台路径分隔符（Unix `:` / Windows `;`） |

## 桥布局决策（M11 第 1 期冻结；第 2 期增补；单库化修订）

单库化（single-lib unification）后 `bridge/`（C ABI 导入）与
`ffi.rs`（导出层）已删除：oakundo/oaknode/oakrender 以 path 依赖
直连，桥改用直接 Rust 类型。

- **`node::Value`** = `include/node/node.h:93` 的 `oaknode_value`
  POD，字段逐字一致（`type`/`num`/`den`/`f[4]`；`type` 取值见
  `node_value_type`）。字符串族输入（k_file/k_text/k_font/
  k_str_combo）无 POD 表示，走 `set_input_string_undoable`。
  `Value::to_node_value`/`Value::from_node_value` 与
  `oaknode::value::NodeValue` 互转（按 POD kind；STRING 族无数据）。
- **节点绑定与回写**（`node.rs`）：进程级身份注册表
  `register_node(project, id)` → 打包身份
  （`oaknode::id::NodeId::identity()`）→
  `Instance::bind_node(identity)`；`node_from_identity` 按身份反查
  `NodeRef { project, id }`（弱引用映射，project 释放后自然失效）。
  `set_input_undoable`/`set_input_string_undoable` 构造未执行的
  `oakundo::undocommand::UndoCommand`（closure-vtable 模式，与
  `oaknode::ops::set_value_at_time_command` 同构）：redo 锁 project、
  `graph.get_mut(id)`、`NodeCore::set_standard_value`；undo 回放
  创建期快照的旧值。失败（未知输入/失效节点/非字符串族输入）
  返回 `NodeBridgeError`，调用方按 no-op 处理。
- **纹理/帧/渲染器值类型**（`render.rs`）：
  `Texture = oakrender::texture::Texture`、`Frame =
  oakrender::texture::Frame`（值语义，drop 自动释放后端 token，
  原 `texture_free`/`frame_free` 调用面删除）、
  `Renderer = Arc<dyn oakrender::backend::GpuContextLike>`（渲染器
  即后端上下文）。`texture_create(params, pixels, linesize)` 构造
  CPU 包装纹理（行跨度感知拷贝，GPU 上传由后端延迟）；
  `texture_get_frame`/`texture_get_params`/
  `renderer_is_open_gl` 为真实实现。**保留桩**：`texture_id` 恒 0
  ——wgpu 后端无 OpenGL 纹理名，GL suite 的 `OpenGLTextureIndex`
  属性与 use_opengl 决策据此回退 CPU 路径。
  `ClipInstance::set_output_texture` 挂入后，`store_output_image`
  取 CPU 帧整帧拷贝（全链路 F32，尺寸不符明确报错；GPU 目标经
  `GpuContextLike::upload` 回写）。
- **undo 打包** = 直接经 `oakundo::undocommand::UndoCommand`；
  paramEditBegin/End 的编辑事务在 `Instance` 上维护深度与累积
  multi（C++ `submit_undo_command` 语义：事务内子命令立即 redo
  生效并并入 multi，editEnd 整体 redo+释放）。
- **param↔node 值转换**（`param.rs`）：`set_from_node`（节点→插件，
  按参数类型映射，维度截断/补零）、`to_node_value`（插件→节点，
  RGB 补 alpha=1，与 C++ `RGBInstance::set` 一致）。字符串参数经
  `node::set_input_string_undoable`。
- **节点绑定**：`Instance::bind_node(identity)`（C++ `set_node_handle`
  的 Rust 侧）；回写只在 `ChangeReason::PluginEdited` 触发，未绑定/
  身份查无时 no-op。

## M11 第 2 期：GL 路径 + ofxColour + 渲染驱动

### GL 渲染路径

- `suites/gl_render.rs`：OfxImageEffectOpenGLRenderSuiteV1
  （clipLoadTexture / clipFreeTexture / flushResources），语义对照
  ofxGPURender.h（vendored）与 HostSupport 插件侧（ofxhImageEffect.cpp:
  2296-2367）。纹理句柄是属性集（12 个属性：OpenGLTextureIndex/
  OpenGLTextureTarget/PixelDepth/Components/PreMultiplication/
  RenderScale/PixelAspectRatio/Bounds/ROD/RowBytes/Field/
  UniqueIdentifier）；存活表持有 Box<PropertySet>（句柄地址稳定）。
  **OpenGLTextureIndex 是真实 GL 纹理名**（GL 模式；0 = CPU 回退）。
  Output clip 的渲染目标绑定由宿主完成（等价 C++
  `attach_output_texture`）；clipFreeTexture 对 Output 不删纹理（宿主
  回读后删除）。
- GL render action：`Instance::render_gl`（与 CPU `render` 并存）——
  action 序列 kOfxActionOpenGLContextAttached → render（in args 带
  kOfxImageEffectPropOpenGLEnabled=1）→ OpenGLContextDetached；
  渲染结果留在 FBO 附着的输出 GL 纹理上，render 返回后宿主
  **glReadPixels 回读**装帧（方案 B，见下）。
- **GL 上下文规则**（ofxGPURender.h "OpenGL Current Context"）：宿主
  只在 Render/Begin/EndSequenceRender/Attach/Detach 期间要求上下文
  current——本实现的约定是 render 驱动一次 `gl_bridge::acquire` 整个
  GL render action（上下文 current 到 guard drop），suite 回调期间
  恒 current。
- 格式协商：插件描述符声明 kOfxImageEffectPropOpenGLRenderSupported
  （"false"/"true"/"needed"）与 kOfxOpenGLPropPixelDepth（可选位深
  列表）；宿主 `pick_gl_pixel_depth` 按管线 F32 约束选型（声明列表
  不含 Float → GL 模式不可行，回退 CPU）。use_opengl 决策在
  render_driver（插件 GL 声明 + 深度协商 + 桥可用）。

### GL 互操作桥（方案 B 落地，`gl_bridge.rs`）

- **macOS 真实实现**（Core OpenGL / CGL，`OpenGL.framework` 直链，
  无新 crate）：进程级共享离屏上下文（3.2 core profile、offline
  renderer 允许），`acquire()` 全局串行 + 置当前线程 current。为每次
  GL 渲染建输出 GL 纹理（尺寸 = 目标帧；F32 → RGBA32F、U8 → RGBA8）
  + FBO 挂载；插件直接画进 FBO；render 返回后 `glReadPixels` 回读
  （垂直翻转、U8 归一化 F32），与 CPU 路径输出格式一致。
- 输入 clip 在 GL 模式经桥上传为真实 GL 纹理（clipFreeTexture 删除）。
- GL 失败回退 CPU（对齐现有失败语义）。Linux（EGL）/Windows（WGL）
  预留 cfg stub。
- 验证：`gl_bridge.rs` 单元测试（clear 已知颜色 → 回读逐像素断言，
  `OAK_GPU_TESTS` 门）+ `tests/gl_render_test.rs` 端到端（GL 测试插件
  真实走 GL 路径、输出已知颜色）。

### ofxColour（OFX 1.4）

- 宿主描述符声明 OCIO 色彩管理（kOfxImageEffectPropColourManagementStyle
  = OCIO + AvailableConfigs = ofx-native-v1.5_aces-v1.3_ocio-v2.3）；
  实例期协商属性（style/config/OCIOConfig=ocio://default，工作空间
  ACEScg 经 clip 色彩空间属性传达）。
- 输入 clip 的 kOfxImageClipPropColourspace 宿主写为 ACEScg；
  GetOutputColourspace action（`Instance::get_output_colourspace`）：
  偏好列表采纳 + "OfxColourspace_<clip>" 交叉引用解析
  （`resolve_colourspace`），结果写回输出 clip
  （`set_output_colourspace`）；插件未实现 → 第一个输入 clip 的
  色彩空间（规范默认）。
- 描述符预定义 GL/colour 属性（ofxGPURender.h/ofxColour.h）：HostSupport
  的 propSet 不创建属性（返回 ErrUnknown），宿主预定义属性宇宙——
  第 1 期缺 GL/colour 声明，第 2 期补齐（见"第 1 期修复"）。

### render_driver（pluginrenderer.cpp 收编）

- `render_driver.rs`：`PluginRenderer::render_plugin`（1857 行 C++
  的渲染流程部分）的 Rust 移植——实例锁、use_opengl 决策、多输入
  clip 收集（effect_input_id / inputs 表 / SimpleSource 回退）、
  getClipPreferences、RoI/RoD 设定、getRegionOfInterest（失败按默认
  整帧继续，C++ 对 BadHandle 即如此）、输出 clip 格式、isIdentity
  短路（透传帧直接拷贝，不调 render action）、参数覆盖
  （apply_param_overrides 移植）、CPU/GL 渲染与输出装配。
- 序列括号：`render_begin_sequence`/`render_end_sequence` 的 C ABI
  （oakrender 对同一实例的一批帧先 begin 后 end，中间逐帧
  `render_job`）。
- 目标形态：oakrender 的 PluginJob 退化为一次 C ABI 调用
  `oakplugin_instance_render_job`（instance.h 新增，见下）。

### include/plugin/instance.h 新增（M11 §4，既有签名不变）

- `oakplugin_instance_render_begin_sequence` / `render_end_sequence`：
  序列括号。
- `oakplugin_instance_render_job(instance, dst, time, clear, interactive,
  effect_input_id, src, inputs[], values[], renderer)`：一帧渲染的
  单一调用；`oakplugin_job_value`（参数覆盖）/`oakplugin_job_texture`
  （输入 clip 纹理）两个 POD 随附。GL 契约（上下文 current + 输出
  附着）见头文件文档注释。

## OFX Interact 宿主（interact + Draw suite，M11 §5）

插件自定义 UI 的宿主侧：Interact 实例在主进程创建（UI 事件宿主），与
worker 进程里的渲染实例并存（OFX 允许同一插件多实例）。

- **`suites/interact.rs`**：
  - `Instance::new_interact`（任务契约）：取插件声明的 overlay interact
    入口（kOfxImageEffectPluginPropOverlayInteractV2 优先、V1 次之；未
    声明则插件 main entry）→ 建 `Interact`（属性表 + tagged INTERACT
    handle）→ 发 `kOfxActionNewInteract`。插件返回 OK → Some；返回
    ReplyDefault 且未声明 overlay 入口（无 interact）→ None；错误 → None。
  - 生命周期：`describe`（kOfxActionDescribe == kOfxActionDescribeInteract）
    → `create_instance`（kOfxActionCreateInstance）→ 事件 → `destroy`
    （kOfxActionDestroyInstance）；**与效果实例同生命周期**——实例销毁
    （`Instance::notify_destroy`）时先连带销毁 interact。
  - 调用面（宿主→插件，返回值如实透传）：`draw(viewport, pixel_scale,
    time, background)`、`pen_motion(pen_pos, pen_down)`、`pen_down`、
    `pen_up`、`key_down(key_sym, key_string)`、`key_up`、`idle`（任务契约
    的 kOfxInteractActionIdle，属宿主扩展）、`gain_focus`/`lose_focus`。
    pen 坐标为视口像素，inArgs 另带 canonical 位置（viewport/像素比）、
    viewport 位置、压力（两态笔映射 0/1）；key 带 kOfxPropKeySym
    （ofxKeySyms.h 关键码）+ kOfxPropKeyString。返回 kOfxStatOK = 插件
    已处理（宿主不再把事件给其他对象），ReplyDefault = 未处理。
  - `OfxInteractSuiteV1`（ofxInteract.h:534）：interactSwapBuffers /
    interactRedraw 把请求记入 `swap_requested`/`redraw_requested`
    （app 侧轮询面）；interactGetPropertySet 返回 interact 属性集句柄。
  - interact 属性集走现有 property suite：PixelScale / ViewportSize /
    BackgroundImage / SuggestedColour / SlaveToParam / BackgroundColour /
    BitDepth / HasAlpha（ofxInteract.h 的 PropertiesInteract；ViewportSize
    取 OFX 1.3 名 "OfxInteractPropViewport"、BackgroundImage 与 Idle 为
    任务契约扩展）。
- **`suites/draw.rs`**（`OfxDrawSuiteV1`，OFX 1.5，vendored ofxDrawSuite.h）：
  getColour / setColour / setLineWidth / setLineStipple / draw / drawText。
  状态（colour/lineWidth/stipple）保存在 `DrawContext`（每次 draw action
  创建、经存活表注册，draw 返回摘除——draw 外调用 → kOfxStatFailed）。
  GL 绘制是**真实渲染**：gl_bridge 的 CGL 上下文是 3.2 core（无固定管
  线），draw 用最小着色器 + VAO/VBO 按正交投影画线/矩形/多边形/椭圆，
  非不透明色按 "over" 合成。drawText 无字体光栅化器 → 如实返回
  kOfxStatErrUnsupported。非 macOS 无 GL 桥 → kOfxStatFailed。
- **GL 上下文模型**：宿主在调 draw 前经 `gl_bridge::acquire` 保持
  current 整个 action（与 WG1 渲染路径同模型）；`acquire` 支持同线程
  可重入（测试"一次 acquire 覆盖 FBO 装配 → draw → 回读"）。
- vendored 头新增：`ofxDrawSuite.h`、`ofxKeySyms.h`（kOfxPropKeySym/
  kOfxPropKeyString + kOfxKey_*）、`ofxPixels.h`（OfxRGBAColourF）——
  官方 openfx（BSD-3-Clause），与既有 vendored 头同源。
- 验证：`tests/interact_test.rs`（生命周期 + 事件参数逐条断言插件侧
  marker 记录；Escape key_down 的 ReplyDefault 透传；实例销毁连带
  destroy）+ GL draw 端到端（`OAK_GPU_TESTS` 门：插件 glClear 暗背景 +
  Draw suite setColour(0.9,0.1,0.2,1) + draw(Rectangle) → 回读断言矩形
  颜色与背景）。

app 接线（WG3b）公共 API：`Instance::new_interact` / `Instance::interact` /
`Instance::describe_interact`、`Interact::{describe, create_instance, draw,
pen_motion, pen_down, pen_up, key_down, key_up, idle, gain_focus, lose_focus,
destroy, props, swap_requested, redraw_requested}`、`host::KEY_*` 关键码、
`fetch_suite("OfxInteractSuite"/"OfxDrawSuite", 1)`。

## 与 M11 §3.5 验收的对照（第 1+2 期现状）

| 验收项 | 状态 |
| --- | --- |
| 0 期 golden master 全绿（描述符 diff 空、CPU 渲染 bit 一致） | **未达成**：0 期基建（`tests/ofx/` 的快照/帧库）尚未生成，`golden_test.rs` 对应用例 `#[ignore]`（原因见各用例 doc）；渲染 golden 另依赖真 liboakrender 的桥。 |
| 测试插件 round-trip + CImg 全量 describe/协商冒烟 | **部分**：测试插件 round-trip 绿（suites/ffi/lifecycle/negotiation/colour/gl/driver）；CImg 全量冒烟被本机 CImg bundle（Natron 分支，属性套件句柄语义不兼容）所阻——`#[ignore]`；`system_misc_ofx_bundle_smoke` 在本机 Misc bundle（同样为 Natron 分支，describe 返回 MissingHostFeature）上 skip，兼容 bundle 的机器照常断言。 |
| 生命周期：create/destroy 配对、重复 scan、render 中途 cancel、alive 无泄漏 | **达成**：lifecycle_test 全绿（含 256 次循环与 stub 渲染取消）。 |
| nm：liboaknode/liboakrender→liboakplugin C++ 符号为 0；HostSupport 移出构建 | 由外层 CMake 接线验证（本 crate 侧：ffi.rs 仅导出 C ABI）。 |
| cargo test（host 内部单测）+ ctest（C ABI 层）双绿 | **达成**（cargo test 两种模式全绿；ctest 属 standalone 树外层）。 |
| M11 §4：GL 路径 + ofxColour + pluginrenderer 收编 | **达成**（本机验证）：OpenGLRender suite v1 + render_gl（GL 上下文/格式协商规则按 ofxGPURender.h）；ofxColour 属性族 + GetOutputColourspace（ACEScg 工作空间）；render_driver 收编 + render_job C ABI（见上）。GL golden（EXR 容差）需 0 期基建 + 本机 GPU 人工确认（`OAK_GPU_TESTS` 门）。 |

## 实现纪律（实现方必读）

1. panic 不得越过插件 FFI：suite 分发表（`suites/mod.rs`）与宿主侧
   插件调用点各自 `catch_unwind` 兜底并映射错误码。
2. 实例以 `Arc<RefBox<Instance>>` 传（`RefBox` 为 facade 边界类型，
   见 `handle.rs`）；跨 FFI 的裸指针只指向堆上稳定对象（props/
   tag 打标），永不外露其地址含义。
3. 共享状态（插件缓存、instance 注册表、线程表）一律 `Mutex`；
   插件可能在其自起线程回调任意 suite（MultiThread suite 存活期）。
4. OFX 语义以 HostSupport 的行为为参照系；每个协商/时序实现点
   必须注释对应 HostSupport 文件行号。
5. 错误码、句柄布局、字符串两段式与 `include/plugin/*.h` 逐字一致。
6. 依赖政策：**优先使用成熟第三方 crate**（serde/quick-xml/thiserror
   等），不重复造轮子。选型要求：crates.io 有维护、许可
   MIT/Apache-2.0/BSD（GPL 兼容）；新增依赖在 README 登记名称与
   理由。OTIO 等大型既有 C++ 库绝不重写——继续经其 C ABI/桥接层
   使用。
7. 跨期句柄（suite 返回的纹理/图像句柄）必须指向**堆上稳定对象**
   （Box/Arc）——栈上临时对象在 suite 返回后悬垂（M11 第 2 期
   gl_render 初版踩过，见"第 1 期修复"）。

## 已知技术点（实现方注意）

- **C 变长参数**：`paramGetValue`/`paramSetValue` 等是 variadic。
  stable Rust 不能 *定义* C-variadic 函数（`c_variadic` 仍不稳定）。
  两个选项：(a) 这几个函数用 nightly 的 `c_variadic`；(b) 用
  build.rs 编一个几十行的 C shim 只承载 variadic 入口再转发。
  选 (b) 保持 stable toolchain。`core::ffi::VaList` 类型本身稳定，
  仅"定义 variadic fn"受限。
  **已落地**：message suite 的 v1/v2 入口即 C shim
  （cbits/ofx_message_shim.c，build.rs 经 `cc` 编译；`cc` 仅 build
  依赖，理由见下）。param suite 的 variadic 同理（届时同方案）。
- **依赖登记**：`cc`（build-dependencies）——编译 C shim 的唯一
  稳妥方式（手写 `Command::new("cc")` 无法处理跨平台 flag/交叉编译；
  零运行时依赖不变）。M11 第 2 期未新增依赖。
- **依赖登记**：`thiserror`（dependencies）——为 crate 错误枚举
  （src/error.rs）派生 `Display` + `std::error::Error` 的惯例实现；
  项目内其他模块（oakotio 等）已采用同版本（"2"），避免手写样板。
- suite 函数表经 `suite_v1()` 等 accessor 暴露（`static` 初始化
  放 lazy/OnceLock 里），`fetch_suite` 只查表。
- **GL 上下文归属**：oakplugin 不持有 GL 上下文（C ABI 无
  make-current 函数）；render_job 的 GL 契约要求调用方（oakrender）
  置上下文 current 并附着输出纹理。GL 纹理上传/回读经 oakrender C
  ABI（其内部确保上下文）。

## 第 1 期修复（M11 第 2 期发现并修复的 phase-1 缺陷）

1. **ffi::oakplugin_instance_render 泄漏帧句柄**：render 失败路径
   `?` 提前返回时不释放 `frame`（texture_get_frame 的保留引用）。
   修复：统一走 `render_driver::write_output_frame`，所有路径释放。
2. **CPU 拷贝假设紧凑行**：fetch_image / store_output_image / ffi
   render 用紧凑行宽拷贝——真实 oakrender 帧可有行填充。修复：
   新增 `frame_linesize_bytes` 导入，行优先拷贝按 linesize。
3. **描述符未预定义 GL/colour 属性**：属性 suite 的 propSet 对未
   定义属性返回 ErrUnknown（与 HostSupport 一致：宿主预定义属性
   宇宙）；第 1 期描述符表缺 kOfxImageEffectPropOpenGLRenderSupported/
   kOfxOpenGLPropPixelDepth/ofxColour 族 → 插件无法声明 GL/colour
   能力。修复：`init_descriptor_props` 补齐预定义（默认
   "false"/空数组/None）。

## 阶段 6a：OpenFX 引擎接线（oaknode/oakrender/oakplugin 收编）

把 OFX 插件接进 oaknode 节点工厂与 oakrender 评估环的接线层
（此前插件侧只有宿主/suite/渲染驱动，未进节点图）。全部落在
crates/，不动 src/（app）与 gpui/。

- **`node_factory.rs`（新增）**：
  - 实例注册表：`register_instance/instance_from_id/unregister_instance`
    （u64 键 ↔ `Arc<RefBox<Instance>>`；进程级存活，对齐 C++ 工厂
    持有；节点经 [`oaknode::nodes::plugin::PluginInstanceHandle`]
    持键）。
  - `register_plugin_nodes()`：遍历宿主插件缓存，filter 上下文优先
    （否则首个），经 `Factory::register_dynamic` 注册动态节点条目
    （已存在 id 跳过，对齐 C++ existing_ids）。返回新注册 id 列表。
  - 参数翻译 `build_core`：15 类 OFX 参数 → oaknode 输入（类型表、
    默认值缓存、颜色语义启发式、combo ChoiceOrder 排序、secret→
    hidden、ui_group/ui_page、min/max/tooltip、clip→纹理输入、
    effect_input 选择），逐条对齐 engine/node/plugins/plugin.cpp。
  - `install_render_executor()`：把 render_driver 装进
    `oakrender::eval::set_plugin_executor`（依赖反转），duplicator
    装进 `oaknode::nodes::plugin::set_plugin_duplicator`。
  - `set_project_extent(w,h)`：normalised 坐标默认值换算基准。
- **`gl_bridge.rs`（方案 B 落地）**：`texture_id` 桩的 GL 互操作评估
  结论——方案 A（wgpu-hal GL 互操作）在 macOS 不可行（Metal 后端无
  GL 命名空间）；**方案 B（独立离屏 GL 上下文 + 回读）已落地**：
  macOS CGL 离屏上下文（进程级共享、`acquire()` 串行 + current）、
  输出 GL 纹理/FBO 挂载、glReadPixels 回读装帧（垂直翻转、格式转
  换）、输入 clip 真实 GL 纹理上传。use_opengl 决策与 GL suite 的
  OpenGLTextureIndex 接通真实 GL 名。Linux（EGL）/Windows（WGL）
  预留 cfg stub。验证：单元测试（clear → 回读断言）+ 端到端
  （GL 测试插件真实渲染）经 `OAK_GPU_TESTS` 门。
- **`progress.rs`**：新增 `UiProgressReporter` trait +
  `ReporterFactory` + `set_reporter_factory`（app 注入点）。render
  未装 C 回调时装静默报告器，progressStart 携 (label,message) 经
  工厂现造 UI 报告器；update 返回 false 即取消。
- **`suites/timeline.rs`**：新增 `ViewerTimeInfo` +
  `ActiveViewerProvider` + `set_active_viewer_provider`（app 注入
  点）。timeline suite 的 getTime/getTimeBounds 在渲染上下文缺失时
  回退活动 viewer 时间源。
- **`render_driver.rs`**：`apply_param_overrides` 增 Double 标量
  NaN/Inf 清洗（回退默认并告警）+ Min/Max 钳制（对齐
  pluginrenderer.cpp:155-177）。
- **`clip.rs`**：`fetch_image` 支持 U8/U16/F16 输入帧归一化转 F32
  （对齐 oliveclip.cpp setInputTexture 的格式转换路径）；转换中的
  NaN/Inf 清洗为 0（oliveclip.cpp copy_pixels 的 scrub）；新增
  `f16_to_f32` 手写位转换。
- **oakrender `eval.rs`**：`JobSpec::Plugin` 扩为携带
  instance/time/effect_input_id/inputs/values；新增
  `PluginExecutor` + `set_plugin_executor` 依赖反转槽；
  `process_plugin_job` 经执行器出帧，失败回退紫帧 (1,0,1,1)；
  `RenderEvalHooks` 实现 `RenderHooks::resolve` 解
  `PluginJobPayload` 盒并执行。
- **oaknode**：`factory.rs` 动态注册面（`register_dynamic`/
  `dynamic_entries`/`create_any` 等）；`nodes/plugin.rs` 重写为
  PluginJobPayload 值模型 + duplicator 槽；`traverser.rs` 纹理输入
  直通；`handle.rs` `get_checked<T>` 按 TypeId 判别盒类型。

app 接线（阶段 6b，不在本 crate 范围）经这些公共入口接入：
`node_factory::register_plugin_nodes`、
`progress::set_reporter_factory`、
`suites::timeline::set_active_viewer_provider`、
`node_factory::set_project_extent`、
`oaknode::factory::Factory::global().dynamic_entries/create_any`。

## 测试

运行（全量，含渲染像素路径的 oakrender 测试桩）：

```sh
cargo test --features test-stubs
cargo tarpaulin --out stdout --features test-stubs   # 覆盖率门槛
```

- 默认 `cargo test` 亦可：桥经 dlsym 解析 oakrender/oaknode/oakundo
  符号（缺失时渲染/回写边界给出可解释错误），桩相关用例不编译；
- `--features test-stubs`：库内 oakrender/oaknode/oakundo 桩
  （`bridge::*::stub`），ffi/clip/param 的桥调用（含像素读写、
  节点回写、undo 打包）全链路可跑——**覆盖率以该模式为准**
  （M11 第 1 期实测 82.93% 行覆盖；第 2 期门槛 ≥80%，见 COVERAGE）；
- 最小测试插件（cbits/oak_test_plugin.c，四个入口：
  org.oak.test-plugin / org.oak.test-plugin.gl /
  org.oak.test-plugin.identity / org.oak.test-plugin.interact）由
  build.rs 编译为共享库，
  `common::test_plugin_dir` 运行时装配成 bundle；不可用时相关用例
  skip；
- 宿主单例无锁：触碰宿主面的用例经 `common::with_host` 串行化。
- **系统级冒烟**：`bridge_test::system_misc_ofx_bundle_smoke` 在
  `/Library/OFX/Plugins/Misc.ofx.bundle` 存在时扫描→describe→
  create_instance→协商；本机 bundle 为 Natron 分支（describe 返回
  MissingHostFeature）时 skip（README 已记录），兼容 bundle 的机器
  照常断言。
- **GL 用例策略**（`gl_render_test.rs`）：`test-stubs` 模式下桩 GL
  渲染器（renderer_is_open_gl/texture_create/texture_id 等）模拟
  GPU——suite 往返、GL render 路径（attach/detach 配对、插件经
  message 上报纹理索引）、错误路径全链路可跑；默认模式（无
  liboakrender）断言优雅降级（GL suite 无上下文 →
  MissingHostFeature、render 回退 CPU）。真实 GPU golden 为
  `OAK_GPU_TESTS` 门（`common::gpu_available`），CI 跳过。
- **golden 用例**（`golden_test.rs`）因 M11 0 期基建（tests/ofx/ 的
  快照与帧库）尚未生成而 `#[ignore]`（原因写在各用例 doc）；其中
  CImg 全量冒烟另受本机 CImg bundle 的 Natron 分支句柄语义所阻
  （详见用例注释），真实插件路径由 Misc 系统冒烟覆盖。

TDD：测试声明与实现声明同步冻结（tests/，函数体 `todo!()`）：

- `handle_test.rs` / `property_test.rs` — 基础设施契约（RefBox
  容器、属性集语义、并发）。
- `suites_test.rs` — 八张 suite 的 round-trip（经最小测试插件，
  "插件视角"的 HostSupport 兼容性背书）。
- `ffi_host_test.rs` / `ffi_instance_test.rs` — C ABI 出口契约
  （每函数一正常一错误路径；两段式字符串边界）。
- `lifecycle_test.rs` — 创建/销毁配对、重复扫描、渲染取消原子性、
  256 次循环无泄漏、并发实例。
- `negotiation_test.rs` — 协商重灾区专项（分量/位深矩阵、RoD/RoI、
  isIdentity、field 透传、sequence 括号）。
- `colour_test.rs` — ofxColour 属性族 + GetOutputColourspace 往返
  （偏好采纳、交叉引用解析、输出写回；ACEScg 工作空间）。
- `gl_render_test.rs` — GL suite 往返/错误路径/像素深度协商矩阵 +
  GL render 路径端到端（无 GPU 优雅跳过策略见上）。
- `interact_test.rs` — Interact 宿主：生命周期 + 事件调用面（插件侧
  marker 逐条断言）、Escape 状态透传、实例销毁连带、draw 真实 GL 渲染
  断言（`OAK_GPU_TESTS` 门）。
- `render_driver_test.rs` — render_job CPU 路径（序列括号、多输入、
  参数覆盖、isIdentity 透传像素断言、无桩降级）。
- `bridge_test.rs` — node/render/undo 三桥（`--features test-stubs`
  全链路；默认模式走 dlsym 缺失的降级路径）+ Misc 系统冒烟。
- `golden_test.rs` — 描述符快照 diff + CPU bit 级/GL 容差渲染
  golden + F32+ACEScg 链路断言（0 期基建未落地，见上）。
- `common/mod.rs` — 测试插件定位、golden 目录、GPU 门、skip 约定。
