# DaVinci Resolve 21 新功能对 oak 的可用性评估报告

| 项 | 内容 |
|:--|:--|
| 日期 | 2026-11-03 |
| 评估基线 | HEAD bf64bee6c（分支 ai-feature） |
| 性质 | 桌面评估，零实测 |
| 结论状态 | 已过内部评事/御史核验（内容级；文件级复核 2026-11-03） |
| 评估对象 | DaVinci Resolve 21 What's New 页面（中文页与英文页；下载入口指向 21.1） |
| 产出 | 本报告为本轮新产出文件，未改动任何代码 |

## 1. 摘要（结论先行）

- Resolve 21 内置 AI（AI IntelliSearch、AI Speech Generator、AI CineFocus、AI Face Age Transformer、AI Face Reshaper、AI Blemish Removal、AI Slate ID、AI UltraSharpen、AI Motion Deblur）属 Blackmagic Neural Engine 体系。基于公开资料，未发现其以 OFX 或公开 API 暴露的迁移路径；在公开资料范围内，oak 没有可直接调用的接口。对 oak 的价值限于借鉴交互与工程范式，或以开源模型自研（需逐项验证，不等于复刻）。
- oak 可直接或低成本获得的能力分三类：标准 OFX 第三方插件（受宿主缺口限制）；DaVinci Resolve Renderer OFX 配合 `.drx` 工程（据社区资料通常需 Studio 授权，待实测与官方确认）；OTIO/FCPXML 文件交换（oak 已有）。
- 最值得借鉴的五项：Lottie/HTML 图形管线、关键帧与曲线编辑器升级、把 AI 结果烘焙为缓存节点（Magic Mask 范式）、MultiMaster 多版本 trim、光流变速与跟踪。
- 本报告为零实测：DRX 渲染一致性、Lottie 渲染、语义检索指标、OFX 参数关键帧与 mask 输入四项均待实机验证。
- 优先级结论：先补 OFX 宿主的关键帧与 mask 输入，再做 Lottie/HTML 与 AI 取证检索；DRX 只作互操作探针，不作核心依赖。

## 2. 评估范围与方法

评估对象为 Resolve 21 官方 What's New 页面所列功能面，而非完整发布说明；版本号精确边界与逐项授权归属以官方 release notes 为准（本报告只核到页面当前内容）。

方法分四步：其一，以官方中文页、英文页做功能盘点，记录功能名、能力描述与依赖技术点；其二，对 oak 仓库做只读代码审计，按模块核对现状，行号以本次 HEAD 为准；其三，按「可迁移性」把每个功能归入 A/B/C/D 四类，分类标准见第 4 节；其四，由大理寺评事两轮退回整改、都察院御史终验，核验记录见附录 B。

证据分级如下表。凡涉及第三方资料的判断，均在文中标注置信度，不将低置信材料当作事实前提。

| 级别 | 来源类型 | 使用方式 |
|:--|:--|:--|
| 1 | 官方产品页（What's New、compare） | 作为功能存在性与授权归属的主证据 |
| 2 | 代码只读实测（本仓库） | 作为 oak 能力底账的证据 |
| 3 | 第三方资料（手册镜像、OpenFX 官网、论坛帖） | 仅作线索，标注低置信，不单独支撑结论 |

「零实测」的边界需明确：本轮未在真机运行 DRX 工程、未渲染 Lottie 文件、未跑语义检索基准、未验证带关键帧或 mask 的 OFX 插件。凡涉及运行结果的表述，一律为验收设计或推断，不写作实测结论。

## 3. Resolve 21 新功能清单

### 3.1 AI（Neural Engine 体系）

