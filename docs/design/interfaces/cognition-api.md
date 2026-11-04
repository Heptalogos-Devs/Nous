# 进程内认知操作合同

通过现有 open_instance 获取各公共 owner。第一版 SDK 就是类型化进程内调用；不另建远程 client/HTTP/MCP 占位。

公开操作的核心值：

```text
RecordRef(instance_id, namespace_id, kind, id, address)
RecordRevisionRef(record_ref, revision_id, ordinal)
SourceBinding(key, content_selection, owner_revision?, usage, name?)
SavedRecord(ref, revision_ref, generation, changed, retrieval_state)
DraftRef(instance_id, namespace_id, id, address)
JournalizeOutcome(status=no_material|draft|adopted, draft_ref?, record?, stage_results)
AnnotationRef(instance_id, namespace_id, id, body_revision)
ConceptRevisionRef(instance_id, namespace_id, concept_id, address, ordinal)
CognitiveQuery(namespace, text, mode=auto, snapshot?, context=[], filters=?, explain=false)
CognitiveHit(record_revision, content_selection, rank, snippet, annotation_ref?, source_summary)
```

示意调用（函数名是本轮合同，参数可封装为明确 request 值）：

```python
async with open_instance(instance) as runtime:
    a = await runtime.records.record_note(namespace, text, remember=False)
    b = await runtime.journalize.run(namespace, sources=[a.revision_ref], adoption="save")
    result = await runtime.cognitive_retrieval.query(CognitiveQuery(namespace, "我学到了什么？"))
    original = await runtime.records.read(result.hits[0].record_revision, sources=True)
```

`runtime.journalize.read_draft` 读取原模型稿与精确来源；`adopt_draft` 可携带编辑正文、来源用途与显式新基准。持久化失败的成功计算保存在异常恢复值 `PendingDraft(produced, context)` 中，可调用 `save_produced(produced, context)` 只重试保存。该方法返回 JournalizeOutcome：正文为 draft，模型精确输出 `!NO_MATERIAL!` 时为 no_material，并保留 ProcessingResult 的 stage_results。

保存操作可使用 `refresh=True` 或 `refresh_mode="lexical"` 接续当前检索；返回的 SavedRecord 保留 saved/具体 revision，并分别给出 retrieval_state、indexed、covered_generation 与 retrieval_error。刷新失败时正文已提交，调用者可直接读取或重新刷新。

Runtime 是组合根公开句柄集合；业务类通过构造参数取得所需 owner，不能在内部拿整个 Runtime 查找服务。

认知 API 输入 exact references 或由 refs 一次解析的逻辑目标；操作自己取得配置和执行上下文，CLI 不拼接数据库事务、模型请求、Annotation 或快照清单。可组合的内部 *_in 方法仅供同一正式采用事务使用。

对未来 HTTP/MCP，复用这些操作与输出，不逐内部函数暴露工具。JSON 字段保留未知时间与真实部分结果；状态码和错误 code 由统一入口映射。
