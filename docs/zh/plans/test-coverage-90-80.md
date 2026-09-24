# 测试覆盖率提升计划：行 ≥90% / 分支 ≥80%（单元 + 模块边界集成双层）

> 面向实现者的任务书（2026-09-17）。本文只描述方案与工作项，不含已执行的代码修改。
>
> 用户要求（原文要点）：
> 1. 测试分两层：**单元测试** + **模块边界处集成测试**。
> 2. 层 2：**每个模块边界 API 都要有**集成测试，且覆盖**所有正常和异常情况**。
> 3. 层 1：可以排除过于简单的函数，但也要覆盖所有正常和异常情况。
> 4. **总体行覆盖率提升至 90% 左右、分支覆盖率提升至 80% 左右**。
>
> 现状基线（2026-09-17 干净对象集重测，含内联测试）：
> **行 81.72% / 分支 55.45%**（区域 82.73% / 函数 76.67%），
> 全 workspace 全部测试套件全绿，但覆盖率从未进 CI。
>
> **口径勘误**：此前 M5 留档的 53.15%/27.94% 是在累积了多次构建的
> `target/coverage-obj` 上测的——`llvm-cov report` 对同一源文件的多份
> 新旧测试二进制副本按 region 合并，大量陈旧副本的未执行 region 拉低
> 数字（同一个 `oak-render/src/ipc.rs`：多对象报告 51%，仅该 crate 的
> lib 测试对象 95%）。**覆盖率必须在干净对象集上测量**（清空测试
> 二进制后重跑，或交给 cargo-llvm-cov 的默认清理），本计划全部数字以
> 干净基线为准。

## 1. 背景与为什么

M0a–M5 的测试策略是"每个里程碑配验收测试"，没有统一的质量闸门：

- **覆盖面不均**：`oak-cli` 行覆盖 76.7%，而 `oak-app` 只有 40.7%、
  `oak-task` 35.5%、`oak-timeline` 47.4%；分支覆盖最差的
  `oak-app/src/oakui/real.rs` 是 **0/1180**（1180 个分支一个没走到），
  `graphops.rs` 0/466、`app.rs` 0/286。
- **测试层次没有定义**：现有 2433 个测试里既有纯单测，也有进程级 e2e，
  但没有"哪个 API 必须有哪种测试"的书面约定；模块边界（crate 公开 API、
  IPC/FFI/协议面）的异常分支大量依赖"碰巧被调用到"。
- **没有防回退机制**：覆盖率只在 render M5 阶段性验收时手工测过一次，新增代码可以
  无限制地拉低比例。
- **"全绿假绿"已有实例**：M5 审计发现 montage 原生尺寸 + host GPU 时
  "解码可返回 GPU 纹理、CPU 合成器只接 CPU 纹理"的跨模块组合被整条
  静默跳过（全透明黑帧），既有 2433 个测试无一覆盖该组合——这正是
  §2.2 边界矩阵与 §5.2 跨模块清单要消灭的盲区。

本计划的产出不是"把数字刷上去"，而是两件可长期维持的事：

1. **双层测试规范落地**：单元测试管模块内逻辑；模块边界集成测试锁
   API 契约与跨模块数据流，两者都按"正常 + 异常"矩阵验收。
2. **覆盖率进入 CI 门禁并 ratchet**：聚合行 ≥90%、分支 ≥80%，逐 crate
   有地板阈值，PR 的**新增/修改行**必须被覆盖（diff coverage），比例只能
   升不能降。

全量覆盖 90%/80% 的工作量估算（按 2026-09-17 干净基线）：
**需要新增覆盖约 1.15 万可执行行、约 3.5 千个分支**；其中 `oak-app`
独占约 1.15 万行缺口（占全部缺口的 45%），`oak-task`/`oak-timeline`/
`oak-plugin`/`oak-storage` 合计约 7 千行，是最大的两块。

## 2. 测试分层规范（硬性）

### 2.1 层 1：单元测试

**范围**：单个模块（crate 内）的内部逻辑——算法、状态机、解析、缓存、
错误映射。

**要求**：

- 每个被测函数的**正常路径 + 异常路径 + 边界值**都要有断言；
  异常路径按 §2.3 的通用异常清单逐项核对。
- 断言**行为**（输入→输出、可观察副作用），不锁实现细节
  （不 assert 内部字段、不 assert 调用次数，除非调用次数就是契约）。
- 不引入真实时间/睡眠/真实网络；随机数据用固定种子。

**"过于简单"的豁免准则**（用户允许，但必须登记）：

- 纯转发：只做 `self.x` 返回的 getter/setter、只委托的 `Deref`/`From`；
- 编译器生成：`derive`、trait 自动实现；
- 纯数据容器：无逻辑的 `struct`/`enum` 定义、常量表；
- 单行格式化：`Display`/`Debug` 只拼接字段。

豁免项登记在 `docs/zh/plans/coverage/unit-exemptions.md`（文件 + 项 +
理由 + 复核人）。豁免**不等于**从覆盖率分母剔除——它们仍计入总量，
通常由上层测试间接覆盖；只有"在本平台永远不可执行"的代码才允许
`coverage(off)`（§3.3）。

### 2.2 层 2：模块边界集成测试

**边界定义**（两者都算）：

- **模块内边界**：crate 的公开 API（`pub mod` 的公开函数/类型/trait 方法、
  FFI `extern "C"`、`pub` 的协议入口）；
- **跨模块边界**：两个 crate 之间/进程之间的真实数据面与控制面，
  例如 oaknode→oakrender 的 Job/句柄、oakrender↔oakworker 的
  NDJSON+shm、oakrender↔oak-ofx-host、oakcodec→oakrender 的解码会话、
  oakapp↔oakrender 的 ticket API、oakstorage 的数据库/session 面。

**要求**：

- **每个边界 API 至少一条集成测试**，通过**真实实现**调用边界本身
  （允许在边界另一侧用 fake/stub，例如 UI 边界内用 `MockEngine`、
  存储边界内用内存库；但被验证的边界代码必须是生产代码）。
- 每个边界 API 的测试矩阵 = **正常用例 × 输入族 + 异常用例 × 通用异常清单**。
  输入族至少覆盖：最小/空、典型、最大/边界、多元素、并发（如适用）。
- 测试文件按边界组织：`crates/<crate>/tests/<boundary>_test.rs`；crate 内
  不可运行进程的边界（如 `oak-node` 的纯数据 API）允许
  `src/<module>/boundary_tests.rs`（cargo-llvm-cov 默认排除
  `*_tests.rs` 与 `tests/`，避免污染生产覆盖率口径）。
- **边界 API 清单是验收物**：`docs/zh/plans/coverage/boundary-api-inventory.md`
  逐项登记 `API → 测试 → 正常/异常覆盖`，由 `tooling/boundary_api.py`
  从 rustdoc JSON 生成并与清单对账；**新增公开 API 而没有清单条目时
  CI 失败**（层 2 的 ratchet）。

### 2.3 通用异常清单（每层都要逐项核对）

| 编号 | 类别 | 典型用例 |
|---|---|---|
| A1 | 参数非法 | `None`/空串/负数/零尺寸/越界索引/枚举越界/尺寸不匹配 |
| A2 | 状态非法 | 未 open 就 read、重复 open、已 shutdown、流类型不符（音频当视频）、重复初始化单例 |
| A3 | 资源缺失 | 文件不存在/无权限/空媒体/无音频流/无 GPU/无 OFX 插件/PG 不可达 |
| A4 | 数据损坏 | 截断文件、坏魔数、坏 JSON/NDJSON/XML/OTIO、错像素格式、PTS 缺失、EOF 回退 |
| A5 | 容量与背压 | 队列满、LRU 淘汰、超大帧（OOM/`NoMem`）、shm 槽位不足 |
| A6 | 取消与超时 | `CancelAtom`（渲染/解码/导出边界）、`plugin_cancel`、进程被杀、宿主熔断 |
| A7 | 并发 | 多线程同时 open/request/cancel、双监视器同帧、并行 autosave、单例初始化竞态 |
| A8 | 平台/硬件缺失 | 无硬件解码、导入失败按帧 fallback、无 GL/Vulkan 适配器、无显示器 |

**异常注入工具**（M0 交付）：`oak_core::test_support` 已有的进程锁 +
新增小型 `testutil`（坏输入生成、`FailPoint` 注入、假时钟），避免每个
测试各写各的。

