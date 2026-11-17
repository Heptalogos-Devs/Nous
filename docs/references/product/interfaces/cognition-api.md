# 进程内认知操作合同

`open_instance` 借用实例资源；`runtime.app.call(操作定义, 类型化请求)` 执行完整用例。本地 CLI、SDK 和宿主使用相同操作及结果类型。namespace 可为名称、稳定 `NamespaceRef` 或默认值；精确记录引用自带域身份并在操作内核验。

```python
from nous.cognition.retrieval.contracts import CognitiveQuery
from nous.cognition.retrieval.operations import QUERY
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
    result = await runtime.app.call(QUERY, CognitiveQuery(text="我学到了什么？", mode="lexical"))
```

`RecordRef` 标识文稿，`RecordRevisionRef` 标识不可变修订。`SourceChoice` 接收正式修订、精确选区或内容来源，并声明 reference、current_basis 或 inspiration；current_basis 要求精确正式修订。修改既有来源使用 `SourceChanges`。

`RecordOutcome.commit` 是正式保存结果，`maintenance` 表示未请求、完成、已启动或未启动的实际情况，`issues` 保存明确失败。已保存正文和维护结果分别读取；取消保留已完成引用。

写作的 new、edit、rewrite 和 reflection 共用 `WRITE_DRAFT`。结果包含已保存产品引用和可选草稿引用。`READ_DRAFT` 读取完整草稿，`REVISE_DRAFT` 产生保留父草稿的新草稿，`ADOPT_DRAFT` 采用完整草稿。基准已变化时采用冲突且草稿保留；重复采用返回原修订。模型成果已保存而草稿组装失败时，`DRAFT_FROM_RESULT` 对该成果发起新的本地组装。

`QUERY` 返回精确快照、捕获代次、文稿组排名依据和实际交付片段。片段自带精确选择，`READ_SELECTIONS` 回读原文；`RECORD_QUERY_FEEDBACK` 接收实际查询结果和交付目标。完整示例见 [host-use.py](../../examples/journal-maintenance/host-use.py)。
