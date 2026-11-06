# 轻量快照描述、批量候选与反馈冻结

## 当前问题与目标

WaveOperations.describe 通过 snapshots.resolve 取得完整含 mapping 的行；describe_candidate 又对每个候选重复 resolve。反馈四项交付结果因而读取五次完整快照。接口表面窄，底层成本随库规模放大。

提供真实的轻量 descriptor 和一次批量 candidate 读取。快照 owner、profile、generation、空间身份和必要摘要由 Wave 自己持有，消费者不解析 collection 名或直接 SQL。

## 合同

```python
async def describe(ref: str | SnapshotRef) -> SnapshotDescriptor: ...
async def describe_candidates(ref: SnapshotRef, keys: Sequence[str]) -> tuple[CandidateDescriptor, ...]: ...
```

describe 只读取所需元数据。owner 与 metadata 保存为快照独立列，collection 名由集合表取得；发布事务同时保存描述与完整 mapping。current 与 describe 不加载 Native 或完整候选 JSON。

describe_candidates 解析快照一次，按请求顺序返回，保持重复 key 的确定语义；缺项一次报告具体 key。暂时仍用整体 mapping 时也只读/解码一次，不逐候选全量读。后续确有规模需要再将候选索引细分存储，本轮不为一个反馈功能重建整套 Wave persistence。

typed SnapshotRef 核验 instance/id/identifier；cognition owner 核验 namespace。显式 snapshot 仍读取当时版本；最终 exact selection 不重新绑定到 current。

## 反馈路径

FeedbackOperations.record_query 接收正式 QueryResult，校验真实 snapshot、实际 selected_context、delivery 及 target。一次调用 describe + 一次 describe_candidates，候选较多也不逐项查询。绑定已知 exact revision/selection 时由共同认知来源能力保留 RevisionSelection。

持久 feedback 的 `QueryFeedback` 保存 QueryContext、交付顺序和操作关联，目标继续使用精确修订/选区。原文、候选集合与 diagnostics 从原查询和精确快照读取。保留当时实际交付事实；未交付候选不被推成负反馈。

保留 category/value/comment 与 accuracy/agreement/usefulness/retrieval 的含义。普通意见不调用模型、不改文稿、不推进 generation。明确更正通过文稿 owner 并与 resolution 同事务提交。

## 性能与并发

本轮仍使用完整当前视图构建；四个同 profile 并发查询复用一次构建。配置和正文准备的重复工作在各自 owner 修复，不能为了降低观察到的延迟绕过来源或使用旧快照假称 current。

跨多个 Namespace/profile 的读取相互独立，索引/Native 加载仍由 Wave 持有。本轮不改变 quality_score、ranking_score、RRF 或文稿分组 top_k 的含义。

## 完成行为

四项反馈不发生五次完整 snapshot 读；轻量 status/describe 不打开 Native；查询反馈准确指向旧版本；长文加八篇短文仍返回九篇分组文稿；保存后维护失败的可读结果与 current freshness 分开。
