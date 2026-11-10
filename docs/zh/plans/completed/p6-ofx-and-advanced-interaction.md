# P6：OFX 与高级交互收尾实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P6。  
> 状态：已完成，下一阶段为 P7（app 内部 crate 依赖清零）。

## 1. 生命周期、进度与取消

- 插件扫描、节点注册、render executor、active viewer time 和 project extent 统一走
  `oak-engine-rust-sdk::ofx`；app 不再维护私有宿主注册或 provider。
- engine 统一管理 inline 取消标志并广播 worker 取消；新 progressStart 会复位 inline
  取消状态。进度 handler 同时桥接 inline 与 worker 事件，注销时两条路径一起移除。
- `shutdown` 先销毁全部 interact、注销进度 handler，再关闭插件宿主。app 的退出菜单
  和最后窗口关闭路径都执行该顺序。

## 2. Interact 与 overlay

- SDK 支持从既有 instance id 创建 interact，并提供 draw、pen、key、idle、focus 和
  redraw/swap 动作。显式 `destroy` 幂等，Arc 克隆不会延迟选择清除时的插件通知。
- app 只保留视口/键盘映射、GL overlay 合成和事件适配。无插件、无 interact、无 GL
  或插件失败均退回基础帧，不影响预览。
- program viewer 使用 SDK 的 redraw/swap 获取接口，不再读取插件内部原子字段。

## 3. 参数曲线与徽标

- 新增拥有型 `Curve`/`ControlPoint` DTO 和 engine canonical JSON 转换，保留显式
  Hermite slope；OFX 参数控件不再依赖 `oak-ofx-plugin` 类型。
- persistent-message 徽标通过 SDK 按既有 instance id 查询，不转移节点所有权。
- app 源码中的 `oak-ofx-plugin` 引用已清零，直接依赖与剩余兼容代码删除留给 P7。

## 4. 验证

- engine 169 个、SDK 115 个测试通过，覆盖 DTO slope roundtrip、实例参数、取消、
  重复初始化、interact 生命周期和 shutdown 后句柄降级。
- app 728 个通过、1 个忽略，覆盖参数控件、pen/key/idle、显式 destroy、GL 不可用
  降级和退出路径；GL overlay 专项仍按 `OAK_GPU_TESTS` 环境门控。
- `cargo test --workspace -- --test-threads=1` 全量通过。
- app/engine 边界基线从 749 降至 664；`git diff --check` 通过。
