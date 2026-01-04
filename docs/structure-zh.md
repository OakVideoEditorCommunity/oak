Olive 项目结构概览（中文）
==========================

这份文档是基于当前仓库目录组织的快速导航，便于后续查找代码位置。

顶层目录
--------
- app: 主应用源码入口，涵盖核心、渲染、UI、插件、节点系统等。
- cmake: CMake 相关脚本与模块。
- docker: 构建/运行相关的容器配置。
- docs: 项目文档（你现在正在看的位置）。
- ext: 可能包含外部依赖或子模块（按需查看）。
- tests: 测试代码与用例。
- third_party: 第三方库及其源码（如 OpenFX HostSupport）。
- build、cmake-build-debug、test_compile: 构建产物或构建目录（通常不需要手动改）。

app 目录（核心模块）
-------------------
- app/core.*: 应用核心入口、初始化流程。
- app/main.cpp: 程序入口点。
- app/version.*: 版本信息与构建元数据。
- app/common: 通用基础设施与工具类（日志、路径、字符串等）。
- app/config: 配置加载与项目设置。
- app/render: 渲染子系统（帧缓存、渲染管线、插件渲染桥接等）。
- app/node: 节点系统，节点类型与图结构的核心逻辑。
- app/widget: UI 控件与节点视图（节点图、参数面板等）。
- app/panel: UI 面板组织与管理。
- app/window: 窗口与主界面。
- app/timeline: 时间线与剪辑管理。
- app/tool: 交互工具（选择、裁剪等）。
- app/undo: 撤销/重做系统。
- app/task: 异步任务与后台作业。
- app/audio: 音频处理与播放。
- app/codec: 编解码相关支持。
- app/shaders: 渲染着色器资源。
- app/ts: 时间/时间轴相关通用类型。
- app/dialog: 对话框与提示类 UI。
- app/cli: 命令行工具入口或相关实现。
- app/pluginSupport: OpenFX 插件 Host 侧实现（Clip/Image/Param/Host/PluginInstance 等）。
- app/packaging: 打包或发布相关逻辑。

重点文件索引（按模块）
----------------------
下面列的是“常用/核心入口”文件，不是完整清单，但足够定位主要流程。

核心入口与全局
-------------
- app/main.cpp: 程序入口。
- app/core.h、app/core.cpp: 应用生命周期与初始化总控。
- app/version.h、app/version.cpp: 版本与构建信息。

渲染系统
--------
- app/render/renderer.h、app/render/renderer.cpp: 渲染主调度。
- app/render/rendermanager.h、app/render/rendermanager.cpp: 渲染队列与任务管理。
- app/render/renderticket.h、app/render/renderticket.cpp: 单次渲染请求。
- app/render/renderprocessor.h、app/render/renderprocessor.cpp: 渲染处理管线。
- app/render/texture.h、app/render/texture.cpp: 纹理/帧数据容器。
- app/render/videoparams.h、app/render/videoparams.cpp: 视频格式参数。
- app/render/job/pluginjob.h、app/render/job/pluginjob.cpp: 插件渲染作业。
- app/render/plugin/pluginrenderer.h、app/render/plugin/pluginrenderer.cpp: OpenFX 插件渲染桥接。

节点系统
--------
- app/node/node.h、app/node/node.cpp: 节点基类与生命周期。
- app/node/param.h、app/node/param.cpp: 节点参数与动画/关键帧。
- app/node/value.h、app/node/value.cpp: 节点值与运行时数据。
- app/node/factory.h、app/node/factory.cpp: 节点注册与创建。
- app/node/traverser.h、app/node/traverser.cpp: 图遍历与求值。
- app/node/plugins/Plugin.h、app/node/plugins/Plugin.cpp: OpenFX 插件节点。

OpenFX Host 侧实现
------------------
- app/pluginSupport/OliveHost.h、app/pluginSupport/OliveHost.cpp: OpenFX Host 入口与消息接口。
- app/pluginSupport/OlivePluginInstance.h、app/pluginSupport/OlivePluginInstance.cpp: 插件实例生命周期与参数管理。
- app/pluginSupport/OliveClip.h、app/pluginSupport/OliveClip.cpp: Clip 实例与图像读写桥接。
- app/pluginSupport/image.h、app/pluginSupport/image.cpp: OpenFX Image 封装与数据映射。
- app/pluginSupport/paraminstance.h、app/pluginSupport/paraminstance.cpp: 参数实例实现。
- third_party/openfx/HostSupport/include/ofxhImageEffect.h: HostSupport 核心接口。

节点 UI（Node View）
-------------------
- app/widget/nodeview/nodeview.h、app/widget/nodeview/nodeview.cpp: 节点视图主控。
- app/widget/nodeview/nodeviewitem.h、app/widget/nodeview/nodeviewitem.cpp: 节点渲染与交互。
- app/widget/nodeview/nodeviewscene.h、app/widget/nodeview/nodeviewscene.cpp: QGraphicsScene 逻辑。
- app/widget/nodeview/nodeviewedge.h、app/widget/nodeview/nodeviewedge.cpp: 连线显示。

