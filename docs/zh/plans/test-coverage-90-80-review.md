# 未提交测试 Review 报告：假测试与实现语义核查

> 审查对象：工作区全部未提交改动（HEAD `800011ef7`，2026-09-20 快照；
> 63 个修改文件 + 11 个新测试文件，+38180 / −254 行）。
> 审查重点（用户指定）：**假测试**、**修改实现语义以匹配测试的行为**。
> 报告落笔：2026-09-22。配套计划文档：[`test-coverage-90-80.md`](test-coverage-90-80.md)。
>
> 标注约定：**[实证]** = 审查会话直接核对过代码原文/手算过数值；
> **[子代理]** = 深查子代理报告、审查会话抽样复核过关键链条。

## 0. 结论摘要

**问题一：是否修改了实现语义以匹配测试？——未发现。**
全部 254 行删除/修改逐条核对：生产侧改动要么是有设计文档回填的 M5
零拷贝功能开发，要么是与文档契约对齐的**真 bug 修复**（`ForceParams`
默认值、`clip.rs::store_output_image`），没有一处是"为了让测试通过而
扭曲实现"。

**问题二：是否存在假测试？——大部分是真测试，但确认 4 组高危 + 10 项中危。**
最严重的不是传统意义的"无断言测试"，而是两类更隐蔽的模式：

1. **测试固化既有实现缺陷**：约 12 个 `undocommands_test.rs` /
   `graphops.rs` 测试把 `block.rs` setter 语义互换缺陷的反向行为
   （roll 不 roll、slide 不 slide、负时间线坐标、块重叠）写成精确期望值
   （§3.1）——测试向实现投降，而非实现向契约收敛。
2. **"环境容忍"逃逸路径**：`Ok|Err` 双向接受、饥饿即跳过、CI 恒跳过
   （§3.2–§3.4）——产品回归与环境污染不可区分，验收测试在门禁 job
   里静默空转。

统计：高危 4 组（含 1 个活的产品缺陷）、中危 10 项、低危约 12 项；
正面确认（合格项）9 项，见 §6。

## 1. 审查范围与方法

- **删除行全查**：`git diff` 的 254 行删除是既有实现语义变更的唯一可能
  载体，逐条核对并手算验证数值等价性（§2）。
- **新测试文件全文审查**：11 个 untracked 测试文件（约 6300 行）由三个
  深查子代理全文通读，审查会话对全部高危论断做了原文复核。
- **内联测试**（约 3 万行新增）：模式扫描（静默 return、`let _ =` 丢弃
  结果、`#[ignore]`、sleep、同义反复 matches!、Ok|Err 全收）+ 高风险区
  抽样精读（`eval.rs`、`ffmpeg.rs`、`worker.rs`、`instance.rs`、
  `history.rs`、`dialogs.rs`、`controls.rs`）。
- **交叉验证**：与 `test-coverage-90-80.md` §7.5 六批自述、
  `render-pipeline-threads.md` M5 回填逐条对账；CI 配置
  （`.github/workflows/ci.yml`）核对跳过条件在门禁 job 上是否成立。
- 未重新运行测试套件（计划文档声称 3069 passed / 0 failed；静态审查
  未发现与该声称矛盾之处）。

## 2. 生产语义核查：全部 254 行删除的定性

| 改动 | 定性 | 证据 |
|---|---|---|
| `oak-core/backend.rs`：YUV 矩阵常数（`65535/56064` 等）删除 | **合法泛化** [实证] | 新 `from_matrix_depth` 在 16-bit 下与旧常数逐位一致（`ay = max/(y_white−y_black) = 65535/56064`；full-range 分支 `(1.0, 0.0, 1.0, 32768/65535)` 亦一致），为 M5 NV12/P010 平面导入服务 |
| `oak-codec/hwdecode.rs`：Linux 设备顺序 CUDA 优先 → VAAPI 优先 | **有意行为变更** [实证] | CUDA/NVDEC 表面无公开句柄导出，零拷贝只能走 VAAPI DMA-BUF；`render-pipeline-threads.md` M5 回填记录了真机（RTX 5070 Ti + nvidia-vaapi-driver）验证 |
| `oak-codec/ffmpeg.rs`：hw transfer / 色彩范围判定 / 帧缓存删除 | **等价重构 + 真修复** [实证] | 范围判定抽成 `frame_colorimetry`（force 优先、YUVJ 判定、`AVCOL_RANGE_JPEG` 分支逐一保留，CPU/GPU 两路共享）；`RefFrame`（`av_frame_ref` 语义）修复 ffmpeg-next `Video::clone` 用 `av_frame_copy` 深拷、丢失硬件表面引用（克隆 VAAPI 帧 transfer 报 EINVAL）的真 bug |
| `oak-task/render.rs`：`ForceParams` 去 derive、手写 `Default` | **真 bug 修复** [实证] | `derive(Default)` 给出 `force_format = 0`（= `PixelFormat::U8`），违反文档契约 `-1 = off`，会把 F32 管线推向 U8 并在 footage 帧上 panic；测试断言的是文档契约（`render_test.rs:348-351`，注释明示 "not 0 = U8"），非实现现状 |
| `oak-plugin/clip.rs`：`store_output_image` 重写 | **真 bug 修复** [实证] | 原实现把像素写入 `texture_get_frame` 的深拷贝，CPU 纹理的插件输出被静默丢弃；现在 CPU 直接回写 `f.data`、GPU 走 upload、Planar 显式报错，与 `render_driver::write_output_frame` 既有修复一致 |
| `oak-render/eval.rs`：`DECODED_FRAMES` LRU 值类型 `Frame`→`Texture`、`render_footage_frame_inner`→`_opts(allow_import)` | **M5 功能** [实证] | planar 条目独立上限 4（硬件表面驻留约束）；resolve 失败按帧回退 CPU staging（解码器会话缓存兜底，不重解码） |
| `oak-render/pipeline.rs`：`DecodeRequest.allow_import` | **M5 功能** [实证] | montage 合成器（CPU 消费者）`false`、footage 路径 `true`，且入缓存键——正面落实计划 §5.2 "staged 请求不得命中导入缓存" 的 M5 审计项 |
| `oak-app/dialogs.rs` 测试期望 180→184 | **合法期望修正** [实证] | 高度 SpinBox 的 `SliderModel`（gpui_widgets）以 step=8、min=120 建网格，180 恰在 176/184 中点、向上吸附；控件自身测试已固化 "snapped to the model's step" 契约（`controls.rs:1654/1720`） |
| `oak-app/oakui/real.rs` / `worker.rs` / `node_factory.rs` / `ofx.rs` 删除行 | **测试改写**，见 §3.3 / §6 | 回放验收测试弱化（高危）；其余为等价重构或断言保留 |

**流程问题**：M5 功能开发（`gpuinterop.rs`、`backend/external.rs`、
`PlanarTexture`、hwdecode 顺序）与六批覆盖率测试**混在同一未提交工作
区**。计划 §9 承诺"覆盖率改造不得引入行为变更"，混提使该不变量无法被
review 验证——本次是人工逐行核对全部删除行才得出"功能与测试可分离"
的结论。建议分开提交。

## 3. 高危问题（合并前应处理）

### 3.1 🔴 undopointer 族测试固化了一个活的产品缺陷 [实证 + 子代理]

**根因**（已提交代码，本次 diff 未改动）：`oak-node/src/block.rs:93-107`
的 `set_length_and_media_out` / `set_length_and_media_in` 相对其声称的
C++ parity 语义**正好互换**：

