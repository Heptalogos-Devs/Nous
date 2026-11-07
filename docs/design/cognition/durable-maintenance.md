# 独立标注工作与当前检索刷新

## 保存后的产品选择

保留 AfterWrite 的 annotation/retrieval 语义，增加执行方式：

```python
class AfterWrite(CognitiveValue):
    annotation: Literal["default", "skip", "run"] = "default"
    retrieval: Literal["defer", "refresh"] = "defer"
    retrieval_mode: Mode | None = None
    execution: Literal["default", "inline", "background"] = "default"
```

default 从配置解析。为了保留当前明确前台体验，恢复包的起步默认 execution=inline；用户或宿主可显式 background。后台只安排明确启用的维护，纯手工/无模型使用仍成立。`retrieval_mode` 隐含 refresh，沿现有 CLI 语义处理。

RecordOutcome 增加 `jobs: tuple[JobRef, ...]`；每个 StageOutcome 可携带其 JobRef。成功登记为 queued/pending，整体 success；前台明确请求阶段失败仍为 partial。任务将来失败不能倒改已经返回的保存事实；Job 状态和 runtime check 呈现后续结果。

自动维护只在实际写入/视图变化时建立必要工作。unchanged 返回不重复排队；调用者明确请求再次标注时可通过 maintenance 操作创建新请求，不伪造新的正文修订。

保存前按正式正文差异、召回成员状态和显式请求决定是否准备标注；写事务重新核对版本与相关成员状态。自动标注不适用时无需模型配置。请求的维护准备不可用时，正文可以提交并返回具体的 partial 阶段；事务中已经请求的队列登记失败则回滚整个写入。

## 两个独立工作

`AnnotationRequest` 固定计算键、语言任务、候选上限及当前性资格；入队值另携精确正文修订、域和可选刷新请求。准备步骤生成 AnnotationPlan，固定正文选区、候选意义及预期活动标注。

`RefreshRequest` 固定检索模式、空间身份和构建设置；入队值另携域、minimum_generation 与发起关联。minimum_generation 表示本次需求的最低覆盖，实际构建捕获当时 G，可覆盖更新的提交。

二者在保存事务中各自入队。初次 refresh 可以在模型标注前完成，让正文先可检索；annotation 失败只影响该工作。annotation 实际改变当前活动标注后，在同一采用事务登记新 generation 的 refresh。

不以“某域已有一个 refresh Job”为由丢弃新需求：旧工作可能已经捕获较早 G。使用具体提交代次和构建计划登记；重复工作开始时发现已满足可以快速结束。先利用现有 profile 构建锁与 generation。

## 标注 workflow

### 准备精确输入

读取固定正文 revision；检查是否仍处于自动维护的当前/可召回范围，已经过期或归档时返回明确 skipped/superseded。人工要求历史标注另走显式操作，不让后台猜测意图。

确定概念候选的精确意义版本、当前活动标注前置条件、精确正文选区与已冻结语言任务，形成 AnnotationPlan。计划保存实际生成需要的值，后续步骤不重新搜索不同候选。准备/生成期间不持有数据库事务。

### 产生语言结果

调用 GenerationOperations 和模型尝试策略，输入为计划固定的正文/候选，成功响应经解释并采用为正式提案。DBOS 步骤只记录提案引用；已存在提案或成功响应可继续，不重新请求模型。

### 保存提案并采用

从正式提案准备采用值。完整维护用例打开短 WriteTx，调用 AnnotationOperations.adopt_in 核验活动资格、复用或创建标注并更新指针；当前视图变化需要刷新时，同事务调用 enqueue_refresh_in。

正文或活动指针已经被别人改变时，保存可读提案/历史结果，返回 superseded/inactive，不覆盖当前状态，也不重发模型试图“追上新版本”。符合条件时采用并推进 generation，再登记 refresh。

Job 结果包括 proposal_result、annotation_ref、activation_state、受影响 generation、related_jobs。用户取消发生在提交之后，已提交的标注和后续入队事实仍存在，不能回滚。

## 刷新 workflow

进入时读取目标 Namespace/profile 的已发布 descriptor；已经覆盖 minimum_generation 且 profile 匹配时结束为 ready。否则调用当前认知检索 owner 的同一 refresh 能力：一次一致捕获 G → 事务外准备/向量/Native 构建 → 原子发布 → 条件更新 current。

捕获之后发生新写入时，本次结果可以为 published_not_current，保留 exact snapshot 与 G；更新需求由后续已登记 Job 或下一次 current 查询处理。没有无限追赶循环，没有旧图加新正文的补偿查询。

Wave 快照采用/发布需要可重放的 build key：同一请求、同一捕获资产和 profile 已登记过则返回原 snapshot。文件发布与数据库登记分开，重启时已有完整资产可以复用，未完成临时资产走既有生命周期清理。

## 前台与后台共享

将 Annotate/Refresh 的完整业务阶段放在 `cognition/maintenance/operations.py` 或现有 owner 的内聚方法中。workflow 调用这些阶段；RecordOperations 的 inline 后处理也调用同一能力。不要复制一套“Job 专用标注”和“Job 专用检索”实现。

RecordOperations 通过一个明确维护协作者完成 prepare-before-write、enqueue-in-write、finish-after-write；WrittenRecords 不依赖队列或完整 Runtime。反馈修订、人工编辑、草稿采用、remember/restore 等实际生产者都迁移。

## 重试、恢复和状态

准备、生成、采用、刷新使用命名步骤，步骤返回只含可序列化值。已记录步骤恢复时返回原结果；业务提交与 DBOS checkpoint 之间的窗口由业务键吸收。显式 `ERROR` 重试通过新的 fork Job 从失败步骤接续，保留旧工作错误与已采用产物。

用户 `resume` 只用于支持恢复的非 ERROR 状态，沿用原 JobRef 和已记录步骤，不能自动改用当前新 Prompt。要使用新模型/模板重新维护，创建明确的新请求。`ERROR` 重试通过独立 JobRef 和 `forked_from` 关系表达，不覆盖既有工作结果。Job result 的 related_jobs 只表示真实后续工作；`wait` 默认等待本项，调用者可以继续等待这些引用，不把事务入队的独立刷新误当 DBOS child workflow。

`runtime.maintenance.request_annotation(target, namespace=..., task=..., refresh=...)` 为当前修订登记新的明确请求，返回 JobRef；请求固定当时的任务及 Prompt，不创建正文修订。明确请求允许标注不参与召回的 Note；可选 refresh 在标注改变当前召回视图时登记后续刷新。正文在排队或生成期间变化时保留实际阶段结果并报告 superseded。

保存时先验证可确定的维护输入。维护未配置而当前业务允许保存时，按前台操作合同返回正文已保存与维护不可用的真实状态；**已经承诺将入队与正文合成一个 WriteTx 时，SQL 入队失败必须让本次事务整体回滚**。实现不能在某个 except 中吞掉入队错误并返回 queued/pending。自动标注只在本次修订确有召回成员资格和有效前置条件时激活；显式对 `remember=False` Note 的标注是合法的，召回成员资格只影响当前视图与刷新。

## 完成路径

后台 save 返回可读 revision 与 JobRef；没有 worker 时明确 queued。运行 worker 后正文可查询，标注失败不阻断文本刷新。恢复不会重复创建提案、Annotation 或 snapshot；过时结果不改变当前活动指针。Job cancel/wait 与实际资源结束的差异在状态和文档中准确呈现。