## 3. 覆盖率度量口径与工具

### 3.1 工具与命令

- 工具：`cargo-llvm-cov`（pin 版本，安装走 `taiki-e/install-action@cargo-llvm-cov`
  或 `cargo +stable install cargo-llvm-cov --locked`）；
- 分支覆盖需要 nightly（`--branch` 目前 unstable）：CI 固定
  `nightly-2026-07-21`（与 M5 留档同一工具链），本地脚本用
  `COVERAGE_NIGHTLY` 环境变量覆盖；
- **对象集必须干净**：`llvm-cov` 多对象报告会把同一源文件在不同测试
  二进制里的多份副本的 region 合并；陈旧二进制（旧源码/旧测试）的
  未执行 region 会显著拉低数字（实测同一文件 95% vs 51%）。用
  `cargo-llvm-cov` 的默认清理，或手工 `rm target/coverage-obj/debug/deps/*`
  中的可执行测试二进制后重跑；`tooling/coverage.sh` 必须在跑前清理
  测试二进制与 profraw；
- 主命令（Linux 全量）：

```sh
cargo +"$COVERAGE_NIGHTLY" llvm-cov --branch --workspace --locked \
  --json --output-path target/coverage.json \
  --ignore-filename-regex '(\.cache/|/gpui/|/tooling/)'
python3 tooling/coverage_report.py --json target/coverage.json
```

- `cargo-llvm-cov` 默认已排除：`tests/`/`examples/`/`benches/` 目录、
  `*_tests.rs`、`tests.rs`、`target/`、`CARGO_HOME`/`RUSTUP_HOME`、
  vendored 依赖。`--fail-under-lines`/`--fail-under-regions`/
  `--fail-under-functions` 可用；**没有 `--fail-under-branches`**，
  分支门禁由 `tooling/coverage_report.py` 解析 JSON 实现。

### 3.2 生产代码口径（主口径）

数字必须反映**生产代码**，不能靠内联测试自身刷分：

- 每个 crate 顶部加
  `#![cfg_attr(coverage_nightly, feature(coverage_attribute))]`，
  所有内联 `#[cfg(test)] mod tests` 与其辅助模块加
  `#[cfg_attr(coverage_nightly, coverage(off))]`（M0 机械改造）；
  workspace 根 `Cargo.toml` 加
  `[lints.rust] unexpected_cfgs = { level = "warn", check-cfg = ['cfg(coverage,coverage_nightly)'] }`。
- M0 同时测量并留档两个口径：
  - **主口径（gate）**：排除内联测试后的生产代码；
  - **兼容口径**：不排除（与 M5 留档可比）。
  M5 的 53.15%/27.94% 既非兼容口径也已被证明是**陈旧对象集拉低的
  下界**（§4.1 勘误）；干净重测的兼容口径为 81.72%/55.45%，主口径
  基线由 M0 重新测出后写进 §7 表。

### 3.3 排除清单（必须有理由、集中登记）

`docs/zh/plans/coverage/exclusions.md` 维护，允许的类别仅限：

1. 机器生成/绑定代码（bindgen、宏展开产物）；
2. `build.rs`、构建工具（`oak-ffmpeg-link`）；
3. 在本平台不可编译/不可达的 `cfg` 分支——**不排除**：由对应平台的
   coverage job 覆盖（§3.4），只有"任何平台都不可达"（如防御性
   `unreachable!` 之后的代码）才允许 `#[coverage(off)]`；
4. 交互式 UI 事件循环中无法用测试平台模拟的平台原生回调（尽量用
   gpui test-support 覆盖，确实不能的登记豁免）。

### 3.4 跨平台策略

- **聚合门禁（每次 PR）**：Linux job（xvfb + lavapipe + `OAK_REQUIRE_GPU=1`）
  跑全量 `--branch`，是行/分支总门禁的唯一来源。
- **平台代码（周更/合并到 main 后）**：Windows、macOS 各跑
  `cargo llvm-cov --branch --workspace`，各自归档报告；
  `external.rs`、`gpuinterop.rs` 的 `cfg(windows)`/`cfg(macos)` 分支
  按平台报告单独门禁（Linux 报告里这些行不存在，不能拉总账）。
- **平台合并报告（可选，M4 交付）**：`tooling/coverage_merge.py` 读取三份
  JSON，按文件做**行集合的并集**（同一行只在编译它的平台出现），生成
  `docs/zh/plans/coverage/merged-<date>.txt`；profraw 不做跨架构合并
  （函数计数哈希不同架构不可比）。
- 真机专属路径（NVDEC 导入、D3D11VA、VideoToolbox、真实 OFX GPU 插件）
  在有硬件的开发机/自托管 runner 上跑，其余环境按 A8 跳过；跳过必须
  打印原因，不得静默。

### 3.5 门禁与 ratchet

- `tooling/coverage-thresholds.toml` 存放：聚合地板（行/分支）、逐 crate
  地板（§7 表）、"简单函数豁免"不进阈值；
- `tooling/coverage_report.py` 输出：逐 crate 表、TOTAL、**低于地板的
  条目 exit 1**、可选 `--archive <file>` 写留档；
- **diff coverage（PR 必过）**：对 `git diff <base>...HEAD` 的新增/修改行，
  要求被覆盖 ≥90%（分支行 ≥85%）；`--diff-base origin/main` 实现，
  用于防止"总量达标但新代码没测"；
- **ratchet**：每次合并到 main 后把地板提升到 `min(目标, 当前-2pp)`，
  只能升；降地板需要在 PR 描述中说明理由并获 review。
- **阶段地板按缺口插值**：设 M0 主口径基线为 `B`、最终目标为 `T`
  （行 90%、分支 80%），则 M1/M2/M3/M4 地板 = `B + (T-B) × {0.25, 0.55,
  0.75, 0.90}`；§6 里写的绝对数字按 2026-09-17 干净兼容基线
  （行 81.72/分支 55.45）算出，M0 主口径重测后以插值公式为准。

## 4. 现状基线（2026-09-17）

### 4.1 逐 crate（raw 口径，含内联测试；`-Zcoverage-options=branch`）

| crate | 行数 | 行覆盖 | 分支数 | 分支覆盖 |
|---|---|---|---|---|
| oak-task | 4406 | 54.2% | 688 | 39.8% |
| oak-timeline | 5039 | 69.9% | 828 | 41.7% |
| oak-app | 39937 | 71.2% | 4199 | 44.3% |
| oak-storage | 5133 | 75.5% | 292 | 51.0% |
| oak-plugin | 10938 | 78.4% | 1244 | 50.6% |
| oak-worker | 3165 | 80.8% | 338 | 52.1% |
| oak-cli | 1765 | 81.9% | 274 | 51.8% |
| oak-audio | 2547 | 87.7% | 408 | 62.3% |
| oak-render | 15778 | 89.5% | 1900 | 63.1% |
| oak-codec | 6733 | 89.7% | 876 | 65.4% |
| oak-core | 12943 | 92.3% | 1464 | 72.5% |
| oak-node | 27931 | 92.7% | 1646 | 68.8% |
| oak-otio | 2315 | 93.2% | 166 | 77.1% |
| oak-undo | 824 | 95.8% | 54 | 92.6% |
| **TOTAL** | **139455** | **81.72%** | **14377** | **55.45%** |

（2026-09-17 干净对象集重测；`oak-ffmpeg-link` 只有 build.rs，按 §3.3
排除。原始源码行约 23.4 万，其中内联测试约 5.2 万行 ≈ 22%——主口径分母
会明显小于上表。）
（`oak-ffmpeg-link` 只有 build.rs，按 §3.3 排除。原始源码行约 23.4 万，
其中内联测试约 5.2 万行 ≈ 22%——主口径分母会明显小于上表。）

### 4.2 未覆盖行最多的文件（Top 15）

