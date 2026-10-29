# P8：独立 SDK 消费者与交付收口

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P8。
> 状态：已完成。示例、C 头文件、SDK 文档与发布元数据均已落地。

## 1. 独立消费者：`examples/sdk_editor`

`examples/sdk_editor/Cargo.toml` 的 Oak 依赖只有 `oak-engine-rust-sdk`，
不依赖 `oak-app` 或任何内部 crate。

headless 主流程（`src/lib.rs::run`）逐步覆盖 P8 验收：

1. 初始化 color/text/audio/OFX/render 后端（headless 用 inline Threads 后端）；
2. 新建工程、导入 `assets/test_clip.mp4`、遍历项目树；
3. 建序列（默认 V1/V2/A1/A2）、放置视频/音频片段；
4. 分割、修剪、移动、波纹删除；
5. 插入效果、写两个关键帧、读取参数快照；
6. source/program 预览 + 音频块（拥有型帧/采样，generation 检查）；
7. 单步 undo/redo 并验证时间线状态；
8. 保存并重开工程（效果与关键帧保留，NodeId 重新解析）；
9. 通过统一任务 API 导出 H.264 MP4 并轮询到成功。

`tests/workflow.rs` 是自动验收入口：

```bash
cargo test -p sdk_editor
```

## 2. 第二个前端

`--gui` 启动零第三方依赖的终端前端（`src/shell.rs`）：用同一个
`EditorSession` 渲染 ANSI 帧，支持播放/逐帧/seek/电平/退出。

```bash
cargo run -p sdk_editor -- --gui /tmp/oak-sdk-demo
```

它证明“同一 SDK 会话可以驱动另一个前端”，而不是必须复用 gpui 应用壳。
（原计划尝试 `minifb` 窗口，但当前构建环境无法拉取新的 registry 依赖；
终端前端保留同样的验收意义且不增加依赖。）

## 3. C 头文件

- 生成器：`tooling/gen_c_header.py`
- 产物：`crates/oak-engine/include/oak_engine.h`
  （30+ POD、13 枚举、600+ 函数；C11 与 C++17 均可编译）
- 数据来源：Rust SDK 的 `unsafe extern "C"` 声明与 `#[repr(C)]` 镜像，
  因此 SDK 与 C 头文件不会各自漂移。
- 布局断言：`Rational`/`TimeRange`/`NodeValue`/`StorageProjectHandle` 用
  `_Static_assert` 固化。
- CI：
  - `python3 tooling/gen_c_header.py --check`（漂移检查）
  - `cc -std=c11 -Wall -Wextra -Werror -fsyntax-only`（头文件可编译）

## 4. SDK 文档与发布

- `crates/oak-engine-rust-sdk/README.md`：生命周期、线程、错误、所有权、
  任务/事件、C ABI 与发布顺序；并作为 crate 文档嵌入。
- `crates/oak-engine/README.md`：引擎分层、C ABI 规则、链接方式。
- `docs/zh/sdk-guide.md`：给新前端的完整 SDK + C ABI 指南。
- 元数据：两个 crate 均带 license/repository/readme/keywords/categories/
  `docs.rs` 配置；内部 path 依赖同时带 `version = "=0.5.2"`。
- 发布顺序（依赖拓扑）：
  `oak-core → oak-ffmpeg-link → oak-codec → oak-audio → oak-undo →
  oak-node → oak-render → oak-ofx-plugin → oak-otio → oak-storage →
  oak-task → oak-timeline → oak-engine → oak-engine-rust-sdk`。
- 本环境 registry 镜像没有这些 crate，因此 `cargo package` 的依赖解析
  会在前序 crate 未发布时失败；`cargo package --list` 已验证打包内容
  （引擎包含 `include/oak_engine.h`，SDK 包含 README 与源码）。

## 5. 验证

```text
cargo test -p sdk_editor                                  1 passed
cargo test --workspace -- --test-threads=1                3305 passed
cargo check --workspace --all-targets                     OK
cargo doc -p oak-engine-rust-sdk --no-deps                OK
python3 tooling/gen_c_header.py --check                   up to date
cc / c++ -fsyntax-only include/oak_engine.h               OK
cargo package -p oak-engine-rust-sdk --list               README + src
cargo package -p oak-engine --list                        include/oak_engine.h
```

## 6. 剩余

- 三平台 CI 构建/链接/冒烟仍待实际运行（P8 本地只验证了本机链接）。
- 若后续恢复窗口 toolkit 依赖，可在 `sdk_editor` 中把终端前端替换/并存为
  `minifb`/`winit` shell，不影响 SDK 与 C ABI。
