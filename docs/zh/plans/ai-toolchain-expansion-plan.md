# AI 工具链扩充方案（T9.1 能力面调研）

- 作者：翰林院编修 `hanlin-audit`（task-34）；2026-11-11
- 性质：**只调研、不改代码**。本文是「把 oak 各种能力包装成 AI 可执行代码」的摸底与分批计划。
- 触发：陛下实测在面板里问「改序列帧率」，AI 答「工具集不支持修改已有序列的帧率」。
- 方法：enumeration + 行号锚定（`grep` 实测），不采信转述；不确定处标**未验证**。
- 现状基线：HEAD `8dcd5256f`（ai-feature），工作树干净。

---

## 0. 结论先行（给陛下的直接回答）

**引擎本身能改已有序列的帧率，缺的不是引擎能力，而是「wire + 工具」两层。**

证据链（三处实码）：

| 层 | 位置 | 内容 |
|---|---|---|
| 引擎编辑函数 | `crates/oak-app/src/oakui/graphops.rs:429` | `pub fn set_sequence_parameters(project, seq, name, width: i32, height: i32, rate: Rational, interlaced: bool) -> Result<(), String>` —— **`rate` 就是帧率，可改已有序列** |
| 格式结构 | `crates/oak-app/src/oakui/engine.rs:65-72` | `VideoFormat { width: u32, height: u32, rate: FrameRate }` |
| UI 已在用 | `crates/oak-app/src/oakui/real.rs:6551` | `RealEngine::update_sequence_parameters(id, name, format, interlaced)` → 调 `graphops::set_sequence_parameters` |

而 AI 面三处都没有它：
- `ai/schema/tools/` 里 **22 个 schema 无一个 `sequence.*`**；
- `crates/oak-extensions/src/host_services.rs` 的 `HostServices` **22 个 wire 方法无序列设置**；
- `ai/oak-ai-tools/src/backend.rs` 的 `EngineBackend` 22 个方法同上。

→ 判断：**先包工具，不必先补引擎**。唯一需要在引擎侧确认的是「改设置要不要进 undo」（见 §5 P0 风险列）。

**其余判断（要点）**：能力面 145 个方法里，真正值得给 AI 的约 **28 个**（P0 8 / P1 12 / P2 8）；其余属 UI 瞬态、纯查询内部态、或已有工具覆盖。扩充的首批应当是**序列/工程设置 + 剪辑精细操作 + 导出渲染**，理由见 §5.0。

---

## 1. 现状

### 1.1 现有 22 工具（冻结件，T0 定）

位置：schema `ai/schema/tools/*.json`（22 个文件）；参数解析 `ai/oak-ai-tools/src/tools/{common,edit,export,media,node,project,render_tools,timeline}.rs`；路由 `ai/oak-ai-tools/src/dispatch.rs`。

| # | 工具 | # | 工具 |
|---|---|---|---|
| 1 | `project.open` | 12 | `timeline.place_clip` |
| 2 | `project.save` | 13 | `timeline.split_clip` |
| 3 | `project.get_overview` | 14 | `timeline.trim_clip` |
| 4 | `media.probe` | 15 | `timeline.move_clip` |
| 5 | `media.import_footage` | 16 | `timeline.ripple_delete` |
| 6 | `media.list_footage` | 17 | `timeline.add_transition` |
| 7 | `node.add_effect` | 18 | `timeline.add_marker` |
| 8 | `node.set_param` | 19 | `render.get_frame` |
| 9 | `node.list_types` | 20 | `render.get_thumbnails` |
| 10 | `timeline.add_track` | 21 | `render.get_audio_levels` |
| 11 | — | 22 | `edit.undo` |

（`edit.undo` 之外无 `edit.redo`；`export.start` 在列。）

### 1.2 六层链路（加一个工具要动的地方）

