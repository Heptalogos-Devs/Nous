# Nous 设计与开发文档

当前能力见[当前工作](CURRENT.md)，保存后维护的完整使用见[操作说明](development/durable-cognition-walkthrough.md)。[可靠认知操作与持久维护](plans/durable-cognition.md)记录身份、来源、原文、执行与 DBOS 维护的交付范围；Journal Maintenance、基础收敛与 Wave Native 计划保留为对应能力的实施依据。

## 全局入口

[架构](ARCHITECTURE.md)定义所有权、数据和依赖方向；[当前工作](CURRENT.md)区分已有能力与正在建设的能力。源码位置由 `src/nous/INDEX.md` 维护。开发发现结构不足时直接修改定义者与所有实际消费者。

模型任务设计、输入准备和输出判断遵循 [LLM 使用与提示词设计](agents/llm-use.md)。

公共服务的调用契约写在对应模块、类和方法的 docstring 中，写法见 [Python 文档规范](agents/docstring-std.md)。先通过[源码索引](../src/nous/INDEX.md)定位服务，再查看其输入、结果、资源归属和错误恢复说明；设计专题说明跨模块规则。

## 专题导航

基础收敛：[应用边界](design/foundation/application-boundaries.md)、[来源与内容记录](design/foundation/content-records.md)、[配置接入](design/foundation/configuration-integration.md)、[执行与生命周期](design/foundation/execution.md)、[内容存储](design/foundation/content-storage.md)。未在本轮替换的路径、数据库、事务、时间和日志专题继续使用各自现行合同。

模型和媒体：[任务与执行](design/models/execution.md)、[任务合同](design/models/task-contracts.md)、[Prompt 设计与资源](design/models/prompts.md)、[输入量与尝试](design/models/budgets.md)、[备用路线](design/models/fallback.md)、[Embedding](design/models/embeddings.md)、[确定性准备](design/media/preparation.md)、[解释阶段](design/media/analysis.md)、[PDF](design/media/documents.md)、[音视频](design/media/video.md)。

Wave：[能力与边界](design/wave/overview.md)、[资产与集合](design/wave/assets.md)、[公共值](design/wave/contracts.md)、[上下文查询](design/wave/query-context.md)、[VCP 计算](design/wave/vcp.md)、[Nous 扩展](design/wave/nous-extensions.md)、[分数与排名](design/wave/ranking.md)、[Native 接口](design/wave/native.md)、[快照与发布](design/wave/persistence.md)、[配置](design/wave/configuration.md)、[产品入口](design/interfaces/wave-cli.md)。

实施与依据：[主计划](plans/wave-native.md)、[实际操作](development/wave-walkthrough.md)、[审查发现](research/structure-review-fdb39e4.md)、[VCP 采用对照](research/vcp-adoption.md)。

本轮补充：[操作所有权](design/foundation/operation-ownership.md)、[多输入规划](design/models/input-planning.md)、[来源读取](design/foundation/provenance.md)、[正文结构](design/foundation/text-structure.md)、[请求尝试](design/models/request-attempts.md)、[使用诊断](design/interfaces/diagnostics.md)、[检索核心](design/wave/engine.md)。

认知设计：[Journal](design/cognition/journal.md)、[修订](design/cognition/journal-revisions.md)、[journalize](design/cognition/journalize.md)、[概念标注](design/cognition/concept-annotations.md)、[多路径投影](design/wave/journal-projection.md)、[关系支持](design/wave/evidence-projection.md)、[时间视图](design/wave/temporal-views.md)。本轮实现范围由当前主计划界定；未来机制单列，不把设计存在当作代码已完成。

认知接入：[文稿操作](design/cognition/written-records.md)、[草稿采用](design/cognition/draft-adoption.md)、[精确引用](design/cognition/citations.md)、[正式概念](design/cognition/concepts.md)、[标注采用](design/cognition/annotation-adoption.md)、[当前检索](design/cognition/current-retrieval.md)、[反馈](design/cognition/feedback.md)、[用户入口](design/interfaces/cognition-cli.md)。

维护与程序使用：[架构](design/cognition/maintenance-architecture.md)、[精确编辑](design/cognition/exact-editing.md)、[写作意图](design/cognition/writing-intents.md)、[反馈处理](design/cognition/feedback-processing.md)、[查询情境](design/cognition/query-use-context.md)、[结构贡献](design/cognition/structure-contributions.md)。

共同接缝：[内容阅读与准备](design/content/reading-and-preparation.md)、[投影准备](design/wave/corpus-preparation.md)、[文稿分组](design/wave/grouped-delivery.md)、[操作结果](design/foundation/operation-outcomes.md)、[Prompt 编辑上下文](design/models/prompt-editing-context.md)、[配置](design/cognition/maintenance-configuration.md)、[维护 CLI](design/interfaces/journal-maintenance-cli.md)、[实例选择](design/interfaces/instance-selection.md)。

本轮专题：[稳定身份](design/foundation/stable-identities.md)、[来源解析](design/cognition/source-resolution.md)、[无损正文](design/content/selection-and-text.md)、[当前视图变化](design/cognition/current-view-mutations.md)、[配置快照](design/foundation/typed-config-snapshot.md)、[执行与资源](design/foundation/execution-and-job-lifecycle.md)、[DBOS 接入](design/foundation/dbos-integration.md)、[持久维护](design/cognition/durable-maintenance.md)、[成功计算](design/models/durable-computation.md)、[宿主与 Job](design/interfaces/jobs-and-host.md)。

后续：[认知使用路线](plans/cognitive-use-roadmap.md)、[工作中的认知使用](plans/cognitive-use-next.md)。

NousQL：[查询语言设计](design/nousql/README.md)。
