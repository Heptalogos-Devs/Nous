# 轻量快照描述、批量候选与反馈冻结

## 快照读取

快照 owner、profile、generation、空间身份和必要摘要由 Wave 自己持有，消费者不解析 collection 名或直接 SQL。候选数据按不可变文稿投影单元保存，快照持有成员与行号关系。

## 合同

```python
async def describe(ref: str | SnapshotRef) -> SnapshotDescriptor: ...
async def describe_candidates(
    ref: SnapshotRef, keys: Sequence[str]
) -> tuple[CandidateDescriptor, ...]: ...
```

describe 只读取所需元数据。owner 与 metadata 保存为快照独立列，collection 名由集合表取得；current 与 describe 不加载 Native 或候选单元。

describe_candidates 按候选键批量读取所需单元，按请求顺序返回并保留重复 key；缺项一次报告具体 key。

typed SnapshotRef 核验 instance/id/identifier；cognition owner 核验 namespace。显式 snapshot 仍读取当时版本；最终 exact selection 不重新绑定到 current。

## 反馈路径

FeedbackOperations.record_query 接收正式 QueryResult，校验真实 snapshot、实际 selected_context、各组交付片段及 target。候选与组分别批量读取；绑定已知 exact revision/selection 时保留 RevisionSelection。

持久 feedback 的 `QueryFeedback` 保存 QueryContext、操作关联、所选组排名依据及实际交付候选，目标使用精确修订/选区；同一事务登记对原快照的正式保留引用。未交付候选不被推成负反馈。

保留 category/value/comment 与 accuracy/agreement/usefulness/retrieval 的含义。普通意见不调用模型、不改文稿、不推进 generation。明确更正通过文稿 owner 并与 resolution 同事务提交。

## 性能与并发

相同视图的并发查询共享准备结果；未变化文稿复用投影单元，Native 完整输入未变化时复用组件。

跨多个 Namespace/profile 的读取相互独立，索引/Native 加载仍由 Wave 持有。本轮不改变 quality_score、ranking_score、RRF 或文稿分组 top_k 的含义。

## 完成行为

四项反馈不发生五次完整 snapshot 读；轻量 status/describe 不打开 Native；查询反馈准确指向旧版本；长文加八篇短文仍返回九篇分组文稿；保存后维护失败的可读结果与 current freshness 分开。