```
① ai/schema/tools/<tool>.json          ← JSON Schema（契约、校验）
② ai/oak-ai-tools/src/tools/<domain>.rs ← 类型化参数解析（Value → typed）
③ ai/oak-ai-tools/src/dispatch.rs       ← 名字 → 方法路由
④ ai/oak-ai-tools/src/backend.rs        ← trait EngineBackend（22 方法）
⑤ crates/oak-app/src/ai/backend.rs      ← HostAdapter<H: HostServices>（JSON ↔ HostServices）
⑥ crates/oak-extensions/src/host_services.rs ← trait HostServices（22 wire 方法）
⑦ crates/oak-app/src/opp/host.rs        ← OppHost 实现（→ graphops/renderops/effectchain/oak_undo）
```

层号对齐代码注释：`⑤` 的 `HostAdapter` 是 T8.1 首辅裁的「方案 B，只依赖 trait 不依赖 OppHost」（见 `crates/oak-app/src/ai/backend.rs:1-30` 文档注释）。**加 1 个工具 = 改 5~6 处**（①③④⑤⑥⑦；②视参数复杂度可选）。这是 §5 架构讨论的起点。

### 1.3 规模基线（实测）

| 面 | 规模 |
|---|---|
| `AppEngine` trait 方法 | **145**（`engine.rs:299-1501`） |
| `graphops` 编辑 API | **117 个 pub fn**（7237 行） |
| `renderops` | 19 个 pub fn（3866 行） |
| `effectchain` | 16 个 pub fn（1697 行） |
| `multicam` | 13 个 pub fn（875 行） |
| `ofx` | 11 个 pub fn |
| `HostServices`（wire） | 22 个方法（452 行） |
| `EngineBackend`（工具面） | 22 个方法（91 行） |
| `OppHost`（引擎落地） | 1140 行 |

---

## 2. 能力面：`AppEngine` 145 方法分域

图例：**副作用** = 是否改工程/文件；**undo** = 是否经 `oak_undo` 组入栈；**UI 瞬态** = 是否属交互中间态（应排除给 AI）。

### 2.1 分域统计

