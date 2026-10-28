# P1：EditorSession、项目库与只读快照实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P1。  
> 实施日期：2026-10-26。SDK 最终命名 Quercus 的批量改名在 P0–P8 全部结束后进行。

## 1. 同实例所有权与阶段修正

RealEngine 在 P2 前仍直接编辑 `Arc<Mutex<Project>>`。write-through 必须绑定同一
实例，因此 P1 没有采用“SDK 打开一份、app 再打开一份”或裸指针还原的捷径。

本阶段交付独立 `EditorSession`，供新消费者和 P2 的 RealEngine 采用。app 中不依赖
当前工程实例的库操作、全局配置和文本初始化先切到 SDK；实时工程绑定、项目级
OCIO/cache/color 切换和图只读清零与 P2 一起完成。

## 2. Engine：真实会话与所有权

`crates/oak-engine/src/editor.rs` 新增 opaque `EditorSessionHandle`，内部持有：

- 唯一的 `Arc<Mutex<Project>>`；
- 当前序列及其 marker list、workarea；
- write-through 绑定句柄。

文件打开、库内创建和库内打开都保留同一实例。`oak_editor_project` 只返回新的
安全 ProjectHandle 引用，不暴露 Arc、Graph 或内部指针。`oak_editor_unbind` 显式
关闭绑定；session Drop 同样解绑，重复 close 无害。

新增工程设置、uuid、cache policy、sequence interlaced、track height/muted/locked
和 block enabled 查询。`coretypes::OakTimeRange` 增加与 SDK 一致的默认值。

## 3. SDK：安全会话与拥有型快照

`crates/oak-engine-rust-sdk/src/editor.rs` 提供：

- `EditorSession::{new,open,open_project_from_library,create_in_library,save,bind_project_to_library,close}`；
- `project()`：同一实时工程的独立 RAII 引用；
- `create_sequence`、`current_sequence`、`set_current_sequence`；
- `snapshot()`：revision、工程树、素材、序列、轨道、片段、当前序列 marker 和
  workarea 的拥有型 DTO；
- `Project::{uuid,setting,set_setting,cache_location,set_cache_location}` 及补充的
  sequence/track/block 查询。

快照覆盖保存前/重开后的结构对比。工程根节点的序列化显示名不作为结构相等条件，
NodeId、树结构、时间线参数和片段范围才是。

## 4. 项目库完整生命周期

`ProjectLibrary` 支持：

- configured/default URI、列表及时间戳和图统计；
- create、delete、rename、duplicate、import、export；
- 从默认库打开 `EditorSession`；
- 全局 flush。

字符串输出分为两类：只读 getter 可以两段查询；create/duplicate/import 有副作用，
必须单次调用。回归测试固定 create 后 1 行、duplicate 后 2 行、import 后 3 行，
防止副作用 API 被调用两次。

## 5. App 已切换的路径

- `graphops` 的库列表、创建、删除、重命名、复制、导入和导出使用 SDK；
  `library_open` 与 bind/unbind 暂留 P2。
- 全局配置读取、快捷键配置目录、效果分组折叠和 P0 默认设置使用 SDK。
- SDK 提供 `Config::global()` 的 Rust 风格配置对象；调用方直接传完整键，
  例如 `Config::global().get_bool("UseProxyMedia", true)`，不暴露 C ABI 的
  `Option<group>` 形状。底层旧自由函数仅为兼容保留。
- proxy 默认参数改由 SDK codec 读取；proxy UI 与旧领域参数转换仍保留到 P4。
- 文本安装和系统字体枚举通过 SDK，继续与 worker 共用后端。

迁移期 app 同时链接旧 rlib 和 engine dylib，存在两份旧/新全局 store。公共配置
写入点在 P2 前同步两者，启动共同加载、退出仅由 SDK 保存，避免音频、硬件解码、
代理任务和 storage 读取到不同值。该兼容 shim 与剩余旧 store 一起退出，不是终态。

## 6. 验证

- `cargo test -p oak-engine -- --test-threads=1`：120 通过。
- `cargo test -p oak-engine-rust-sdk -- --test-threads=1`：84 通过；doc-tests 2 个既有忽略。
- `cargo test -p oak-app -- --test-threads=1`：735 通过、2 个集成测试通过、1 个既有忽略。
- `python3 tooling/check_app_engine_boundary.py`：基线由 1,217 收紧至 1,193，只减不增。

新增关键用例：

- `session_owns_current_sequence_and_auxiliary_queries`；
- `null_handles_fail_closed`；
- `project_settings_and_snapshot_field_queries`；
- `file_round_trip_preserves_the_owned_snapshot`：真实素材、嵌套 bin、流参数、
  enabled 的 undo/redo、revision、保存与重开；
- `project_settings_and_cache_round_trip`；
- `invalid_inputs_and_shared_close_semantics`：无效 URI/NodeId、序列切换、clone 共享 close、
  session 释放后 ProjectHandle 仍有效；
- `library_lifecycle_and_live_session`：无效库 URI、空参数/缺失行、create/duplicate/import
  各只增加一行、统计、复制与导入内容独立、自定义库拒绝、Drop/close 后删除、禁用配置；
- `idiomatic_config_uses_full_keys_and_native_rust_types`：完整键与 bool/i64/f64 原生类型；
- `text_backend_install_and_font_families`：安装、非空枚举及缓存一致性；
- 既有 `engine_library_lifecycle_with_sqlite` 与 proxy 对话框端到端测试。

## 7. P2 必须接续的事项

- RealEngine 采用 `EditorSession`，删除剩余实时 `oak-storage` 绑定。
- 项目级 OCIO/cache/color 切换与工程采用同步完成。
- 用会话快照替换实时只读图遍历，再迁移全部编辑和 undo。
- 删除双 store 同步 shim 及旧 rlib 链接产生的重复全局状态。