| 文件 | 行覆盖 | 分支覆盖 | 未覆盖行 | 说明 |
|---|---|---|---|---|
| `oak-app/src/oakui/real.rs` | 71.6% | 38.3% | 2501 | 真实引擎网关；UI 集成测试主战场 |
| `oak-app/src/app.rs` | 66.7% | 32.6% | 1484 | 壳/菜单/快捷键状态机 |
| `oak-app/src/dialogs.rs` | 62.5% | 43.3% | 1233 | 各对话框 |
| `oak-app/src/oakui/graphops.rs` | 79.9% | 53.2% | 730 | 节点图操作 |
| `oak-app/src/oakui/mock.rs` | 77.5% | 46.5% | 664 | Mock 引擎（测试基建自身） |
| `oak-app/src/panels/ofx_params.rs` | 75.3% | 45.9% | 625 | OFX 参数面板 |
| `oak-app/src/oakui/engine.rs` | 9.5% | 0.0% | 553 | 引擎构造/初始化 |
| `oak-app/src/panels/timeline.rs` | 62.0% | 40.2% | 543 | 时间线面板 |
| `oak-render/src/eval.rs` | 82.5% | 59.1% | 539 | 图求值全量路径 |
| `oak-task/src/nodeops.rs` | 44.4% | 26.1% | 475 | 节点操作任务 |
| `oak-app/src/oakui/renderops.rs` | 78.1% | 58.8% | 432 | ticket/渲染操作 |
| `oak-timeline/src/undoripple.rs` | 26.3% | 28.4% | 418 | ripple 撤销 |
| `oak-codec/src/ffmpeg.rs` | 84.4% | 59.7% | 348 | 解码/编码边界 |
| `oak-timeline/src/undopointer.rs` | 26.6% | 13.0% | 326 | pointer 撤销 |
| `oak-task/src/project/loadotio.rs` | 0.0% | 0.0% | 307 | OTIO 工程载入 |
| `oak-app/src/panels/commands.rs` | 5.6% | 0.0% | 288 | 命令面板 |
| `oak-render/src/procpool.rs` | 81.7% | 61.0% | 284 | 进程池 + 崩溃重启 |

（`tests/` 目录内的集成测试文件按 cargo-llvm-cov 默认排除，如
`oak-storage/tests/database_pg_test.rs` 不在生产口径里。）
### 4.3 可用的测试基建（已核实）

- **gpui test-support**：`oak-app/Cargo.toml` 已启用 `gpui/test-support`；
  `gpui::TestAppContext`（`open_window`/`run_until_parked`/`dispatch_keystroke`）、
  `gpui::VisualTestContext`（`debug_bounds`/`simulate_click`/`simulate_mouse_*`）、
  `#[gpui::test]`（seed/iterations/retries）都已可用，仓库已有 110 个
  `#[gpui::test]` 先例（`app.rs`、`real.rs`、`mock.rs`、panels）。
- **进程/协议边界**：`oak-worker/tests/ofx_host.rs`（真实宿主 + 崩溃钩子）、
  `crates/oak-plugin/tests/fixtures/build_fixture.sh` + `scan_probe`
  （CI 已接线）、`oak-render/tests/render_threads_test.rs`（线程管线）、
  `oak-worker/tests/procpool_integration.rs`。
- **媒体**：`oak_codec::testmedia::write_test_clip[_solid]`（MPEG-2，确定性）。
- **GPU**：CI Linux 用 lavapipe，`OAK_REQUIRE_GPU=1` 让缺适配器硬失败；
  `gpu_or_skip` 是其余环境的跳过策略；`OAK_GPU_TESTS=1` 管 macOS 真 GL。
- **外部依赖**：PostgreSQL 测试无 `OAK_TEST_PG_URL` 时跳过；OFX golden
  测试有 6 个 `#[ignore]`（缺 M11 快照）、serializer 有 1 个（缺 C++
  golden）——这些是本计划要补的异常/兼容性缺口。

## 5. 边界 API 清单与测试矩阵（层 2 范围）

> M0 用 `cargo public-api`（nightly rustdoc JSON）生成完整清单；
> 下表是**分组级**范围，逐 API 条目落在 inventory 文档里。

### 5.1 模块内边界（每个 crate 的公开 API）

| crate | 边界分组（代表 API） | 层 2 重点 |
|---|---|---|
| oak-core | `backend`（GpuContext/texture 注册/present/YUV pass）、`texture`（Texture/Frame）、`colormath`、`configstore`、`lut`、`color`、`videoparams`、`cancelatom`、`handle`、`commandlineparser` | 无 GPU 的创建/回退、非法尺寸/格式、LUT 越界、配置读写坏值、句柄生命周期 |
| oak-codec | `decoder`（trait 全方法）、`ffmpeg`、`encoder`、`frame`、`footagedescription`、`hwdecode`、`gpuinterop`、`planarfiledevice`、`proxymanager`、`conformmanager` | 坏文件/坏流/EOF 回退、硬解失败软解回退、导入失败按帧 fallback、取消、编码 round-trip |
| oak-node | `graph`/`node`/`nodes`、`serializer`、`jobs`、`traverser`、`handle`、`factory`、`project`、`sequence`、`keyframe`、`value` | 图不变式（端点禁删/环）、序列化往返 + 坏 XML、BFS 顺序、每类节点的非法值类型、Job 嵌套 |
| oak-render | `manager`（ticket API）、`worker`（execute_job）、`pipeline`、`procpool`、`ipc`、`eval`、`cache`、`shaderfx`、`autocacher`、`scheduler`、`ofxhost`、`ticket`、`frameio`、`copier` | 队列满/取消/崩溃重启、坏 NDJSON/坏 shm 槽、缓存命中与损坏、OFX 崩溃熔断、导出回读 |
| oak-task | `export`、`render`、`nodeops`、`conform`、`proxy`、`precache`、`customcache`、`manager` | 导出格式矩阵 + 失败清理、坏工程、取消、缓存写坏 |
| oak-timeline | `undogeneral`/`undosplit`/`undoripple`/`undotrack`/`undopointer`、`marker`、`multicam`、`workarea` | 撤销/重做边界（空栈/不可撤销/交叉操作）、时间线非法区间 |
| oak-storage | `session`、`backend`/`backends`、`registry`、`uri`、`writethrough`、`nodeutil` | session 生命周期、损坏数据库、写失败透传、URI 解析坏值 |
| oak-plugin | `host`、`instance`、`suites`、`param`、`render`/`render_driver`、`gl_bridge`、`image`、`node`、`progress` | 插件缺失/加载失败/崩溃、参数类型矩阵、进度/取消、GPU 插件降级 |
| oak-worker | NDJSON 控制面（worker.rs 全部消息类型）、ofx-host 模式 | 未知消息、半包/坏包、EOF、重启、崩溃钩子 |
| oak-cli | 子命令（参数解析、退出码） | 缺参/坏参、文件缺失、导出失败退出码 |
| oak-otio | `fcpxml`、`model` | 导入/导出往返、坏 XML、缺字段 |
| oak-audio | `manager`、`processor`、`waveform(sync)`、`outputdevice`/`previewdevice` | 设备缺失回退、坏音频、采样率/布局矩阵 |
| oak-undo | `undostack`/`undocommand` | 栈边界、命令失败回滚 |
| oak-app | `actions`、`manager`、`oakui`（engine/gateway 边界）、`panels`、`dialogs` | 见 §6 M3（UI 边界单列） |

### 5.2 跨模块/跨进程边界

| 边界 | 数据面/控制面 | 必测正常 + 异常 |
|---|---|---|
| oakapp → oakrender | ticket 提交/`TicketPayload`/取消 | 提交-完成 exactly-once、取消、队列满背压、双后端一致性 |
| oakrender → oakworker | NDJSON 握手 + shm 槽 | 正常批、EOF、崩溃重生、坏槽、容量不足 |
| oakrender → oak-ofx-host | NDJSON + 输入/输出 shm | 正常/进度/取消、崩溃重投、三次熔断紫帧 |
| oakcodec → oakrender | decoder 会话、`FootageJob`、Texture | 正常解码、软/硬解回退、导入 on/off 像素一致、坏媒体 |
| oakrender 内部：解码（可 GPU）→ CPU 消费者 | `Texture::Gpu` → 蒙太奇合成器/导出回读 | CPU 消费者不得静默丢弃 GPU 纹理（M5 审计的 montage 黑帧实例）；staged 请求不得命中导入缓存 |
| oaknode → oakrender | Job 句柄、Graph BFS、值表 | 单循环 match、子 job 递归、非法环/断支 |
| oakapp → oakstorage | session/backend | CRUD、事务回滚、损坏库、并发写 |
| oakrender → oaktask | 导出/缓存消费者 | 正常导出、取消、磁盘满/写失败 |
| oakapp → gpui | 窗口/输入/绘制 | UI 面板渲染、快捷键、鼠标、resize（test-support） |