| 官方名称（中/英） | 能力 | 依赖技术点 | Studio 归属 |
|:--|:--|:--|:--|
| AI 智能搜索 / AI IntelliSearch | 按语义检索媒体与时间线内容 | 语音转写、视觉特征索引 | 未逐条确认 |
| AI 语音生成 / AI Speech Generator | 文本生成配音 | 语音合成模型 | 未逐条确认 |
| AI 对焦 / AI CineFocus | 后期调整画面焦点位置与强度 | 深度估计 | 未逐条确认 |
| AI 面部年龄变换 / AI Face Age Transformer | 调整人物面部年龄表现 | 人脸生成模型 | 未逐条确认 |
| AI 面部重塑 / AI Face Reshaper | 调整面部比例与轮廓 | 人脸关键点与生成 | 未逐条确认 |
| AI 祛斑 / AI Blemish Removal | 去除皮肤瑕疵 | 皮肤分割与修补 | 未逐条确认 |
| AI 场记板识别 / AI Slate ID | 识别场记板并回填元数据 | 文字识别、镜头匹配 | 未逐条确认 |
| AI 超锐化 / AI UltraSharpen | 提升细节锐度 | 超分模型 | 未逐条确认 |
| AI 运动去模糊 / AI Motion Deblur | 抑制运动模糊 | 去模糊模型 | 未逐条确认 |

Magic Mask 属既有 Neural Engine 能力，21 的变化是「渲染到位置」，见 3.3。上表「未逐条确认」指官方 compare 页未逐条标注每个 AI 功能的授权归属，逐项归属待 release notes 或实机确认。

### 3.2 Cut & Edit

| 官方名称 | 能力 | 依赖技术点 |
|:--|:--|:--|
| 关键帧与曲线编辑器更新 | ease in/out、loop、ping-pong、relative、多片段同调、归一化曲线、速度变化四点贝塞尔 | 关键帧模型与曲线求值 |
| Fusion 特效动画化 | 在剪辑页对 Fusion 特效做动画 | 参数桥接与关键帧 |
| HTML/Lottie 支持 | 导入 Lottie/HTML 作为图形层 | 矢量/网页渲染器 |
| Text+/MultiText 改进 | 多文本图层与排版增强 | 文本渲染 |
| 智能媒体夹视图 | 按内容特征组织媒体夹 | 元数据与索引 |

### 3.3 Color

| 官方名称 | 能力 | 依赖技术点 |
|:--|:--|:--|
| MultiMaster Trim Manager | 一条时间线按多套色彩设置输出多版本 | 多版本 trim 与色彩管理 |
| Magic Mask 渲染到位置 | 把遮罩结果烘焙到指定节点位置 | 分割、跟踪、缓存 |
| 节点编辑器图层列表 | 以图层列表管理节点 | 节点图 UI |
| 群组调色版本 | 群组级调色版本管理 | 调色版本模型 |

### 3.4 Fusion

| 官方名称 | 能力 | 依赖技术点 |
|:--|:--|:--|
| Krokodove 动态图形 | 内置动态图形工具集 | 节点与矢量图形 |
| 宏命令编辑器检查器 | 宏参数编辑界面 | 宏系统 |
| 音频驱动 Fusion 动画 | 用音频包络驱动参数 | 音频分析到参数映射 |
| USD 25.11 / Hydra 2.0 | 三维场景与渲染代理互操作 | USD 运行时 |

### 3.5 Fairlight

| 官方名称 | 能力 | 依赖技术点 |
|:--|:--|:--|
| 音频轨道文件夹 | 轨道分组管理 | 轨道模型 |
| 六频段片段 EQ | 片段级六段均衡 | 片段级音频处理 |
| EQ 与电平匹配器 | 均衡与响度对齐 | 响度测量与滤波 |
| Chain FX | 单片段最多六个效果串联 | 效果链与延迟补偿 |

### 3.6 Photo 与交付/沉浸式

| 官方名称 | 能力 | 依赖技术点 |
|:--|:--|:--|
| 新照片页面 | 相册、光箱、RAW、ResolveFX/OFX、AI 工具集、云端协作 | 图像管线与协作 |
| MainConcept H.265/MV-HEVC | 高效编码与多视图编码 | 编码器授权 |
| VR180/360 | 沉浸式素材剪辑与输出 | 球面投影 |
| 社媒直传 | 直接发布到社交平台 | 平台接口 |
| 即刻画中画 | 快速生成画中画 | 变换与合成 |
| Final Draft IntelliScript | 剧本导入与对齐 | 剧本解析 |
| ATEM Mini 导入 | 导入 ATEM 录制素材 | 设备素材解析 |
| 光流变速 | 基于光流的时间重映射 | 光流估计 |
| 先进降噪 | 时域/空域降噪 | 降噪模型 |
| 跟踪跟随物体 | 跟踪结果驱动物体变换 | 点/平面跟踪 |
| Apple Immersive 注视点渲染 | 沉浸式视频的注视点渲染 | 平台专属管线 |
| Neural Engine 硬件支持 | 支持 M 系列与 Snapdragon X Elite | 平台推理后端 |

