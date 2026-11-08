# OFX / GPU 预览交接文档

日期：2026-11-08
范围：Quercus engine + Oak viewer 的 OFX、GPU 零拷贝、异步 readback、4K 播放性能。
接手人：Kimi

---

## 0. 一句话结论

这一周的核心目标是：

```text
硬件解码 -> GPU 合成/特效 -> 尽可能零拷贝上屏
```

当前最终状态是：

- **普通播放**：CUDA/NVDEC import + GPU montage + 同步/可选异步 readback，1080p 约 28~30fps；
- **4K 原生**：仍受 F32 readback 限制，约 4~5fps；
- **OFX GL 被临时默认关闭**：这是一个已知回归，必须由接手人恢复；
- **Vulkan 共享纹理零拷贝没有恢复**：共享 wgpu device/queue 在 OFX/VAAPI/GL 场景会出现 fence 死锁或 `Parent device is lost`；
- **彩条**已加“保留上一帧”兜底，但 preview miss 的根因还没完全消除。

---

## 1. 当前 commits

Engine（`/home/sunyu/Projects/quercus-engine`）：

```text
4a104ac fix(ofx): keep CPU plugin output targets on CPU
f621f43 fix(render): keep CPU OFX graph layers on the CPU compositor
03a5ac1 fix(ofx): default to the CPU image-effect path
e4d68fa fix(preview): keep async readback opt-in until project-open is safe
4fdf66e perf(readback): bulk-copy tightly packed rows
04c4e77 fix(gpu): serialize CUDA import with async readback
d38518a fix(gpu): hold the VMM slot guard on resolved textures
6445335 perf(gpu): import hardware frames into the engine's own context
5beb043 perf(decode): prefer NVDEC/CUDA hardware decode on Linux
37e47cb fix(codec): make VAAPI import opt-in to avoid driver stalls
a1c0b08 perf(render): start GPU montage from a transparent placeholder
2c9f6e4 fix(tasks): replace the locked receive loop with a condvar work queue
984144c fix(render): keep CPU layers off the GPU queue and track device loss
a6a7e4a fix(gpu): do not abort on wgpu device loss during readback
6865134 fix(gpu): avoid PendingWrites lock on render-thread uniform uploads
```

Oak（`/home/sunyu/Projects/oak-editor`）：

```text
83ec10791 chore: pick up CPU OFX output target
0a3e96ca0 chore: pick up CPU OFX graph fallback
8254a064f chore: pick up OFX CPU-only default
eb7e5c745 fix(viewer): require OAK_GPU_SHARED=1 explicitly
87247bb3d chore: pick up safe sync preview default
9d638773f fix(viewer): keep the last displayed frame across cache invalidation
6a1a9e95f chore: pick up bulk readback copy
c29193096 chore: pick up async readback serialization
32f20a96c chore: pick up VMM slot lifetime guard
89f41384e chore: pick up hardware import into isolated engine context
6adf4bc9e chore: pick up CUDA decode preference
2f7e81a47 fix(viewer): hold the last real frame instead of synthetic bars on a cache miss
c82d9ceed chore: pick up VAAPI import opt-in
d79d570e4 fix(viewer): isolate the engine queue by default
```

Oak 的 `quercus-engine` submodule 当前指向 engine 的 `4a104ac`。

---

## 2. 背景与目标

最初目标：

```text
4:2:0 / 4:2:2 / 4:4:4 / RGB
U8 / U16 / F16 / F32
任意分辨率
VAAPI / CUDA / D3D11VA / VideoToolbox 硬件解码
GPU 缩放、YUV->RGB、montage 合成、上屏
```

用户明确要求：

- Linux 一等公民；
- 硬件帧不要用 CPU `sws_scale`；
- 完整 F32 工作链；
- 接受 GPU 内拷贝；
- 跨平台硬件验证可以延后。

后来线上问题集中在：

1. 有 OpenFX 插件的 clip 播放一段时间冻结；
2. 打开工程 panic / 长时间无响应；
3. GPU clip 播放 12fps；
4. UI 冻结；
5. 彩条。

---

## 3. 本周修改逐项说明

### 3.1 task pool 的接收锁

**问题现象**

gdb 看到多个 `quercus-task-*` 停在：

```rust
let guard = rx.lock();
guard.recv()
```

**修改**

`crates/quercus-engine/src/task.rs`

- 从 `mpsc::Receiver` + `Mutex` 阻塞接收，改为：