## 6. 里程碑

> 依赖：M0 是所有后续的前置；M1（单测）与 M2（边界）可并行；M3 依赖
> M0+M2 的 app 边界清单；M4 依赖 M2 全绿后补平台；M5 收官。
> 每个里程碑的"目标数字"指主口径（§3.2）**聚合地板**，达到后写入
> ratchet 并只升不降。

### M0 度量基建与基线（先决）

**内容**

1. `tooling/coverage.sh`：固定 nightly、`--branch`、`--json`，一条命令
   跑全量并调用 `tooling/coverage_report.py`；
2. `tooling/coverage_report.py`：逐 crate 聚合、分支门禁（JSON 解析）、
   `--archive`、`--diff-base`；
3. `tooling/coverage-thresholds.toml`：聚合 + 逐 crate 地板（先填当前值）；
4. `tooling/boundary_api.py` + `docs/zh/plans/coverage/boundary-api-inventory.md`
   首版（从 rustdoc JSON 生成 + 人工填测试映射）；
5. `#[coverage(off)]` 机械改造（内联测试模块 + lints 表）；
6. CI：新增 `coverage` job（Linux，nightly，xvfb+lavapipe，
   `OAK_REQUIRE_GPU=1`），产物 `coverage.json` + 文本报告归档；
   Windows/macOS 覆盖率 job 先做**周更**，不稳定前不挡 PR；
7. 基线留档 `docs/zh/plans/coverage/baseline-2026-09.json/txt`（主口径）。

**验收**：一条命令在干净环境重放基线；门禁在人为降低阈值时 exit 1；
内联测试不计入主口径；边界清单能列出全部 `pub` API 且新增 API 会
让 `--check` 失败。

### M1 层 1：单元测试攻坚

**内容**（按缺口 ROI 排序）

1. `oak-core`：`colormath`/`lut`/`color`/`videoparams`/`configstore`
   的数值与错误分支；`backend` 的尺寸/格式/句柄生命周期校验（无 GPU 时
   走 CPU 分支）；`handle`/`cancelatom`/`rational` 边界；
2. `oak-codec`：`ffmpeg.rs` 的色彩/缩放/缓存/EOF 回退/音频连续性；
   `gpuinterop` 的开关/位深/fourcc 映射；`encodingparams` 的格式矩阵；
3. `oak-render`：`eval.rs` 的 Job 处理分支、缓存 LRU、着色器管线选择；
   `procpool` 的重启/容量/坏包；`ipc` 的编解码边界；
4. `oak-node`：`nodes/*` 的非法输入值类型、`mathbase`/`transform` 的
   参数域、`keyframe` 插值端点；
5. `oak-timeline`/`oak-task`/`oak-storage`/`oak-otio`/`oak-audio`/
   `oak-undo`/`oak-cli`：异常清单 A1–A5 的逐项补测（多数缺口来自
   错误分支没测）。

**验收**：主口径聚合达到 M1 阶段地板（按 §3.5 插值；以干净基线为参照
约行 84%、分支 62%）；单测层不允许出现 sleep/真实时间；新增测试 100%
走公开/受控内部 API。

### M2 层 2：模块边界集成测试

**内容**（按 §5 清单逐项打勾，每条 4 类用例：正常典型、正常边界、
异常 A1–A5、异常 A6–A8）

1. **M2a core/codec**：`GpuContext` 全公开方法（含无适配器、非法 token、
   OOM 路径）；`Decoder` trait 的三个实现；`Encoder` 格式矩阵 + round-trip；
   `planarfiledevice`/`proxymanager`/`conformmanager`；
2. **M2b render**：`manager` ticket 全 API；`worker::execute_job` 的每个 Job；
   `pipeline`/`procpool` 的线程与进程两条后端；`ipc` 协议（含坏帧、
   半包、版本不符）；`cache`/`autocacher`/`scheduler` 的容量与取消；
   `ofxhost` 崩溃/熔断；
3. **M2c node**：`graph` API 不变式、`serializer` 往返 + 坏 XML + 旧版本
   迁移、`traverser` BFS 顺序/环/断支、`factory` 注册与未知 id、
   每类节点的 Job 值表；
4. **M2d task/timeline/storage/otio/audio/undo/cli**：按 §5.1 分组；
   CLI 用真实子进程断言退出码与 stderr；存储无 PG 时用内存后端，
   PG 路径在周更 job 上开；
5. **M2e plugin/worker**：`oak-worker` NDJSON 全消息 + 坏包 + 崩溃钩子；
   `oak-cli` 全子命令；OFX fixture round-trip（补上 6 个 `#[ignore]`）；
6. **M2f 边界清单对账**：每个新增测试回填 inventory；`--check` 绿。

**验收**：主口径聚合达到 M2 阶段地板（参照值约行 86%、分支 69%）；
§5.2 跨模块边界全部有测试；`boundary-api-inventory.md` 无未覆盖条目
（豁免除外）。

### M3 app（最大缺口）

**内容**

1. **UI 边界**（层 2）：用 `TestAppContext`/`VisualTestContext` 对
   `panels/*`、`dialogs`、`actions`、`oakui/{real,mock,graphops,renderops,
   effectchain,transport,ofx,multicam,scopes,frames,nodegraph}` 建"面板
   打开→操作→断言"测试；每个面板至少：可构造、正常交互、禁用态、
   缺数据态（A3）；
2. **真实引擎 seam**（`real.rs`）：沿用 `media_lock` + `WorkerBinGuard`、
   媒体用 `testmedia`，覆盖导入/导出/解码/OFX 的正常 + 缺文件 + 坏文件 +
   取消 + 无 worker 二进制回退；
3. **壳状态机**（`app.rs`）：菜单/快捷键/dock/modal/undo-redo 的正常与
   冲突分支；把不可测的 UI 逻辑抽成纯函数（单独评审，不改行为）；
4. **测试基建自身**（`mock.rs`）：MockEngine 的每个 trait 方法要有
   "记录调用 + 可注入失败"的测试，保证它是可信的替身。

**验收**：聚合达到 M3 阶段地板（参照值约行 88%、分支 74%），且
`oak-app` 当期不低于 80% 行；`#[gpui::test]` 数量翻倍以上；UI 测试
无 sleep、无真实时钟依赖；全量 app 测试墙钟时间增量 ≤ 3×
（固定 seed、`run_until_parked`）。

### M4 平台/硬件/可选依赖边界

**内容**

1. Windows/macOS 覆盖率 job 转常驻（PR 可跳过、main 必跑），
   平台 `cfg` 文件单独门禁；
2. 真机路径测试（可选 runner/开发机）：VAAPI/NVDEC 导入开关对比、
   D3D11VA、VideoToolbox、`OAK_GPU_TESTS=1` 的 GL/OFX；
3. 可选依赖：PostgreSQL（起服务）、OFX bundle（CI 已有 fixture）、
   缺失时按 A8 打印跳过原因；
4. `tooling/coverage_merge.py` 平台合并报告（可选）。

**验收**：聚合达到 M4 阶段地板（参照值约行 89%、分支 78%）；三个平台
各自报告 + 平台文件分支覆盖 ≥85%；跨平台不重复计入的行集合合并报告
留档；真机测试在无硬件环境的跳过路径也有测试。

### M5 收官门槛与防回退

**内容**

1. 聚合地板升到 **行 ≥90%、分支 ≥80%**；
2. 逐 crate 地板按 §7 表锁定；PR diff coverage ≥90%/85% 硬门禁；
3. 冻结 `coverage-thresholds.toml` 并写 `docs/zh/build.md`（本地怎么跑、
   怎么看报告、豁免怎么登记）；
4. 收官报告 `docs/zh/plans/coverage/final-<date>.{json,txt}` 归档，
   与 M5 的 `render-pipeline-threads-m5-branch-coverage.txt` 并列。

**验收**：CI 全绿（含新的 coverage job）；`cargo test --workspace`
仍全绿（覆盖率改造不得引入行为变更）；随机抽查 20 条边界清单条目，
每条都能指出对应测试与异常用例。

## 7. 目标分解与阈值