| 域 | 方法数（约） | 代表方法（行号） | 副作用 | undo | UI 瞬态 |
|---|---|---|---|---|---|
| A 时钟/播放/监看 | 12 | `request_frame:252`、`play/pause/step:255-261`、`clock_frame:332`、`cpu_frame:336`、`scope_data:342` | 否（瞬态渲染） | — | **部分**（帧请求是 UI 拉取） |
| B 序列/格式 | 8 | `sequence_parameters:1142`、`create_sequence_with_params:1149`、`update_sequence_parameters:1213`、`open_sequence_id:1136` | **是** | **否**（`push(`=0，见 §5 风险） | 否 |
| C 轨道 | 4 | `add_track:346`、`remove_track:351`、`set_track_height:368`、`delete_empty_tracks:561` | **是** | 是（`graphops::add_track:1392` 走 `push(`，`track_locked` 亦入栈） | 否 |
| D 剪辑（结构） | 12 | `split_clip:488`、`split_at_playhead:492`、`delete_clip:661`、`delete_in_to_out:704`、`trim_selection_to_playhead:684`、`roll_selection_to_playhead:694`、`nest_clips:715` | **是** | 是（`split_clip:2488`、`delete_clip:3121` 走 `push(`/`undogeneral`） | 否 |
| E 剪辑（批量/属性） | 10 | `set_clips_speed:706`、`duplicate_clips:673`、`nudge_clips:680`、`rename_clips:675`、`toggle_clips_enabled:677`、`clipboard_*:664-671`、`select_item:371`、`set_selected_clips:380` | **是**（选择/剪贴板为会话态） | 部分（`set_clips_speed:3357`/`duplicate_clips:3422` 有 undogeneral 命令，`trim_clip:2590` **未检出** `push(`） | **部分**（`select_item`/`set_selected_clips` 是 UI 选择态） |
| F 特效/节点 | 13 | `add_effect:414`、`add_effect_to_clip:426`、`effect_params:440`、`set_effect_param:446`、`node_library:505`、`add_node_at:547`、`add_adjustment_layer:357`、`apply_effect_event:374`、`apply_node_graph_event:480`、`protected_graph_nodes:396`、`ofx_interact_target:475`、`effect_push_button:459`、`selected_graph_node:387` | **是**（event 类除外） | 是（effectchain `grouped` 漏斗 + GroupGuard） | **是**（`apply_*_event`、`effect_push_button`、`ofx_interact_target`、`selected_graph_node`） |
| G 素材/工程浏览器 | 11 | `import_footage:850`、`drop_footage:873`、`replace_footage:590`、`rename_entry:602`、`delete_entry:609`、`create_folder:1162`、`create_text_footage:1171`、`drop_generator_clip:1181`、`drop_transition_at:1200`、`entry_path:576`、`project_entry_name:582` | **是** | 是（`create_text_footage_node:385` 走 `push_command`） | 部分（`entry_path` 是浏览器展示） |
| H 标记/工作区 | 6 | `add_marker_at_playhead:626`、`remove_marker_at_playhead:631`、`commit_workarea:643`、`clear_workarea:655`、`workarea:621`、`set_workarea_preview:637` | **是** | `marker_add:3158` 走 `push(`；`set_workarea_preview` 是**预览态** | **`set_workarea_preview` 是** |
| I 历史 | 7 | `can_undo:718`、`can_redo:721`、`undo:724`、`redo:727`、`history_entries:732`、`history_index:739`、`jump_history:746` | 是（改文档态） | 自指 | 否 |
| J 工程文件/库 | 14 | `new_project:751`、`open_project_path:756`、`export_project_path:763`、`close_project:767`、`save_project_as:825`、`library_*:787-839`（9 个）、`storage_*:776-781` | **是** | **否**（文件级操作不入 undo） | 否 |
| K 导出/渲染 | 4 | `start_export:904`、`start_export_with:909`、`start_export_of:932`、`save_frame:1038` | **是**（写文件） | 否 | 否 |
| L 代理媒体 | 14 | `use_proxy_media:951`、`proxy_rows:1227`、`proxy_generate:1251`、`proxy_task_progress:1258`、`proxy_delete:1263`、`proxy_set_enabled:1269`、`proxy_*_custom_params:1281-1297`、`proxy_effective_params:1304`、`clip_footage_entries:1311`、`proxy_reveal:1275`、`proxy_state/row:1233/1241` | **是**（除查询） | 否（代理属旁路产物） | `proxy_reveal` 是 UI |
| M 多机位 | 12 | `multicam_state:1378`、`multicam_angle_frame:1388`、`multicam_eligible:1396`、`multicam_enable_selected:1412`、`multicam_switch_to:1425`、`multicam_wizard_footage:1443`、`multicam_wizard_sync_offsets:1453`、`multicam_create_sequence:1467`、`multicam_enabled_on_selection:1403` | **是**（除 query/eligible） | 是（`multicam_disable`/`SWITCH_LABEL` 走 `push_command`，`real.rs:7239/7324`） | `multicam_angle_frame` 是预览取帧 |
| N 同步/链接 | 4 | `sync_eligibility:1323`、`sync_clips_by_source_time:1330`、`sync_clips_by_waveform:1336`、`toggle_clip_links:1350` | **是** | 是（链接改动经 graphops 命令） | 否 |
| O 设置/偏好 | 11 | `use_proxy_media:951`、`playback_divider:973`、`stop_on_last:999`、`waveform_mode:1019`、`project_ocio_config:1067`、`project_cache_location:1083`、`project_color_settings:1099` 及各 setter（960/981/1007/1026/1075/1089/1110） | **是**（工程/应用设置） | 否 | 否 |
| P **UI 瞬态（应排除）** | 6 | `begin_param_drag:710`、`end_param_drag:712`、`set_eyedropper_armed:1480`、`eyedropper_armed:1486`、`eyedropper_picked:1492`、`take_eyedropper_result:1501` | 会话态 | 否 | **是** |
| Q 引擎元信息 | 3 | `backend_name:1431`、`waveform_cache:845`、`create:312` | 否 | — | `backend_name` 对 AI 无意义 |

> 计数口径：`sed -n '299,1505p' engine.rs | grep -cE '^\s+fn [a-z_]+'` = **145**；其中带 `&mut self` 的 **67**（有副作用候选），只读 `&self` 的 **51**，其余为关联函数（`create`）。分域数为**归类近似**（部分方法跨域，如 `use_proxy_media` 兼 L/O），逐条不重复计数合计 145。
>
> **域数 = 17**（A–Q，含 P「UI 瞬态」这一排除域）。

