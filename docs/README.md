# Nous 设计与开发文档

项目内 design 文档定义当前产品和工程合同；代码按当前设计实现。新的真实需求暴露结构问题时，修改负责专题与实现所有者，迁移调用方。旧代码形状和测试没有内部兼容特权。

## 阅读入口

[整体架构](ARCHITECTURE.md)说明所有权和依赖；[当前工作](CURRENT.md)记录能力和当前目标。[模型与媒体实施计划](plans/model-runtime.md)是当前施工入口。

专题按独立责任组织。配置、内容、模型、Prompt、媒体转换、媒体解释和命令呈现分别维护。文档较短也保留独立主题；同一规则只在负责专题定义，其余位置引用。

## 文档职责

- `design/foundation/`：实例、配置、执行、存储、网络、时间及观测。
- `design/models/`：资源、请求、Prompt、输出、缓存、额度和向量空间。
- `design/media/`：输入范围、各模态准备、解释策略与派生结果。
- `design/interfaces/`：正式入口及其呈现。
- `plans/`：当前完整能力的实施依赖、迁移与交付。
- `development/`：本地开发环境接入和实际使用通路。
- `research/`：决定所采用的外部依据与取舍。
- `sources/`：原样讨论，保留意图、提案与纠正；不逐段直接视为已批准合同。
- `legacy/`：退役资料，只作有目的的研究参考。

更新决定时原地修正文档，不在现行正文积累旧方案。`CURRENT.md` 不列讨论归档清单。与当前任务无关的文档不为格式统一而重写。

## 本轮专题导航

入口与依赖：[模型计划](plans/model-runtime.md)、[配置接入](design/foundation/configuration-integration.md)、[工具 CLI](design/interfaces/tools-cli.md)。

内容与模型：[CAS](design/foundation/content-storage.md)、[内容记录与结果](design/foundation/content-records.md)、[网络](design/foundation/networking.md)、[凭据](design/foundation/credentials.md)、[资源](design/models/resources.md)、[Prompt](design/models/prompts.md)、[任务合同](design/models/task-contracts.md)、[执行](design/models/execution.md)、[Embedding](design/models/embeddings.md)。

媒体：[准备与选择](design/media/preparation.md)、[图片](design/media/images.md)、[文档](design/media/documents.md)、[音频](design/media/audio.md)、[视频](design/media/video.md)、[解释与派生](design/media/analysis.md)。

实际使用：[本地网关](development/local-gateway.md)、[通路验证](development/model-media-walkthrough.md)。