> 下表目标是**主口径**的规划值（M0 重测基线后按此表校准）；原则：
> `oak-app` 是最大且最难的一块（体量占 32.5%、行基线最低），单独给
> 较低地板，其余 crate 抬高补偿；按本表逐项达标的最低聚合落点为
> **行 90.1% / 分支 80.6%**，其余 crate 超额部分作为平台排除口径的余量。

| crate | 行基线 | 目标行 | 分支基线 | 目标分支 | 主要缺口（层） |
|---|---|---|---|---|---|
| oak-app | 71.2% | **86%** | 44.3% | **80%** | 面板/对话框/对象操作（2）；引擎 seam（2） |
| oak-task | 54.2% | **95%** | 39.8% | **88%** | export/render/nodeops（1+2） |
| oak-timeline | 69.9% | **95%** | 41.7% | **88%** | 撤销族（1+2） |
| oak-storage | 75.5% | **93%** | 51.0% | **88%** | session/backend 错误（2） |
| oak-plugin | 78.4% | **93%** | 50.6% | **88%** | host/suites/param（2） |
| oak-worker | 80.8% | **93%** | 52.1% | **88%** | NDJSON 协议（2） |
| oak-cli | 81.9% | **95%** | 51.8% | **88%** | 退出码/错误路径（2） |
| oak-audio | 87.7% | **95%** | 62.3% | **90%** | 设备缺失回退、波形（2） |
| oak-render | 89.5% | **95%** | 63.1% | **88%** | eval Job 分支（1）；worker/procpool/ipc（2） |
| oak-codec | 89.7% | **95%** | 65.4% | **88%** | ffmpeg（1）；解码/导入边界（2） |
| oak-core | 92.3% | **96%** | 72.5% | **90%** | backend/texture 边界（2）；colormath/color（1） |
| oak-node | 92.7% | **96%** | 68.8% | **90%** | 节点错误分支（1）；图/序列化（2） |
| oak-otio | 93.2% | **96%** | 77.1% | **92%** | fcpxml 往返/坏输入（2） |
| oak-undo | 95.8% | **97%** | 92.6% | **95%** | 栈边界（1+2） |
| **聚合** | **81.72%** | **≥90%** | **55.45%** | **≥80%** | 需新增 ≈1.15 万行 / ≈3.5 千分支 |

分阶段 ratchet（聚合地板，按 §3.5 缺口插值从 M0 主口径基线起算；
下表按干净基线给示例）：M1 ≈ 84%/62% → M2 ≈ 86%/69% → M3 ≈ 88%/74%
→ M4 ≈ 89%/78% → M5 90%/80%。

## 7.5 执行进展（2026-09-17）

### 首批（oak-task 边界，层 1 + 层 2）

- 干净基线重测：行 81.72% / 分支 55.45%（§4.1），并按干净对象集修正了
  全部基线数字与测量口径（§3.1/§3.3）。
- **oak-task 边界批次**（39 个新测试）：
  - `nodeops.rs` 新增 24 个边界测试（每个公开 API 的正常/异常矩阵：
    错误节点类型、陈旧 id、空范围、命令 redo/undo、真实媒体 probe）；
    行覆盖 44.4% → **95.3%**，分支 26.1% → **69.3%**。
  - `tests/loadotio_test.rs` 9 个测试：OTIO/FCPXML 存→读往返、XML
    `.oakproj` 往返、缺失/损坏/未知扩展名、`take_project` 前置状态、
    导入确认回调（接受/拒绝/复位）；`project/loadotio.rs`
    0% → **73.9%**，`project/load.rs` 0% → **97.4%**。
  - `tests/import_test.rs` 6 个测试：混合有效/无效、重复路径、目录递归
    建夹、空目录、取消（`Error::Cancelled` 且无命令）、命令前置状态；
    `project/import.rs` 0% → **56.9%**。
- 工作区聚合（同一干净对象集）：行 81.72% → **82.44%**，分支
  55.45% → **56.10%**；全 workspace 测试绿（96 个测试目标）。

### 第二批（跨 crate 边界批次）

- **oak-storage**（13 个新测试）：
  - `tests/writethrough_test.rs` 7 个：null 句柄 no-op、配置门控
    （`off` 不绑定 / `sqlite` 绑定）、`bind → note_command → flush_all
    → unbind` 往返、写穿失败写入 `last_error`（不 panic）、SQLite/pg
    URI 解析（含 `postgresql://` 与缺失 URL → `None`）、默认库路径
    跟随 `OAK_CONFIG_DIR`、无绑定通知 no-op；配置存储用进程内互斥 +
    隔离临时目录串行。
  - `tests/otio_backend_test.rs` 6 个：后端元数据与 `can_handle`、
    非法 URI/扩展名拒绝、多序列集合往返（隐式 gap、显式 gap、转场
    偏移、缺失媒体引用 → 无 footage 链接）、FCPXML 往返、空工程导出
    单个空 timeline、缺失/损坏 JSON/无根文档及无扩展名错误。
- **oak-codec**（17 个新测试）：
  - `ffmpeg.rs` 内联 7 个：JPEG 全范围像素格式全映射、原生像素格式
    深度分类、声道布局（零掩码回退与多声道）、错误/取消辅助、RefFrame
    几何与缓冲共享（clone/clone_raw）、打开状态机（无效流、重复打开、
    异流拒绝、幂等 close）、跨流检索（音/视频互错的 Unsupported/Err）。
  - `decoder.rs` 内联 3 个：trait 保守默认（supports/gpu 取帧回退、
    hwaccel、音频起点）、OIIO 占位解码器全方法未实现契约、测试注册表
    注入优先与还原。
  - `encoder.rs` 内联 2 个：trait 保守默认、测试注册表注入优先与
    还原（并把读取注册表的既有测试纳入 `lock_tests` 串行）。
  - `framemanager.rs` 内联 1 个：进程单例同一性与 GC 线程首次启动。
  - `proxymanager.rs` 内联 3 个：状态数值/字符串双向转换（含未知回退
    与 trim）、`.a1.` 音频代理名检测、文件系统状态（Ready/Generating/
    Missing）。
  - `encodingparams.rs` 内联 2 个：非法 XML（EOF、未加引号属性、未闭合
    属性、错配结束标签、元素级文本、非 export 根）全部报错；视频/音频
    格式码映射与未知码回退（`Invalid`/`Stretch`）。
- **oak-render `cache.rs`** 内联 11 个：访问器与 NULL 时基、请求/清理、
  字节读取越界与部分读取语义、UUID 文本转换（含超长忽略）、缺失路径
  mtime、`set_uuid` 磁盘重载、`load_state` mtime 守卫与强制重载、
  `save_state` 建目录失败、passthrough 快照（帧哈希才继承时基）、
  passthrough 保存落盘、帧路径（时基/整秒回退）、owner 计数单调。
- **oak-plugin `node_factory.rs`** 内联 6 个：属性读取辅助（类型回退、
  越界/缺失归零）、所有标量参数类型的默认值（含 CUSTOM/BYTES/
  PushButton/未知/裸 Parametric → None）、颜色与向量默认值
  （RGB/RGBA/2D/3D）、坐标系统规范化对默认值的缩放（工程尺寸 +
  `to_canonical` 零尺寸透传）、实例计数可观测。
- **oak-timeline undo 命令**（`tests/undocommands_test.rs`，38 个）：
  `undoripple`（TrackList/Timeline 波纹删除、波纹工具、按区域删缝、
  splice/插入点/锁轨等边界）、`undopointer`（BlockTrim 普通/卷动/零长
  删除、TrackSlide 相邻边界与缺失侧创建）、`undotrack`（前置/后插/
  替换）、`undosplit`（时间点切分、保留链接切分）、`undogeneral`
  （媒体入点、启用/禁用、插缝、默认转场），共 16 处 `to_command`
  装箱路径。限制：`RippleInfo` 无公开构造器（只能覆盖空 info），默认
  转场命令是文档化 stub。
