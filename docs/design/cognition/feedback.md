# 反馈与明确更正

## 反馈目标

Feedback 指向 exact record revision/selection、某个检索命中（含 snapshot）、或调用方提供的整次 query context。logical 地址在提交时固定版本；不把后来的新版本当成当时评价目标。

字段为 namespace、target、category（usefulness / agreement / accuracy / retrieval）、value（positive / negative / neutral）、comment、author、recorded_at，以及可选 applied_revision。category 与 value 是意见，不能自动转换成真实性概率或图增益。

## 操作

```text
feedback(target, category, value?, comment?) -> FeedbackRef
feedback_correct(target_revision, expected_revision, replacement,
                 sources?, reason, author?) -> FeedbackRef + SavedRecord
```

仅评价不修改 Journal、不自动调用模型、不重建索引。针对检索的评价不直接重写正文。显式 replacement 进入 records 的同一 append_in；新 revision 与已应用反馈在同一短事务提交，失败时不把反馈标为已应用。

引用目标与 expected_revision 不一致时返回冲突。CLI editor 先保存原稿基准，编辑结束后再提交；通过 stdin/文件直接更正必须显式提供 expected（精确 ref 已包含）。正文替换、引用和时间仍使用同一校验。

用户描述“这里不对”但未给替换内容时，保存反馈。可显式请求 draft-correction 语言草稿，沿 journalize 的目标版本合同处理，程序不暗中修改认识。

## 查看与诊断

反馈列表可以按目标/类别读取。对整次查询，调用者提交公开查询结果（query text、snapshot、命中 refs），无需为了未来可能反馈而记录每次查询的一整套持久对象。CLI 可用 `--query-result result.json`，Agent 直接传类型化值。

完成输出表达“已记录反馈”或“已更正为 revision N”，附可读短引用和检索是否待更新。来源、操作关联与失败原因经共同错误与日志能力返回。
