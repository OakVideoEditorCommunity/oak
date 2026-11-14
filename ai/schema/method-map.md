# T0 工具面 ↔ OPP/1 §8 方法对照表

> 本文是 `ai/schema/` 冻结产物的一部分：工具 JSON Schema（`tools/*.json`，22 个，
> JSON Schema draft-07）、合法/非法样本（`samples/`）、防漂移测试（`test_schema.py`）。
> 权威来源：`docs/zh/plans/external-plugin-protocol.md` §8（宿主 API 方法，
> v1 全量）与 §5/§6（公共数据类型、编辑事务）；设计依据
> `docs/zh/plans/ai-agent-design.md` §3（工具面是契约、事务编排不暴露给 LLM）。
> 方法名与字段名一律英文，正文说明用中文。
>
> 测试口径：`test_schema.py` 从协议 Markdown 的 §8 区段按表格首列正则抽取方法名，
> 与表② 双向对账；表① 与 `tools/` 文件集合双向对账。协议改了测试就红。

## 表① 工具 ↔ OPP §8 方法（逐工具）

| # | 工具（schema 文件名即工具名） | 既有工具底稿（28 个） | OPP 方法 | 关系 | 说明 / 改名理由 |
|---|---|---|---|---|---|
| 1 | `project.open` | `open_project` | `project.open`（§8.2） | 一对一（改名） | 点号风格对齐 OPP；`path` 缺省时新建空工程是工具面扩展（OPP v1 恒带 path），T1 适配层实现 |
| 2 | `project.save` | `save_project` | `project.save`（§8.2） | 一对一（改名） | `path` 可选，与 OPP 一致（缺省存回原位） |
| 3 | `project.get_overview` | `get_project_overview` | `project.get_info`（§8.2）+ `timeline.get_structure`（§8.4） | 一对多（合并） | 一次读全工程上下文，省一次往返；LLM 编辑前的入口 |
| 4 | `media.probe` | `probe_media` | `media.probe`（§8.3） | 一对一（改名） | 参数逐字一致 |
| 5 | `media.import_footage` | `import_footage` | `media.import_footage`（§8.3） | 一对一（改名 + 参数对齐） | 单 `path` 放宽为 OPP 的 `paths` 数组，失败项按路径单列 |
| 6 | `media.list_footage` | `list_footage` | `media.list_footage`（§8.3） | 一对一（改名） | 参数逐字一致 |
| 7 | `timeline.add_track` | `add_track` | `timeline.add_track`（§8.4） | 一对一（改名 + 字段改名） | 底稿 `kind` → OPP `type` |
| 8 | `timeline.place_clip` | `place_clip` | `timeline.place_clip`（§8.4） | 一对一（改名 + 裁剪参数） | 裁掉底稿 `out`/`track_kind`/`track_index`/`with_audio`/`name`（OPP 无此参数：铺满素材剩余长度，收边走 `timeline.trim_clip`）；轨定位改为 OPP 的 `track` id |
| 9 | `timeline.split_clip` | `split_clip` | `timeline.split_clip`（§8.4） | 一对一（改名 + 字段改名） | 底稿 `clip_id` → OPP `clip` |
| 10 | `timeline.trim_clip` | `trim_clip` | `timeline.trim_clip`（§8.4） | 一对一（改名 + 字段改名） | 底稿 `clip_id` → `clip`；`side` 枚举逐字一致 |
| 11 | `timeline.move_clip` | `move_clip` | `timeline.move_clip`（§8.4） | 一对一（改名 + 字段对齐） | 底稿 `new_in` → `in`、`to_index` → `track`；按 OPP `in` 必填 |
| 12 | `timeline.ripple_delete` | `ripple_delete` | `timeline.ripple_delete`（§8.4） | 一对一（改名 + 字段改名） | 底稿 `clip_id` → `clip` |
| 13 | `timeline.add_transition` | `add_transition` | `timeline.add_transition`（§8.4） | 一对一（改名 + 参数对齐） | 底稿 `left_clip_id`/`right_clip_id`/`kind` → OPP 单 `clip` + `side` + `type`；`type` 枚举取引擎 `SHADER_IDS`：`crossdissolve`/`fade`/`wipe`/`slide`（`crates/oak-node/src/nodes/transitions.rs`），底稿的 `dissolve` 映射为 `crossdissolve` |
| 14 | `timeline.add_marker` | `add_marker` | `timeline.add_marker`（§8.4） | 一对一（改名 + 字段对齐） | 底稿命名色板（green/red/…）→ OPP §5 `Color` 十六进制串；`time` 按 OPP 必填（底稿缺省取播放头） |
| 15 | `node.add_effect` | `add_effect` | `node.add_effect`（§8.5） | 一对一（改名 + 语义升级） | 底稿只有 `fade_in`/`fade_out`/`volume` 三种 otio 后端效果；冻结面改为引擎节点 type id（如 `org.olivevideoeditor.Olive.blur`）或 OFX identifier，OFX 动态类型导致 id 为开放集（不设 enum） |
| 16 | `node.set_param` | `set_param` | `node.set_param`（§8.5） | 一对一（改名 + 参数拍平） | 底稿 `kind` + `params{}` → OPP `node`/`key`/`value`；取值范围按 §5 规则回填进 schema（见表③） |
| 17 | `node.list_types` | `list_effects` | `node.list_types`（§8.5）+ `node.get_params`（§8.5） | 一对多（合并） | 新增可选 `node` 参数走 `get_params` 半边：给 id 即回读该实例参数（value/default/min/max/choices） |
| 18 | `render.get_frame` | `get_frame` | `render.get_frame`（§8.6） | 一对一（改名 + 参数对齐） | 底稿 `width` → OPP `max_size{width,height}`；`format` 枚举逐字一致；`sequence`/`footage` 二选一（互斥约束见 `tools/render.get_frame.json` 的 `allOf.oneOf`：协议未规定 both-present，T0 自行拒绝并记为开放项；`sequence` 参数约定见表③ sequence（序列参数）行） |
| 19 | `render.get_thumbnails` | `scan_timeline` + `make_contact_sheet` | `render.get_thumbnails`（§8.6） | 多对一（合并） | 两个底稿工具按目标二选一归并：时间线传 `sequence`、素材传 `footage`；`count ≤ 64` 逐字沿用 |
| 20 | `render.get_audio_levels` | `get_audio_levels` | `render.get_audio_levels`（§8.6） | 一对一（改名 + 参数对齐） | 底稿 `path`/`start`/`end`/静音阈值参数 → OPP `sequence` + `TimeRange` + `resolution`（静音检测属 T2 取证面，不进冻结工具） |
| 21 | `export.start` | `export_render` | `export.start`（§8.8） | 一对一（改名 + 参数对齐） | 底稿 `path` → `output_path`、`track_index`/`start`/`end` 裁剪（OPP v1 不开放）、底稿 `draft`/`normal`/`best` 预设枚举撤销（OPP `preset` = Oak 预设名，开放集） |
| 22 | `edit.undo` | `undo_last_action` | `edit.undo`（§6，非 §8） | 一对一（改名 + 裁剪参数） | 底稿 `count` 删除（协议无此参数）；描述里写死 §6 规则 5：撤销整栈、含用户本人操作，仅用户明确要求时调用 |

