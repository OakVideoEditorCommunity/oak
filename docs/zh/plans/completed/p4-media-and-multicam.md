# P4：素材、代理、波形、同步与多机位实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P4。  
> 状态：P4A–P4E 已完成，P4 整体完成；下一阶段为 P5。

## 1. 子阶段

- P4A：素材导入、替换/relink、项目浏览器写操作和真实媒体 undo/重开验证。
- P4B：素材参数覆盖与素材/sequence 互转。
- P4C：代理生成/删除/启停、自定义参数、进度与异步任务。
- P4D：缩略图、波形 DTO、同步 eligibility 与偏移估算。
- P4E：多机位建序列、角度切换、关联撤销和 app 切换。

## 2. P4A 范围

- `ReplaceFootage` 先探测新文件，成功后替换并保留代理、生成参数与源时间设置；
  完整 footage 状态支持 undo/redo，失败不改变工程或历史。
- 项目浏览器的重命名和删除复用会话语义命令，不复制工程。
- 使用真实媒体验证 NodeId 稳定、undo/redo、保存重开和错误路径。

## 4. P4B 范围

- `SetFootageProxyParams` / `ClearFootageProxyParams` 使用 SDK `ProxyParams`，
  支持自定义/effective 查询和 undo，不向 UI 暴露 codec 内部类型。
- `CreateSequenceFromFootage` 从首个视频流继承尺寸、帧率和隔行设置；音频素材使用默认格式。
- `PlaceNestedSequenceClip` 复用既有嵌套放置，完成 footage→sequence 与 sequence→嵌套片段路径。

## 5. P4B 验证

- 参数覆盖通过真实素材验证：自定义/effective 查询、清除、undo/redo 均正确。
- `CreateSequenceFromFootage` 正确继承 64x64 和 10fps；嵌套 sequence 片段成功放置。
- app 的 `engine_proxy_rows_and_params` 与空时间线自动建 sequence 测试通过。
- engine / SDK 全量回归：engine 168 个、SDK 98 个测试通过。
- 内部引用基线由 895 收紧至 893。

## 3. P4A 验证

- SDK 的替换/relink、重命名和删除测试通过；替换保留稳定 NodeId、undo/redo 和保存重开。
- app 的 `engine_replace_footage_and_entry_lookups` 与 `entry_rename_and_delete_are_real`
  通过，删除同时恢复图节点和文件夹槽位。
- `DeleteEntry` 与图编辑 `RemoveNode` 分离，避免浏览器挂载语义被图删除覆盖。
- engine / SDK 全量回归通过；app 全量 720 个通过、1 个忽略、15 个既有 worker/共享内存
  环境失败，无新增素材或历史失败。

## 6. P4C 范围

- `EditorSession::generate_proxy` 通过引擎统一任务注册表异步转码，返回可轮询
  进度、标题、错误和终态的 SDK `Task`，不在 UI 线程执行 ffmpeg。
- 会话记录 footage/task 对应关系，拒绝重复生成；`complete_proxy` 只接受匹配的
  活动任务，避免旧 worker 覆盖新任务状态。状态查询优先采用活动任务，其次读取
  磁盘状态和已记录失败。
- `DeleteProxy` 删除正式及 working 文件并清除素材关联，活动任务期间拒绝删除；
  文件删除不伪造 undo。`SetProxyEnabled` 只切换解码策略，不隐式启动 worker。
- app 的生成、进度、完成、删除、启停、自动启动、缓存路径和并发上限均改走 SDK；
  完成时才失效预览帧，进度更新不反复清空播放缓存。

## 7. P4C 验证

- 真实媒体 SDK 测试覆盖自定义参数继承、异步生成、进度/终态、重复生成与活动
  任务删除错误、Ready、启停、删除和失效 ID。
- app 的 `engine_proxy_rows_and_params`、`engine_proxy_generate_and_delete` 和
  `proxy_lifecycle_and_autostart_policy` 通过；覆盖自动启动、失败、opt-out、
  ready 跳过和缺失输入错误路径。