### 2.2 其他可自动化面（AppEngine 之外）

| 面 | 位置 | 规模 | 对 AI 价值 |
|---|---|---|---|
| `graphops` 编辑原语 | `oakui/graphops.rs` | 117 pub fn / 7237 行 | **最高**：真正的编辑语义（trim/slide/slip/roll/nest/transition/marker/workarea），多数已由 AppEngine 包一层 |
| `renderops` 渲染/导出 | `oakui/renderops.rs` | 19 pub fn / 3866 行 | 高：`render_sequence_frame`、`render_audio_range`、`spawn_export:1368`、`encoding_params_with_settings:1239` |
| 导出编码参数 | `engine.rs::ExportSettings` + `renderops:1239` | — | 高：容器/编码/码率/工作区范围，AI 说「导出 1080p H.264」要靠它 |
| `effectchain` 特效链 | `oakui/effectchain.rs` | 16 pub fn / 1697 行 | 中高：`grouped` 漏斗（已加 `GroupGuard`）、参数读写 |
| `multicam` | `oakui/multicam.rs` | 13 pub fn / 875 行 | 中：建多机位序列、切机位 |
| 代理媒体 | `engine.rs:951-1304` + `real.rs::proxy_*` | 14 方法 | 中：AI 说「先出代理再剪」 |
| OFX 插件 | `oakui/ofx.rs` | 11 pub fn（多为 UI 交互/绘制） | 低：`active_interact`/`draw_interact_composite` 属交互面 |
| 波形/示波器 | `waveform.rs`、`scopes.rs` | — | 低：UI 呈现 |
| 无头 CI 面 | `ai/ci/**` | — | 已具（T6） |

---

## 3. 缺口对照：22 工具 ↔ 能力面

判定口径：**已覆盖** = 面板真实路径（`PanelBackend::Real`）能跑出结果；**降级** = 能跑但语义缩水；**未覆盖** = `UNSUPPORTED` 或无对应工具。

| 现有工具 | 引擎落点 | 状态 | 备注 |
|---|---|---|---|
| `project.open` | `graphops::load_ove` | 降级 | schema 有 `fps`/`name`，wire 签名不带 → 被丢弃（`ai/backend.rs:26-30` 自陈） |
| `project.save` | `graphops::save_ove` | 已覆盖 | |
| `project.get_overview` | `project_name` + `sequence_video_params` + `sequence_length:517` | 已覆盖 | 真引擎 fps 实测 **30000/1001**（T8.5） |
| `media.probe` | 无独立 probe 入口 | **未覆盖** | 回 `UNSUPPORTED`；需 `oak_codec` 直连 |
| `media.import_footage` | `graphops::import_footage:879` | 已覆盖 | |
| `media.list_footage` | `footage_ids` + `footage_duration_seconds:814` | 降级 | `path` 恒 null（`clip_media_filename` 未接） |
| `timeline.add_track` | `graphops::add_track:1392` | 已覆盖 | `sequence`/`index` 参数被丢 |
| `timeline.place_clip` | `place_footage_clip` | 已覆盖 | 时长靠 probe 素材，probe 不了则 `INVALID_PARAMS` 硬失败（T7.2 D3） |
| `timeline.split_clip` | `split_clip:2488` | 已覆盖 | |
| `timeline.trim_clip` | `trim_clip:2590` / `ripple_trim_clip:2639` | 已覆盖 | |
| `timeline.move_clip` | `move_clip:2901` / `move_clip_to_track:2973` | 已覆盖 | |
| `timeline.ripple_delete` | `ripple_delete_clip` | 已覆盖 | |
| `timeline.add_transition` | `add_transition_at_seam:1848` / `_at_edge:1945` | **未覆盖** | 缝隙推断（`GAP_TRANSITION_SEAM`）未定义 |
| `timeline.add_marker` | `marker_add:3158` | **未覆盖** | 持久化未裁（sidecar vs 图节点；上游 example-mcp 选 sidecar） |
| `node.add_effect` | `effectchain::insert`（需 gpui `Context`） | **未覆盖** | 宿主在 IO 线程，无 `cx`（notes §3） |
| `node.set_param` | `effectchain::set_param`（同上） | **未覆盖** | 同上 |
| `node.list_types` | `oak_node::factory` | 已覆盖 | `category` 是 Debug 形态 |
| `render.get_frame` | `renderops::render_sequence_frame` | 降级 | 仅 sequence 分支；footage/shm 未接 |
| `render.get_thumbnails` | 同上（循环，上限 64） | **未覆盖** | schema `range` vs host `clips` 语义错层（`ai/backend.rs:430`） |
| `render.get_audio_levels` | `renderops::render_audio_range` | **未覆盖** | schema `{sequence,range,resolution}` vs host `{clip,buckets}`（`ai/backend.rs:438`） |
| `export.start` | `renderops::spawn_export:1368` | **未覆盖** | `ExportSession` 持非 `Send` 接收端（notes §3） |
| `edit.undo` | `oak_undo::global::undo` | 已覆盖 | 无 `edit.redo` |

