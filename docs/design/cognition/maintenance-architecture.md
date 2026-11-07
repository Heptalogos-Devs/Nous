# 文稿维护的所有权与程序内使用

## 目标和稳定分工

Journal 的身份、正文历史、来源和修订属于认知领域。读取、写作、编辑、反馈和检索具有不同的完整操作；后续 Episode、Ego、Social、WorkContext、Dream 通过这些操作使用 Journal，不读取其私有表、不模拟 CLI。

本轮保留 Note/Journal 共用的有限文稿保存能力。完整修订始终是不可变正文及元数据；局部修改是产生新正文的一种输入方式。记录本身、概念标注、模型处理结果和可重建快照分别拥有自己的生命周期。

```text
CLI / 宿主程序
  → 绑定 Namespace 的实际应用操作
      ├─ RecordOperations → WrittenRecords + RecordReader → CAS / Artifact / Database
      ├─ WritingOperations → TaskCatalog / GenerationOperations → DraftStore
      │                                          └─ RecordOperations.adopt
      ├─ AnnotationOperations → Concept owner + 内容结构 → 原子采用
      ├─ FeedbackOperations → 意见及处理关联
      └─ CognitiveRetrieval → capture + projection → Wave → Native

RecordOperations 按 AfterWrite 选择前台维护或在正文事务中登记持久工作；WrittenRecords 不持有标注、检索或 DBOS。
```

没有公共方法从整个 Runtime 查找依赖。Runtime 负责装配及生命周期，所有实际依赖在构造时传入。

## 代码归属

写作、标注和检索分别由 `cognition/writing/`、`cognition/annotation/` 与 `cognition/retrieval/` 按实际职责组织。

| 文件 | 责任 |
|---|---|
| `cognition/writing/records.py` | 文稿创建、修订、成员状态与事务内采用；不依赖标注或检索。 |
| `cognition/writing/reading.py` | 精确读取、批量读取与历史；来源冻结集中到 cognition/sources，复用共同内容视图。 |
| `cognition/writing/operations.py` | RecordOperations：一个完整根操作中的输入准备、短事务、提交后维护、结果与部分完成。不能只逐方法转发。 |
| `cognition/writing/drafts.py` | 成功提案保存、读取与采用关联；支持 new/edit/rewrite/reflection。 |
| `cognition/writing/generation.py` | 创建型 journalize、精确编辑建议、显式全文重写、跨篇反思；复用共同生成。 |
| `cognition/writing/tasks.py` | 语言任务自己的输入解释、输出类型和产物，由组合根静态注册。 |
| `cognition/writing/contracts.py` | 本目录请求、提案与结果类型。公开 RecordRef/Namespace 等共同身份仍留在 cognition/contracts.py。 |
| `cognition/annotation/` | 标注计划、语言任务、正式提案、采用和读取。 |
| `cognition/maintenance/` | 维护计划、事务组合与两个持久工作流。 |
| `cognition/retrieval/capture.py` | 一个 ReadTx 捕获文稿、来源、标注与概念版本。通过负责者的批量 `*_in` 读取。 |
| `cognition/retrieval/projection.py` | 正式知识视图到 CorpusView 的纯语义映射；声明候选布局，不逐候选读/解析正文。 |
| `cognition/retrieval/operations.py` | 当前视图协调、查询、刷新和类型化结果。不得复制 Native 排名或分组。 |
| `cognition/retrieval/contracts.py` | QueryContext、QueryResult、交付范围和值类型。 |

领域表定义由组合根显式收集，分别位于 cognition/schema.py、concept_schema.py 和 feedback_schema.py。

AnnotationOperations 负责提案、计划、采用和批量读取；ConceptOperations 负责意义版本。MaintenanceOperations 解释维护政策并组合认知 owner 的事务参与方法。

## 公共程序入口

```python
async with open_instance(locator) as runtime:
    c = await runtime.cognition(namespace="default")
    note = await c.records.add_note("一次实际经历", remember=False)
    journal = await c.records.create_journal("# 结论\n\n正文")
    view = await c.records.read(journal.record.revision_ref, sources=True)
    answer = await c.retrieval.query(CognitiveQuery(text="发生了什么？"))
```

`runtime.cognition(namespace)` 返回 `CognitiveServices`：只绑定稳定 NamespaceRef（不缓存可变名称和代次作为权威）及共享依赖的 records、writing、annotations、concepts、retrieval、feedback。该对象只汇集实际操作，不再封装第二份业务实现，也不持有 SQL transaction。多个 Namespace 句柄可在同一 Runtime 同时使用；不修改全局“当前 Namespace”。每次根操作照常取得共同配置快照、期限和取消上下文。