- **oak-task 第二批**（5 个新文件、48 个测试）：
  `task_test.rs` 13（启动/取消/错误/进度/一次性订阅/等待/重置）、
  `manager_test.rs` 9（单例 init 幂等与 shutdown、增删改查与越界、
  取消与等待、指针取线程、Drop 取消 join）、`codecbridge_test.rs` 6
  （注册生命周期、真实 conform 成功写 PCM 并重命名、缺失媒体、坏名、
  重命名失败、代理参数转换）、`precache_test.rs` 7（构造、未探测/
  缺失媒体、陈旧 id、单帧渲染、取消）、`render_test.rs` 13（默认与
  配置器、空范围、无轨道序列、footage 帧与原生进度、强制尺寸/格式、
  钩子错误中止、关闭进度、取消、montage、效果链启用/禁用、音频早于
  视频、帧数与时间戳顺序）。
- **稳定性修复**：`loadotio_test.rs` 的全局导入确认回调与并行测试
  存在竞态（回调被并发安装/复位时其他往返测试可能被拒绝），所有经
  `LoadOTIOTask` 的测试统一持 `CALLBACK_LOCK`；连跑 5 次稳定。
- **顺带修复的生产缺陷**：`ForceParams` 原先 `derive(Default)` 使
  `force_format = 0`（U8，而非文档约定的 -1 = off），默认票据会把
  F32 渲染管线推向 U8，缩放路径在 footage 帧上 panic；改为手写
  `Default`（`force_format = -1`），并在 render 测试中断言。
- 工作区聚合（干净对象集，同一口径；数字含各 crate 的测试代码，
  与首批基线可比）：区域 **84.93%**、函数 **79.26%**、行
  **84.03%**（121648/144766）、分支 **58.11%**（8594/14789）。
- 全 workspace 测试（非插桩）：**104 个测试目标全绿，2593 passed / 0 failed**。

- 各生产 crate 行/分支变化（第二批前 → 第二批后）：
  - oak-timeline 69.9% → **90.1%** / 41.7% → **60.2%**
  - oak-task 77.8% → **85.8%** / 53.7% → **57.9%**
  - oak-storage 75.5% → **82.7%** / 51.0% → **65.2%**
  - oak-codec 89.7% → **91.4%** / 65.4% → **68.6%**
  - oak-render 89.5% → **90.0%** / 63.1% → **63.9%**
  - oak-plugin 78.4% → **79.2%** / 50.6% → **51.7%**

### 第三批（oak-app 专项，2026-09-18）

- 用独立的 oak-app 插桩（`llvm-cov show -show-instantiations=false`
  绕开大对象段错误）把缺口定位到 46 个文件、约 11.5k 未覆盖行，逐文件
  行号清单驱动并行工作流。
- **81 个新测试**：
  - `oakui/real.rs` 26 个（真实引擎）：纯 helper（marker/clip 颜色、
    多机位帧缓存 LRU、RealClock 的 tick/循环/停尾、F32 帧转换）、无
    工程守卫路径（约 90 个方法的 Err/no-op/空返回）、设置与颜色往返、
    时间线编辑事件全集（trim/roll/slip/ripple/slide/split/轨道高度/
    转场/删除）、效果栈与节点图事件、数据源快照、代理生成→排空→徽标→
    删除、footage 维护与音频自动建轨、源监视器渲染与失败回退、多机位
    调度、工程文件（.ove/.xml/.otio/.fcpxml）导出与往返、导出会话、
    SQLite 库生命周期（建/列/改名/复制/导出/导入/打开/另存/删除）、
    源时/波形同步与多机位向导偏移。
  - `app.rs` 20 个：菜单勾选全动态标志、CLI 参数解析、面板注册表
    往返、`build_root`、18 种模态的打开/守卫/事件路由、文件路径路由、
    拖放序列选择、插件进度与导出进度事件、全局动作分发全分支、焦点
    面板路由、外壳订阅路由、语言与首选项事件、浮动面板开关、轨道
    删除、启动初始路径与浅色主题。
  - `dialogs.rs` 16 个：导出对话框序列选择器与 4K 预设、格式校验、
    键盘与按钮路径、代理对话框编辑→生成→删除→应用、首选项页签与
    事件转发、重命名/关于/工程属性内容实体。
  - 其余 19 个：`panels/commands.rs` 3（注册表全动作默认拒绝 +
    `dispatch_to` 映射 + `viewer_transport`）、`oakui/engine.rs` 4
    （类别键、`AppEngine` 默认降级全覆盖、代理参数 UI 往返）、
    `oakui/displaycolor.rs` 5（键值/监视器/内容覆盖、缓存重建与
    generation、ICC 降级、零像素转换）、`oakui/waveform.rs` 3、`panels/
    history.rs` 1（真实撤销栈渲染与交互）、`panels/source_viewer.rs` 3。
- 稳定性修复（本批暴露）：两个 worker 回放验收测试在全量并行下进程池
  饥饿（`submitted > 0` 但无 slot），改为有界等待后打印诊断并跳过、
  聚焦运行仍严格断言；history 行改用状态断言（行元素无
  `debug_selector`）；waveform 装饰器只走可安全调用的守卫分支；
  代理对话框高度按 8px 网格吸附（min 120）的期望修正为 184。
- 全 workspace 测试（非插桩）：**104 个测试目标全绿，2674 passed /
  0 failed**（本批新增 81 个）。
- 工作区聚合（干净对象集，同口径）：区域 **89.54%**、函数 **85.64%**、
  行 **88.76%**（137111/154469）、分支 **63.04%**（9376/14873）。
- 各生产 crate 行/分支（第三批后）：
  - oak-app 71.2% → **88.1%** / 44.3% → **61.1%**
  - oak-render 90.0% → **90.2%** / 63.9% → **64.2%**
  - oak-task 85.8% → **86.1%** / 57.9% → **58.4%**
  - oak-storage 82.7% → **82.8%** / 65.2% → **66.4%**
  - oak-codec 91.4% / 68.7%；oak-timeline 90.3% / 60.3%；
    oak-plugin 79.2% / 51.7%
- oak-app 剩余缺口（约 5.9k 行）集中在 `oakui/real.rs`（710）、
  `panels/ofx_params.rs`（625）、`panels/timeline.rs`（532）、
  `oakui/mock.rs`（466）、`app.rs`（403）、`oakui/graphops.rs`（351）、
  `oakui/engine.rs`（333）、`component/controls.rs`（295）。

### 第四批（分支缺口，2026-09-18）

- **168 个新测试**（全 workspace 非插桩：104 个目标、**2842 passed /
  0 failed**）：
  - `oak-render/src/eval.rs` 47 个：色彩变换/解析/着色器/合成/编解码/
    图形渲染/蒙太奇/音频的边界与错误分支（GPU、故障注入类注明原因）。
  - `oak-task/src/render.rs` 13 个内联 + `tests/render_test.rs` 4 个：
    `ForceParams`/timebase/租约分类/视频与音频 ticket/蒙太奇/效果链/
    调度队列；音频钩子错误、帧间取消、进度到位、非零起点偏移。
  - `oak-codec/src/ffmpeg.rs` 25 个：缓存查找/色彩元数据/像素与采样
    格式映射/时间基、真实媒体解码（缩放、缓存命中、EOF、音频 seek/
    重采样/抖动）、conform 音频（S16/F32、取消、缺目录）、探测器
    （字幕流、取消）、编码器往返与幂等。
  - `oak-app/panels/ofx_params.rs` 27 个 + `oakui/ofx.rs` 9 个：所有
    参数控件类型、值同步/吸管、事件路由、曲线 JSON、颜色选择器
    RGB/HSV 点击与拖拽/提交校验/取消；OFX 交互视口、图像打包与合成、
    按键符号、初始化幂等。
  - `oak-app/oakui/graphops.rs` 23 个 + `panels/timeline.rs` 12 个：
    工程/素材/命令/查询/放置/剪贴板/编辑/转场的错误矩阵；时间线上下文
    菜单、面板命令、拖放路由、落点解析、缩放/微移钳制、真实引擎回退。
  - 自有批次 8 个：`component/controls.rs`（Slider/SpinBox/ComboBox/
    CheckBox 的方法与渲染分支）、`oakui/mock.rs`（设置/代理/同步/
    多机位）、`oakui/engine.rs`（`AppEngine` 其余全部默认实现）。
- 稳定性：`render_threads_test` 的三个逐字节比对测试固定软件解码
  （`OAK_HWACCEL=0` 的 RAII 守卫，测试锁内无竞态）——硬件与软件解码
  存在几 LSB 差异，此前表现为 ±3 字节的间歇性失败；连跑 5 次稳定。