版本边界说明：官方 compare 页仅概述 Neural Engine、Magic Mask、立体工具、更多 ResolveFX/Fairlight FX 属 Studio，未逐条标注每个 AI 功能。故本报告对「Studio 独占」只作类别级判断，逐项归属待确认。

## 4. 可迁移性四分类

| 类别 | 定义 | 本报告归入的功能 |
|:--|:--|:--|
| A 可借鉴 | 借鉴概念、交互或架构，不主张代码复用 | Lottie/HTML 图文管线；关键帧 ease/loop/ping-pong/relative 与多片段同调；Magic Mask 烘焙缓存范式；MultiMaster 多版本 trim；光流变速；3D 跟踪与先进降噪；音频驱动参数；片段级 EQ 与 Chain FX |
| B 经 OFX 获得 | 通过标准 OFX 宿主能力加载第三方插件；含 DRX Renderer | 第三方 OFX filter/generator/transition；DaVinci Resolve Renderer OFX + `.drx`（仅此归 B） |
| C 文件交换 | 通过时间线交换格式互通 | OTIO/FCPXML |
| D 不可复用 | 基于公开资料未见迁移路径 | Neural Engine 专有模型；Blackmagic Cloud；Fairlight 硬件生态；Apple Immersive 链路；相机联机采集白名单 |

B 类的适用边界需要收紧：oak 目前只能跑静态参数型 OFX 插件。依赖参数动画、mask 输入、CUDA/Metal 渲染套件，或依赖 tracker/paint 上下文的插件，会不可用或降级（证据见第 5 节）。DRX Renderer 归 B 但有额外前提：据社区资料通常需 Resolve Studio 授权，且时域特效与 Magic Mask 不随调色迁移（授权归属与时域行为待官方确认）。

C 类的代价是语义损失。交换会丢节点图、关键帧、色彩管理设置与效果参数；oak 已支持 OTIO JSON 与 FCPXML 双向，但无 AAF/EDL。因此文件交换只能承担「把剪辑结构搬过去」，不能承担「把调色搬过去」。

关键澄清两点。其一，ResolveFX 是 Blackmagic 内置滤镜，不等于 OFX；据 Resolve 21 手册（第三方镜像，低置信）记载，ResolveFX 不在 Fusion Studio 等其他宿主可用；Magic Mask、IntelliSearch 等也未以 OFX 暴露。其二，oak-ofx-plugin 遵循 OFX 标准，能加载同一批第三方 OFX，但加载不了 Resolve 自家 AI。这两点使「通过 OFX 白拿 Resolve AI」在公开资料范围内缺乏依据。

## 5. oak 能力底账

