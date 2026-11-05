# 进程内认知操作合同

通过 `open_instance` 持有唯一 Runtime，通过 `await runtime.cognition(namespace)` 取得绑定稳定 Namespace 身份的实际操作集合。同一 Runtime 内可以同时使用多个域的句柄；修改默认域不会改变已取得句柄的归属。

## 公共值与调用

```text
RecordRef(instance_id, namespace_id, kind, id, address)
RecordRevisionRef(record_ref, revision_id, ordinal)
SourceBinding(key, selections: tuple[ContentSelection, ...], owner_revision?, usage, name?)
SourceRequest(target, usage=reference)
AfterWrite(annotation=default|skip|run, retrieval=defer|refresh, retrieval_mode?)
SourceChanges(preserve=True, add=(), remove=())
CommittedRecord(ref, revision_ref, namespace, title?, changed, committed_generation, in_recall)
RecordOutcome(record, status=success|partial, annotation, retrieval, operation)
```

以下为调用片段；完整可运行程序见 [host-use.py](../../examples/journal-maintenance/host-use.py)。各操作的参数、异常和副作用以其源码 docstring 为调用入口。

```python
from nous.cognition import AfterWrite, CognitiveQuery
from nous.cognition.writing.contracts import SourceRequest
from nous.runtime.bootstrap import open_instance

async with open_instance(instance) as runtime:
    cognition = await runtime.cognition("default")
    note = await cognition.records.add_note(text, remember=False)
    written = await cognition.writing.journalize(
        [SourceRequest(target=note.record.revision_ref)],
        adoption="save",
        after=AfterWrite(annotation="skip"),
    )
    result = await cognition.retrieval.query(CognitiveQuery(text="我学到了什么？"))
    original = await cognition.records.read(result.groups[0].revision_ref, sources=True)
```

`cognition.records` 提供 create_journal、add_note、read、replace、apply、diff、history、list、archive、restore、set_recall、select_sources 与 adopt。replace/apply 的基准为精确 revision；sources 使用 SourceChanges；未提供时保留原绑定，新增和移除都不要求重新构造整个旧目录。CLI 的原始来源以 SourceRequest 进入相同根操作，在操作内固定真实来源，不在入口拼事务。正文标题没有显式值或 H1 时保存为 None；本地化默认名称属于 presentation。

`RecordOutcome.record.committed_generation` 固定本次正文提交所处代次；annotation/retrieval 的 StageOutcome 独立给出实际状态与代次。正常 defer 返回 success，检索 pending；已启用维护失败返回 partial、已提交引用与共同 OperationIssue。取消与期限保留原异常类型及已完成引用。CLI 输出该结果后以 3 表示 partial，宿主直接处理类型化 status。

## 草稿与保存恢复

`cognition.writing.read_draft` 读取写作草稿、冻结上下文和精确来源；`cognition.records.adopt` 完成正式采用及 adopted_revision 关联；人工全篇覆盖通过 DraftOverride(markdown, source_changes) 提供。再次采用返回原正式修订；已经采用后传入不同覆盖正文或来源返回冲突。人工覆盖仍保留原模型 ProcessingResult 和编辑血缘。

成功计算的持久化失败通过异常恢复值 `PendingDraft(produced, context)` 保留，可调用 `cognition.writing.drafts.save_produced(produced, context)` 仅重试保存。精确输出 `!NO_MATERIAL!` 返回 no_material，并保留 ProcessingResult 与 stage_results，不建立空草稿。

Runtime 负责装配与资源生命周期；业务操作以构造参数取得依赖。WrittenRecords 只做文稿事务内采用，RecordReader 只做读取，DraftStore 管成功草稿，RecordOperations 完成提交与独立维护。`*_in` 方法用于同一正式采用事务，普通宿主调用上述完整操作。后续入口使用相同操作和类型。

`cognition.writing.propose_edit(base, instruction, scopes?/blocks?, sources, feedback_refs, adoption)` 提出局部修改；rewrite 生成全篇，reflect 至少接收一份精确 Journal 并另建一篇。三者返回与 journalize 相同的 WritingOutcome；draft-only 默认可直接查看 diff 和完整正文。程序内采用或 CLI --save 接同一 RecordOperations.adopt，没有额外审批步骤。