```rust
Mutex<VecDeque<Job>> + Condvar
```

- worker 不再持锁阻塞在 `recv()`。

**commit**

```text
2c9f6e4 fix(tasks): replace the locked receive loop with a condvar work queue
```

**注意**

这不是最终根因；它只是去掉了“看起来像死锁”的接收锁。

---

### 3.2 GPU readback 不再让进程 abort

**问题现象**

OFX `fetchImage` -> `Texture::to_frame` -> `GpuContext::download` -> `device.poll` 触发 wgpu：

```text
ContextWgpuCore::handle_error_fatal
```

**修改**

`crates/quercus-core/src/backend.rs`

- `download` / `wait_idle` / `wait_idle_for` 对 `device.poll` 做 `catch_unwind`；
- 增加 `device_lost` 标志；
- device lost 后后续 readback 直接返回错误，不再反复 poll。

**commit**

```text
a6a7e4a fix(gpu): do not abort on wgpu device loss during readback
```

---

### 3.3 render thread 不再走 `queue.write_buffer`

**问题现象**

gdb：

```text
quercus-render
  -> composite_montage_gpu
  -> run_shader_pass
  -> Queue::write_buffer
  -> PendingWrites mutex
```

**修改**

`crates/quercus-core/src/backend.rs`

- uniform 改为 `mapped_at_creation: true` + `get_mapped_range_mut()`；
- 不再从 render thread 调 `queue.write_buffer`。

**commit**

```text
6865134 fix(gpu): avoid PendingWrites lock on render-thread uniform uploads
```

---

### 3.4 16-bit normalized plane 能力检查

**问题现象**

`planar-matrix-bg` bind group invalid，之后 device lost。

**修改**

- Oak 窗口 device 请求 `TEXTURE_FORMAT_16BIT_NORM`；
- engine `import_opaque_fd_plane` 对 R16/RG16/RGBA16 做 feature 检查，不支持直接 CPU fallback。

**commits**

```text
6222135 fix(gpu): reject unsupported 16-bit external plane formats
407755f9b fix(viewer): enable 16-bit norm textures for engine imports
```

---

### 3.5 Rgba16Float 上屏

**问题现象**

gpui surface bind group 使用：

```text
TextureSampleType::Float { filterable: true }
```

engine 直接交 `Rgba32Float` 时 bind group invalid。

**修改**

- engine 在 host-adopted device 上才交 texture；
- 交付前 GPU 转 `Rgba16Float`；
- 用 `take_texture_handle` 转移 token 生命周期。

**commits**

```text
40088bc fix(gpu): present through a filterable Rgba16F texture
779cf50 fix(preview): only hand off textures on the host-adopted device
```

---

### 3.6 CUDA VMM slot 生命周期

**问题**

CUDA VMM slot 在完成回调未推进时会无限增长；4K 每 slot 32MB。

**修改**

- slot 上限 8；
- slot guard 附加到 resolved texture；
- 异步 readback 时 `FrameReadback` 持有 guard。

**commits**

```text
2303d1a fix(gpu): cap CUDA VMM slots when completion callbacks stall
d38518a fix(gpu): hold the VMM slot guard on resolved textures
```

---

### 3.7 CUDA import 被 host-only gate 挡住

**问题**

`try_import_hw_frame` 里有：

```rust
if !GpuContext::host_gpu_installed() {
    return None;
}
```

Oak 改为独立 engine context 后，这个 gate 永远 false，所有 CUDA 帧都走 CPU staging。

**修改**

- 删除 host-only gate；
- engine 自己的 Vulkan context 也允许 import。

**commit**

```text
6445335 perf(gpu): import hardware frames into the engine's own context
```

**验证**

C0024 1080p：

```text
imports=30 transfers=0
render p50≈24ms
readback p50≈7ms
约 28~30fps
```

---

### 3.8 VAAPI import 默认关闭

**问题现象**

冻结日志停在：

```text
[vaapi-import] descriptor=...
```

之后进入 `vkCreateImage` / `vkAllocateMemory`，驱动卡死。

**修改**

- VAAPI import 改为 opt-in：

```bash
QUERCUS_GPU_IMPORT_VAAPI=1
```

- Linux 解码优先 CUDA/NVDEC。

**commits**

```text
37e47cb fix(codec): make VAAPI import opt-in to avoid driver stalls
5beb043 perf(decode): prefer NVDEC/CUDA hardware decode on Linux
```

