# P4：素材、代理、波形、同步与多机位实施记录

> 对应 [app-pure-sdk-and-engine-facade.md](app-pure-sdk-and-engine-facade.md) 的 P4。  
> 状态：P4A/P4B 已完成，P4C 代理生成与异步任务待启动；后续阶段不提前宣称完成。

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
