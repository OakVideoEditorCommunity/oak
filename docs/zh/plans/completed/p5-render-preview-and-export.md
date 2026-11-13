# P5：渲染、异步预览、播放与导出实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P5。  
> 状态：P5A–P5D 已完成，P5 收尾完成，下一阶段为 P6。

## 1. 子阶段

- P5A：统一导出、取消、错误和 OTIO/FCPXML 任务。
- P5B：source/program `PreviewSession`、传输、异步帧、音频块、时钟、电平与缓存。
- P5C：montage、render ticket、进程池帧句柄和 worker 故障隔离迁移。
- P5D：切换 `real.rs`、`renderops.rs`、`audio_thread.rs`、viewer、导出对话框和 scopes。

## 2. P5A 范围与验证

- app 的媒体导出复用 SDK `Task`，以独立 cancellation handle 和后台轮询适配既有
  `ExportEvent`，不把等待或渲染放到 UI 线程。
- 导出参数和格式/编解码器表改用 SDK DTO；自定义范围、尺寸、帧率、码率、色彩和
  位深映射保持原语义。
- OTIO/FCPXML 打开与保存统一走任务 API。加载结果通过 storage migration handle
  保留同一项目 Arc，不重开、不复制工程。
- 独立 SDK 测试覆盖导出成功、协作取消、错误和 OTIO roundtrip；app 参数、错误路径和
  inline 后端端到端导出均通过。

## 3. P5B 首轮范围与验证

- `PreviewSession::source/program` 拥有各自的 editor session 和 viewer，支持 seek、
  play、pause、step、rate 与外部时钟 `tick`；program playhead 仍走会话语义命令。
- `PreviewFrameRequest` 在提交时捕获时间戳，后台渲染返回拥有型 F32 RGBA 帧，避免
  延迟结果被误认成新 seek 的帧；支持取消和一次性结果转移。
- `EditorSession::from_project` 允许 source/program 两个会话共享同一 Arc，同时保持
  独立的预览状态。
- 独立测试验证 10fps/25fps 逐帧、播放/暂停、倍率、零边界、program playhead 和两类
  异步帧。音频块、电平、缓存失效拓扑及 app 切换尚未宣称完成。

## 4. P5B 音频、时钟与缓存完成记录

- `AudioBlock` 提供拥有型 interleaved F32、时间范围、采样率、声道布局和逐通道电平；
  后台混音覆盖 montage、media-in、静音轨道、代理音频及绝对采样网格。
- 有 RenderManager 时保留 ticket/worker 路径；headless 消费者在 engine 后台线程使用
  同一 mixer。共享内存音频只复制一次并立即释放。
- `PreviewAudioRequest` 与帧请求都携带 generation，seek/显式 invalidate 可识别并丢弃
  过期结果。`PreviewSession` 支持设备推送、输出时钟、设备电平和播放暂停恢复；
  无输出设备时保留外部时钟回退。
- app 的音频 range 与播放预取已切到 SDK 任务，失败仍按预期长度补静音，既有
  `AudioPrefetch` 排序和主时钟策略不变。
- 独立测试验证 source/program 样本一致、48 kHz 帧数、非零电平、静音轨道和输出设备
  可用时的推送/时钟路径。app 的渲染入口、预取和时钟回退测试通过。
- engine / SDK 全量回归通过，其中 engine 169 个、SDK 110 个测试通过。

## 5. P5C 范围

- 视频 montage 迁入 engine，保留效果链、调整层、多机位、代理媒体选择与原生分辨率
  钳制。P5C ticket 显式使用 montage，避免 graph-snapshot 模式绕过这些语义。
- `VideoFrame` 统一 inline/ticket/worker 结果，明确 `Cpu`、`Gpu`、`SharedMemory` 来源和
  `F32Rgba`/`Bgra8` 布局；支持原始字节与转换后的 F32 样本。
- 共享内存帧在 engine 内复制并立即释放，SDK 只获得引用计数的拥有型 DTO；任务结果
  仍只能转移一次。`PreviewSession` 已切换到同一视频 DTO。
- `NativeMirror` 覆盖视频元数据 roundtrip，worker 失败后可以继续渲染，不为失败任务
  保留结果槽。

## 6. P5C 验证

- inline 测试覆盖 source/program、F32/BGRA8 和非黑像素；manager ticket 测试覆盖
  调整层压黑和真实 32×32 代理对 128×128 请求的钳制。
- 构建 `oak-worker` 后，沙箱外进程池测试通过：`SharedMemory` 来源、BGRA8 字节数、
  非黑像素、失败后成功重渲染及克隆释放。沙箱内同一测试明确记录为跳过，未冒充通过。
- `PreviewSession` 的传输与异步帧回归仍通过；engine / SDK 全量回归通过，其中
  engine 169 个、SDK 113 个测试通过。

## 7. P5D app 切换与验证

- `real.rs` 的 source/program 播放窗口改为轮询 SDK 视频任务；预取保留 Playback
  优先级、距离排序和 generation 失效，暂停/冷缓存首帧也不再同步等待 UI 渲染。
- 过期任务先协作取消并保留到终态后释放，避免在 UI 线程 join worker。全分辨率
  填充与代理帧竞态时先落地代理及 scopes，再选择全分辨率显示图。
- `renderops.rs` 的音频、视频和导出运行路径统一使用 SDK 任务；render manager
  初始化也走 SDK。viewer/scopes 使用同一拥有型 `VideoFrame`，不再持有或释放
  worker 共享内存槽。
- 多机位角度的单轨 montage 迁入 engine，SDK 提供单轨视频任务；app 不再为角度
  面板构建私有 ticket。
- `audio_thread.rs`、旧 montage/ticket helper 和旧帧变体只剩兼容测试代码，运行
  路径不再调用；物理删除和内部 crate 依赖清零属于 P7。
- engine / SDK 单线程全量回归通过，其中 engine 169 个、SDK 114 个测试通过。
- app 沙箱外全量回归通过：730 个通过、1 个忽略；真实 `oak-worker` 进程后端
  验证 `SharedMemory` 来源、拥有型帧和后续重渲染，播放、冷 source、暂停 seek、
  播放后 seek、取消、导出及 scopes 路径均通过。
- `cargo test --workspace -- --test-threads=1` 全量通过；app/engine 边界守卫与
  `git diff --check` 均通过。