```rust
/// Set the length, keeping the media out anchored (C++
/// `Block::set_length_and_media_out`): ...
pub fn set_length_and_media_out(&mut self, length: Rational) {
    let out = self.in_() + self.length();
    self.range = TimeRange::new(out - length, out);  // ← 实际锚定 OUT、移动 IN
    self.media_in = self.range.in_();                // ← 时间线 in 写进 media_in（坐标空间混淆）
}
```

C++ Olive 的 `set_length_and_media_out` 是 IN 锚定、OUT（及 media_out）
移动。该互换同时违反三处成文契约：block.rs 自己的 C++ parity 注释、
`oak-timeline/src/common.rs:39`（"`k_trim_out`: trim the out point"）、
`graphops.rs:2703-2707`（roll_edit 文档："the boundary moves without
disturbing anything else"）。附带缺陷：`media_in := range.in_()` 把媒体
入点写成时间线入点；真锚定 media_out 应为 `media_in = media_out - length`。

**净效果**（`undopointer.rs` 的机械移植在互换后的 setter 上运行）：

- **roll edit 不 roll**：`undocommands_test.rs:625-648`
  `block_trim_roll_edit_resizes_adjacent_clip` 断言 first=[0,50)、
  second=[50,100) roll 到 25 后得 **first=[25,50)、second=[50,125)**
  ——接缝 50 纹丝不动，first 的 IN 移动、second 的 OUT 外扩。真 roll
  应为 first=[0,25)、second=[25,100)。**无偏差标注**；doc 只描述
  "没有插 gap" 这一表层现象。
- **slide 不 slide**：`:736-760` 断言被滑块不动（[50,100)），两个邻居
  反向膨胀，previous 出现**负时间线入点 [-20,50)**。
- graphops 内联测试（`graphops.rs:6048-6071`）是**自觉固化**：NOTE 明说
  "Assert the actual outcome so a future semantic fix has to update this
  expectation deliberately"，并把 `media_in = -2` 写进期望——诚实且有价值
  （修复时强制改写），但 NOTE 自身措辞也不准（称锚定 follower 的 OUT，
  断言显示其 OUT 从 20 移到 18、锚定的是 IN）。
- 数值例（与 graphops 测试一致）：a=[0,10)、b=[10,20) roll 到 12 →
  a=[-2,10)（负入点、media_in=-2）、b=[10,18)，接缝仍在 10，b 尾部反被
  截掉 2 秒。若 b 还有后继，会产生块重叠/留洞（`undopointer.rs:160-174`
  的补偿 gap 按 C++ 假设放在 `[out, out+diff)`，与实际移动方向矛盾）。
- **缺陷是活的**：`real.rs:4772-4798` 把 UI 事件
  `TimelineEvent::ClipRollRequested` 直接接到 `graphops::roll_edit`。
- 仓库内 `undosplit.rs:200-214`、`undoripple.rs:342-358`、
  `undogeneral.rs:865-872` 的作者显然发现了互换并做了补偿性选边 + 注释，
  唯独 undopointer 的 BlockTrim/TrackSlide 与 undogeneral 的
  Resize/InsertGaps 未补偿。[子代理]

**加重项** [子代理]：

- 4 处测试**恰好省略了唯一能暴露矛盾的断言**：`:593-620`（gap=[50,75)
  与 second=[50,100) 重叠，只断言序号不断言 span）、`:653-708` 两处
  （被 trim 块 redo 后实为 [-25,50)，不断言）、`:861-889`（gap 与 tail
  重叠，tail 只断言 id）。
- **全文件（1488 行、38 测试）无一处断言 trim/slide/ripple/split 后的
  media_in**，夹具一律 media_in=0、timeline in=0，使两个内容损坏缺陷
  不可见：(a) `media_in := range.in_()`；(b) `undoripple.rs:342-346`
  splice 修剪不动 media_in——删除 [25,50) 后余块 [25,75) 实际播放**被删
  内容**而非后移内容。
- `:565-588` 普通 trim 测试的注释把反转当规范写（"Trim-out is
  out-anchored: the clip's in shifts to 25"），与 `common.rs:39` 直接
  矛盾，会误导后来者。
- `:1108-1122` ResizeWithMediaIn：注释说 "keeping the out point fixed"，
  断言却是 in 锚定、out 移动；实现与命令名 `WithMediaIn` 及其 doc
  （`undogeneral.rs:105-107`）相反。
- `:1173-1216` InsertGaps 两测试：既有 gap 向左生长越过插入点并与前块
  重叠，注释把左向生长表述为预期，而 `undogeneral.rs:866-872` 在另一
  命令里明说 out-anchored "would push the in point negative"。

**定性**：作为产品缺陷 **高**（指针编辑族整体反向 + 负坐标 + 重叠 +
内容损坏，挂在真实 UI 路径）；作为测试问题 **中-高**（graphops 一处是
诚实的 characterization test；undocommands_test 多处无标注固化且回避
关键断言）。计划文档第四批"记录（未改生产代码）"只登记了 roll_edit
一条，实际波及面大得多。

**修复顺序建议**：先补 `RippleInfo` 公开构造器与 media_in 断言维度，再
统一裁决 block.rs:93-107 的语义互换，届时上述期望值按 graphops NOTE 的
意图 "deliberately" 改写。

### 3.2 🔴 `oak-task/tests/render_test.rs:932-951`：全批次唯一"永不失败"的测试 [实证]

`private_dispatcher_renders_frames_and_audio_through_workers` 对两次真实
渲染的结果做双向接受：

```rust
match outcome {
    Ok(()) => { /* 真断言：order、size、audio */ }
    Err(error) => {
        eprintln!("private dispatcher worker render unavailable: {error}; skipping");
    }
}
```

`f32_outcome` 同构。Err 分支不区分**环境缺失**（worker 池起不来）与
**产品失败**（`"Frame render ticket failed"`、`shm_frame_to_texture` 的
BGRA8/F32 转换回归、poll pump 超时、解码失败）——全部放行。一旦 worker
二进制存在，本测试**不可能失败**，恰好掩盖它名义上要测的 ShmFrame/
ShmAudio 交付缝与 dispatcher teardown 的任何回归。

叠加 `:881-884` 的"缺二进制即 return"（`locate_oak_worker` 依赖
`target/{debug,release}/oak-worker` 已存在，而 `cargo test -p oak-task`
不构建同 workspace 的 oak-worker 二进制）：定向运行/干净 CI 下长期静默
跳过，私有 dispatcher 路径**实际覆盖可能为零**——覆盖率数字虚高的典型
来源。

**修法**：Err 分支按错误内容分流——环境性错误（pool 启动失败）才跳过，
产品性错误必须失败；或至少断言 `error` 匹配已知环境错误串。

### 3.3 🔴 `oak-app/oakui/real.rs`：三个回放验收测试的"饥饿即跳过→通过" [实证]

program-window、display、source-window 三个验收测试均新增
`eprintln! + return`（计为 pass）的逃逸路径：

- program-window：`pumps == 2000 && filled == 0` 跳过；pump 上限
  5000→15000。
- display：`pumps >= 1200 && peak < 0 && slots == 0` 跳过；原"playhead
  60→120 间 peak 严格增长"的两点采样断言弱化为"任意时刻 advances > 0
  一次"；pump 上限 9000。
- source-window：**跳过条件仅 `pumps == 2000`，不检查槽位数**（real.rs
  约 :16396）——注释声称 "skip only when the shared pool delivered
  nothing"，但若帧已交付而 `preview_slot_frame` 服务侧有 bug（candidate
  存在、hit 恒 None），同样被伪装成池饥饿跳过。program-window 至少有
  `filled == 0` 判别，此处没有。

