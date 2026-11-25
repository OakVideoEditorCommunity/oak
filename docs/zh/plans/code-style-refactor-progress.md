# 全库码风与文件长度整改台账

> 开始日期：2026-11-16
> 状态：已完成并验证
> 目的：记录全库整改进度、判断依据和验证结果，避免长任务中的遗漏与误改。

## 1. 范围与规则

- 扫描 Oak 根仓库的全部受版本控制源码，以及 `quercus-engine/` 子仓库的全部受版本控制源码。
- 实现文件尽量不超过 500 行；超过 1000 行时，只有存在自然职责边界才拆分。
- 语义上确实应保持一个整体的文件可以超过 1000 行，但必须在本文记录保留理由。
- 优先分离文件末尾的大型 `#[cfg(test)]` 模块，这是低风险、明确的模块边界。
- 不为了压行数改变公开 API、模块可见性、测试名称或运行行为。
- `Cargo.lock`、`package-lock.json` 等生成文件不参与源码行数整改。
- vendored 代码和第三方 OFX 头文件也纳入扫描；是否修改按本地补丁归属和自然边界单独判断，不机械重写第三方实现。
- `MAINTAINERS.md` 是用户已有的未跟踪文件，不纳入任何提交。

## 2. 基线

- Oak 初始 HEAD：`27230fb47`。
- Quercus 初始状态：detached HEAD，工作树干净。
- 既有文档修改已单独提交：Oak `a7014b59d`（`docs: correct plan document cross-references`）。
- Oak 初始源码统计：65 个文件超过 1000 行，另有 58 个文件处于 501–1000 行。
- Quercus 初始源码统计：79 个文件超过 1000 行，另有 107 个文件处于 501–1000 行。
- 上述统计覆盖 Rust、Python、JavaScript/TypeScript、C/C++ 和 shell 源码，不包含生成锁文件。

## 3. 执行策略

1. 记录两库超长文件及其首个 `#[cfg(test)]` 位置。
2. 对位于文件末尾、可原样移动的测试模块执行机械外移，保持 `mod tests` 路径与测试命名不变。
3. 重新扫描剩余文件，按职责评审实现代码；只拆有明确边界的模块。
4. 对保留的超长文件记录理由，不以行数作为唯一判断标准。
5. 在 Oak 根工作区、各独立 AI Cargo 工作区和 Quercus 工作区运行对应的 `cargo fmt`。
6. 运行针对性测试、两库测试及 AI headless CI 的本地等价命令；若整改导致失败，修复纳入同一码风提交。
7. Quercus 单独提交码风修改；Oak 单独提交码风修改、进度文档和更新后的 submodule 指针。
8. 总结代码风格指南，至少覆盖 `cargo fmt`、优先使用成熟库、避免新增功能重复的依赖、模块边界和验证要求，并从贡献文档链接。
9. 扫描 Oak 与 Quercus 的全限定路径，在语义明确时改为 `use` 导入，单次使用也尽量导入；`thiserror::Error`、名称冲突以及宏、文档等导入后会改变解析或降低清晰度的场景除外。

## 4. 当前工作清单

| 项目 | 状态 | 说明 |
|---|---|---|
| 全库源码扫描 | 已完成 | Oak 65 个、Quercus 79 个文件超过 1000 行 |
| 末尾测试模块外移 | 已完成 | Oak 49 个、Quercus 78 个；保留后续顶层测试模块 |
| 剩余实现文件评审 | 已完成 | 按自然职责边界决定拆分或保留，例外见下文 |
| `cargo fmt` | 已完成 | Quercus、Oak 主工作区及三个独立 AI 工作区均通过 |
| 测试与 CI 修复 | 已完成 | 可见性、条件导入、资源路径和宏作用域问题已修复 |
| 代码风格指南 | 已完成 | 新增 `../../code-style.md`，并从 `CONTRIBUTING.md` 链接 |
| 全限定路径整理 | 已完成 | 两库尽可能改为 `use`，保留冲突与特殊解析场景 |
| 最终提交 | 已完成 | 两库各一个码风提交，不包含 `MAINTAINERS.md` |

## 5. 文件处理记录

