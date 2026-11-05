# 意见保存、处理与准确更正

## 意见与操作分开

category 为 accuracy/usefulness/agreement/retrieval，value 为 positive/negative/neutral，comment 为原意见。程序不按 comment 的关键词猜测是否执行修改。意见没有后续动作时仍可保存和阅读。

```text
record(target: ExactRevision | ExactSelection | QueryTarget, ...)
read(feedback_ref)
list(target?, category?, unresolved_only?, page)
resolve(feedback_ref, action: acknowledged | not_applied, note?)
```

查询目标使用 [查询情境](query-use-context.md)，不再由 CLI/调用者拼造任意字典。API 和 CLI 提供按文稿/类别读取，用户能看到意见针对哪个旧版本、有无处理及相关结果。

## 明确修改

调用者给出具体 edits 或完整正文时，调用 RecordOperations.apply/replace，传入 feedback_refs。文稿修订和反馈应用关联在同一短 WriteTx 提交。FeedbackOperations 提供 `resolve_in(tx, ..., applied_revision)`，不拥有第二套文稿更正方法和保存后维护逻辑。

自然语言意见可以显式交给 `WritingOperations.propose_edit`。输入包括用户指定的反馈 comment、精确基准和范围；反馈本身被固定引用。仅产生提案时状态仍未应用；采用后才记录 applied_revision。

不把 feedback category=accuracy/value=negative 自动换算成模型调用、图权重或真实性。`intent=correction` 由调用者选择，并使用当前 basis owner 的依赖失效规则；普通语言编辑不误伤历史引用。

## 处理记录

反馈原意见保持不变；增加一项小型处理记录：action、note、mapping_note?、applied_revision?、resolved_at、actor?。同一个提案重复采用不重复产生处理。多个意见可以由同一修订解决，分别关联。

调用者可以明确选择“不采用”或“已了解”并说明原因，不需要为了结束一条反馈修改正文。不建设审批状态机、自动工单分配或反馈队列。

CLI 使用 feedback record/read/list/resolve 管理意见，journal patch/replace 或 revise 执行明确修改，均调用相同领域操作。

## 历史和来源

反馈针对提交时的明确旧版本，不被后来 current 覆盖。修改范围外的正文及原来源由共同编辑保存；新来源须有正式绑定。反馈 comment 属于用户意见，参与模型输入时按意见提供，不能登记为已经证实的外部事实。

处理成功的回复说明“意见已记录”“修改提案已保存”或“已更正为第 N 版”；已提交而维护失败时使用共同 partial 诊断。原文与提案都能在没有模型时读取。

`feedback_refs` 原子关联正文更正时，逐项核对目标属于同一 Namespace 和基准文稿/精确选区。已在旧版本给出的意见若要应用到另一个新基准，由调用者明确选择并在提案中保存映射说明；不能在采用时偷偷换目标。整次查询的有用性意见保持 query 目标，不因为一次文稿修改就自动标成已解决准确性问题。

## 显式旧版意见映射

`feedback_mappings: Mapping[str, str]` 按反馈引用保存调用者说明：为什么这条旧版意见适用于所选新基准。普通 feedback_refs 无映射时仍要求 exact base；映射只允许同一 Namespace 的同一文稿，说明必须非空。跨文稿意见和没有指定交付文稿的整次 QueryResult 意见不能借映射随文稿修改解决。

`WritingOperations.propose_edit(..., feedback_refs=..., feedback_mappings=...)` 通过 FeedbackOperations.select_for_edit 取得经过核验的实际意见，把说明固定在 DraftContext.feedback_mappings。采用时同一 resolve_in 重新核验并在正文采用事务中保存 resolution.mapping_note 与 applied_revision。没有提案的手工 apply/replace 也接收相同参数和核验规则。

```python
mappings = {feedback.address: "日期意见仍适用于当前版本；新增的末段与该意见无关。"}
proposal = await cognition.writing.propose_edit(
    current.revision_ref, "只更正日期", feedback_refs=[feedback.address],
    feedback_mappings=mappings, adoption="save",
)
```

手工正文修改的 sources 使用 [SourceChanges](exact-editing.md#来源映射继承)：省略时保留旧目录，add 只提供新绑定，remove 提供确切旧键。删除后正文还引用该键会失败，意见文字本身不被作为事实来源加入目录。