**统计：已覆盖 11 / 降级 4 / 未覆盖 7**（合计 22）。

---

## 4. 关键缺口 top10（按「陛下问一句话却答不出」的频次排）

| # | 缺口 | 陛下会怎么说 | 引擎现成吗 | 缺哪层 |
|---|---|---|---|---|
| 1 | **改序列设置（帧率/分辨率/名/隔行）** | 「把序列改成 29.97」 | **有**：`graphops:429` | wire + 工具 |
| 2 | **导出参数（容器/编码/码率/范围）** | 「导出 1080p H.264 到桌面」 | 有：`encoding_params_with_settings:1239` + `spawn_export:1368` | wire（`Send` 问题）+ 工具 |
| 3 | 剪辑精细操作（slide/slip/roll/nest/接缝转场） | 「这个剪辑往前挪 5 帧，接缝加叠化」 | 有：`slide_clip:2755`、`slip_clip`、`roll_edit:2704`、`nest_clips:3760`、`add_transition_at_seam:1848` | 工具（+ 转场缝隙推断规则） |
| 4 | 剪辑属性（速度/重命名/启用/链接） | 「这段 2 倍速」 | 有：`set_clips_speed:3357`、`rename_blocks:3386`、`set_blocks_enabled:3329`、`set_clips_linked` | wire + 工具 |
| 5 | 轨道属性（静音/锁定/高度） | 「把这条音轨静音」 | 有：`set_track_muted:1513`、`set_track_locked:1519`、`set_track_height:1525` | wire + 工具 |
| 6 | 标记/工作区持久化 | 「在这打个标记」 | 有 `marker_add:3158`，**持久化未裁** | 决策 + 工具 |
| 7 | 节点参数（真实生效） | 「调这个调色的饱和度」 | 有，但要 gpui `Context` | **架构**（§5） |
| 8 | 工程库（列出/另存/复制工程） | 「把上个工程复制一份」 | 有：`library_*:787-839` | wire + 工具 |
| 9 | 代理生成 | 「先出代理再剪」 | 有：`proxy_generate:1251` | wire + 工具（长耗时） |
| 10 | 多机位 | 「建个三机位序列」 | 有：`multicam_create_sequence:1467`、`multicam_switch_to:1425` | wire + 工具 |

---

## 5. 分批方案（核心）

### 5.0 先做哪批、为什么

**判断依据三条**：
1. **陛下卡点优先**：「改帧率」这类**设置类**操作一句话就能说清、无歧义、失败可回滚，是 AI 最该先会的；
2. **改动成本 vs 语义价值**：设置类只读+写少量字段（`set_sequence_parameters` 一次调用），而多机位/代理要异步任务与状态机，性价比低；
3. **风险可控**：设置类**不入 undo**（实测 `push(` = 0），不会污染撤销栈；而剪辑结构操作要么入栈要么得补栈，风险更高但在 P0 也必须给（否则「剪」这件事做不全）。

→ **P0 = 序列/工程设置（第一优先）+ 剪辑精细操作 + 导出渲染**；P1 = 轨道/剪辑属性 + 标记/工作区 + 工程库；P2 = 代理/多机位/同步/OFX。

