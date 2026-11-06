# 查询情境、交付结果与效用反馈

## 当前落点

本轮建立类型化查询结果和反馈目标，保存被评价时实际使用的情境；不实现自动权重学习、不自动改写 Journal、不修改 VCP 质量分。

```text
QueryContext:
  request_text
  selected_context[]             # 本次实际参与的片段，角色、顺序、权重、精确来源
  omitted_context_count
  namespace_id
  snapshot_ref: SnapshotRef(instance_id, id, identifier)
  captured_generation, profile_signature, space_signature?
  mode, effective_query_options
  filters, core_concept_refs[], temporary_cues[]
  purpose?                       # 调用者有提供时

QueryResult:
  context: QueryContext
  groups: SearchGroup(revision_ref, representative: CandidateRef, passages)[]
  delivery: DeliveredSelection(revision_ref, selection, candidate_ref, content, group_rank, candidate_rank)[]
  coverage, operation_context
```

`CognitiveQuery`、`QueryContext` 和 `QueryResult` 位于 `cognition.retrieval.contracts`。正式 `query` 返回 Pydantic QueryResult，使用属性读取并通过 `model_dump_json`/`model_validate_json` 固定或恢复。SearchPassage 的 selection 保留正式修订归属，其余字段提供 candidate_ref、content、origin_contents、完整原文 text、CLI 摘要 snippet、排名、通道、证据根与概念引用；文稿 sources 使用完整 SourceBinding 多选区值。

QueryContext 由共同 query preparation 的实际选择生成。不能仅回显请求中后来被窗口裁掉的全部上下文，也不能只存 mutable WorkContext ID。纯 query 无上下文时这些字段为空，不要求伪造工作状态。Native aggregate_context 在实际窗口、衰减、角色权重和向量有效性处理后给出选中的 fragment keys；Wave 将它们映射成含确切 ContentSelection 或原文字的 SelectedContext，并记录实际 decayed_weight/role_weight。lexical、禁用 context 或 mix=0 时不把未参与片段列入 selected_context；effective_query_options 保存真正使用的检索及 context 参数。

`delivery` 表达当前 API 实际返回的精确版本与选区。它不声称主 Agent 已经阅读或采用。今后 Steward 的选段、Host 的实际编入输入和用户的使用评价分别提供自己的事实，不能把未交付候选当负反馈。

## 类型化所属范围

Wave 的 CorpusView/快照提供 `owner`：manual 或 cognition，cognition 时明确 namespace_id。用于权限/归属的字段由正式构建者给出并保存；collection name 只用于命名。

CognitiveRetrieval 和 FeedbackOperations 通过 Wave 的类型化 descriptor 读取 owner。`WaveOperations.describe(str | SnapshotRef)` 返回 SnapshotDescriptor，其 ref 固定 instance_id/id/identifier，owner 使用 manual/cognition 判别类型；`describe_candidates(snapshot_ref, keys)` 一次读取完整候选映射，按请求顺序返回 exact content、origin_contents、group_key 和冻结 metadata。带完整 SnapshotRef 时校验所有身份字段。描述操作只读轻量列，不加载 Native；见[快照读取边界](../wave/snapshot-read-boundary.md)。

## 反馈保存

QueryResult 默认是返回值。反馈保存 QueryFeedback：QueryContext、实际 delivery 顺序和 operation_context，以及所评价的目标选区。候选原文、完整 diagnostics 与可从精确快照读取的内容留在各自 owner。

`FeedbackOperations.record_query(result: QueryResult, category, *, target?, value?, comment?)` 校验正式 Namespace、快照与实际交付目标；CLI --query-result 反序列化同一公共类型。记录反馈不调用模型、不改变正文或 generation。

用户评价某一候选时，使用当时的 exact revision/selection，而非提交时重新解析 logical ref 到最新。需要比较两份结果时，保存实际交付顺序与正负对象；暂不从点击推断效用。

## 未来的研究责任

情境性效用可能影响同一 Work 中的后续检索、重排或候选过滤。反馈研究须区分内容错误、是否赞同、此时是否有用和目标任务是否完成，并考虑展示位置与实际暴露。

VCP 的 quality_score 保持其数学含义；未来效用调整使用自己的受作用域约束的排序贡献，不把“这次有用”写成全局真实性概率。LongMemEval 的知识更新/时间问题和显式检索反馈研究可提供实验题型，不能取代 Nous 自己的真实使用材料。

本轮只保存可用的 context/target，并让意见可查询、可关联处理。WorkContext、Steward 和自适应排序是后续设计，见路线计划。