---

### 3.9 GPU montage 不再每帧 clear submit

**修改**

`crates/quercus-render/src/eval.rs`

- 第一层从 1×1 transparent placeholder 开始 alpha-over；
- 去掉正常路径的 `clear_texture -> queue.submit`。

**commit**

```text
a1c0b08 perf(render): start GPU montage from a transparent placeholder
```

---

### 3.10 Oak 隔离 engine queue

**问题**

UI 主线程：

```text
Surface::get_current_texture
  -> device.fence.read()
```

render thread：

```text
Queue::submit
  -> fence.write
  -> pending_writes.lock()
```

共享 wgpu queue 时形成 fence/pending 锁阻塞，UI 完全冻结。

**修改**

Oak 默认不把窗口 device/queue 交给 engine：

```rust
// 仅 OAK_GPU_SHARED=1 才共享
```

engine 使用自己的 wgpu context；preview 通过 CPU readback 交付。

**commits**

```text
026604cd5 fix(viewer): do not share the GL wgpu queue with the engine
d79d570e4 fix(viewer): isolate the engine queue by default
eb7e5c745 fix(viewer): require OAK_GPU_SHARED=1 explicitly
```

---

### 3.11 OFX GL 默认关闭（已知回归）

**背景**

OFX GL 是用户特效的主力路径；不能长期关闭。

但当前 OFX GL 路径会造成：

- `Parent device is lost`；
- `write_output_frame` download 时 crash；
- GPU dst texture upload/download。

**临时修改**

- `crates/quercus-ofx-plugin/src/render_driver.rs`
  - `use_opengl` 需要显式 `QUERCUS_OFX_GL=1`；
- `crates/quercus-ofx-plugin/src/node_factory.rs`
  - CPU 插件输出改为 CPU dst，不再 GPU dst；
- `crates/quercus-render/src/eval.rs`
  - graph 合成遇到 CPU 图层直接 CPU compositor，不上传。

**commits**

```text
03a5ac1 fix(ofx): default to the CPU image-effect path
f621f43 fix(render): keep CPU OFX graph layers on the CPU compositor
4a104ac fix(ofx): keep CPU plugin output targets on CPU
```

**这是临时 stability fallback，不是最终方案。**

---

### 3.12 彩条

**原因**

Oak `cpu_frame` 在 preview cache 完全 miss 时返回：

```rust
synthetic_frame_samples(frame)
```

**修改**

- `MonitorFrameCache.last` 保留上一帧；
- `RealEngine.last_display` 跨 cache invalidation 保留最后一帧；
- 只有从未显示过任何帧时才允许彩条。

**commits**

```text
2f7e81a47 fix(viewer): hold the last real frame instead of synthetic bars on a cache miss
9d638773f fix(viewer): keep the last displayed frame across cache invalidation
```

---

### 3.13 异步 readback

**实现**

- `GpuContext::download_async` + 专用 readback 线程；
- `Texture::to_frame_async`；
- preview 可走 async；
- `external_lock` 串行化 CUDA VMM import 与 readback；
- 异步默认关闭，只有 `QUERCUS_ASYNC_READBACK=1` 才打开。

**commits**

```text
04c4e77 fix(gpu): serialize CUDA import with async readback
e4d68fa fix(preview): keep async readback opt-in until project-open is safe
```

**注意**

async + CUDA import 的 device lost 仍未完全解决，所以默认关闭。

---

## 4. 当前可复现基准

命令：

```bash
cd /home/sunyu/Projects/quercus-engine
cargo run -p quercus-render --example bench_readback -- \
  "/home/sunyu/Videos/20260404双创宣传片素材拍摄/C0024.MP4" 20 1920 1080
```

当前结果（dev build）：

```text
imports=20 transfers=0
render   p50≈24~27ms
readback p50≈7~8ms
稳态约 28~30fps
```

4K：

```bash
... 10 3840 2160
```

结果：

```text
render   p50≈89ms
readback p50≈90ms
约 4~5fps
```

---

## 5. 已知未解决问题

### P0：OFX GL 必须恢复

当前强制 CPU 只是避免崩溃：

```text
QUERCUS_OFX_GL=1
```

才启用 GL。

但用户的特效是主要功能，不能默认 CPU。需要恢复 GL，同时避免 device lost。

建议方向：