### 5.1 P0（8 个工具，先做，建议两周内）

| 工具名（建议） | 引擎调用（行号） | 参数 | 风险 | 估算 |
|---|---|---|---|---|
| `sequence.get_settings` | `AppEngine::sequence_parameters:1142` / `graphops::sequence_video_params` | `{sequence}` | 无（只读） | 1 文件 +2 测 |
| **`sequence.set_format`** | **`graphops::set_sequence_parameters:429`**（`rate: Rational`） | `{sequence, name?, width?, height?, rate?, interlaced?}` | **不入 undo**（`push(`=0）：AI 改完撤销不了 → 要么明示「设置类不可撤销」，要么补一条 undo 命令（**需首辅裁**） | 1 文件 +4 测（含 30000/1001 往返） |
| `sequence.create` | `create_sequence_with_params:219` / `AppEngine:1149` | `{name, width, height, rate, interlaced}` | 无（新序列，可删） | 1 +2 |
| `sequence.list` | `AppEngine::sequence_entries:918` | `{}` | 无 | 1 +1 |
| `timeline.slide_clip` | `slide_clip:2755` | `{clip, offset_frames}` | 入栈；边界溢出需钳制 | 1 +3 |
| `timeline.slip_clip` | `slip_clip`（graphops 内） | `{clip, offset_frames}` | 入栈；素材边界 | 1 +2 |
| `timeline.set_transition` | `add_transition_at_seam:1848` + `set_transition_length:2125` + `transition_of_clip` | `{clip_a, clip_b?, kind, length_frames}` | **缝隙推断规则缺失**（`GAP_TRANSITION_SEAM`）——先做「显式 seam」版，不做自动推断 | 1 +3 |
| `export.start`（重做） | `encoding_params_with_settings:1239` + `spawn_export:1368` | `{sequence, path, format, codec?, bitrate?, range?}` | `ExportSession` 非 `Send`：需事件泵（notes §3）；长耗时需进度查询 | 2 文件 +4 测（+ 进度工具） |

> **P0 里最硬的一件是 `sequence.set_format` 的 undo 决策**：设置类通常不进撤销栈（改帧率不会「撤销」回去），但若陛下期望「AI 改了能撤回」，就得补 `SetSequenceParametersCommand`。**建议：先按「不入栈 + 回复里明示」实现，同时列为待裁项。**

### 5.2 P1（12 个工具）

| 工具名 | 引擎调用 | 参数 | 风险 |
|---|---|---|---|
| `track.set_muted` | `set_track_muted:1513` | `{track, muted}` | 入栈 |
| `track.set_locked` | `set_track_locked:1519` | `{track, locked}` | 入栈 |
| `track.set_height` | `set_track_height:1525` | `{track, height}` | UI 属性，入栈与否待确认 |
| `clip.set_speed` | `set_clips_speed:3357` | `{clips[], speed}` | 入栈（undogeneral 存在） |
| `clip.rename` | `rename_blocks:3386` | `{clips[], name}` | 入栈 |
| `clip.set_enabled` | `set_blocks_enabled:3329` | `{clips[], enabled}` | 入栈 |
| `clip.set_linked` | `set_clips_linked` | `{clips[], linked}` | 入栈 |
| `clip.duplicate` | `duplicate_clips:3422` | `{clips[], count?}` | 入栈 |
| `timeline.nest_clips` | `nest_clips:3760` | `{clips[], name?}` | 入栈；嵌套序列创建副作用 |
| `timeline.set_marker`（重做） | `marker_add:3158` + `markers_of` | `{sequence, time, name?, color?}` | **持久化决策未定**（sidecar vs 图节点） |
| `timeline.set_workarea` | `workarea_set:1008` / `workarea_set_undoable` | `{sequence, start, end}` | 有 undoable 变体，优先用之 |
| `project.library_list` / `project.save_as` | `library_*:787-839`、`save_project_as:825` | `{}` / `{name}` | 文件级，不入栈 |

### 5.3 P2（8 个工具）

