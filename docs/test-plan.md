# Olive Testing Strategy and Plan

This document describes the automated testing strategy for Olive, including unit tests, integration tests, and CI execution.

## Goals

- Maximize automation and reduce manual testing.
- Cover all modules with at least one automated test.
- Keep integration tests headless (no GUI interaction).
- Make failures reproducible on Windows/macOS/Linux CI.

## Test Layers

### 1) Unit Tests (GoogleTest)
- Focus: small units, deterministic behavior, no GUI.
- Location: `tests/gtest/`.
- Execution: `ctest` target `olive-gtest`.

### 1.5) Module Smoke Tests (GoogleTest)
- Focus: compile-time and link-time coverage for GUI-heavy modules without instantiating widgets.
- Location: `tests/gtest/module_smoke_test.cpp`.
- Execution: `ctest` target `olive-gtest`.

### 2) Integration Tests (GoogleTest)
- Focus: cross-module flows without GUI (e.g., serialize → deserialize → resolve).
- Location: `tests/gtest/` (prefixed with `ProjectSerializer`, `TaskManager`, etc.).

### 3) Legacy Tests (Olive macro tests)
- Existing tests in `tests/general`, `tests/timeline`, `tests/compositing` remain.

## Module Coverage Map

Each top-level module has at least one test that exercises its core API or serialization path.

- `app/common`: `common_current_test.cpp`, `common_xmlutils_test.cpp`
- `app/config`: `config_test.cpp`
- `app/node`: `node_value_test.cpp`, `node_keyframe_test.cpp`, `node_serialization_test.cpp`
- `app/node/project/serializer`: `project_serializer_test.cpp`
- `app/render`: `render_videoparams_test.cpp`, `render_audioparams_test.cpp`
- `app/timeline`: `timeline_marker_test.cpp`
- `app/undo`: `undo_stack_test.cpp`
- `app/task`: `task_taskmanager_test.cpp`
- `app/codec`: `codec_frame_test.cpp`
- `app/pluginSupport`: `plugin_support_test.cpp`
- `app/audio`, `app/cli`, `app/dialog`, `app/panel`, `app/tool`, `app/ui`, `app/widget`, `app/window`: `module_smoke_test.cpp`

If a module has a GUI dependency (e.g., widgets), tests focus on non-visual data/model components.

## Integration Test Details

### Project Serializer Roundtrip
- Creates a minimal project with a built-in node.
- Saves to XML via `ProjectSerializer::Save`.
- Loads with `ProjectSerializer::Load`.
- Verifies that nodes are restored.

### Task Manager Execution
- Adds a dummy task to `TaskManager`.
- Waits for completion using an event loop.
- Verifies the task ran.

## Headless Execution

- Tests avoid QWidget usage.
- CI sets `QT_QPA_PLATFORM=offscreen` to prevent GUI initialization issues.

## Continuous Integration

CI runs on Windows, macOS, and Linux:

1. Install system dependencies (Qt, FFmpeg, OpenImageIO, OpenColorIO, OpenEXR, PortAudio, Expat).
2. Configure with `-DBUILD_TESTS=ON`.
3. Build with CMake + Ninja.
4. Run `ctest` with output on failure.

### Dependency Installation Notes
- Linux: use distro packages (`apt` on Ubuntu) for Qt6, FFmpeg, OpenImageIO, OpenColorIO, OpenEXR, PortAudio, Expat, OpenGL headers.
- macOS: use Homebrew for Qt6 and media/color/image libraries.
- Windows: use system installers where available (Qt via `install-qt-action`), and vcpkg for the remaining C/C++ libraries.

## Adding New Tests

- Place new unit tests in `tests/gtest`.
- Use GoogleTest conventions.
- Prefer deterministic fixtures and local-only resources.
- When adding a new module, add at least one unit test and one integration scenario if applicable.