- OFX GL 使用独立 GL context（`gl_bridge`）；
- 不要给 CPU 插件 GPU dst；
- GPU 插件输出若为 GL texture，必须走 EGL/GL 导入，不能用 wgpu download；
- GL 与 wgpu 的同步点（`glFinish` / external semaphore）要有超时；
- 不要和 gpui 共享 wgpu device/queue。

### P0：打开工程第一次 CUDA 初始化耗时 30s

日志：

```text
[hwfmt] selected AV_PIX_FMT_CUDA
... gap_ms=30540 ...
```

后续每次 render gap 3~5s，但 `job_ms` 只有 12~74ms。

说明：

- render thread 在等任务；
- 不在 GPU 合成；
- 可疑点：
  - 第一次 CUDA/NVDEC 初始化；
  - OFX CPU/graph 任务；
  - Oak preview 调度。

需要加：

- `render_video` 任务级计时；
- `process_plugin_job` 计时；
- `render_graph_frame` 计时；
- Oak `update_preview_window` 计时。

### P0：Vulkan 共享纹理零拷贝未恢复

共享 device/queue 会复现：

```text
fence.write / fence.read 死锁
Device::poll: Parent device is lost
```

当前默认隔离，`OAK_GPU_SHARED=1` 仍然危险，不要用于生产。

### P1：4K F32 readback

每帧 132MB：

```text
3840 × 2160 × 16 bytes
```

需要：

- GPU 转 `Rgba16Float` / `Bgra8` 再回读；
- 或真正的 Vulkan 共享纹理。

### P2：彩条

已用 last frame 兜底，但 preview miss 根因未消失。

需要日志：

```bash
OAK_DEBUG_PREVIEW=1
```

观察 `[preview] to_display ... took Nms`，以及 preview slot 是否频繁 miss。

---

## 6. 建议接手顺序

1. **先恢复 OFX GL 可用**
   - 目标是 `QUERCUS_OFX_GL` 默认开启；
   - 同时不 device lost；
   - CPU plugin 仍 CPU dst；
   - GL plugin 使用独立 GL context + 正确的纹理生命周期。

2. **补任务级计时**
   - `render_video` 的 graph/montage/OFX 分段时间；
   - 找到 30s 和 3~5s gap 的真实来源。

3. **4K 回读格式**
   - 先做 GPU `F32 -> Rgba16Float` 再回读；
   - 观察 4K readback 是否从 90ms 降到 ~45ms。

4. **最后再考虑 Vulkan 共享纹理**
   - 不要直接恢复旧实现；
   - 需要 watchdog、独立 fence、失败回退；
   - 任何共享 queue 卡死都必须能退出到 CPU。

---

## 7. 不要重复的失败实验

- 不要默认把 gpui 的 wgpu device/queue 直接交给 engine；
- 不要在 render/task 线程直接 `device.poll(wait_indefinitely)`；
- 不要让 CPU OFX 插件写 GPU dst；
- 不要在 CUDA import 和 readback 并发时不加串行化；
- 不要用 `vkCreateImage`/`vkAllocateMemory` 无查询直接导入 VAAPI DMA-BUF；
- 不要依赖 `device.poll` panic 被 catch 后继续使用同一个 lost device。

---

## 8. 关键代码位置

| 文件 | 作用 |
|---|---|
| `crates/quercus-ofx-plugin/src/render_driver.rs` | OFX CPU/GL render、use_opengl 判定、write_output_frame |
| `crates/quercus-ofx-plugin/src/node_factory.rs` | plugin job dst、CPU/GPU 输出选择 |
| `crates/quercus-ofx-plugin/src/gl_bridge.rs` | Linux EGL / GL 零拷贝桥 |
| `crates/quercus-codec/src/gpuinterop.rs` | CUDA VMM / VAAPI import、slot pool、external lock |
| `crates/quercus-core/src/backend.rs` | GpuContext、download、async readback、device poll |
| `crates/quercus-core/src/backend/external.rs` | DMA-BUF / OPAQUE_FD import |
| `crates/quercus-render/src/eval.rs` | montage/graph GPU 合成、CPU fallback |
| `crates/quercus-engine/src/preview_video.rs` | preview 路径、GPU texture handoff |
| `crates/oak-app/src/oakui/gpu.rs` | Oak device/queue 注册、共享开关 |
| `crates/oak-app/src/oakui/real.rs` | preview cache、彩条 fallback、full-res |
| `crates/oak-app/src/oakui/displaycolor.rs` | 显示色彩/LUT 门控 |
