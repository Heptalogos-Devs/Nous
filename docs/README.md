# Nous 设计与开发文档

`design/` 定义当前合同，`plans/` 安排已授权工作，`sources/` 保存原样讨论。当前施工入口为 [共同能力收敛与 Journal 接入准备](plans/architecture-repair.md)。Wave Native 的前一轮计划保留为该能力的实施依据。

## 全局入口

[架构](ARCHITECTURE.md)定义所有权、数据和依赖方向；[当前工作](CURRENT.md)区分已有能力与正在建设的能力。源码位置由 `src/nous/INDEX.md` 维护。开发发现结构不足时直接修改定义者与所有实际消费者。

## 专题导航

基础收敛：[应用边界](design/foundation/application-boundaries.md)、[来源与内容记录](design/foundation/content-records.md)、[配置接入](design/foundation/configuration-integration.md)、[执行与生命周期](design/foundation/execution.md)、[内容存储](design/foundation/content-storage.md)。未在本轮替换的路径、数据库、事务、时间和日志专题继续使用各自现行合同。

模型和媒体：[任务与执行](design/models/execution.md)、[任务合同](design/models/task-contracts.md)、[Prompt](design/models/prompts.md)、[输入量与尝试](design/models/budgets.md)、[备用路线](design/models/fallback.md)、[Embedding](design/models/embeddings.md)、[确定性准备](design/media/preparation.md)、[解释阶段](design/media/analysis.md)、[PDF](design/media/documents.md)、[音视频](design/media/video.md)。

Wave：[能力与边界](design/wave/overview.md)、[资产与集合](design/wave/assets.md)、[公共值](design/wave/contracts.md)、[上下文查询](design/wave/query-context.md)、[VCP 计算](design/wave/vcp.md)、[Nous 扩展](design/wave/nous-extensions.md)、[分数与排名](design/wave/ranking.md)、[Native 接口](design/wave/native.md)、[快照与发布](design/wave/persistence.md)、[配置](design/wave/configuration.md)、[产品入口](design/interfaces/wave-cli.md)。

实施与依据：[主计划](plans/wave-native.md)、[实际操作](development/wave-walkthrough.md)、[审查发现](research/structure-review-fdb39e4.md)、[VCP 采用对照](research/vcp-adoption.md)。

本轮补充：[操作所有权](design/foundation/operation-ownership.md)、[多输入规划](design/models/input-planning.md)、[来源读取](design/foundation/provenance.md)、[正文结构](design/foundation/text-structure.md)、[请求尝试](design/models/request-attempts.md)、[使用诊断](design/interfaces/diagnostics.md)、[检索核心](design/wave/engine.md)。

认知设计：[Journal](design/cognition/journal.md)、[修订](design/cognition/journal-revisions.md)、[journalize](design/cognition/journalize.md)、[概念标注](design/cognition/concept-annotations.md)、[多路径投影](design/wave/journal-projection.md)、[关系支持](design/wave/evidence-projection.md)、[时间视图](design/wave/temporal-views.md)。本轮实现范围由主计划界定；完整 Journal 计划单列，不把设计存在当作代码已完成。

NousQL：[查询语言手册](design/nousql/README.md)。

## 更新规则

一项规则只在其负责专题定义。决定改变时替换原文并迁移关联例子；不在正文保留失效方案的兼容说明。CURRENT 不列讨论归档清单。研究提出的新规则与源码已经具有的能力分别写明；正式 Spec 中已选方案就是本次施工合同。

修改已有专题时以仓库当前正文为底稿，只编辑受到新决定影响的段落；保留用户和开发者补充的仍有效合同。外部 Spec 更新用上下文补丁，不用目录覆盖。若补丁基线后已发生修改，阅读冲突段与后续决定后合并，禁止整文件选 ours/theirs。原始讨论保留不动，正文不追加相互冲突的旧新版本。