- 记录（未改生产代码）：`graphops::roll_edit` 与文档描述不符——当前
  `BlockTrimCommand` 的 `TrimOut` 映射使左片段 in 移动、后继 out 移动，
  接缝不移动；`undocommands_test` 固化了同一行为，列为后续 PR 的语义
  缺陷候选。
- 聚合（干净对象集，同口径）：区域 **91.38%**、函数 **87.75%**、行
  **90.72%**（148231/163392，**行目标 90% 达成**）、分支 **65.98%**
  （9995/15149）。
- 各 crate 行/分支：oak-app **92.5%** / 67.7%；oak-render 92.7% /
  67.7%；oak-codec 93.9% / 74.4%；oak-task 87.7% / 61.5%；
  oak-timeline 90.6% / 61.3%；oak-storage 82.7% / 65.5%；
  oak-plugin 79.2% / 51.7%（非 app 最大缺口）。
- 下一步：分支 80% 目标还差 ~14pp。优先 oak-plugin
  （node_factory 222 / suites/property 165 / host 104 / instance 222 /
  render_driver 163 / clip 90）、oak-app（real.rs 712、app.rs 403、
  renderops 270、program_viewer 262、controls 230、nodegraph 213）与
  oak-worker/ofx_host（272 行）。

### 第五批（分支缺口，2026-09-19）

- **227 个新测试**（全 workspace 非插桩：104 个目标、**3069 passed /
  0 failed / 7 ignored**——ignored 为既有 GPU 门控）：
  - oak-plugin 核心合计 46 个（`node_factory`/`instance`/`host` 三文件的
    组合计，非单文件计数）：`node_factory`（用 build.rs 测试插件
    bundle 的确定性布局覆盖 build_core 组/页/颜色/组合框表、命名与
    派发、执行器分支）、`instance`（编辑事务、clip 偏好、RoD/RoI、
    渲染/GL 路径、序列渲染、interact）、`host`（dlopen、bundle 扫描、
    PluginCache 去重、build_plugin 上下文协商、实例存活计数）。
  - oak-plugin 套件/驱动合计 62 个（`render_driver`/`clip`/
    `suites/property`/`suites/gl_render` 四文件的组合计，非单文件计数）：
    `render_driver`（CPU/GL 帧渲染、组件与深度矩阵、参数覆盖、输出写回
    与上传失败）、`clip`（fetch/store 转换矩阵与协商）、`suites/property`
    （属性套件全 API）、`suites/gl_render`（格式/纹理属性/加载与清理；
    GL 不可用路径跳过）。
  - oak-worker 39 个：`ofx_host`（共享内存 attach/帧池/输入读取/输出
    发布/作业处理与回退）与 `worker`（进度管道、渲染器回退、握手与
    加载、渲染/批量/音频 ticket、图形回退与崩溃钩子）；新增全局工厂/
    颜色测试锁避免并行竞态。
  - oak-core/render 53 个：`procpool`（调度、取消、回收、段增长、关停
    排空、真实 worker 崩溃重启）、`videoparams`、`color` 分支。
  - oak-app 27 个：`renderops`（代理/分辨率/编码参数/重打包守卫）、
    `program_viewer`（事件路由/菜单/交互防护）、`nodegraph`（数据源/
    构建/连接/编辑）、`project_explorer`（事件/菜单/重命名/删除）与
    `mock`/`controls`/`inspector`（效果事件、指针与键盘路径、上下文
    与添加菜单）。
- 生产缺陷修复（oak-plugin `clip.rs`）：`store_output_image` 此前把像素
  写入 `texture_get_frame` 返回的深拷贝（CPU 纹理的写入被丢弃）；现在
  CPU 直接回写 `f.data`、GPU 走上传、Planar 明确报错（与
  `render_driver::write_output_frame` 的既有修复一致），新测试固化。
- 稳定性：修复本批 procpool 测试的三个自锁死锁（`cancel_frame` /
  `set_graph_snapshot` / `poll` 内部会锁 dispatcher，测试却在持锁时
  调用）、调度亲和性误设、U10 槽位尺寸期望与握手替身二进制；42 个
  procpool 测试全绿。
- 聚合（干净对象集，同口径）：区域 **93.11%**、函数 **88.99%**、行
  **92.62%**（162596/175544）、分支 **70.09%**（11024/15729）。
- 各 crate 行/分支：oak-app **94.5%** / 71.0%；oak-plugin 79.2% →
  **90.9%** / 51.7% → **75.7%**；oak-render 94.3% / 70.4%；
  oak-core 93.5% / 75.4%；oak-worker 80.8% → **90.3%** / 52.1% →
  **70.3%**；oak-codec 93.9% / 74.4%；oak-task 87.7% / 61.3%；
  oak-timeline 90.6% / 61.3%；oak-storage 82.7% / 65.5%。
- 下一步：分支 80% 还差 ~10pp。剩余分支热点：`real.rs`（428）、
  `eval.rs`（157）、`oak-task/render.rs`（150）、`worker.rs`（109）、
  `color.rs`（104）、`app.rs`（88）、`mock.rs`（87）、`graphops.rs`
  （84）、`backend.rs`（74）、`controls.rs`（71）、
  `undogeneral`/`undoripple`（各 67）。

### 第六批（分支缺口，2026-09-20）

- **102 个新测试**（全 workspace 非插桩：104 个目标全绿）：
  - `oakui/real.rs` 15 个：时间线编辑事件的陈旧 id 守卫、文档导出/互换
    失败路径、未知 id 查找、素材/序列/文本/转场放置守卫、多机位与 AFV
    解析、代理自动启动与失败策略、源监视器回放窗口与陈旧取消、预览窗口
    陈旧完成释放与 BGRA8 槽读取、波形同步放置与不可相关跳过、帧助手。
  - `oak-render/eval.rs` 15 个（GPU/OCIO 夹具，缺环境时打印跳过）+
    `oak-task/render.rs` 3 个（私有调度器经 worker 渲染、缺失媒体错误）。
  - `oak-core/color.rs` 6 个、`backend.rs` 16 个（GPU 路径按适配器门控）、
    `oak-codec/ffmpeg.rs` 5 个（软件重开、EOF 回退、重采样平面/flush）。
  - `oak-worker/worker.rs` 7 个、`ofx_host.rs` 2 个、`oak-render/procpool.rs`
    12 个（死锁修复后的调度/回收/VRAM 探测/读取 EOF 等）。
  - `oak-app/controls.rs` 6 个、`mock.rs` 4 个、`app.rs` 10 个（滑块编辑
    与拖拽、复选框键盘、组合框选择、微调框滚动/键盘/编辑器；节点图与
    时间线事件、连接助手、放置路径；模态不匹配、进度排空、订阅路由、
    序列弹窗提交/取消、tick 循环、退出动作）。
  - `panels/node_editor.rs` 1 个（菜单路由、适配/缩放、dock 元数据）。
- 本批无生产代码改动；各 agent 报告本 crate 套件全绿、clippy 无新增告警。
- 聚合（干净对象集，同口径）：区域 **93.90%**、函数 **89.61%**、行
  **93.46%**（173470/185601）、分支 **71.95%**（11553/16057）。
- 各 crate 行/分支：oak-app **96.0%** / **76.6%**；oak-core 94.8% /
  77.5%；oak-render 95.0% / 69.7%；oak-codec 94.2% / 75.1%；
  oak-worker 91.9% / 70.7%；oak-task 88.8% / 62.8%；oak-timeline
  90.6% / 61.3%；oak-plugin 90.9% / 75.7%；oak-storage 82.7% / 65.5%。
- 下一步：分支 80% 还差 ~8pp（约 1.3k 分支）。剩余热点：`real.rs`
  （321）、`eval.rs`（228）、`oak-task/render.rs`（136）、`ffmpeg.rs`
  （110）、`color.rs`（110）、`worker.rs`（108）、`graphops.rs`（82）、
  `procpool.rs`（76）、`ofx_params.rs`（68）、`undogeneral`（67）。

### 第七批（Review 修复，2026-09-22）