| 能力 | 现状 | 证据与缺口 |
|:--|:--|:--|
| OpenFX 宿主 | 部分实现 | 四上下文 Filter/Generator/Transition/General（`crates/oak-ofx-plugin/src/host.rs:1039-1060`）；八张 suite（`crates/oak-ofx-plugin/src/lib.rs:21-23`）；GL+CPU；独立宿主进程（`crates/oak-worker/src/ofx_host.rs:18-28`） |
| OFX 参数动画 | 未实现 | 普通参数无关键帧，`paramGetNumKeys` 恒 0（`crates/oak-ofx-plugin/src/suites/param.rs:586`）；parametric 参数有 Hermite 曲线编辑（`crates/oak-ofx-plugin/src/param_curve.rs:17-32`、`crates/oak-app/src/panels/ofx_params.rs:1045-1070`），与普通参数关键帧是两回事 |
| OFX mask 与高级上下文 | 缺口 | tracker/paint 上下文被过滤；optional mask 未真正喂纹理；无 CUDA/Metal render suite |
| 色彩管理 | 部分实现 | OCIO 2.5.2、ACEScg/F32、LUT、10-bit 上屏、节点式调色；GPU 色彩 blit 仍为 pass-through |
| 关键帧/曲线 | 内核已实现 | Hold/Linear/Bezier 与贝塞尔控制点（`crates/oak-node/src/keyframe.rs:30-56`）；曲线编辑器控件存在（`crates/oak-app/src/widgets/curve_editor/mod.rs:1-10`）；UX 尚未对齐 ease/loop/ping-pong/多片段同调 |
| 音频 | 部分实现 | 波形、VU/EBU R128 LUFS、重采样、混音；无总线、效果链与 EQ |
| 跟踪/稳定 | 未实现 | 只读审计未见点跟踪、平面跟踪、稳定实现；抠像与遮罩已有 |
| 互操作 | 部分实现 | OTIO JSON 读写与 FCPXML 双向（`crates/oak-otio/src/fcpxml.rs:762/803/1314/1349`）；无 AAF/EDL |
| AI/Agent | 部分落地 | `agent/mcp-oak` 共 28 个工具（媒体 7、工程 6、时间线 11、渲染 4），取证层已实现；AI 剪辑 T0-T6 仍为规划（`docs/zh/plans/ai-editing-feature-plan.md`） |
| 扩展点 | 单一 | 除 OFX 外无 Lua/Python/JS 绑定（`crates/oak-extensions` 仅 hello world）；无 Lottie/HTML |

底账的两点读法。第一，oak 的 OFX 宿主是「能加载、能出图」的静态层，缺的是动画与遮罩两条输入通路；这决定了 B 类能力的天花板。第二，关键帧内核与 parametric 曲线是两套已存在但未打通的设施，P0/P2 的工作量主要在桥接与 UX，而非从零建模。

## 6. 建议路线

里程碑口径以仓库根 `README.md` 的 Roadmap 为准：0.5 为 Rust Rewrite（当前），0.6 为 Color, Audio & Performance，0.7 为 Animation, Tracking & Collaboration，0.8 为 Stability，1.0 为 Production Ready。`docs/zh/README.new.md` 中的旧版路线图（0.5 动画跟踪 / 0.6 稳定性）为历史稿，不作为本报告基线。

### 6.1 P0（先行验证，低风险）

| 事项 | 目标 | 验收口径 | 风险与依赖 |
|:--|:--|:--|:--|
| OFX 参数关键帧 | 打通 `paramGetNumKeys`/`GetKeyTime`/`GetKeyValue` 与求值 | 带关键帧参数的 OFX 插件在 oak 渲染出动画；静态参数回归不破 | 需改 suite 与节点值求值，注意 undo |
| OFX mask 输入 | 把 optional mask 作为纹理喂给插件 | 带 optional mask 的插件收到正确 mask 纹理，边缘/反转/羽化符合 OFX 语义 | 需纹理通路与 mask 生命周期管理 |
| DRX Renderer 实测 | 验证互操作可行性 | oak 加载该 OFX 并套用 `.drx`；与 Resolve 参考帧比较 5 帧（片段时长 10/30/50/70/90%），Rec.709 8-bit，逐帧 ΔE2000 均值 ≤2.0、最大 ≤3.0；不含时域/镜头依赖特效 | 阈值仅为建议值，实验前冻结；据社区资料通常需 Studio 授权（待官方确认）、Linux 可用性、EULA 确认 |
| Resolve 21 OTIO/FCPXML 回归 | 确认交换保真 | 切点、转场、轨道序、时码保真；输出丢失清单 | 依赖 Resolve 侧导出样本 |

### 6.2 P1（对齐 0.6）

