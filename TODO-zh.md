# TODO

## 目标
- 实现 2–3 秒预渲染的 LRU 缓存和代理剪辑功能，并以“小步快跑”的方式在现有架构中逐步落地，确保每一步都可编译。

## 现有架构中的落点
- 播放/渲染调度：`app/render/renderprocessor.cpp`, `app/render/plugin/pluginrenderer.cpp`, `app/node/traverser.cpp`
- 插件节点输入/默认值：`app/node/plugins/Plugin.cpp`
- Clip 图像/纹理获取：`app/pluginSupport/OliveClip.cpp`, `app/pluginSupport/OliveClip.h`
- 节点与值系统：`app/node/node.h`, `app/node/node.cpp`, `app/node/value.h`
- 工程序列化：`app/node/project/serializer/*`

## LRU 缓存计划（代码改动 + 集成点）

### 步骤 1（可编译）：新增缓存类型但不接入逻辑
- 新增缓存模块，例如 `app/render/cache/framecache.h/.cpp`。
- 定义：
  - `FrameCacheKey`（图哈希/版本、时间、参数、代理模式、渲染缩放）。
  - `FrameCacheEntry`（AVFrame 或 Texture + 元信息 + 字节数 + 最近访问时间）。
  - `FrameCache` API：`get(key)`、`put(key, entry)`、`invalidateByVersion(version)`。
- 先只编译通过，不改变行为。

### 步骤 2（可编译）：图版本号/失效机制
- 在 `Node` 或渲染入口维护图版本号。
- 当参数变化、连线变化时递增。
- 渲染侧可读取版本号用于缓存失效。

### 步骤 3（小行为）：仅缓存当前帧
- 在 `renderprocessor.cpp` 播放路径上：
  - 先查缓存，命中则直接显示。
  - 未命中则正常渲染，并写入缓存。
- 缓存预算先设很小，风险低。

### 步骤 4（小行为）：预渲染窗口
- 增加队列，渲染 [now, now+N]，N=2–3 秒。
- 并发限制（例如 2–3 个任务），避免抢 UI。
- 优先级：当前帧 > 近未来。
- Seek 时取消/丢弃过期任务。

### 步骤 5（行为）：LRU 淘汰
- 按内存预算/帧数上限淘汰最久未使用。

### 步骤 6（行为）：CPU/GPU 策略
- 默认缓存 CPU 帧，播放时再上传 GPU。
- GPU 缓存可作为后续优化开关。

### 步骤 7（可观测性）
- 统计命中率、平均渲染耗时、掉帧。
- Debug 构建下输出日志。

## 代理剪辑计划（代码改动 + 集成点）

### 步骤 1（可编译）：数据模型与序列化
- 在 clip 元数据里增加：
  - `proxy_path`、`proxy_width`、`proxy_height`、`proxy_codec`、`proxy_fps`。
- 在 `app/node/project/serializer/*` 写入/读取。

### 步骤 2（小行为）：代理选择策略
- 增加全局/每 clip 的代理模式：
  - `Auto`、`ForceProxy`、`ForceOriginal`。
- 在媒体解析层根据模式决定用原片还是代理。

### 步骤 3（行为）：代理生成
- 新增后台转码任务（复用现有渲染/导出流程）。
- 生成完成后更新元数据。

### 步骤 4（行为）：UI 接入
- 增加“生成代理”“重链接代理”入口。
- 在剪辑或预览上显示代理标识。

### 步骤 5（验证）
- 对比代理与原片的时间精度、音画同步。
- 导出默认使用原片。

## 小步快跑执行顺序（每步可编译）
1) 新增缓存模块/类型（不接入）。
2) 增加图版本号与失效接口。
3) 播放路径只缓存当前帧。
4) 预渲染 2–3 秒窗口 + 并发限制。
5) LRU 淘汰策略。
6) 图版本变更触发失效。
7) 统计与日志。
8) 代理元数据字段 + 序列化。
9) 代理选择策略（Auto/Force）。
10) 代理生成任务 + UI 入口。

## 待确认问题
- 缓存预算默认值（按硬件分级）。
- 代理文件默认存储路径。
- 是否做 GPU 纹理缓存。
