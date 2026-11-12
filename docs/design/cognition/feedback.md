# 反馈与明确更正

## 反馈目标

Feedback 指向 exact record revision/selection、某个检索命中（含 snapshot）、或调用方提供的整次 query context。记录文稿意见时提供精确版本；当时评价的版本保持固定。

字段为 namespace、target、category（usefulness / agreement / accuracy / retrieval）、value（positive / negative / neutral）、comment、author、recorded_at，以及可选 resolution（action、note、mapping_note、applied_revision、resolved_at、actor）。category 与 value 是意见，不能自动转换成真实性概率或图增益。

## 操作

```text
feedback.record(target, category, value?, comment?) -> FeedbackView
RecordOperations.apply/replace(target, ..., expected=base_revision, feedback_refs=[...], feedback_mappings={...}) -> RecordOutcome
feedback.read/list/resolve(...) -> 已有意见及处理结果
```

仅评价不修改 Journal、不自动调用模型、不重建索引。针对检索的评价不直接重写正文。明确 edits/replacement 进入 RecordOperations 的共同提交；新 revision 与已应用反馈在同一短事务提交，失败时不把反馈标为已应用。

意见目标与基准不一致且没有明确映射说明时返回冲突；同一文稿旧版意见的显式映射见 [反馈处理](feedback-processing.md#显式旧版意见映射)。CLI editor 先保存原稿基准，编辑结束后再提交；通过 stdin/文件直接更正必须显式提供 expected（精确 ref 已包含）。正文替换、引用和时间仍使用同一校验。

用户描述“这里不对”但未给替换内容时，保存反馈。可显式请求 writing.propose_edit，根据精确基准与范围形成局部修改提案；不进入 journalize。意见查看、处理关联与引用规则见 [反馈处理](feedback-processing.md)。

## 查看与诊断

反馈列表可以按目标/类别读取。对查询反馈，调用者提交正式 QueryResult；反馈 owner 保存实际问题、参与上下文、精确快照及操作关联。指定组和片段索引或已交付选区时，额外保存该组排名依据和所评价的实际片段。CLI 可用 `--query-result result.json`，程序可直接传类型化值。快照 owner 使用正式字段验证；见[查询情境](query-use-context.md)。

完成输出表达“已记录反馈”或“已更正为 revision N”，附可读短引用和检索是否待更新。来源、操作关联与失败原因经共同错误与日志能力返回。
