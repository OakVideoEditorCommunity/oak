# OpenFX 支持 TODO（中文说明）

下面是 README 里 OpenFX TODO 的中文翻译与详细说明。每条都尽量解释“是什么、为什么需要、该往哪里改”。

1) 实现插件发现与加载流程
- 现状：`app/pluginSupport/OliveHost.cpp` 里只创建了 Host 和 PluginCache，但没有扫描路径/加载 OFX 插件。
- 为什么需要：没有完整的发现/加载，就无法让用户看到插件或实例化插件。
- 可能改动：补全 `loadPlugins()`，遍历指定目录（如 OFX 标准路径），调用 OpenFX 的插件缓存加载逻辑，注册可用插件并让 UI/节点系统可用。

2) 输出剪辑图像的缓冲区管理（Output Clip）
- 现状：`OliveClipInstance::getImage()` 返回空的 OFX Image，没有分配像素内存，也没有设置 `kOfxImagePropData`。
- 为什么需要：插件渲染时会往 `kOfxImagePropData` 写入像素，如果这里没分配就会导致崩溃或黑屏。
- 可能改动：为 Output Clip 分配像素缓冲（例如 `std::vector<uint8_t>`），填充 `kOfxImagePropData`、`kOfxImagePropRowBytes`、`kOfxImagePropBounds`，并保证生命周期覆盖渲染过程。

3) 输入剪辑图像的拉取（Input Clip Fetch）
- 现状：`OliveClipInstance::getImage()` 对输入剪辑直接返回一个空 Image，没有真正把输入帧填进去。
- 为什么需要：多数插件需要输入图像做处理，没有输入就无法正确工作。
- 可能改动：根据当前时间 `time` 从渲染管线/缓存/纹理中取出输入帧，设置 OFX Image 的 data/bounds/rowBytes 等属性并返回。

4) 渲染路径中设置每帧输出数据与 ROD/Bounds
- 现状：`app/render/plugin/pluginrenderer.cpp` 中已接了 Image->AVFrame，但仍需要保证每帧输出图像属性正确（ROD/Bounds）。
- 为什么需要：插件对 ROD 和 bounds 非常敏感，用错会导致裁剪错误或错位。
- 可能改动：在渲染前或 render action 前，按当前时间/ROI 计算并设置 Output Clip 的 `kOfxImagePropBounds`、`kOfxImagePropRegionOfDefinition` 等。

5) 参数类型支持不完整（Param Instances）
- 现状：`OlivePluginInstance::newParam()` 只支持少量类型，`app/node/plugins/Plugin.cpp` 里也没有处理 Group/Page 等。
- 为什么需要：复杂插件大量依赖 String/3D/Custom 等参数，不支持就会缺参数或崩溃。
- 可能改动：补齐 String、Double3D/Integer3D、Group/Page、Custom/Bytes 等参数实例，并在节点输入映射里增加对应类型。

6) editBegin/editEnd、Progress、Timeline 等回调还只是空实现
- 现状：`OlivePluginInstance` 中多处函数是空壳或默认返回。
- 为什么需要：插件在编辑参数、显示进度、根据时间线上下文渲染时依赖这些回调。
- 可能改动：实现 editBegin/editEnd 通知；progressStart/Update/End 与 UI 进度条连接；timelineGetTime/GotoTime/Bounds 与工程时间轴连接。

7) 持久消息展示与清理机制需要完善
- 现状：我们已在 Host/Instance 里保存消息并能弹窗，但 UI 面板还需要稳定地展示、更新和清理。
- 为什么需要：插件经常用 persistent message 提示错误或警告，需要可追踪、可清除。
- 可能改动：统一消息存储（Host/Instance），提供 UI 列表、清除按钮、计数徽标，并保证信号更新。

8) Project Extent / Fielding 行为待确认
- 现状：`OlivePluginInstance::getProjectExtent()` 有 “TODO” 注释。
- 为什么需要：OFX 插件会根据项目尺寸、扫描线场信息做渲染决策。
- 可能改动：明确工程是否支持不同 extent/fielding，确保返回值与项目设置一致。

9) OpenGL Render Suite 支持
- 现状：`OliveClipInstance::loadTexture()` 返回 null，OpenGL 渲染路径未实现。
- 为什么需要：有些 OFX 插件只支持 OpenGL 渲染，不支持 CPU 渲染。
- 可能改动：要么实现 OpenGL texture 的加载与生命周期，要么在 host capability 中明确禁用 OpenGL render。

---

如果你希望，我可以把这些 TODO 拆成“优先级 + 预计工作量 + 依赖关系”的形式，方便你逐条推进。