CLI 解析 locator、文件和显式参数后使用相同句柄。引用本身携带并校验实例与域。普通 `open_instance` 负责业务资源，`run_host` 明确启动同循环持久工作；Job 管理的受限独立客户端见 [宿主与 Job](../interfaces/jobs-and-host.md)。

## 提交与后续维护

`WrittenRecords` 返回 `CommittedRecord`：稳定/精确引用、changed、committed_generation、title、是否在召回范围。它不返回假的 indexed 状态，不发模型请求。

`RecordOperations` 返回 `RecordOutcome`：committed record、annotation outcome、retrieval outcome、operation context、`success|partial`。显式请求失败仍保留已提交内容；取消保留原异常类型并带已完成引用。

```text
AfterWrite:
  annotation: default | skip | run
  retrieval: defer | refresh
  retrieval_mode?: 当前正式模式
  execution: default | inline | background
```

default 读取 `cognition.annotation.on_save`；不在 CLI 重复判断。正常 defer 是成功且检索 pending，不记作 partial；明确请求或配置启用的标注/刷新失败才是 partial。前台由共同维护能力在提交后执行；background 在正文事务中登记独立标注/刷新工作，提交后返回 jobs 引用，见 [持久维护](durable-maintenance.md)。

MaintenanceOperations 将 AfterWrite、正文是否改变和召回成员状态解释为一次 MaintenancePlan，固定执行方式、实际请求、已知不可用项及相关成员状态前置条件。写事务核对该前置条件，提交后按计划组合结果；未请求的阶段不冻结模型。

人工修改、提案采用和反馈更正都调用相同事务内提交。相关反馈的解决记录、提案采用关联和正文修订一起提交；后续维护发生在提交之后。维护失败不能导致再次生成或再次修订正文。

## 持久工作与后续认知

DBOS 由应用操作在相同事务中登记后续工作，领域保存不依赖 DBOS。WorkContext/Steward 消费正式引用和 QueryResult；Episode 等新增领域可以形成自己的投影，不继承一个万能 Memory 基类。当前没有这些对象时不建空服务、注册槽或事件总线。

## 返回类型与稳定导出

`CognitiveServices` 的公开能力是其实际服务对象；读取由 RecordOperations 暴露完整方法，内部委派给无维护依赖的 reader。用例返回的形状固定为：

```text
CommittedRecord:
  ref, revision_ref, namespace, title?, changed
  committed_generation, in_recall
RecordOutcome:
  record: CommittedRecord
  status: success | partial
  annotation: StageOutcome
  retrieval: StageOutcome
  operation: OperationLink
  jobs: tuple[JobRef, ...]
```

StageOutcome 使用 not_requested、pending、ready、failed；已入队的 pending 附实际 JobRef，失败带 OperationIssue，成功可附实际 generation。`record.committed_generation` 不随稍后的标注变化。`RecordOperations.read` 返回 RecordView，含精确版、当前版、正文与 ContentReadView 来源，和写入结果不同。

`nous.cognition` 导出 CognitiveQuery、AfterWrite、RecordOutcome、SourceRequest、SourceChanges、DraftOverride、NamespaceRef 和 RevisionSelection；`nous.content` 导出精确编辑值。包 `__init__` 提供当前正式 API 的显式导出。示例见 `docs/examples/journal-maintenance/host-use.py`。

RecordView 保留 `body_selection: ContentSelection`，并提供携带正式修订的 RevisionSelection；查询选段同样保留其归属，见 [来源解析](source-resolution.md)。正文仍在 body 字段。RevisionDiff 提供 `left/right` 精确引用与 `unified` 可读差异。SearchGroup.passages 中每项有原文 `text` 与 exact selection，供宿主直接消费，不要求再解析 CLI 字符串。这些是现有数据的正式返回值，不增加重复持久正文。

## 避免装配环

RecordOperations 依赖纯文稿核心、DraftStore、反馈解决写入以及标注/检索操作；后者都只依赖 RecordReader 和明确引用，不回调 RecordOperations。WritingOperations 可以完成 produce→保存草稿→调用 RecordOperations.adopt，它不被文稿核心反向依赖。FeedbackOperations 不再持有完整 WrittenRecords/RecordOperations 以便自己修订，明确编辑由调用方交给 RecordOperations。一个方向完成采用，其他 owner 提供同事务参与方法，不通过回调钩子相互启动。