| 工具名 | 引擎调用 | 风险 |
|---|---|---|
| `proxy.generate` | `proxy_generate:1251` + `proxy_task_progress:1258` | 异步长耗时，需任务态与轮询 |
| `proxy.list` / `proxy.set_enabled` | `proxy_rows:1227`、`proxy_set_enabled:1269` | 低 |
| `multicam.create` | `multicam_create_sequence:1467` + wizard 两步 | 多步向导，中间态多 |
| `multicam.switch` | `multicam_switch_to:1425` | 入栈（`SWITCH_LABEL`） |
| `sync.by_waveform` | `sync_clips_by_waveform:1336` | 计算重、结果不稳 |
| `node.set_param`（重做，真生效） | `effectchain::set_param`（需 `cx`） | **架构问题**：需 §5 方案解 |
| `render.get_thumbnails`（重做） | `render_sequence_frame` 循环 | 语义错层，需改 schema 或改 host 签名 |
| `media.probe`（补齐） | `oak_codec` 直连（无现成入口） | 需新引擎入口 |

**合计候选新工具：8 + 12 + 8 = 28 个**（其中 3 个是「重做/补齐既有」）。

---

## 6. 架构评估（**报首辅裁**）

### 6.1 三个方案

| 方案 | 做法 | 改动面 | 优点 | 缺点 |
|---|---|---|---|---|
| **A 直接加进冻结件** | 在 `ai/schema/tools/` + `EngineBackend`（22→50 方法）直接扩展 | 破 T0 冻结；`HostAdapter`、`MockBackend` 都要补 28 个实现 | 单一工具面，无分叉 | **破坏冻结件**；`EngineBackend` 是 trait，加方法 = 所有 impl 必须实现（或加默认 `UNSUPPORTED`）；Mock 与测试全面返工 |
| **B 新开模块 v2** | `ai/oak-ai-tools/src/tools/v2/` + 新 trait `EngineExt`（继承 `EngineBackend`），22 个旧工具原样冻结 | 冻结件不动；新模块独立演进 | 冻结件语义保住（T0 是可审计基线）；可先只实现 `EngineExt` 里真正要用的方法 | 两层 trait；dispatch 需按名分派到两个面；文档两处 |
| **C 直连引擎（绕开 OPP wire）** | 面板侧新 backend 直接持 `ProjectRef` 调 `graphops`，不经 `HostServices` | 省掉 ⑥⑦ 两层 | 加一个工具改动面从 6 层降到 4 层；`node.set_param` 的 `cx` 问题可另解 | **放弃 OPP 复用**（T7.1/T7.2 两笔封存资产）；事务/撤销语义要重做一遍；若将来插件进程回来要再包一层 |

### 6.2 我的建议：**B 为主，C 局部**

- **B 的理由**：①冻结件是 T0 定、且被验收过的可审计基线，破它等于让「22 工具冻结」这句话失效，后续无法判断回归；②`EngineBackend` 的 trait 加方法会强迫 `MockBackend`（1435 行）和所有测试同步扩张，成本远高于新开 trait；③v2 模块可以**只长不减**，未来 OPP 与内置 agent 两条路都还能吃。
- **C 局部**的理由：`node.set_param` / `node.add_effect` 卡在「需要 gpui `Context`」——这是 wire 层（IO 线程）**结构性做不到**的，不是包装问题。要么（C1）在面板侧（UI 线程）加一个「UI 线程执行队列」，要么（C2）给 effectchain 增加无 `cx` 的纯数据入口。**建议 C2**（加纯数据入口），因为它同时惠及插件与内置 agent，且不引入线程队列复杂度。
- **不建议 A**：唯一好处是「一个工具面」，但代价是冻结基线消失 + Mock 返工，与我方「冻结件只增不改」的既有纪律（`opp-implementation-notes.md:107`）冲突。

### 6.3 待首辅裁的三件事

1. **扩充走 B（新模块 v2 + `EngineExt`）还是 A（直接改冻结件）**——我建议 B。
2. **`sequence.set_format` 要不要进 undo**（`graphops:429` 实测不入栈）——我建议「先不入栈 + 回复明示」，若陛下要求可撤销则补命令。
3. **`node.*` 的 `cx` 墙**：走 C2（effectchain 加无 `cx` 纯数据入口）还是 UI 线程执行队列——我建议 C2。

