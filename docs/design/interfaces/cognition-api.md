# 进程内认知操作合同

`open_instance` 借用实例资源；`runtime.app.call(操作定义, 类型化请求)` 执行完整用例。本地 CLI、SDK 和宿主使用相同操作及结果类型。namespace 可为名称、稳定 `NamespaceRef` 或默认值；精确记录引用自带域身份并在操作内核验。

```python
from nous.cognition.retrieval.contracts import QueryRequest, SaveEvidence
from nous.cognition.retrieval.operations import QUERY, SAVE_EVIDENCE
from nous.cognition.writing.contracts import AddNote, AfterWrite, NewDraftRequest, SourceChoice
from nous.cognition.writing.generation import WRITE_DRAFT
from nous.cognition.writing.operations import ADD_NOTE
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import open_instance

async with open_instance(instance) as runtime:
    note = await runtime.app.call(
        ADD_NOTE,
        AddNote(body=InlineText(text="一次实际经历"), remember=True,
                after=AfterWrite(annotation="skip")),
    )
    draft = await runtime.app.call(
        WRITE_DRAFT,
        NewDraftRequest(sources=(SourceChoice(target=note.commit.revision_ref),)),
    )
    result = await runtime.app.call(QUERY, QueryRequest(text="我学到了什么？", save=True))
```

`RecordRef` 标识文稿，`RecordRevisionRef` 标识不可变修订。`SourceChoice` 接收正式修订、精确选区或内容来源，并声明 reference、current_basis 或 inspiration；current_basis 要求精确正式修订。修改既有来源使用 `SourceChanges`。

`RecordOutcome.commit` 是正式保存结果，`maintenance` 表示未请求、完成、已启动或未启动的实际情况，`issues` 保存明确失败。已保存正文和维护结果分别读取；取消保留已完成引用。

写作的 new、edit、rewrite 和 reflection 共用 `WRITE_DRAFT`。结果包含已保存产品引用和可选草稿引用。`READ_DRAFT` 读取完整草稿，`REVISE_DRAFT` 产生保留父草稿的新草稿，`ADOPT_DRAFT` 采用完整草稿。基准已变化时采用冲突且草稿保留；重复采用返回原修订。模型成果已保存而草稿组装失败时，`DRAFT_FROM_RESULT` 对该成果发起新的本地组装。

`QUERY` 返回 `RetrievalOutcome`，其中 `evidence` 包含查询意图、捕获快照、材料、实际交付片段、有效程序参数及覆盖情况。显式保存或使用 Steward 时，`evidence_ref` 固定正式成果；`materials.read` 可以用该引用与 E 键回读。普通查询先返回内存值；之后可用 `runtime.app.call(SAVE_EVIDENCE, SaveEvidence(evidence=result.evidence))` 保存同一交付，再用于反馈和工作记录。阅读整理通过 `steward.arrange` 处理已保存证据，`feedback.record_query` 使用已保存的证据引用及可选 E 键。

需要精简机器输出时，`runtime.app.call_name(name, request, select=("/字段/路径",))` 使用 JSON Pointer 选择序列化后的结果，返回指针到值的对象；空选择返回完整结果。`Application.describe` 始终提供完整操作 Schema。

`DeliveryRequest.budget` 使用 `nous.wave.contracts.DeliveryBudget(amount=..., unit="characters" | "bytes" | "tokens")`。字符和字节预算按实际交付的原文计量，`evidence.delivery_usage` 记录实际用量；没有声明分词器时，token 预算返回能力不可用。每个已找回材料保留排名与 `delivery_status`，没有片段时可以沿结果中的阅读入口继续读取。

`work.create/read/revise/set_active/record_use/list_uses` 操作持续工作。所有传输的请求、结果和事件 Schema 均来自同一 Application 目录。