| 事项 | 目标 | 验收口径 | 风险与依赖 |
|:--|:--|:--|:--|
| Lottie/HTML | 图层级图形导入与渲染 | 拖入 `.lottie`/`.json` 以带 alpha 片段渲染；覆盖缩放、循环、透明通道 | 渲染器选型与许可待定 |
| AI 取证与语义检索 | 对齐 `ai-editing-feature-plan` T0-T6，以 Whisper/CLIP 类开源模型为候选 | 预先冻结 20 条查询 × 50 段人工标注素材，Recall@5 ≥0.8、误报率 ≤20%（建议值，实验前冻结） | 基线实验前由评事确认；开源模型质量、速度、显存成本未测 |
| Magic Mask 式烘焙 | 分割/跟踪结果缓存为节点 | 结果可缓存、编辑后失效可重算、渲染确定 | 依赖跟踪能力；缓存失效策略需设计 |
| MultiMaster 多版本 trim | 一条时间线按 N 套色彩设置出 N 版 | 一条时间线出 N 版且互不污染 | 非 multicam，归 0.6 Color 主题 |

### 6.3 P2（对齐 0.7）

| 事项 | 目标 | 验收口径 | 风险与依赖 |
|:--|:--|:--|:--|
| 曲线编辑器 UX | ease/loop/ping-pong/relative、多片段同调 | 与内核关键帧模型一致，多选片段同调不串值 | 与 0.7 贝塞尔里程碑合并排期 |
| 点/平面跟踪与稳定 | 补齐 0.7，并打通 OFX tracker 上下文 | 跟踪点可解算、稳定可回读、tracker 上下文插件可加载 | 工作量大，需先做数据模型 |
| 音频总线/EQ/Chain FX | 参考 Fairlight 设计 | 总线可路由、EQ 与效果链可保存重开 | oak 现无总线/效果链，建议随 0.6 Audio 单独立项 |

超出当前 Roadmap 的功能（光流变速、3D 物体跟踪、先进降噪、人脸生成式编辑）建议另立议题，不硬塞进 0.6/0.7。理由是这四项都依赖 oak 尚不具备的基础设施（光流、跟踪解算、时域多帧管线、生成模型推理），塞入现有里程碑会稀释验收口径。

## 7. 决策建议

- 不追 Resolve AI 的「功能复刻」，优先把 oak 自己的 AI 剪辑闭环（取证预筛 → 工具调用 → 渲染回读 → 事务撤销）做扎实；Resolve 的 AI 交互只作参考。
- 先补 OFX 宿主的关键帧与 mask 输入。这是第三方生态与 AI 效果插件的前置条件，比自研模型更快见效。
- Lottie/HTML 是用户感知最强的空白项，建议在 0.6 立小项，先做单图层 alpha 渲染，再谈交互。
- DRX 只作互操作探针，不作核心依赖：据社区资料通常需 Studio 授权，叠加闭源插件与 Linux 未验证两项，不宜写进产品路径。
- 曲线编辑器与 0.7 的贝塞尔里程碑合并排期，复用现有内核，避免重复造轮子。
- MultiMaster 与 multicam 分开立项，二者数据模型与验收口径不同，混合会拖慢两边。

## 8. 风险与未决问题

| 风险/未决项 | 影响 | 处置建议 |
|:--|:--|:--|
| 零实测 | DRX、Lottie、语义检索、OFX 关键帧/mask 均未真机跑通 | 按 P0 顺序补实测，结论回填本报告 |
| 免费/Studio 边界不清 | 授权判断可能偏差 | 以官方 release notes 逐项核对 |
| DRX 在 Linux + oak OFX 宿主未验证 | 互操作结论可能不成立 | 先做最小样本，失败即降级为 D |
| 插件获取与 EULA | 法务风险 | 由法务确认后再纳入验收 |
| 第三方来源置信度低 | OpenFX 官网、论坛帖、手册镜像均非官方 release notes | 仅作线索，不入结论 |
| Lottie/HTML 渲染器选型与许可未定 | 可能返工 | 选型前先做许可清单 |
| 开源 AI 模型未做基准 | 指标可能达不到验收线 | 先跑 20×50 小样本再定基线 |
| 版本号与发布日期 | 21 精确版本以官方为准 | 本报告只核到页面当前内容（下载指向 21.1） |
| 代码行号漂移 | 引用失效 | 行号以 HEAD bf64bee6c 为准，引用前复核 |
| 里程碑编号口径 | 根 `README.md` 与 `docs/zh/README.new.md` 的路线图不一致 | 已裁定以根 `README.md` 为准；`README.new.md` 为历史稿，建议后续清理或标注归档 |

## 9. 附录