**底稿裁剪说明（28 → 22 的账）**：22 个冻结工具来自 23 个底稿工具
（`scan_timeline` 与 `make_contact_sheet` 合并计 1 个），另有 5 个底稿工具不进冻结面：

- `get_capabilities` — 本地后端能力矩阵探测，非 OPP 方法；T1 起以握手 `limits`
  与能力位（§7）为准，工具面不重复暴露。
- `list_operations` — 会话操作日志属 T1 执行层审计（`.ops` 落盘），非协议方法，
  不给 LLM 直读。
- `scan_scene_changes` — 取证预筛属 T2 `ai/prescan`（独立进程、零 LLM 调用），
  不属于 OPP 工具面。
- `extract_frames` — 与 `render.get_thumbnails`（素材目标）语义重叠，归并后裁剪。
- `seek` — 播放头控制随 `playback.*` 一并 defer（见表②）；`render.get_frame`
  显式带 `time`，不依赖播放头状态。

## 表② OPP §8 方法 → 工具或 deferred（全量 38 条）

| OPP §8 方法 | 处置 | 理由 |
|---|---|---|
| `session.hello` | deferred | 握手/会话生命周期归 T1 适配层，不是模型可调用的剪辑工具 |
| `session.ping` | deferred | 心跳保活，适配层内部行为 |
| `session.shutdown` | deferred | 会话关闭，适配层内部行为 |
| `session.log` | deferred | 日志走适配层 stderr/Oak 日志面板；工具失败用结构化错误回喂 LLM |
| `events.subscribe` | deferred | 事件订阅是 app→agent 方向，属 T4 hooks 设计，非请求-响应工具面 |
| `project.get_info` | `project.get_overview` | 与 `timeline.get_structure` 合并为一次上下文读取（一对多） |
| `project.open` | `project.open` | 一对一 |
| `project.save` | `project.save` | 一对一 |
| `media.probe` | `media.probe` | 一对一 |
| `media.list_footage` | `media.list_footage` | 一对一 |
| `media.import_footage` | `media.import_footage` | 一对一 |
| `timeline.get_structure` | `project.get_overview` | 与 `project.get_info` 合并（见上） |
| `timeline.add_track` | `timeline.add_track` | 一对一 |
| `timeline.place_clip` | `timeline.place_clip` | 一对一 |
| `timeline.split_clip` | `timeline.split_clip` | 一对一 |
| `timeline.trim_clip` | `timeline.trim_clip` | 一对一 |
| `timeline.move_clip` | `timeline.move_clip` | 一对一 |
| `timeline.delete_clip` | deferred | 既有 28 工具只有波纹删除；非波纹删（留洞删除）在 T0 无对应素材与用例，T1 实测有需求再补 |
| `timeline.ripple_delete` | `timeline.ripple_delete` | 一对一 |
| `timeline.add_transition` | `timeline.add_transition` | 一对一 |
| `timeline.add_marker` | `timeline.add_marker` | 一对一 |
| `timeline.set_workarea` | deferred | 工作区是宿主 UI 选择概念；AI 粗剪的区间诉求由 `render.get_thumbnails.range` 等参数表达，既有工具面亦无对应 |
| `node.list_types` | `node.list_types` | 与 `node.get_params` 合并为一个工具（一对多） |
| `node.add_effect` | `node.add_effect` | 一对一 |
| `node.get_params` | `node.list_types` | 合并为 `node.list_types` 的可选 `node` 参数 |
| `node.set_param` | `node.set_param` | 一对一 |
| `node.set_keyframe` | deferred | 既有工具面无关键帧能力；T0 只冻结静态参数设置，关键帧编辑待 T1 落地后按需立项 |
| `node.remove` | deferred | 既有工具面无节点移除；T0 的回退手段是 `edit.undo`（整事务撤销），不单独立工具 |
| `render.get_frame` | `render.get_frame` | 一对一 |
| `render.get_thumbnails` | `render.get_thumbnails` | `scan_timeline` + `make_contact_sheet` 合并（多对一） |
| `render.get_audio_levels` | `render.get_audio_levels` | 一对一 |
| `playback.play` | deferred | GUI 回放属宿主运行时，无头态不可用（既有能力矩阵已列 known gap）；AI 视觉闭环用 `render.get_frame` 按时刻采样，不需要连续播放 |
| `playback.pause` | deferred | 同 `playback.play` |
| `playback.seek` | deferred | 同 `playback.play`；`render.get_frame` 显式带 `time`，不依赖播放头 |
| `playback.get_state` | deferred | 同 `playback.play`；T0 的状态读取入口是 `project.get_overview` |
| `export.start` | `export.start` | 一对一 |
| `export.cancel` | deferred | 既有工具面无导出取消；T0 导出只发起，进度/完成/取消经 T4 hooks 的 `export.*` 事件处理 |
| `shm.release` | deferred | shm 槽位租约与流控是 T1 执行层/SDK 内部职责；铁律：LLM 永不接触 shm 与事务令牌 |

