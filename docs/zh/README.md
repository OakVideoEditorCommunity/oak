## Oak Video Editor

Oak 视频编辑器是 Olive 的重命名分支，目标是打造更完善、更友好的剪辑体验。

## 界面

![screenshot](../imgs/screenshot.png)


# 功能

- 类似Premiere的快捷键体验
- OpenFX插件支持，这是Davinci Resolve使用的插件体系
- 端到端色彩管理支持
- 10bit上屏
- OCIO LUT支持
- 代理剪辑工作流程
- 借助OpenTimelineIO和Final Cut Pro XML与Davinci Resolve、Premiere Pro和Final Cut Pro交换时间线

## 下载

[v0.5.0](https://github.com/OakVideoEditorCommunity/oak/releases/tag/v0.5.0-alpha2)

## 构建说明
请查看[构建指南](build.md)

## 路线图

| 版本 | 主题 | 核心交付物 |
|:--|:--|:--|
| **0.5（当前）** | **Rust重写** | Rust重写后的Oak |
| **0.6** | **调色、音频与性能** | AI视频剪辑、示波器（波形/矢量/直方图）、三向色轮面板、多机位剪辑支持、BWF 时间码同步、音频表（LUFS/VU）、批量渲染队列 |
| **0.7** | **动画、跟踪与协作** | 贝塞尔关键帧曲线编辑器、基础点跟踪、画面稳定器 |
| **0.8** | **稳定性里程碑** | 项目文件格式冻结（向后兼容承诺） | 1.0 前的"封版"测试期 |
| **1.0** | **生产就绪** | 文档完整、安装包、已知问题清单、社区支持渠道 | 宣告"可用于严肃项目" |
