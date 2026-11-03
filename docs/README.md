# Nous 设计与开发文档

`design/` 定义当前合同，`plans/` 安排已授权工作，`sources/` 保存原样讨论。当前施工入口为 [结构收敛与 Wave Native](plans/wave-native.md)。

## 全局入口

[架构](ARCHITECTURE.md)定义所有权、数据和依赖方向；[当前工作](CURRENT.md)区分已有能力与正在建设的能力。源码位置由 `src/nous/INDEX.md` 维护。开发发现结构不足时直接修改定义者与所有实际消费者。

## 专题导航

基础收敛：[应用边界](design/foundation/application-boundaries.md)、[来源与内容记录](design/foundation/content-records.md)、[配置接入](design/foundation/configuration-integration.md)、[执行与生命周期](design/foundation/execution.md)、[内容存储](design/foundation/content-storage.md)。未在本轮替换的路径、数据库、事务、时间和日志专题继续使用各自现行合同。

模型和媒体：[任务与执行](design/models/execution.md)、[任务合同](design/models/task-contracts.md)、[Prompt](design/models/prompts.md)、[输入量与尝试](design/models/budgets.md)、[备用路线](design/models/fallback.md)、[Embedding](design/models/embeddings.md)、[确定性准备](design/media/preparation.md)、[解释阶段](design/media/analysis.md)、[PDF](design/media/documents.md)、[音视频](design/media/video.md)。

Wave：[能力与边界](design/wave/overview.md)、[资产与集合](design/wave/assets.md)、[公共值](design/wave/contracts.md)、[上下文查询](design/wave/query-context.md)、[VCP 计算](design/wave/vcp.md)、[Nous 扩展](design/wave/nous-extensions.md)、[分数与排名](design/wave/ranking.md)、[Native 接口](design/wave/native.md)、[快照与发布](design/wave/persistence.md)、[配置](design/wave/configuration.md)、[产品入口](design/interfaces/wave-cli.md)。

实施与依据：[主计划](plans/wave-native.md)、[实际操作](development/wave-walkthrough.md)、[审查发现](research/structure-review-fdb39e4.md)、[VCP 采用对照](research/vcp-adoption.md)。

## 更新规则

一项规则只在其负责专题定义。决定改变时替换原文并迁移关联例子；不在正文保留失效方案的兼容说明。CURRENT 不列讨论归档清单。研究提出的新规则与源码已经具有的能力分别写明；正式 Spec 中已选方案就是本次施工合同。