**产品真坏（worker 永远交不出帧）与环境保护不可区分**——验收测试失去
回归守卫功能。计划文档声称"聚焦运行仍严格断言"，但**代码里没有任何
机制区分聚焦/全量运行**，该声称不成立。

**复合隐患** [实证]：`WorkerBinGuard`（`real.rs:8044-8056`、
`renderops.rs:3659-3671`）硬编码 `../../target/debug/oak-worker`：

- coverage job 的 target 目录是 `target/llvm-cov-target/`，干净环境下
  `target/debug/oak-worker` 不存在；
- `procpool.rs:2402` `resolve_worker_bin` 对 `OAK_WORKER_BIN` **不检查
  存在性**直接采用 → spawn 失败 → 槽位恒空 → 跳过路径必然触发；
- 即普通 CI（`cargo test --workspace` 会把 oak-worker 建到
  `target/debug`）能真跑，**coverage job 里这组验收测试静默空转**——
  恰好在产出覆盖率数字的那次运行里。

对照：`procpool.rs:2922` 测试侧 `find_real_worker` 对 env 覆盖做了
`p.exists()` 检查，守卫没有。

**修法**：跳过条件补"槽位已交付但未服务→必须失败"判别（source-window
尤其）；给严格模式一个真实机制（如 CI 设 `OAK_STRICT_PLAYBACK=1` 时禁止
跳过）；`WorkerBinGuard` 改为从 `current_exe()` 推导（与
`find_real_worker` 一致）。

### 3.4 🔴 CI 门禁 job 上永久跳过的两组测试 [实证]

**(a) ICC 门控（`oak-core/color.rs`，17 处跳过）**：`system_icc()`
（color.rs:770-783）只探测固定系统路径（macOS ColorSync、Linux
colord/ghostscript），而 `ci.yml:46-53` 的 apt 列表**不含任何 ICC 包**
→ Linux CI（唯一聚合门禁 job）上恒 None → `display_icc_bgra8_never_outputs_black`
（文档自称"viewer 黑屏回归守卫"）等测试**永久跳过**，只在非门禁的
macOS 周更 job 上真跑。修复便宜：`apt-get install icc-profiles-free`
（或 colord），或仓库内置一个小 sRGB ICC 测试资产。

**(b) footage_import（`oak-render/tests/footage_import_test.rs`）** [子代理，关键链条已复核]：
CI 是 lavapipe、无 `/dev/dri`，VAAPI/CUDA 设备创建必败且负缓存进程级
粘滞（`hwdecode.rs:197-201`）→ `HW_IMPORTS == 0` → 三个测试恒跳过、
cargo 计为 passed、**零断言执行**。加重项：

- `common/mod.rs:61-71` 的 `gpu_context_for_import()` 直接 `?` 返回
  None，**绕过了仓库自己的 `OAK_REQUIRE_GPU=1` 硬失败策略**（对照
  `backend.rs` 的 `gpu_or_skip`/`shared_gpu_or_skip` 会 panic；同目录
  `render_threads_test.rs:776/824` 用的是合规版本）；
- 计划文档承诺"跳过不等于通过（报告里单列）"**未实现**——无 `#[ignore]`、
  无 skip 上报机制，CI 报表里与真跑过的测试不可区分。

测试本体质量不错（见 §6），问题在门禁信号缺失。M5 真机验收已留档
（`render-pipeline-threads-m5-branch-coverage.txt`），但 PR 门禁对该
文件恒零信号，应在 CI 报告侧显式标注（warning annotation）或接入计划
M4 的平台/真机 job。

## 4. 中危问题