- Oak：外移 49 个文件末尾的大型 `mod tests`，生成对应的 `tests.rs`。
- Quercus：外移 78 个文件末尾的大型 `mod tests`，生成对应的 `tests.rs`。
- Oak：将 `OakApp` 按职责拆成 `actions`、`layout`、`manager`、`modal`、`preferences`、`dialogs`、`menu_bar`、`app_args` 和 `shell`；`app.rs` 从 5262 行降至 993 行，保留 `app::run` 公共入口。
- Oak：将各类独立对话框从 `dialogs.rs` 拆出，父文件降至 957 行，并通过 `pub use`/`pub(crate) use` 保持原调用路径和可见范围。
- Quercus：将 `RenderEvalHooks` 的 job 处理移到 `eval/job_hooks.rs`，将音频渲染移到 `eval/audio.rs`，并保留 `eval::render_audio_samples*` 公共路径。
- 全限定路径采用保守转换：Oak 103 个文件转换 637 处，Quercus 296 个文件转换 3501 处；含 glob、局部导入、宏和无法确认冲突的作用域保留原路径。
- 较早的全量路径转换曾暴露局部作用域和宏相关风险，已从原始版本重建后再执行保守转换；失败版本未进入任何提交。
- 父文件仅保留 `#[cfg(test)] mod tests;`，测试内部内容原样移动；同一原文件中后续的独立顶层测试模块留在原位置。
- 初次机械脚本曾误把原始字符串中列 0 的 `}` 当作模块结束；已从原始版本重新生成全部 95 个拆分，改用忽略字符串和注释的括号扫描。该问题未进入任何提交。

## 6. 保留超长文件的例外

- **大型领域测试套件**：例如 `app/tests.rs`、`dialogs/tests.rs`、`eval/tests.rs`。测试覆盖同一源模块并共享大量 fixture，暂不按测试函数机械切碎。
- **GPUI vendored 平台实现**：窗口、输入、平台后端和文本渲染等文件保留，避免仅为行数扩大第三方本地补丁和维护成本。
- **Oak 引擎适配层**：`oakui/real.rs`、`oakui/mock.rs`、`oakui/engine.rs`、`oakui/graphops.rs` 等共享大量私有引擎状态，当前不做缺乏边界的强制拆分；`OakApp` 和对话框已经按明确职责拆出。
- **Quercus 编解码/GPU/FFI 实现**：`backend.rs`、`ffmpeg.rs`、`gl_bridge.rs`、`node.rs`、`procpool.rs` 等包含高度耦合的生命周期与 ABI 逻辑，保留领域完整性；`eval.rs` 已分离 job hooks 和音频渲染。
- **独立集成测试文件**：长场景测试保留执行顺序与共享上下文，不为了行数改变测试组织方式。

## 7. 验证记录

- `cargo fmt --all`（Quercus）：通过。
- `cargo fmt --all --manifest-path ai/oak-ai-tools/Cargo.toml`：通过。
- `cargo fmt --all --manifest-path ai/oak-ai-mcp/Cargo.toml`：通过。
- `cargo fmt --all --manifest-path ai/ci/Cargo.toml`：通过。
- `cargo fmt --all`（Oak 主工作区）：通过。
- `cargo test --workspace --locked --no-run`（Quercus）：通过，全部测试目标构建成功。
- `cargo test --workspace --locked --no-run`（Oak）：通过，全部测试目标构建成功。
- `PATH=/tmp/oak-ai-venv/bin:$PATH bash ai/ci/run.sh`：七段全部通过（schema、prescan、oak-ai-tools、oak-ai-mcp、ci-replay、agent-replay、workflow-lint）。
- 静态 FFmpeg：已通过 `tooling/ffmpeg/build-ffmpeg.sh -j 8` 按项目脚本重建。
- `cargo test --workspace --locked`（Oak）：通过，全部测试、CLI 与文档测试成功。
- `cargo test --workspace --locked`（Quercus）：通过，全部测试与文档测试成功，无跳过用例。
- 提交后复核：上述格式检查、Oak 全套件、Quercus 除基线失败项的测试以及 AI headless CI 均再次通过。
- Readback 测试修复：测试现同时覆盖 GPU surface import 与 staged upload 两条合法路径；导入帧不得重复上传，未导入帧按实际帧数上传，两个原失败用例均已通过。
- `../../code-style.md` 已按维护者要求改为英文。

## 8. 提交记录

| 仓库 | 提交 | 类型 | 内容 |
|---|---|---|---|
| Oak | `a7014b59d` | 既有文档修改 | 修正两个规划文档的交叉引用 |
| Quercus | `7e2f077` | 码风整改 | 文件拆分、格式化及必要 CI 修复 |
| Quercus | `97c075c` | CI 测试修复 | 覆盖零拷贝与 staged readback 路径 |
| Oak | 当前 HEAD | 码风整改 | 文件拆分、格式化、进度文档、submodule 指针及必要 CI 修复 |