### A. 功能-能力映射矩阵

| Resolve 功能 | 可迁移类型 | oak 现状 | 建议优先级 |
|:--|:--|:--|:--|
| AI IntelliSearch | D（概念可 A） | 无检索；取证层已有 | P1（自研检索） |
| AI Speech Generator | D | 无 TTS | 暂不排期 |
| AI CineFocus | D | 无深度估计 | 暂不排期 |
| Face Age/Reshape/Blemish | D | 无生成式人像 | 暂不排期 |
| AI Slate ID | D（可借鉴元数据流） | 无场记板识别 | 暂不排期 |
| AI UltraSharpen | D | 无超分 | 暂不排期 |
| AI Motion Deblur | D | 无去模糊 | 暂不排期 |
| Magic Mask | A（烘焙范式） | 有遮罩/抠像，无跟踪与烘焙 | P1 |
| 关键帧/曲线升级 | A | 内核已实现，UX 未对齐 | P2 |
| Lottie/HTML | A（管线） | 无 | P1 |
| MultiMaster | A（多版本 trim） | 无调色版本管理 | P1 |
| Fairlight EQ/Chain FX | A（设计参考） | 无总线/效果链/EQ | P2 |
| 3D/物体跟踪 | A（概念） | 无点/平面跟踪 | 超出 Roadmap |
| DRX Renderer | B | OFX 宿主可加载，未实测 | P0（探针） |
| OTIO/FCPXML | C | 双向已支持 | P0（回归） |
| Blackmagic Cloud | D | 无云端协作 | 不排期 |
| Photo 页面 | D（相册/RAW 概念可参考） | 无照片工作区 | 不排期 |

### B. 内部核验记录

- 大理寺评事第一轮退回：分类存在重叠（ResolveFX 与 OFX 混用）、对 Resolve AI 可迁移性推断过度。整改：单列关键澄清，D 类改为「未见迁移路径」表述。
- 大理寺评事第二轮退回：优先级与现有里程碑错配、代码证据粒度不足。整改：P0/P1/P2 与第 6 节验收口径一一对应，代码引用补到函数与行号。
- 都察院御史终验：通过。
- 说明：内容已核验，文件为本轮新产出；核验不改变「零实测」性质。

### C. 来源与置信度

| 来源 | 类型 | 置信度 | 用途 |
|:--|:--|:--|:--|
| https://www.blackmagicdesign.com/cn/products/davinciresolve/whatsnew | 官方 | 高 | 功能清单主证据 |
| https://www.blackmagicdesign.com/products/davinciresolve/compare | 官方 | 高 | Studio 归属类别级判断 |
| https://docs.imaster.org/davinci-resolve-21-manual/part-8-fusion-fundamentals/ch.-83-using-open-fx,-resolve-fx,-and-fuse-plugins | 第三方手册镜像 | 低 | ResolveFX 与 OFX 区分的线索 |
| https://openeffects.org/ | 第三方 | 低 | OFX 标准背景 |
| https://forum.blackmagicdesign.com/viewtopic.php?f=21&t=199664 | 论坛用户帖 | 低 | 版本行为线索 |
| https://forum.blackmagicdesign.com/viewtopic.php?f=21&t=199352 | 论坛用户帖 | 低 | 版本行为线索 |

### D. 术语表

| 术语 | 含义 |
|:--|:--|
| OFX/OpenFX | 开放插件标准，定义宿主与图像处理插件的交互接口 |
| ResolveFX | Blackmagic 内置滤镜集合，非 OFX，不在其他宿主通用 |
| DRX | DaVinci Resolve eXchange，Resolve 的工程交换格式（`.drx`） |
| OTIO | OpenTimelineIO，开放时间线交换格式 |
| FCPXML | Final Cut Pro 的 XML 时间线交换格式 |
| Neural Engine | Blackmagic 的机器学习推理体系 |
| Magic Mask | 基于分割与跟踪的遮罩工具 |
| MultiMaster | 一条时间线按多套色彩设置输出多个版本的能力 |

本报告不构成对 Blackmagic Design 产品的逆向工程或再分发建议；仅评估公开标准与公开资料范围内的互操作路径。
