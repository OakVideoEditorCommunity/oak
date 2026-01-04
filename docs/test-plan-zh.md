# Olive 测试策略与计划

本文档描述 Olive 的自动化测试策略，包括单元测试、集成测试以及 CI 执行方式。

## 目标

- 尽量自动化，减少人工测试。
- 覆盖所有模块（至少一个自动化测试）。
- 集成测试保持无 GUI（头less）。
- 在 Windows/macOS/Linux 上可重复运行。

## 测试层级

### 1) 单元测试（GoogleTest）
- 目标：小范围、确定性、无 GUI。
- 目录：`tests/gtest/`。
- 执行：`ctest` 里的 `olive-gtest`。

### 1.5) 模块冒烟测试（GoogleTest）
- 目标：对 GUI 相关模块做编译期/链接期覆盖，不实例化控件。
- 目录：`tests/gtest/module_smoke_test.cpp`。
- 执行：`ctest` 里的 `olive-gtest`。

### 2) 集成测试（GoogleTest）
- 目标：跨模块流程但不依赖 GUI（例如序列化→反序列化）。
- 目录：`tests/gtest/`（如 `ProjectSerializer`、`TaskManager`）。

### 3) 现有测试（Olive 宏测试）
- 目录：`tests/general`、`tests/timeline`、`tests/compositing` 保持不变。

## 模块覆盖映射

每个顶层模块至少有一个测试用例。

- `app/common`：`common_current_test.cpp`、`common_xmlutils_test.cpp`
- `app/config`：`config_test.cpp`
- `app/node`：`node_value_test.cpp`、`node_keyframe_test.cpp`、`node_serialization_test.cpp`
- `app/node/project/serializer`：`project_serializer_test.cpp`
- `app/render`：`render_videoparams_test.cpp`、`render_audioparams_test.cpp`
- `app/timeline`：`timeline_marker_test.cpp`
- `app/undo`：`undo_stack_test.cpp`
- `app/task`：`task_taskmanager_test.cpp`
- `app/codec`：`codec_frame_test.cpp`
- `app/pluginSupport`：`plugin_support_test.cpp`
- `app/audio`、`app/cli`、`app/dialog`、`app/panel`、`app/tool`、`app/ui`、`app/widget`、`app/window`：`module_smoke_test.cpp`

若模块包含 GUI 依赖，则测试聚焦于其非可视逻辑/数据结构。

## 集成测试说明

### 项目序列化回归
- 创建最小项目并添加内置节点。
- 使用 `ProjectSerializer::Save` 写出 XML。
- 再用 `ProjectSerializer::Load` 读回。
- 验证节点恢复。

### 任务管理器执行
- 向 `TaskManager` 添加一个 DummyTask。
- 使用事件循环等待完成。
- 验证任务确实执行。

## 无 GUI 运行

- 测试避免使用 QWidget。
- CI 中设置 `QT_QPA_PLATFORM=offscreen` 防止 GUI 初始化问题。

## 持续集成

CI 在 Windows/macOS/Linux 上执行：

1. 安装依赖（Qt、FFmpeg、OpenImageIO、OpenColorIO、OpenEXR、PortAudio、Expat）。
2. `-DBUILD_TESTS=ON` 配置。
3. 使用 CMake + Ninja 构建。
4. 运行 `ctest` 输出失败信息。

### 依赖安装说明
- Linux：优先使用发行版系统包（Ubuntu 上用 `apt`）安装 Qt6、FFmpeg、OpenImageIO、OpenColorIO、OpenEXR、PortAudio、Expat、OpenGL 头文件。
- macOS：使用 Homebrew 安装 Qt6 和图像/色彩/多媒体相关库。
- Windows：尽量使用系统安装器（Qt 通过 `install-qt-action`），其余 C/C++ 库通过 vcpkg 安装。

## 新增测试规范

- 新测试放在 `tests/gtest`。
- 使用 GoogleTest 规范。
- 尽量保持确定性与无外部依赖。
- 新模块至少增加 1 个单元测试 + 1 个集成场景（可合并）。