| # | 位置 | 问题 | 标注 |
|---|---|---|---|
| M1 | `manager_test.rs:164-182` ↔ `manager.rs:84-99` | `init()` doc 声称 "Create the singleton **and register the codec task submitter**"，实现只置 `codec_submitter_registered: false`；`register_codec_task_submitter` 全仓库**零生产调用点**（仅 codecbridge.rs:94 定义）。测试断言 `Some(false)` 把矛盾固化为"契约"。结合 codecbridge "submission is synchronous" 的过渡契约，判断为文档过期（或注册逻辑缺失），需裁决 | [实证] |
| M2 | `render_test.rs:471-494` | `forced_size_and_format_are_applied`：设置了 `force_format = F32`，断言只有 `recorder.frames == vec![(32,32)]`（尺寸）；Recorder 不记录像素格式，format 是否应用到交付纹理**不可证伪** | [子代理] |
| M3 | `render_test.rs:621-676` | `clip_effect_chain_is_walked_and_bypassed`：enabled=false 与 true 两分支断言**完全相同**（order、frames）；若 `clip_effects()` 回归为完全不收集效果链，测试照过。fixture sanity 只验证测试自建图 | [子代理] |
| M4 | `writethrough_test.rs:126-161` | `backend_enabled_by_config_binds_and_round_trips` 名义 "round_trips"，但**从不读回 sqlite、不断言 `library.db` 文件存在**；write-through 成功仅以 `last_error == None` 反推。`save_project` 退化为静默 no-op 不可检出 | [子代理] |
| M5 | `undocommands_test.rs:528-556` | `RippleInfo` 字段私有且**全仓库无公开构造器** → 只能构造空 info → `ripple()` 在 `is_empty()` 守卫处立即 return → "断言状态未变"**不可能失败**；`TrackListRippleToolCommand` 主体约 125 行对集成测试不可达，行为覆盖为 0。测试注释与计划文档均有诚实标注，但覆盖率数字（oak-timeline 90.1%）因此有误导性。根修：给 `RippleInfo` 加公开构造器 | [子代理，构造器缺失已 grep 复核] |
| M6 | `loadotio_test.rs:294-310`、`otio_backend_test.rs:388-401` | 两处 FCPXML round-trip 只断言 `sequence_labels == ["Edited"]`；轨道/块/footage 全丢也过（对照 OTIO 版有完整结构断言） | [子代理] |
| M7 | `footage_import_test.rs:165` | 导入 on/off 容差 `max_diff < 0.15`，注释两处声称"the same tolerance the hardware-vs-software decode test uses"，实际先例（`oak-codec/src/realmedia_tests.rs:567`，未被本次修改）是 **0.08**——引用失实，容差约为先例 2 倍。测试不空洞（图案通道间距 ≥0.65、逐像素 max、test2 ACEScg 用 0.05 且实测 0.011），但应改注释论证放宽理由（swscale 色度滤波 vs GPU 双线性）或对齐 0.08 | [实证：两处阈值均已 grep 核对] |
| M8 | 无断言/弱断言冒烟测试 | `worker.rs:3188` `renderer_create_variants_are_tolerant`（**零断言**，纯"不 panic"遍历）；`worker.rs:2391` `renderer_creation_falls_back_or_succeeds_headless`（expect + 丢弃 `has_renderer()` 结果）；`instance.rs:2123` `trace_probe_branches`（5 个调用仅 1 个断言）；`history.rs` `rows_render_and_interact`（`on_menu(MENU_UNDO/REDO)` 只调用不断言效果，如 undo 后条目数应减少）；`writethrough_test.rs:232-244`（零断言 + 依赖不受保护的"无残留 binding"前提）。注释均有坦白，但这正是计划 §8.2 自己警告的"为行数写无意义 case"。CI 有 lavapipe + `OAK_REQUIRE_GPU=1`，本可断言确定结果而非"两种结局都接受" | [实证：前四个已读原文] |
| M9 | `footage_import_test.rs:136/145、217/226` | `OAK_GPU_IMPORT` 用裸 `set_var`/`remove_var` 非 RAII：中途 `expect("staging decode")` panic 会泄漏 `"0"`；串行锁下**把后续测试的真失败掩蔽成静默跳过**。同文件（render_threads_test）明明有 `SoftwareDecodeGuard` 先例可仿。另 `remove_var` 不恢复外部预设值 | [子代理] |
| M10 | `oak-core/src/backend/external.rs` | 611 行 unsafe 平台 FFI（DMA-BUF/D3D11 handle/IOSurface 导入）**零单测**；唯一覆盖是恒跳过的 footage_import（§3.4b）。计划 M4 的平台门禁 job 未接线 | [实证：grep 无 #[test]] |

## 5. 低危问题（简列）

- `render_test.rs:352-355`：configurator 边界段（`set_max_inflight(0/
  usize::MAX)` 等）调用后零断言；`max.max(1)` 钳制无公开 getter 可验。
- `render_test.rs:405-428`：名义 "generated **(transparent)** frame"
  只断言尺寸，不检查像素透明。
- `import_test.rs:219-220`：断言消息声称 "fails with Cancelled"，实际
  只 `is_err()`，未 `matches!(Err(Error::Cancelled))`（源码确实返回
  Cancelled，测试弱化了它）。
- `codecbridge_test.rs:142/162/185/205`：失败路径仅 `is_err()` 不断言
  错误变体/消息，无法区分失败原因（如 bad-output-name 若因 probe 失败
  而 err 也会通过）。
- `precache_test.rs:254-282`：doc 声称验证 window/progress
  configurators，`set_max_inflight(1)`/`set_native_progress_signalling(false)`
  设置后零对应断言，其余断言与相邻测试重复。
- `task_test.rs:266-292`：setter/getter 回环、`system_time_ms() > 0`、
  结尾无断言的 `emit_progress(2.0)`。
- `writethrough_test.rs:95-105`：null 句柄断言是真的，其后四次
  bind/unbind/flush 调用零断言。
- `otio_backend_test.rs:296-317`：三种非法 URI 仅 `is_err()`，不区分
  错误原因。
- `undocommands_test.rs:428-487`：ripple 删 gap 仅摘成员关系（util.rs
  文档化偏差），时间线净效果为零；删区域后洞从 [60,80) 迁到 [50,70)、
  可见时间线无变化——注释只描述机制不评语义。
- `gpuinterop.rs:649-652`：`OAK_GPU_IMPORT` set/remove 不恢复外部预设值
  （有 `lock_tests` 守卫，实际风险低）。
- `footage_import_test.rs` `sample()` 未断言 `frame.format == F32` 就按
  f32 解释字节（对照 render_threads_test.rs:232 有此断言）。
- `render_test.rs:891-930`：测试中途 `RenderManager::shutdown()` +
  env 覆盖，恢复在断言之前——render panic 会泄漏全局状态（后续
  `render_serial()` 自愈）。

## 6. 合格项（正面确认，防止误伤）

1. **`render_threads_test.rs` +30/−0 纯新增**（`git diff --stat` 核对）：
   `assert_same_frame` 仍是严格逐字节相等、无容差；计数器期望
   （`(6,6,0)`、lru_hits=1/decodes=4）、双侧地面真值图案、时间戳逐帧
   断言全部保留。旧 flake 靠**消除解码路径分歧**修复（hwdecode 设备负
   缓存进程级粘滞 → 同进程两侧走不同解码路径），不是放宽容差。
2. **`OAK_HWACCEL=0` 软解守卫合规**：该二进制全部 13 个测试第一条语句
   都是 `let _lock = lock()`，守卫在锁内设置、drop 逆序先恢复 env 再
   放锁；`hwdecode.rs` 每次开流实时读 env，守卫真实生效。代价（记录）：
   硬解下的管线字节一致性从此无测试，hw/sw 像素等价仅剩
   `realmedia_tests.rs:504` 一处（0.08 容差，无硬解机器同样跳过）。
3. **逐字节比对不是"自己比自己"**：inline 与 pipeline 是两条真实执行
   机器，两侧用不同文件名隔离 eval/decode 缓存键；footage_import 的
   on/off 对比同样用 `fs::copy` 换文件名隔离缓存。
4. **`worker.rs` "needs 16000 bytes" 断言**：只是提取变量重排，未弱化；
   `node_factory.rs` 曲线 JSON 断言等价重构 + 补 helper 双分支覆盖。
5. **ForceParams 测试断言的是文档契约**（`-1`，注释明示 "not 0 = U8"），
   与手写 `Default` 及 doc 完全一致——契约验证，非投降。
6. **codecbridge 真实 conform 测试是真断言**：两通道 `.pcm` 存在
   （metadata 失败即 panic）、非空（len>0）、`.working` 已重命名消失；
   提交为源码确认的同步路径，无竞态。不足（低）：未校验 PCM 字节数
   是否等于预期样本量。
7. **loadotio 的 CALLBACK_LOCK 真消除了竞态**：锁覆盖全部回调消费点
   （XML 路径测试不持锁是正确的——源码确认不触碰回调）；`take()`
   一次性消费 + 毒化锁 `into_inner()` 恢复，残留回调会导致后续测试
   **响亮失败**（`.expect("loaded project")`）而非静默通过。
8. **`eval.rs` OCIO 内联测试抽查质量高**：真 LUT（红通道加倍）、真
   resolve 管线、逐像素数值断言（0.25→0.5、green 不变、alpha 保持）；
   跳过仅在 OCIO 库缺失时触发，CI 有真实静态 OCIO（`ocio-env.sh` +
   `OCIO_RS_ENABLE_REAL=1`）；GPU 跳过经 `shared_gpu_or_skip` 在
   `OAK_REQUIRE_GPU=1` 下硬失败，设计正确。
9. **`ffmpeg.rs` 内联测试抽查合格**：像素格式映射矩阵、声道布局回退、
   RefFrame 几何/缓冲共享、打开状态机、跨流检索错误——均有具体期望值。
   新测试文件无 `#[ignore]`、无 sleep。

## 7. 建议行动（按优先级）

1. **裁决 `block.rs:93-107` setter 互换**（产品缺陷，§3.1）：修复时按
   graphops NOTE 的意图 "deliberately" 改写约 12 个期望值；先给
   `undocommands_test.rs` 的无标注固化处补偏差 NOTE、给全文件补
   media_in 断言维度、给 `RippleInfo` 加公开构造器（M5）。
2. **消灭"双向接受"**（§3.2）：`render_test.rs:932` 的 Err 分支按错误
   内容分流，产品性错误必须失败。
3. **修回放验收测试的跳过判别**（§3.3）：source-window 补槽位检查；
   给严格模式真实机制（CI 设 env 禁止跳过）；`WorkerBinGuard` 改从
   `current_exe()` 推导。
4. **修 CI 恒跳过**（§3.4）：apt 装 ICC 包；`gpu_context_for_import`
   尊重 `OAK_REQUIRE_GPU`；实现计划承诺的"跳过单列"（CI annotation
   或汇总）。
5. **对齐 `manager.rs::init()` 文档与实现**（M1）。
6. 小修批次：M2/M3 补格式与效果链断言、M4 补读回、M7 改容差注释、
   M9 改 RAII、M8 在 CI 断言确定结果、§5 各项。
7. **流程**：M5 功能与覆盖率测试分开提交（§2 末）。

## 8. 审查局限

- 内联测试约 3 万行，采取"删除行全查 + 模式扫描全查 + 高风险区抽样
  精读"策略，未逐行读完；[子代理] 标注项中未注明"已复核"的细节以
  子代理报告为准。
- 未重新运行测试套件与覆盖率；文中覆盖率数字引自计划文档 §7.5 自述。
- `block.rs` 互换缺陷的 C++ Olive 原始语义对照基于代码注释与仓库内
  其他模块（undosplit/undoripple/undogeneral）的补偿性注释推断，未
  直接核对 C++ 源码。

## 9. 处理记录（2026-09-22）

本报告的高/中危与 §5 条目已按下列状态处理；仅 §3.1 的语义裁决按报告
建议延后（准备项已落地，修复清单已细化）。

### §3.1（准备项已落地，语义裁决延后）

**已用仓库内 C++ 上游源码核准语义**（此前报告 §8 的局限已解除）：

- `.cache/olive-upstream/app/node/block/block.cpp`：`Block::set_length_and_media_out/in`
  只调用 `set_length_internal(length)`，**从不移动时间线 in/out**；
  in/out（`in_point_`/`out_point_`）由 Track 的布局（`track.cpp:469-470,514-515,575-579`）
  按前后块与长度推导。
- `.cache/olive-upstream/app/node/block/clip/clip.cpp`（非 reverse 剪辑）：
  - `set_length_and_media_out`：`media_in` **不变**（media out 随长度移动）；
  - `set_length_and_media_in`：`media_in += old_length - length`（media out 固定）。
- `.cache/olive-upstream/app/timeline/timelineundopointer.cpp` / `timelineundogeneral.cpp`：
  `BlockTrimCommand` 的 TrimIn/TrimOut 与 `BlockResize*` 的调用与本仓库一一对应，
  因此缺陷只在 Rust 的两个 setter（绝对 range 模型把 Track 布局效果错误地编码进
  setter）。

**结论：不是两个函数简单对调即可**。同一 C++ 函数在不同命令里需要不同的净时间线
锚定：

| 命令 | 时间线锚定 | media 调整 |
|---|---|---|
| `BlockResizeCommand` | in 固定、out 移动 | 无 |
| `BlockResizeWithMediaInCommand` | in 固定、out 移动 | `media_in += old−new` |
| `BlockTrimCommand`(TrimIn) | out 固定、in 移动 | `media_in += old−new` |
| `BlockTrimCommand`(TrimOut) | in 固定、out 移动 | 无 |

另有 `TrackSlide`、`TrackListInsertGaps`、`TrackListRippleToolCommand`、multicam 等
调用点与 undosplit/undoripple/undogeneral 的补偿性选边需要一并裁决；应作为独立
PR 处理（先按 §7 第 7 条把 M5 与覆盖率测试分开提交）。

**本批已落地**：

- `RippleInfo::new(block, append_gap)` + `block()`/`append_gap()` 访问器；
  `undocommands_test.rs` 新增 2 个真实 info 测试（resize 往返、append_gap
  插入/撤销），使 `TrackListRippleToolCommand::ripple()` 主体可达（§M5）。
- 补齐 `media_in` 断言维度：roll-edit、`BlockResizeWithMediaInCommand` 往返；
  相关期望值加 **KNOWN-SWAP** 标注（roll、slide、insert-gaps、
  ResizeWithMediaIn），语义修复时按标注"故意"改写。
- 修正 `graphops.rs` 内联测试 NOTE 的措辞（follower 锚定的是 IN，不是 OUT）。

### 高危（其余三项）

- **§3.2 ✅**：`render_test.rs` 私有调度器测试的 Err 分支按错误内容分流——
  仅 `Error::Failed` 且消息以 `"render worker pool"` 开头（池创建/启动失败）
  或缺少 worker 二进制（`locate_oak_worker` 提前打印跳过）才跳过；产品性错误
  （`"Frame render ticket failed"`、shm 转换、时间戳等）一律 panic。管理器/
  环境用 RAII `DispatcherEnvGuard` 恢复，断言前显式 `restore()`；两个交付腿
  均断言 `formats == [F32]`。
- **§3.3 ✅**：三个回放验收测试的跳过条件收紧为**零交付**（`filled`/`delivered`
  为历史最大值，一旦有过交付就禁用跳过）；"交付过但服务/显示从未取用" 走有界
  失败上限并打印 `serve_misses` 等诊断。新增 `OAK_STRICT_PLAYBACK=1` 严格模式
  （跳过改为 panic，含 `#[should_panic]` 自测）。`WorkerBinGuard` 改用
  `renderops::test_worker_bin()` 解析链（已有 env → `current_exe()` 同级 →
  `target/{debug,release}` 兜底），仅在文件真实存在时设置变量并恢复旧值；
  `process_backend_preview_path_is_zero_copy` 不再硬编码路径。
- **§3.4 ✅**：CI apt 增加 `icc-profiles-free`；同时给 `system_icc()` 补
  Ubuntu 实际路径 `/usr/share/color/icc/sRGB.icc`（实测 5 个 display-ICC
  测试真跑，BGRA8 中灰精确为 `[128,128,128,255]`）。`gpu_context_for_import`
  尊重 `OAK_REQUIRE_GPU=1`（硬失败），所有跳过路径打印 `SKIP:` 标记；文件头注
  记录 lavapipe CI 恒跳过与真机验收出处。

### 中危

- **M1 ✅（裁决：文档化同步契约）**：证据为 C++ `taskmanager.cpp` 仅创建
  管理器、移植版 codecbridge 的 "submission is synchronous" 契约与
  `TaskManager::init` 无生产调用点。已把 `manager.rs` 模块/`init`/`shutdown`
  文档与字段说明改为"不注册"，测试断言 `init` 后未注册、且手工翻标志不会安装
  oakcodec 回调。
- **M2 ✅**：`Recorder` 记录 `formats`/首像素；新增生成帧路径的强制 **U8**
  测试（可证伪），另断言 F32 腿的交付格式。
- **M3 ✅**：夹具效果从 Invert 换成 **Opacity = 0**；enabled=true 必须全透明、
  enabled=false 必须非透明——效果链丢失或忽略 enable 标志都会失败。
- **M4 ✅**：writethrough 往返测试插入 `"Written Through"` 文件夹后写穿，断言
  `library.db` 存在且非空，并用全新 `backend().load_project` 读回 uuid 与标记。
- **M5 ✅**：`RippleInfo::new` + `block()`/`append_gap()`，新增 2 个真实 info
  测试（resize 往返、append_gap 插入/撤销），`ripple()` 主体可达。
- **M6 ✅**：两处 FCPXML 往返补齐轨道/块/footage/转场偏移断言
  （转场居中偏移 `3/8`）。
- **M7 ✅**：容差对齐先例 0.08；真机实测 sRGB 0.00878 / ACEScg 0.01141，注释
  改为正确论证（GPU 双线性色度 vs swscale 滤波）并打印实测值。
- **M8 ✅**：`worker.rs` 渲染器变体测试改为结果不变量断言（成功↔`is_open_gl`、
  失败↔阶段错误串）、`has_renderer()` 与工厂结果一致、崩溃钩子 env 用 RAII、
  一次性告警补并发存活断言；`instance.rs` 的 5 个 trace 调用全部断言返回值；
  `history.rs` 断言 undo/redo 后 `history_index` 与 `done` 变化（行数按面板
  契约保持不变，已在注释说明）。
- **M9 ✅**：`StagingDecodeGuard` RAII + 显式 drop；外部预设的 `OAK_GPU_IMPORT`
  值会被恢复。
- **M10 ✅**：`external.rs` 新增测试模块：顶部文档列明未单测的 unsafe 平台体与
  对应平台 job；4 个测试覆盖纯映射（Vulkan/IOSurface 格式）、参数校验（DMA-BUF
  零尺寸/非平面格式）与 D2 别名视图回归。

### 低危 / §5

- oak-task：`import_test` 断言 `Err(Error::Cancelled)`；`codecbridge_test` 同时
  断言桥接层消息与直接驱动 `ConformTask`/`ProxyTask` 的路径原因；
  `precache_test` 用真实运行兑现"窗口/进度"声明；`task_test` 观测进度钳制；
  `render_test` 的配置器边界、透明帧、shutdown 恢复顺序均已修正。
- `gpuinterop.rs`/`footage_import_test.rs` 的 env 泄漏改 RAII（含恢复外部预设）。

### 追加修复

- **新暴露 flake**：`app::tests::preferences_general_tab_writes_cache_ahead_and_storage`
  在满载并行下"点击→按键"焦点交接竞态（1/4 次读到 120）。点击后补一次
  effect flush + 一帧绘制再派发按键；通过。

### 验证

- 各 crate：oak-task 133、oak-storage 74、oak-core 456+12、oak-render 全部、
  oak-worker 全部、oak-plugin 208+、oak-app lib 544（含 `OAK_STRICT_PLAYBACK=1`
  一次）、oak-timeline 全部。
- 全 workspace 终验（2026-09-22）：**104 个测试目标、3179 passed / 0 failed /
  7 ignored**（ignored 为既有 GPU 门控）。
- 覆盖率（插桩终测，干净对象集同口径）：区域 93.96%、函数 89.59%、行
  **93.49%**（180742/193331）、分支 **71.58%**（11805/16491）。分支较修复前
  微降 0.37pp——本批刻意移除了若干"容忍型/同义反复"的执行路径（§3.3 交付即
  失败、M3 夹具更换、M8 去重），属测试质量的预期代价。

## 10. 第二轮全量审查（2026-09-22）

> 触发：§9 修复完成后，用户要求全量复审。范围：全部未提交改动
> （66 文件，+38731/−281），含第七批修复自身。方法：五个深查子代理
> 分区逐行通读（oak-app 面板群 19 文件、real.rs/renderops、
> oak-render/oak-worker 269 个测试、oak-plugin/oak-codec 约 250 个测试、
> oak-task/core/timeline/storage），审查会话对全部高危论断与 24 条修复
> 声称的关键链条做了原文实证。标注约定同前（[实证]/[子代理]）。

### 10.1 修复声称验证总裁定

**24 条声称：20 条完全属实，4 条部分属实，0 条虚假陈述。**
所有核对到的新断言均为可证伪的真断言。上一轮四个高危全部实质化解：

- **§3.1**：语义裁决延后**有据**——`.cache/olive-upstream` 的
  `block.cpp:62-79`（C++ setter 只调 `set_length_internal`，时间线位置
  由 Track 布局推导）与 `clip.cpp:95-124`（非 reverse 时 media_out 版
  不动 media_in、media_in 版 `+= old−new`）经原文核对，支持"不是简单
  对调、需逐命令裁决"的结论；§8 原局限（未核对 C++ 源码）解除。准备
  项落地：`RippleInfo::new` 被 2 个真实测试使用（resize 往返断言
  `[-10,100)`+`media_in==-10`、append_gap 断言块数 1→2+Gap 类型，
  ripple 主体确认可达）；KNOWN-SWAP 标注 6 处；`clip_media_in` 断言
  1→11 处。[实证]
- **§3.2**：`worker_pool_environment_error` 分流精确（`Error::Failed` +
  `"render worker pool"` 前缀，全仓库仅池创建/启动两处产生该前缀，
  产品错误消息穷举核对均不匹配）；`DispatcherEnvGuard` RAII + 断言前
  显式 restore；两腿断言 `formats == [F32]`。**"永不失败"测试已消灭。**
  固有残留：worker 存在但握手/协议回归导致 `start()` 失败仍归环境跳过
  （有打印）。[实证+子代理]
- **§3.3**：三验收测试统一"历史零交付才可跳过"（`filled/delivered`
  取历史最大值，一旦有交付跳过分支永久不可达；source-window 旧缺陷已
  修）；"交付但未服务"走有界 `assert!(pumps < FAIL_PUMPS)` 失败并携
  `serve_misses` 诊断；`OAK_STRICT_PLAYBACK=1` 严格模式 + 设计正确的
  `#[should_panic]` 自测；`test_worker_bin()` 解析链（env 存在性过滤→
  current_exe 同级→target 兜底，`None` 时不动 env）修掉了硬编码复合
  隐患；早期 playhead 断言修复与生产 prune 语义精确匹配。**残留：
  ci.yml/cd.yml 均未设置 `OAK_STRICT_PLAYBACK`**——门禁 job 上零交付
  仍为绿色跳过（可辩护的权衡：全量并行下池饥饿本征 flaky，且兄弟测试
  对管理器启动失败一律硬失败、worker 全坏会在别处红；但应在文档显式
  登记该取舍）。[实证+子代理]
- **§3.4**：ci.yml 增 `icc-profiles-free`（注释精确指向黑屏守卫）+
  `system_icc()` 补 `/usr/share/color/icc/sRGB.icc`（该包实际落点）
  [实证]；`gpu_context_for_import` 经 `require_gpu_adapter()` 在
  `OAK_REQUIRE_GPU=1` 下硬失败，footage_import 三处跳过均带 `SKIP:`，
  头注如实登记 lavapipe 恒跳过与真机验收出处 [子代理，头注已实证]。
  残留（低）：color.rs(15)/eval.rs(4)/procpool.rs(2) 的旧式 `skipping`
  打印未统一为 `SKIP:` 标记（CI 上因 ICC/OCIO/二进制修复均不再触发）。
- **M1–M10 与低危批**：M1（文档改为事实 + 真注册表查询断言）、M2
  （Recorder formats + 强制 U8 可证伪测试）、M3（Opacity=0 夹具，双分支
  断言互斥）、M4（sqlite 落盘 + load_project 读回 uuid/label）、M5、
  M7（0.08 对齐先例，实测值打印）、M8（history.rs 真撤销栈三维断言；
  instance.rs 5 个 trace 调用全断言且分支敏感；worker.rs 渲染器不变量
  +`has_renderer` 一致性+EnvGuard+并发存活）、M9（StagingDecodeGuard
  恢复外部预设）全部属实。[实证+子代理]
- **部分属实 4 条**：
  1. **M6**：otio_backend_test 全齐（轨道/块/footage/转场 3/8 居中
     偏移，算术核对正确）；loadotio_test 补了轨道/块/footage 但无转场
     断言（夹具本就无转场）——声称"两处"只兑现一处。
  2. **M10**：external.rs 顶部文档如实列明未单测平台体；4 测试中 3 个
     完全真实（Vulkan/IOSurface 格式映射、DMA-BUF 参数校验精确变体）；
     **"D2 别名视图回归"守卫不可证伪**——只对三种别名 `.expect()` 视图
     创建成功，而删掉生产修复行（external.rs:607 `dimension: Some(D2)`）
     后 `create_view` 照样成功（派生 D2Array 合法），真正的回归（planar
     pass 的 D2 layout 校验拒绝 bind group）只在**创建 bind group** 时
     显现，测试未建 bind group。[实证]
  3. **M8-worker 残留**：渲染器两测试仍是 Ok|Err 双向接受（错误串前缀
     对即过），未按 `OAK_REQUIRE_GPU` 分流成确定断言——CI（lavapipe）上
     "渲染器创建失败"回归不可检出。比 §3.4 已实现的硬失败策略弱一档。
  4. **§3.3-CI 接线**：见上。
- 追加修复核实：`source_monitor` 早期 playhead（属实，与生产 prune
  语义逐值核对）；`preferences_general_tab` 点击→按键竞态（属实，补
  `run_until_parked`+整帧 draw+再 park 的同步节拍，断言保持严格
  `==128` 无容差，非弱化；配置键 RAII Restore）。[实证+子代理]
- dialogs 184 期望值再确认：gpui_widgets `SliderModel` 吸附实现为
  `((raw−min)/step).round()`（半值远离零）→ (180−120)/8=7.5→8→184，
  与控件既有测试固化的契约自洽。[子代理，公式已核对]

### 10.2 全量深查新发现（第一轮未覆盖区域）

**高（1 项）**

| 位置 | 问题 |
|---|---|
| `panels/inspector.rs:564-568` | **恒真断言**：`assert!(debug_bounds(..).is_none() \|\| ..is_some())` 逻辑上永不失败；该测试设置 `pending_add=Some(0)` 并 draw 后本应断言 add 菜单 `.is_some()`。全工作区唯一一处恒真断言。[实证] |

**中（12 项）**

| 位置 | 问题 |
|---|---|
| `oak-render/cache.rs:999-1023` | mtime 守卫测试同义反复：`validate` 先污染内存使文件与内存恒同构，`load_state` no-op/守卫反转/真跳过三种实现均通过；名义两个行为均不可证伪。修法：两次 load 间 invalidate 内存范围或用干净实例。[实证] |
| `pipeline.rs:959` ↔ `eval.rs:5636` | 进程级 working-space 竞态：pipeline 10 个测试 `pin_legacy_working_space` 不取 eval 的模块私有 `WORKING_SPACE_TEST_LOCK`，同一 lib 测试二进制并行——`footage_working_lut` 的 `is_some()` 有真实 flake 窗口。修法：锁提为 crate 级。[实证] |
| `eval.rs:1371-1390`（生产） | **M5 planar-resolve 失败→staging 回退分支零覆盖**，且现有测试缝（`decoded_frames` 全局缓存 + `planar_texture()` helper）本可单元级注入；留档"单机走不到"对该分支不成立。M5 最重要容错路径之一。[子代理] |
| `oak-app/mock.rs` ↔ `ofx_params.rs:4886/4825` | MockEngine 未实现 `set_effect_param`/`effect_push_button` 的记录与失败注入（计划 M3.4 缺口）→ 两个"路由/点击触发"名义测试不可证伪；生产侧 `let _ =` 丢弃 Err 加剧不可观测。[子代理] |
| `controls.rs:1842-1856`（及 1424-1434） | checkbox 键盘测试不订阅 `CheckBoxEvent::Toggled`，只断"状态不变"——space/enter 处理整体失效不可检出（同仓库 ofx_params.rs:2489 有正确示范）。[子代理] |
| `displaycolor.rs:558-578` | 缓存/generation 测试用恒真 `>=` 断言，"同 key 命中不重建/换 key 重建"语义均未验证；:594-599 五个指纹调用零断言。[子代理] |
| `oakui/gpu.rs` | **全文件零测试**（M5 功能文件）：`upload_rgba16f` 参数校验分支（触碰 GPU 前返回）与 `build_display_lut` 纯 CPU 色彩数学均可确定性测试而未测。[子代理] |
| `project_explorer.rs:677-733` | RealEngine 菜单/打开/导入段：注释声称四种效果，整段零断言（同文件 mock 版对相同行为逐一断言，证明可断）。[子代理] |
| `real.rs:14789` + `:16234` | 代理 drain `matches!(Ready\|Failed)` 双向接受**原样保留**（§9 未声称处理），叠加弱断言后两文件内**无任何测试断言真实转码产出 Ready**——代理生成 happy path 无守卫。[子代理] |
| `real.rs:16147-16155` | "注释声称-结果丢弃"假断言：声称"删除后 name/path 查找 miss"，两个查询结果 `let _ =` 丢弃。[子代理] |
| `renderops.rs:3749` | `ensure_render_manager()` 失败→SKIP+return；同情形 real.rs 一律 panic（10 处）——同仓库双标，管理器 init 回归可被静默跳过。[子代理] |
| `undocommands_test.rs:647-648` | KNOWN-SWAP 漏网：注释仍把互换行为写成规范（"Trim-out is out-anchored: the clip's in shifts to 25"），与 common.rs:39 及 §9 C++ 裁决表直接矛盾。另 :874/:907、:659-687 三处固化无标注；:820 slide 标注措辞（"grow into each other"）与断言（next 缩短）不符。[实证] |

**中低/低（择要）**

- `hwdecode.rs:331` 全局 `CREATE_ATTEMPTS` 计数器断言未取 `lock_tests`
  ——并发的 ffmpeg 真实媒体测试会递增它，唯一可能产生 CI 间歇红的
  竞态（VDPAU 选型已封死污染面，仅剩自身受竞态）。[实证+子代理]
- `decoder.rs:899`/`encoder.rs:362` 注册表注入还原非 panic 安全
  （`registry_guard()` 只是锁、Drop 不还原；断言 panic 则 stub 注册表
  残留进程，后续测试经 poison 恢复静默用假解码器）。[实证]
- `node_factory.rs:2206` 只测自建 mock 入口的边界分支（零生产代码）；
  `:1672` 读两次全局计数断言相等（近同义反复 + 无锁 flake）；`:2350`
  无断言尾段；`gl_render.rs:1200` 零断言（注释诚实）。[实证+子代理]
- `external.rs:715` DMA-BUF 校验测试自建软跳过不受 `OAK_REQUIRE_GPU`
  约束（Linux CI 是 Vulkan，不触发）。
- `backend.rs:2886` `user_config_env_override` 非 RAII（M9 同类未修到
  此处）；`:3579` `OAK_REQUIRE_GPU` 翻转窗口与 16 个 GPU 测试竞态（仅
  无 GPU 主机）；`:2951` shared-slot 测试锁纪律不一致（潜在）。
- `color.rs:1164/1332` if-let 包裹全部断言的静默逃逸（CI 有真 OCIO，
  应硬断言）；OCIO 缺失跳过仍静默（无 eprintln）；`:1420` 不持
  config_lock 且恢复非 RAII。
- 裸 `is_err()` 无消息断言（系统性偏弱，非个别假测试）：graphops 23
  个错误矩阵（分支隔离夹具 + 精确成功腿维持可失败性）、real.rs 无工程
  守卫约 35 处（同文件 :11490 有正确示范）、encodingparams 11 个非法
  XML（生产明确区分两种失败原因而未断）。
- "结果丢弃/弱断言"群：source_viewer.rs:375-454（config 动作前后读取
  全丢弃、transport 9 个只断 handled）、program_viewer.rs:843（workarea
  三事件未断）、timeline.rs:2410（约 20 个命令只断 handled 布尔）、
  app.rs:8786（`exit_action_quits` 零断言）、eval.rs:5467/5557（无断言）
  /6087（OAK_PERF 名义不符）/5585（"and_clears" 半截）、procpool.rs
  :4752/:4764（smoke/半验注入）、worker.rs:2179（frame_failed 不断错误
  文本，同族 :2339 证明可断）、nodeops.rs:1332（零断言，名称诚实）、
  writethrough_test.rs:96/277（§5 指出未修，未声称）、precache "窗口"
  半边仅 liveness 间接兑现。
- env 非 RAII 残留（与已修 M9 同款）：ofx.rs:926 `MARKER_ENV`、
  program_viewer.rs:964 `HOME`、eval.rs:6095 `OAK_PERF`、
  renderops.rs:2881 配置开关、worker.rs:1992 颜色设置。
- 卫生：`crates/oak-task/tarpaulin-report.html` 覆盖率工件混入源码树
  （内嵌他机绝对路径，不应提交）；计划文档"nodeops 32 个"实为 24 个、
  node_factory/render_driver 的批次计数为组合计（文档口径应注明）。
- `ofx_host.rs:68-77` 生产 `emit` 的 `cfg!(test)` 早退：为测试改形的
  轻微实例（shipping 行为不变，集成测试真管道补偿），记录在案。

### 10.3 正面确认（防误伤）

- **无成建制造假**：约 500+ 个新测试中，纯假（恒真/零断言/零生产代码）
  共 6 个，均有诚实注释或属笔误级；无 `#[ignore]`、无理由 sleep（全部
  有界轮询+deadline+超时 panic）；故障注入类全部真注入且断言注入生效
  （真 SIGSEGV+marker、/bin/false OFX host→紫帧、假 nvidia-smi 六形态）。
- **修复未引入弱化**：preferences 竞态修复是补同步节拍而非放宽容差；
  render_threads 逐字节断言原样；分支覆盖 −0.37pp 是移除容忍路径的
  如实代价且已留档解释。
- 教科书级真测试代表（各区各选）：node_editor.rs:844（绕过面板证明
  守卫在产品代码）、procpool.rs:4780（PATH 注入六形态+精确换算）、
  worker.rs:1757（跨进程 shm 游标环游+0xAB 传播）、eval.rs:4415（GPU
  旋转单像素精确+全帧 lit==1）、clip.rs:600（f16 −0 位级保留）、
  encodingparams.rs:1096（C ABI offset_of 18 项冻结锁）、app.rs:6610
  （"全分支"名副其实）、dialogs.rs:6483（按键改派+磁盘内容往返）、
  ofx.rs:920（真插件 marker 参数串+生命周期顺序）、history.rs:325
  （真引擎撤销栈三维断言）。

### 10.4 最终判定与建议

**判定：修复批次诚信且实质有效；工作区达到可提交状态，但下列 8 项
建议先修（均为小改动）：**

1. `inspector.rs:564` 恒真断言改 `.is_some()`（一行）。
2. `undocommands_test.rs:647` 补 KNOWN-SWAP 标注（并顺手修 :820 措辞、
   补 :874/:907、:659-687 三处）。
3. `cache.rs:999` mtime 测试改为可证伪构造。
4. pipeline/eval 的 working-space 锁提为 crate 级共享。
5. `external.rs:744` D2 守卫补 bind group 创建（或改名去掉"回归守卫"
   声称）。
6. `real.rs:14789` 代理 drain：至少在转码可用的环境断言 Ready（可按
   ffmpeg 可用性分流，不可用才容忍 Failed）。
7. `renderops.rs:3749` SKIP 改 panic（与 real.rs 对齐）。
8. 删除 `crates/oak-task/tarpaulin-report.html`。

**登记事项（不阻塞提交）**：`OAK_STRICT_PLAYBACK` 的 CI 接线取舍；
`hwdecode.rs:331` 补 `lock_tests`（CI 间歇红风险）；decoder/encoder
注册表还原改 RAII；M5 planar 回退分支注入测试；gpu.rs 纯 CPU 分支
补测；MockEngine `set_effect_param` 族记录缺口；旧式 `skipping` 打印
统一 `SKIP:` 标记；block.rs setter 语义裁决独立 PR（§9 清单）。

**提交拆分（重申 §2 末建议）**：M5 功能（gpuinterop/external/Planar/
hwdecode 顺序/RefFrame）与覆盖率批次（测试+两处 bug 修复+文档）分开
提交；tarpaulin 工件与本报告/计划文档更新可并入文档提交。

### 10.5 §10.2/§10.4 处理记录（2026-09-22）

**"建议先修 8 项"：全部完成。**

1. ✅ `inspector.rs` 恒真断言 → 以具有 debug selector 的分组头断言菜单
   已渲染，并断言 `pending_add == Some(0)` 生效。
2. ✅ `undocommands_test.rs` KNOWN-SWAP 标注补齐：:647 措辞改为与
   `common.rs:39`/§9 裁决表一致，:659-687、:874、:907 补标，slide 处
   措辞修正（next 缩短）。
3. ✅ `cache.rs` mtime 守卫测试改为可证伪构造（干净实例装载 + 内存/文件
   不再恒同构），守卫反转或真跳过都会失败。
4. ✅ working-space 锁提升为 crate 级 `eval::working_space_test_lock`，
   pipeline 与 eval 两侧共同持锁。
5. ✅ `external.rs` D2 守卫补 bind group 创建（临时删修复行可复现失败，
   已还原）。
6. ✅ `real.rs` 代理 drain 按 ffmpeg 可达性分流：可达硬断言 `Ready` +
   `ProxyBadge::Ready`；不可达打印 `SKIP:` 并容忍 `Failed`（两种环境均
   实测）。
7. ✅ `renderops` 的 `ensure_render_manager` 失败测试改为硬断言（仅
   "管理器已初始化" 保留自保护跳过）。
8. ✅ 删除 `crates/oak-task/tarpaulin-report.html`（及其绝对路径泄漏）。

**中危 12 项：**

- ✅ MockEngine 记录 `set_effect_param`/`effect_push_button` 调用（含
  失败注入开关），`ofx_params` 生产侧改错误日志；两个名义测试改为完整
  路由矩阵 + 类型转换值断言（曲线 JSON 先 `set_points` 再发事件）。
- ✅ `controls.rs` checkbox 键盘测试订阅 `CheckBoxEvent::Toggled` 并断言
  事件序列；space/enter 处理整体失效会失败。
- ✅ `displaycolor.rs` generation/链缓存改为精确断言（同 key 不重建、
  换 key 恰好 +1、指针同一/不同），5 个指纹逐一断言。
- ✅ `oakui/gpu.rs` 补 4 个测试：`upload_rgba16f` 参数校验、f16 打包
  （钳制/字节序/256 对齐行距）、display LUT 网格完整性与确定性、key
  跟随色彩设置。
- ✅ `project_explorer.rs` RealEngine 段补断言（序列切换、导入计数、
  菜单选择与开关状态、引擎分支前置条件）。
- ✅ `real.rs` 两处丢弃的 name/path 查询改为断言；删除后 rename no-op
  一并断言。
- ✅ `hwdecode.rs:331` 取 `lock_tests`；decoder/encoder 注册表注入改
  RAII `RegistryGuard`（panic 不再泄漏假解码器）。
- ✅ `color.rs` if-let 逃逸改硬断言（OCIO 缺失才打印跳过）；配置锁
  RAII。`backend.rs` env 覆盖 RAII、共享上下文测试锁纪律统一、
  `OAK_REQUIRE_GPU` 竞态窗口消除。
- ✅ env 残留 RAII：`ofx.rs` MARKER_ENV、`program_viewer.rs` HOME、
  `eval.rs` OAK_PERF、`worker.rs` 颜色设置、`renderops.rs` 配置开关。
- ✅ 计划文档计数修正（nodeops 32 → 24，组合批次注明口径）。

**登记（不阻塞提交，遗留项）**：`eval.rs` M5 planar 回退分支注入测试
（需要专用测试缝）；`node_factory.rs` 三处弱断言与 `gl_render.rs` 零断言；
旧式 `skipping` 打印统一 `SKIP:`；`OAK_STRICT_PLAYBACK` 的 CI 接线取舍；
`block.rs` setter 语义裁决独立 PR（§9 清单）。

**验收（2026-09-22）**：全 workspace **104 个测试目标、3185 passed /
0 failed / 7 ignored**（ignored 为既有 GPU 门控）。本批生产改动仅
`MockEngine` 记录字段与 `ofx_params` 错误日志两处可观测性增强。