---

## 7. 排除项与理由

| 排除项 | 位置 | 理由 |
|---|---|---|
| `begin_param_drag` / `end_param_drag` | `engine.rs:710-712` | 拖拽交互中间态：AI 无鼠标，语义只有「连续拖动开始/结束」，应改为一次性 `node.set_param` |
| `set_eyedropper_armed` / `eyedropper_armed` / `eyedropper_picked` / `take_eyedropper_result` | `1480-1501` | 吸管是**人机取色**交互，AI 应直接传 RGB 值 |
| `set_workarea_preview` | `637` | 预览态（松手才 `commit_workarea`），AI 应直接 `commit` |
| `apply_effect_event` / `apply_timeline_event` / `apply_node_graph_event` | `374/485/480` | UI 事件泵入口，不是编辑语义；AI 应调具体编辑函数 |
| `select_item` / `set_selected_clips` | `371/380` | 选择是 UI 会话态；AI 用显式 id 列表参数即可 |
| `backend_name` | `1431` | 引擎元信息，对创作无意义 |
| `waveform_cache` / `scope_data` / `cpu_frame` / `clock_frame` | `845/342/336/332` | 渲染/波形中间产物，AI 需要的是「图」而非内部缓存 |
| `request_frame` / `play` / `pause` / `step` / `tick` | `252-265` | 播放控制属**用户观看**行为，AI 不应驱动播放（要预览走 `render.get_frame`） |
| `proxy_reveal` | `1275` | 打开文件管理器，纯 UI |
| `ofx_interact_target` / `effect_push_button` | `475/459` | OFX 插件交互面板控件，属人机界面 |
| `protected_graph_nodes` / `selected_graph_node` | `396/387` | 节点图 UI 选择态 |
| `drop_footage` / `drop_transition_at` | `873/1200` | 拖放（drag&drop）语义，AI 应用显式 `place_*` |
| `storage_*` | `776-781` | 存储后端状态，属部署关注点而非创作 |

**排除共性**：UI 瞬态（中间态无对错）、人机交互（鼠标/吸管/拖放）、内部呈现（缓存/波形/元信息）——这三类即使包成工具，AI 也只会用错，反而扩大误用面。

---

## 8. 未验证与后续

1. **分域计数是归类近似**：145 是 `fn` 行数实测，17 域是人工归类；跨域方法（`use_proxy_media`、`set_clips_speed`）只记一次。
2. **undo 覆盖只做了抽样**：`push(` 仅 14 处、`real.rs` 仅 3 处 `push_command`，`trim_clip:2590` 未检出入栈点——**每个 P0/P1 工具落地前必须逐个实测「改完能否撤销」**，不能凭本文推断。
3. **`set_sequence_parameters` 不入 undo 已实测**（该函数体内 `push(`/=0），但**是否由调用方包组未验证**（`real.rs:6551` 的 UI 路径可能在外层开组）。
4. **导出 `Send` 问题的解法未验证**（`ExportSession` 持非 `Send` 接收端，需事件泵；P0 落地时验证）。
5. **`media.probe` 的 `oak_codec` 直连路径未验证**（T7.2 记「无独立 probe 入口」）。
6. **多机位/代理的异步任务态**（进度、取消、失败恢复）未细查，P2 前需单独调研。
7. 本文**未改任何代码**（只新建本文件）；`crates/` 与 `ai/` 零改动。

---

## 附：统计数字速查

| 项 | 数字 |
|---|---|
| `AppEngine` 方法 | **145**（其中 `&mut self` 67、只读 51、关联 1，其余为默认体） |
| 分域 | **17**（A–Q，Q 之后的 P 为 UI 瞬态排除域） |
| `graphops` pub fn | **117** |
| 其他面 | renderops 19 / effectchain 16 / multicam 13 / ofx 11 |
| 现有工具 | **22**（已覆盖 11 / 降级 4 / 未覆盖 7） |
| 候选新工具 | **28**（P0 **8** / P1 **12** / P2 **8**） |
| 排除项 | **13 类**（见 §7） |
