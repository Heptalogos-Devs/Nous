# Note 与 Journal 的首条持续认知链

## 本轮完成的产品能力

记录经历 → 形成或手写 Journal → 采用精确正文和来源 → 标注并绑定正式概念 → 查询当前认识 → 读取旧引用 → 反馈/修订 → 当前检索更新。

可以直接手写 Journal，无需先生成 Note；Note 默认持久保存但不参与普通长期召回，`remember` 可直接纳入。journalize 缺省返回可读草稿，`--save` 或配置可以直接采用，不设置普遍审批。正文与标注分别更新。第一版完整实现这些操作及它们的实际失败结果。

前置基础已经存在。继续使用 Python 3.14、uv、SQLite/SQLAlchemy、Pydantic、Markdown-it-py、Jinja、已选模型协议依赖、Execution、CAS、TextView、Embedding 与 Wave Native；无需新增后端框架。

## 施工合同

| 责任 | 文档 |
|---|---|
| 模块和复用 | [接入架构](../design/cognition/integration.md) |
| 文稿身份、操作与数据库 | [正式文稿](../design/cognition/written-records.md)、[存储](../design/cognition/storage.md) |
| 来源及草稿 | [引用](../design/cognition/citations.md)、[草稿与采用](../design/cognition/draft-adoption.md) |
| 正式概念与标注 | [Concept](../design/cognition/concepts.md)、[标注采用](../design/cognition/annotation-adoption.md) |
| 当前使用与时间 | [当前检索](../design/cognition/current-retrieval.md)、[时间与纠正](../design/cognition/temporal-use.md) |
| 反馈 | [反馈](../design/cognition/feedback.md) |
| 配置和外观 | [配置](../design/cognition/configuration.md)、[CLI](../design/interfaces/cognition-cli.md)、[进程内调用](../design/interfaces/cognition-api.md) |
| 真实结果 | [使用通路](../development/journal-use.md) |

## 先把文稿与事务建立在真实内容上

新增浅层 `src/nous/cognition/`。按 integration 的职责组织，不套 controller/service/repository 三层目录。Note 和 Journal 共享有限的文稿版本保存能力；各自输入、可召回性和 Journal 用途分别校验。不要另建 Memory 正文容器。

在纯 schema 中定义 Namespace、文稿、revision、引用、草稿、Concept、Annotation 和 Feedback；由组合根显式注册。使用同一 Database/WriteTx、Clock、Proquint 和 Artifact/CAS。实现默认域、记录、创建、编辑、精确读取、历史、归档/恢复与列表。编辑器写回必须携带读取时的基准版本，冲突返回用户编辑稿和当前引用，不自动覆盖。

正文源链接、owner revision refs、实际材料与来源用途一起保存。引用带到旧版本时不重解析为最新内容。用户不需要操作 prepare/publish/attach；完整操作封装这些步骤。

## 把领域任务收回真正的所有者

把 `models/input.py` 和 `models/tasks.py` 中的 concept-annotation 专名分支收回该静态任务定义。提供少量明确的输入准备/输出解释方法，由 bootstrap 组装 TaskCatalog；不增加动态插件 DSL。

将一次语言计算与其输出准备封装为可复用 `produce`，公共 run 和 journalize 共用同一模型执行路径。`ResultStore.prepare_outputs/adopt_in` 继续拥有正式采用。journalize 将 ProcessingResult 和 DraftDescriptor 一起提交；Journal 采用是其后的独立短事务。参见 draft-adoption 的精确失败合同。

保留真实 fallback/流逻辑，不新写一个 journal 专属客户端。按上下文保存必要阶段结果，调用共同 add_error_context；error_report 接收真实 next_action，区分同路线重试和切换。移除 TaskAssignment.media_strategy，媒体操作和调用参数决定策略，配置说明及消费者一起迁移。

## 正式 Concept 和标注只接入必要能力

建立稳定 Concept 及不可变意义版本，正文标注绑定 exact body revision。模型只提出局部概念及已有候选键；程序按明确策略采用。重复 label 可共存；只允许确切版本复用或明确新建，不以向量阈值自行合并。

annotation propose、read、adopt 接成完整操作。采用新概念、绑定、occurrence、path 和 active annotation 指针在一个 WriteTx。模型生成成功后正文被他人修改：保存提案，返回冲突，不重跑、不把位置搬到新正文。

## Wave 接入类型化认知视图

重构 `WaveOperations.build` / `WaveSnapshots.build`：把路径清单导入移到输入适配，类型化 CorpusView 是实际构建入口。实验清单和认知投影共用资产构建、Native、发布与读取。认知 owner 提供冻结文稿/概念/关系/来源；Wave 不查认知私表。

`CognitiveRetrieval` 捕获一致 Namespace 视图，按 current-retrieval 发布/复用快照并物化精确版本。保存与检索刷新分开，current 查询默认取得捕获时的完整当前视图；原地修订后旧图贡献不能残留。snapshot 精确读取保持旧版本。

本轮采用按需完整快照，复用已有向量。没有后台任务回执、事件总线或增量 overlay。构建期写入不被长期阻塞；本次查询固定其捕获视图，后来的变化留给下次查询。

## 接通反馈、外观与诊断

实现反馈评价和显式 correction。纠正事务同时提交 revision 和已应用反馈，未修改请求的评价只记录评价。current_basis 的明确依赖可以派生待复核状态，普通引用不触发连锁失效。

CLI 的 note/journal/journalize/concept/query/read/feedback 调同一公共操作。正文及短引用优先，JSON 保持类型；精确输入、实际错误、已保存草稿、检索进度与操作关联可向外读取。runtime check 汇总认知与投影状态，无模型请求。

## 删除与迁移范围

源码生产者、消费者、Schema、配置、包导出、模型资源、CLI、INDEX、AGENTS 和现行文档一并迁移。旧实验 tools 继续保留其普通处理能力，但它们不拥有另一套 Journal 规则；旧内部参数和双实现随替换删除。

已无效的测试和其脚手架随旧路径删除，有价值的情境迁到正式用户链。既有网关补丁、样本生成工具和开发实例不动；需要重建某实例时按已有授权执行。

## 实际开发依赖

先确定引用/存储/事务；文稿手工能力与静态任务封装可并行；草稿采用依赖两者；Concept/Annotation 的存储可并行但由同一负责人统筹共享值。检索视图等到正式版本和来源已就位后接入。最后完成反馈、呈现和整条操作。不要按文件数或每个测试拆提交/审查。

## 完成条件

一个用户能够无需清单 JSON：记录两段经历、形成草稿、采用日记、绑定概念、自然语言找回、纠正内容、重新查询当前结果，并按之前的引用读回旧正文。归档退出当前图；同名概念不误合并；无模型手写与词项检索可用；索引或模型失败保留已提交内容与清楚诊断；并发编辑不丢稿。

由项目所有者执行独立验证。开发者围绕当前真实故障运行有关操作，不恢复 CI、全量门禁、逐 Helper 测试或多层审批。必要的数学/并发关键合同可以保留小型回归；成果以使用链和所有权成立为准。