> §8.10 `ui.*` 未逐条列入上表：协议 §8.10 正文只指向 §11，段内没有可抽取的方法名。
> 处置同样是 **deferred**——AI 剪辑工具面不含任何 UI 方法（声明式面板属设计文档 A4，
> 等 OPP P4）。
> §8.8 正文提到的 `export.progress` 是 §9 事件而非 §8 方法，同理不入表。

## 表③ 公共类型约定（T0 现状 → T1 衔接）

| 约定项 | T0 冻结现状 | T1 衔接 |
|---|---|---|
| Rational（有理秒） | schema `oneOf` 第一分支：`{"num": integer, "den": integer ≥ 1}`，`den > 0`；OPP §5 称其为全协议唯一时间表示 | 适配层原样透传给 OPP，不做十进制化 |
| Frame index（帧号） | schema `oneOf` 第二分支：非负 `integer`。帧号是工具面的糖：适配层按目标（sequence/footage）帧率换算成 Rational 并量化到帧栅格 | 换算规则在 T1 执行层单点实现；禁止在 schema 里出现裸浮点秒（`oneOf` 硬拒 `1.5` 这类小数）。已知边界：JSON Schema 无法区分 `1` 与 `1.0`，整数值浮点按帧号 1 接受，小数部分非零一律拒绝 |
| txn（事务令牌） | **22/22 个工具 schema 均不携带 `txn` 字段**（测试断言）。OPP §8 中标"事务：是"的方法在协议层要求 `txn` | T1 执行层按"一次工具调用 = 一个动作 = 一个 undo group"自动 `edit.begin(label)` → 串行发方法 → 成功 `edit.commit`、任一步失败 `edit.abort` 并结构化回喂；`txn` 由适配层持有，LLM 永不接触（ai-agent-design §3） |
| sequence（序列参数） | 时间线/渲染/导出工具的 `sequence` 除 `render.get_audio_levels`、`export.start`（OPP 必填）外一律可选，缺省 = 活动序列 | T1 适配层解析活动序列并显式补上；OPP 侧恒显式传参 |
| EntityId | `clip`/`track`/`footage`/`sequence`/`node` 均为不透明字符串（OPP §5），schema 只约束 `minLength ≥ 1` | 禁止解析 id 格式；工程重载后 id 全部作废，靠重新拉取恢复 |
| Color | `#RRGGBB` 或 `#RRGGBBAA`（OPP §5），`timeline.add_marker.color` 用 `pattern` 约束 | 底稿的 6 色命名板不再进 schema；T1 适配层做 hex → 引擎标记色转换 |
| TimeRange | `{"in": Rational\|帧号, "out": Rational\|帧号}` 左闭右开（OPP §5），用于 `render.get_thumbnails`、`render.get_audio_levels` | `out > in` 与越界由 T1 运行时校验并返回结构化错误（schema 无法跨字段比较） |
| Rate（帧率） | `project.open.fps`：正整数或 `{num, den}`（如 30000/1001），裸浮点 `29.97` 被拒 | 新建工程时适配层按 fps 建序列；打开已有工程忽略该参数 |
| Effect param range（效果参数范围） | `node.set_param` 用 `if/then` 按 `key` 回填（来源 `crates/oak-node/src/nodes/*.rs`）：**有界** `temperature_in` 1000–40000、`tint_in` −1–1、`saturation_in` 0–4、`mix_in` 0–1、`left_in`/`top_in`/`right_in`/`bottom_in` 0–1；**只有下界**（同名 key 跨节点类型上界不一致，取并集后上界开放）`opacity_in` ≥0、`radius_in` ≥0、`volume_in` ≥0、`threshold_in` ≥0、`strength_in` ≥0、`feather_in` ≥0、`lower_tolerance_in` ≥0 | **显式 open（不回填、不臆造）**：`gamma_in`（gamma.rs 0–4 / colorcorrect.rs 0.2–5 / grade.rs 无范围 → 并集无界）、`amount_in`（sharpen.rs 明示无界）、`upper_tolerance_in`（chromakey.rs 的 min 属性在 C++ 侧 disabled）、`method_in`（blur/math/despill 各自 choice 集不同）、`color_in`（noise.rs 是 bool、despill.rs 是 color）、其余全部 key。开放 key 一律先调 `node.list_types(node)`（OPP `node.get_params`）读回引擎实测 min/max/choices 再设值 |
