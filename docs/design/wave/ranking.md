# 候选通道、质量与排名

## 多路召回

lexical 使用 Tantivy/Jieba；dense 使用相同正式空间的 query/document 向量与 USearch。TagMemo 产生增强向量与相关概念；RiverMemo 消费 original/enhanced/local/transfer、词项、直接锚等候选并集，完整执行其读出。

保留候选原来来自哪些通道及各通道名次。candidate_budget 与 top_k 分开；去重在明确位置完成，不能每个包装层都 truncate 一次。

## 当前 RiverMemo 评分合同

以 d5c019e 的实际 `rivermemo_wormhole.rs`、调用处和 `RIVERMEMO_RRF_SCORE_CONTRACT.md` 为准：

- `quality_score` 对应原 `final_score`，保留 clipped 四项基线 B+P+G+A。
- `ranking_score` 是实际排序键；触发严格 wormhole RRF 时使用原有最大路线贡献公式。
- 无触发/提前返回路径中 ranking_score 初始化为 quality_score。
- 排序按 ranking_score 稳定执行，同值保留基线次序。

质量分不保证随返回 rank 单调递减。用户阈值针对声明的 quality_score；UI 和其它算法不可把结果按 quality_score 重新排序。保留 B/P/G/A、截断量、Ω、路线贡献及 activation 情况，详细信息只在 explain 展开。

RiverMemo 未触发条件 RRF 时，`score_kind` 为 `rivermemo_quality`；触发时为 `rivermemo_wormhole_rrf`，表明 ranking_score 的实际单位。两种情况下 quality_score 都保留候选自身的质量分。

条件 RRF 保留当前候选落地与全局见证门控，多个路线取最大贡献；不能把所有 wormhole 权重无条件相加，也不把 RRF 解释成余弦或概率。

## 混合与自有通道

混合不同单位时采用明确的名次融合，例如 RRF `Σ weight_lane/(k+rank_lane)`。每个 lane 的重复候选先以精确 candidate identity 合并一次。已有 RiverMemo 内部条件 RRF 的结果作为一个 lane 参与外部混合；不得又把其内部同一批子通道逐个重复投票。

默认 hybrid 在完整 VCP 资产存在时采用已包含词项、原向量与场通道的 RiverMemo 最终顺序，不再给这些内部通道重复 RRF 投票；资产只有 lexical/dense 时采用两通道 RRF 并明确缺失 VCP。显式增加独立 DTSC 等 lane 时才执行额外名次融合。mode 表示操作选择，score_kind 表示实际数学单位，二者不必同名。

实际执行外部混合时 score_kind 为 hybrid_rrf，仍保留各 lane 的 quality/ranking 原值。纯 lexical/BM25、dense/cosine、TagMemo、RiverMemo、DTSC 返回自己的 score_kind。多种分数不能混入一个无单位 score 字段后直接求和。

## 近重复与来源

精确候选身份去重必做；语义近重复折叠是可配置的结果呈现能力。复用 VCP ResultDeduplicator 的批量向量/范数思路，用 Rust 完成热点余弦。缺少向量的候选保留，语义折叠不删除正式对象或独立来源。

默认按最终排名保序选择代表，记录 folded_refs 及相似度。相同文字、不同来源仍可在 sources 中展开。来自同一根的多份派生不能充当独立支持，但有独特内容的候选仍有资格返回。

## 返回

```text
WaveHit:
  rank, candidate_ref, content_ref, selection
  quality_score?, ranking_score, score_kind
  channels, concept_matches, evidence_roots
  folded_refs, counterevidence_refs
WaveResult:
  snapshot_ref, hits, query_summary, algorithm_versions
  scope, coverage, diagnostics?
```

用户默认看到正文片段、名称、相关概念和短引用。图、矩阵、前驱记录和全部参数在 inspect/explain 中展开，不把内部全部字段塞进普通工具返回。
