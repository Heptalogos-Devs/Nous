# 候选通道、质量与排名

## 多路召回

lexical 使用 Tantivy/Jieba；dense 使用相同正式空间的 query/document 向量与 USearch。TagMemo 产生增强向量与相关概念；RiverMemo 消费 original/enhanced/local/transfer、词项、直接锚等候选并集，完整执行其读出。

保留候选原来来自哪些通道及各通道名次。各通道预算、候选联合预算与最终组数分开；候选联合、显式校准与读出为独立算子，读出不再次召回。

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

`rank.fuse` 在文档分组时让每个输入排名对同一材料组至多贡献一次，未出现在某一路的组在该路贡献为零。等权 RRF 会奖励跨路重合：例如 k=60、两路各取前20组时，两路都排第20的组得分为 `2/80=0.025`，高于只在一路排名第1的 `1/61≈0.01639`。这种次序来自名次政策，是否有利于某项任务取决于该任务的候选与权重。

重复引用同一个排名节点只计算一次；不同节点即使来自同一观测或具有相关输入，仍按各自声明权重贡献，不因此成为独立事实支持。`ProgramRunFacts` 保存实际参与节点的输入引用、展开后的权重和 k，必要时可沿图查看共同祖先；详细候选读出由 trace 保留。

默认 dense 使用原问题向量召回；有明确检索情境时，情境词项分支按实际质量贡献融合。缺少向量时入口按配置使用 text_only。hybrid、research、RiverMemo、DTSC 和 TagMemo 作为可配置程序。融合记录其输入来源和权重，程序名和 score_kind 分别表示执行配方和实际数学单位。

外部 RRF 的 score_kind 为 rrf，并保留各 lane 的原值和名次。纯词项、向量、RiverMemo 和 DTSC 返回各自的尺度；多种单位不同的分数不直接相加。

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