参数 UI（Param View）
--------------------
- app/widget/nodeparamview/nodeparamview.h、app/widget/nodeparamview/nodeparamview.cpp: 参数面板主控。
- app/widget/nodeparamview/nodeparamviewitem.h、app/widget/nodeparamview/nodeparamviewitem.cpp: 参数项容器与布局。
- app/widget/nodeparamview/nodeparamviewwidgetbridge.h、app/widget/nodeparamview/nodeparamviewwidgetbridge.cpp: 参数类型到控件的桥接。
- app/widget/nodeparamview/nodeparamviewtextedit.h、app/widget/nodeparamview/nodeparamviewtextedit.cpp: 多行文本参数控件。

面板与窗口
----------
- app/panel/panelmanager.h、app/panel/panelmanager.cpp: 面板管理器与切换逻辑。
- app/panel/timebased/timebased.h、app/panel/timebased/timebased.cpp: 时间基面板基类（时间轴/视图共享逻辑）。
- app/panel/node/node.h、app/panel/node/node.cpp: 节点面板入口。
- app/panel/param/param.h、app/panel/param/param.cpp: 参数面板入口。
- app/window: 主窗口与窗口级 UI 结构。

时间线/播放核心
--------------
- app/node/output/viewer/viewer.h、app/node/output/viewer/viewer.cpp: Viewer 输出节点（播放头/长度/渲染请求）。
- app/widget/viewer/viewer.h、app/widget/viewer/viewer.cpp: Viewer 面板与播放控制。
- app/widget/timelinewidget/timelinewidget.h、app/widget/timelinewidget/timelinewidget.cpp: 时间线 UI 与交互主控。

进度与任务 UI
------------
- app/dialog/progress/progress.h、app/dialog/progress/progress.cpp: 通用进度对话框。
- app/widget/taskview/taskviewitem.h、app/widget/taskview/taskviewitem.cpp: 任务进度条展示。

撤销/编辑分组
------------
- app/undo/undocommand.h、app/undo/undocommand.cpp: UndoCommand 与 MultiUndoCommand 的基础实现。
- app/undo/undostack.h、app/undo/undostack.cpp: 撤销栈（无原生“批量编辑”接口）。
- app/pluginSupport/OlivePluginInstance.h、app/pluginSupport/OlivePluginInstance.cpp: OpenFX editBegin/editEnd 触发时创建批量撤销分组。
- app/pluginSupport/OlivePluginInstance.cpp: DeferredRedoCommand 包装已应用的命令，避免批量 push 时重复执行。
- app/pluginSupport/paraminstance.h、app/pluginSupport/paraminstance.cpp: 参数 Set 走统一的 SubmitUndoCommand 接口，支持批量合并。

与 OpenFX 相关的主要位置
-----------------------
- app/pluginSupport: OpenFX HostSupport 的封装与 Olive 侧实现。
- app/render/plugin: 插件渲染调度与帧处理逻辑。
- app/node/plugins: 插件节点定义与 UI 参数桥接。
- third_party/openfx: OpenFX HostSupport 源码与接口头文件。

构建与配置
----------
- CMakeLists.txt: 根构建配置入口。
- cmake/: 自定义 CMake 模块与工具链脚本。

其他说明
--------
- README.md: 项目整体说明与开发入口。
- TODO-zh.md: OpenFX 支持的中文 TODO 说明。

流程图/调用关系（ASCII）
-----------------------
OpenFX 插件渲染主流程（逻辑简化）：
```
Node(Graph)
  -> app/node/plugins/Plugin.cpp
  -> app/render/plugin/pluginrenderer.cpp
      -> app/pluginSupport/OlivePluginInstance.cpp
          -> app/pluginSupport/OliveClip.cpp
              -> app/pluginSupport/image.cpp
                  -> app/render/texture.cpp / AVFrame 映射
```

OpenFX 参数 UI 生成流程（逻辑简化）：
```
OFX Param Descriptor
  -> app/pluginSupport/OlivePluginInstance.cpp (newParam)
  -> app/node/plugins/Plugin.cpp (Node Input 生成)
  -> app/widget/nodeparamview/nodeparamview.cpp
  -> app/widget/nodeparamview/nodeparamviewwidgetbridge.cpp (控件桥接)
```

插件消息展示流程（逻辑简化）：
```
OFX Host Message
  -> app/pluginSupport/OliveHost.cpp (保存消息)
  -> app/pluginSupport/OlivePluginInstance.cpp (发出消息数量变化)
  -> app/widget/nodeview/nodeviewitem.cpp (节点右上角徽标)
  -> app/widget/nodeparamview/nodeparamviewitem.cpp (面板顶部消息)
```