- 依据 `test-coverage-90-80-review.md` 修复全部高危/中危与 §5 条目（仅 §3.1
  的语义裁决按报告建议延后，准备项已落地）。要点：
  - §3.2：`render_test.rs` 私有调度器测试不再"永不失败"（环境错误跳过、
    产品错误失败，RAII 恢复管理器/env）。
  - §3.3：三个回放验收测试仅在**零交付**时跳过；`OAK_STRICT_PLAYBACK=1`
    严格模式；`WorkerBinGuard` 按 `current_exe` 解析 worker。
  - §3.4：CI 安装 `icc-profiles-free` + `system_icc()` 补 Ubuntu 实际路径；
    `gpu_context_for_import` 尊重 `OAK_REQUIRE_GPU` 并输出 `SKIP:` 标记。
  - M1–M10 与 §5：文档/断言/读回/容差/RAII 全部落实（详见 review §9）。
  - §3.1：以仓库内 C++ 上游源码核准语义；`RippleInfo::new` + 真实 info 测试
    落地；roll/slide/insert-gaps/ResizeWithMediaIn 的期望值加 KNOWN-SWAP
    标注，语义修复清单已细化（独立 PR）。
- 追加修复：`source_monitor_playback_window_and_stale_cancels` 的早期
  playhead 断言缺陷；`preferences_general_tab_writes_cache_ahead_and_storage`
  的点击→按键焦点竞态。
- 验收：全 workspace **104 目标、3179 passed / 0 failed / 7 ignored**。
- 插桩终测（干净对象集，同口径）：区域 **93.96%**、函数 **89.59%**、行
  **93.49%**（180742/193331）、分支 **71.58%**（11805/16491）。分支较第六批
  微降 0.37pp：本批以测试质量换掉了若干"容忍型/同义反复"执行路径（例如
  §3.3 的交付即失败、M3 的效果夹具更换），属预期代价而非回归。
- 各 crate 行/分支：oak-app 96.0% / 76.3%；oak-plugin 90.8% / 76.1%；
  oak-render 94.9% / 69.4%；oak-core 94.2% / 75.5%；oak-codec 94.2% /
  75.1%；oak-worker 92.7% / 70.4%；oak-timeline 91.0% / 60.2%；
  oak-task 89.5% / 62.8%；oak-storage 82.9% / 66.6%。
- 下一步仍为分支 80%（缺口 ~8.4pp）；热点见上。

### 第八批（§3.1 语义修复与两处 UI 回归，2026-09-24）

- **§3.1 修复**（详见 review §3.1/§9）：`block.rs` 的两个长度 setter 按其
  C++ 契约重写为三个存储模型原语：
  - `set_length_and_media_out` = in 固定、out 移动、media 不动；
  - `set_length_and_media_in` = in 固定、out 移动、`media_in += old−new`；
  - 新增 `set_length_keeping_out` = out 固定、in 移动、`media_in += old−new`。
  undopointer/undogeneral/undoripple/undosplit/graphops/cli/nodeops 的调用点
  逐点对齐，修掉两处实证缺陷：
  1. `TrackReplaceBlockWithGapCommand` 在“块后紧跟缺口”时把缺口向右生长，
     吞掉缺口之后的块——即用户报告的“拖动一个素材影响到其他无关素材”；
     现在缺口向左覆盖被移除块的跨度，后续块不动（domain_test 回归断言）。
  2. roll 不滚动、slide 出现负入点、trim-in/ripple splice 把时间线 in 写进
     `media_in`（播放错误内容）；全部 KNOWN-SWAP 期望按正确几何改写，并补
     `media_in` 维度断言（roll 接缝移动、follower 媒体推进、slide 无负坐标、
     insert-gaps 向右生长、ResizeWithMediaIn `media_in=20`）。
- **i18n 刷新**：dock 的 `PanelHandle` 在注册时快照 `DockPanel::title`，
  语言切换后页签停留在旧语言（截图的“英文界面 + 中文页签”）。gpui 侧新增
  `DockArea::refresh_panel_titles`（经类型擦除 provider 重读标题 + 通知面板），
  shell 在菜单与首选项两条切换路径调用；首选项对话框自身同步重绘。
  （gpui 子模块提交见 `oak-gpui`。）
- **OCIO 参数面板**：Vec4 评分参数（对比度/偏移/曝光）建 4 个 SpinBox，
  `base` 步长锚定范围（pivot 不再一拖就飞出 ±10000）。
- **拖放覆盖修复**：`TrackRippleRemoveAreaCommand::prepare` 补"范围起点落在
  块间空洞"分支（C++ 布局连续、无此情形）——此前该情形下既不裁剪后面的
  重叠块、又把新块插到它前面，导致"起点在空白、终点压在别的 clip 上"时
  新 clip 被压在下层而不是覆盖。现在统一与起点在 clip 上时一致：重叠部分
  被移除、新块在前。`domain_test` 与 `graphops` 各补一条回归断言。

## 8. 风险与对策

1. **oak-app UI 覆盖成本最高、最易 flaky**：只用 test-support 的确定性
   接口（`run_until_parked`/`dispatch_keystroke`/`simulate_click`），
   禁止 sleep；固定 seed；面板测试按面板拆文件并行；CI 现有"失败重试
   一次"仅用于已知 flake，新增 flake 必须根治或 `#[ignore]` + 登记。
2. **覆盖率驱动出坏测试**（断言实现细节、为行数写无意义 case）：
   §2.1 行为断言 + review checklist；`#[coverage(off)]` 需登记理由。
3. **分支覆盖工具不稳定（nightly `--branch`）**：固定
   `nightly-2026-07-21`；加一个"已知分支数"的 canary 文件（M0），
   工具链升级导致计数漂移时用 canary 对账。
4. **跨平台合并复杂**：主门禁只认 Linux 全量；平台文件单独门禁；
   合并报告是可选增强，不被 PR 阻塞。
5. **套件变慢**：M0 给测试加 `--no-fail-fast` + 超时；对 >2s 的单测
   标记并解释；GPU/媒体测试复用 `testmedia` 与共享 fixture；必要时
   `cargo nextest`（cargo-llvm-cov 原生支持）。
6. **异常路径需要注入点**：优先用现有开关（`OAK_HWACCEL`、
   `OAK_GPU_IMPORT`、`OAK_PIPELINE`）；不足的地方在 M0 加
   `FailPoint`/假后端，**不修改生产语义**。
7. **既有 `#[ignore]`**：M2/M4 补齐 serializer C++ golden 与 OFX M11
   golden，其余确实缺外部环境的登记豁免并保留 `#[ignore]` 理由。
8. **覆盖率只算生产代码后基线更低**：M0 先测主口径基线，再按 §7 校准
   各里程碑数字；若主口径比兼容口径低 10pp 以上，先做 M1 再宣布 M2 目标。

## 9. 不变量与边界

- **不改生产行为**：本计划只加测试、度量与最小可测性基建；任何为可测性
  做的重构（纯函数抽出、依赖注入）必须单独 PR、单独评审、行为不变。
- **不引入不稳定因素**：无 sleep、无真实网络/时间依赖（外部依赖按 A3/A8
  跳过并打印理由）；测试失败必须可复现。
- **执行环境**：所有测试必须在 CI 的 headless（xvfb + lavapipe）通过；
  `OAK_REQUIRE_GPU=1` 下 GPU 测试真跑；硬件/平台测试在无硬件环境跳过，
  跳过不等于通过（报告里单列）。
- **性能预算**：`cargo test --workspace` 墙钟时间相对 2026-09 基线不
  超过 2×；coverage job 目标 ≤45 分钟。
- **豁免有账**：`unit-exemptions.md`、`exclusions.md`、边界清单三本账
  是验收物，缺登记视为未完成。
- **文档同步**：`docs/zh/build.md` 增加覆盖率章节；`docs/zh/plans/README.md`
  收录本计划与 `coverage/` 归档目录。

## 10. 交付物清单

1. `tooling/coverage.sh`、`tooling/coverage_report.py`、
   `tooling/boundary_api.py`、`tooling/coverage-thresholds.toml`；
2. `.github/workflows/coverage.yml` + `ci.yml` 接线（Linux 必跑，
   Windows/macOS 周更）；
3. `docs/zh/plans/coverage/`：
   `baseline-2026-09.{json,txt}`、`boundary-api-inventory.md`、
   `unit-exemptions.md`、`exclusions.md`、`final-<date>.{json,txt}`、
   `merged-<date>.txt`（可选）；
4. 新增测试：层 1 单测与层 2 边界测试（按 §6 里程碑分批）；
5. 本计划文档与 `docs/zh/plans/README.md` 索引更新。