- engine / SDK 全量回归通过，其中 SDK 99 个测试通过。
- 内部引用基线由 893 收紧至 870。

## 8. P4D 范围

- `task::extract_waveform` 在引擎线程提取完整音频流，`Waveform` 以引用计数
  拥有不可变 DTO，提供真实声道数、采样率、窗口大小和逐通道 min/max；任务结果
  只能转移一次，失败保留任务错误。
- `task::render_thumbnail` 支持 sequence 与视频 footage，统一处理 GPU/CPU readback
  stride，返回拥有型 F32 RGBA `Thumbnail`。app 在既有后台线程等待任务并写 PNG，
  不再接触共享内存帧、GPU texture 或内部 render ticket。
- `ClipSyncInfo` 提供时间线/media range 和可选源时间码；同步 eligibility 不再
  直接读取 footage behavior。
- SDK 暴露普通/变速 envelope 相关性估算和置信度。app 删除重复的相关性实现，
  并在多机位向导边界显式转换 lag 符号约定，保持既有时间线放置语义。
- 波形提取结果携带实际采样率，修复调用方固定假定 48 kHz 的隐患；音频流参数
  使用 audio ordinal，而非容器 stream id。
- 单一原生元数据与 ABI POD 的转换统一实现 `NativeMirror`；可变长载荷使用独立、
  信息完整的 metadata 类型，不以丢弃数据的 `to_native` 凑接口，并有统一 roundtrip 测试。

## 9. P4D 验证

- SDK 真实媒体测试覆盖双声道波形、一次性结果转移、缺失文件失败、缩略图尺寸和
  F32 像素、引用计数克隆，以及 clip sync DTO。
- 纯算法测试覆盖普通偏移、有效窗口 mask、变速估算、多机位正负 lag 和短输入。
- app 的 waveform、源时间/波形同步、多机位偏移和真实 PNG 异步管线定向测试通过；
  缩略图测试不再依赖 worker/render-manager 环境。
- oak-audio / engine / SDK 全量回归通过，其中 engine 169 个、SDK 104 个测试通过。
- 内部引用基线由 870 收紧至 864。

## 10. P4E 范围

- `SetMulticamEnabled` 按源 sequence 分组执行启停；`SwitchMulticam` 复用
  oak-timeline 的切换/split 命令，并把 AFV 音频重连放入同一 undo 组。
- `CreateMulticamSequence` 创建并挂载角度源序列，按偏移放置音视频角度，
  连接 multicam source array；空主机时间线自动放置视频和链接 AFV 音频，且不切换
  用户当前 sequence。
- SDK 提供 multicam 状态、连接 sequence、角度轨道和 playhead clip 查询。
  角度 lag 与底层相关性的符号约定在 app 向导边界显式转换。
- app 的启停、切换、向导、drop、面板状态和角度轨道均改走 SDK；删除旧
  `oakui/multicam.rs` 重复实现。同步放置与 renderops 代理状态也改用 SDK，
  不再保留 P4 范围内的媒体/同步直调入口。

## 11. P4E 验证与 P4 出口

- 独立 SDK 双角度测试覆盖启停、普通/split 切换、AFV 音频跟随和 undo/redo。
- 向导向真实音视频工程验证偏移、空时间线主机、AFV、当前 sequence 保持及保存重开；
  参数数量不匹配在修改工程前被拒绝。
- app 的向导、drop、启停、AFV、源时间/波形同步和公开参数校验定向测试通过。
- `engine_multicam_angle_frames_render` 仍受既有 render-manager 启动环境限制，未计为
  通过；角度轨道和源 sequence 改由无渲染 SDK 断言覆盖。
- engine / SDK 全量回归通过，其中 engine 169 个、SDK 107 个测试通过。
- 内部引用基线由 864 收紧至 780。P4A–P4E 出口均满足，独立消费者可以从素材
  导入走到代理、波形、同步和多机位。
