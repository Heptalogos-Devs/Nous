# 按文稿召回与交付候选

## 查询语义

正式认知查询的 top_k 表示最多返回多少篇不同文稿，文稿中再给出有限个有用片段。实验 Wave 可以显式选择 candidate 粒度。二者都是当前产品操作，由同一检索核心实现。

```text
ResultGrouping:
  by: candidate | document
  top_k: positive integer
  passages_per_group: positive integer

CandidateMetadata:
  group_key / group_row
  exact_document_revision
```

group_key 在构建时固定；来源根、物理 Artifact 和展示标题都不能替代它。同一正式修订的 whole 和局部候选属于同组；不同 Journal 即使 bytes 相同也不合并。旧快照保持自己的分组信息。

删除 CognitiveRetrieval.query 的 `top_k * 4` 与本地去重。Python 不再按质量分重新排序，不根据输入长度增加补偿检索。

## 召回之前执行分组政策

只在最终 hits 去重无法找回已被候选截断挤掉的短文稿。本轮在 `native/retrieval` 的公共 recall 中应用分组政策，六种模式和高级 CandidatePool 复用。

1. 每条启用通道具有明确的候选工作量与组目标。配置的 lane limit=0 仍然关闭该通道。对于 document 模式，该通道目标是其配置允许的不同组数，同时限定每组最多保留 passages_per_group 个候选；有效单位在 QueryContext/diagnostics 标明。candidate 模式继续按候选数量解释。
2. lexical 使用 Tantivy 公共 Collector/SegmentCollector 按 group fast field 聚合最大得分与前若干片段。段间按真实得分归并，不能用访问顺序抢占名额。无需重写 BM25。
3. Dense/TagMemo/场向量使用同一个 USearch 索引。先获取有序批次，保留每组有限最佳候选；已达到本通道片段限额的组从后续过滤集合排除，再请求剩余组，直到达到组目标、集合耗尽、取消或明确工作量上限。过滤集合由本通道已完成一批后确定，不能在 ANN 回调中按访问顺序计数。返回访问/覆盖/是否截断；不在 Python 反复 search，也不增加第二个向量索引。
4. direct_anchor 使用相同组收集器；按真实通道依据排序，同分按稳定键。不要按内部 row 顺序直接 take 后宣称是最佳锚。
5. 合并通道后，先保证每个被选组的一项代表，再分配其余片段额度。总候选上限保持明确；若配置不足以容纳请求组数，返回实际原因/有效上限，不伪造“已经完整覆盖”。

现有 `query_k/bm25_k/...` 在 document 模式中解释为该通道的组目标；`max_union_candidates` 仍是实际送往读出的候选上限。因此给定 top_k 超过可容纳组数时，在查询准备报参数约束或明确截断。该单位变更写入配置说明与有效参数摘要，不静默改变历史快照的数据。

## 固定候选池与 VCP

组选取结束后，把保留行及每路真实 score/rank 组成固定 CandidatePool。VCP 使用固定池校准/读出，不在内部再次按单篇候选额度截断，把同样的组挤掉。调用方提供 CandidatePool 时只在所给池内工作，绝不暗中额外召回。

候选的 B/P/G/A、quality_score、ranking_score、条件 Wormhole RRF、DTSC 各自公式不变。分组改变的是参与候选与交付粒度，不把多段分数相加成文稿的额外证据。最终文稿排名使用本次最终排序中最靠前的实际候选；每篇保留该候选及随后合格片段。片段重叠可用已有 exact range/近重复策略消除，但不得替换该组最佳排名。

同一观测继续支持 Thinking；不为了按组交付重新执行 Sense 或把质量分当排序键。返回组内每个片段自身分数与全局候选排名，外层另给文稿排名。

## 返回类型

```text
SearchGroup:
  group_key, revision_ref, title?
  representative: exact candidate ref
  passages[]: exact selection + scores + candidate rank
  group_rank

SearchCoverage:
  requested_groups, returned_groups
  eligible_groups_known?
  recall_exhausted, truncated, reason?
```

请求十篇而库中只有五篇匹配，返回五篇是正常结果。库中有五篇匹配却因一篇长文的片段占完初始池只返回一篇，是本轮必须消除的行为。ANN 的近似召回仍保持其真实性质；没有计算全部组就不能报告 exhaustive。

## 具体迁移

`native/retrieval/src/grouping.rs` 统一组内保留及先代表后片段的分配，`lexical/grouped.rs` 实现 Tantivy collector，`query/recall.rs` 接通 lexical/dense/anchor 与通道合并，`query.rs` 负责同一次观测和最终交付。Candidates 的 metadata_json 固定 `group_keys`、每候选 `group_rows` 和 curve documents；PyO3 透传这些元数据，Native 在构建时校验并随快照保存。Tantivy 每行保存 group fast field。组号与 source roots 完全分离。

`WaveQuery(grouping="document", passages_per_group=2, top_k=10)` 请求最多十篇文稿，每篇最多两个片段；`grouping="candidate"` 保留实验逐候选查询。CLI 使用 `nous tools wave query "SNAPSHOT" "TEXT" --grouping document --passages-per-group 2 --top-k 10`。准备查询时先校验组目标能容纳在 max_union_candidates 内，然后取得查询向量。

Wave 返回 `groups` 和用于逐候选解释的 `hits`。每组给出 group_key/group_rank、representative 的 snapshot/key、revision_ref、可空 title 和完整 passages；每个 passage 保留本次最终读出的 candidate_rank、质量分、排名分、精确选区及该选区的完整原文 text；snippet 单独提供最多 500 字的界面摘要。`context_summary.grouping` 记录有效粒度、组数、片段数、通道单位和 group_scan_limit。`coverage.lanes` 的 examined_rows 是该通道实际检查的候选行数；ANN 不提供内部图访问数，不用它冒充全库覆盖。各通道另给 recall_exhausted/truncated/reason 和确知的 eligible_groups_known。

Native 构建配置包含 `candidate_grouping=document`，正式当前投影会依现有 profile 失配机制重新构建。没有 group rows 的内部旧 Native 资产需重建；正文和业务修订继续由 cognition 保留。

高价值通路采用用户报告的一篇约 246 KB 长文＋四篇相关短记录：先按真实匹配词项确认五组存在，再运行各模式。默认组预算足够时 top-k=10 应交付所有五组，并保留长文的有效片段；过滤后的合法组数另报。不要为这个样本增加特定内容规则。

## 有界候选扫描与实际名次

向量通道每批结束记录已检查行集合及已饱和组；下一批同时排除已检查行，避免部分组不足 passages_per_group 时反复得到同一候选。没有新增行时结束。每通道遵循 group_scan_limit；总 deadline/取消继续由现有执行与 Native token 控制。不能创建独立线程池或一个查不到就无限加 k 的循环。

通道返回的 rank 明确是其**本次分组候选输出**中的顺序，附 rank_scope=grouped_lane；保留原始 score，不能伪称已经计算全库逐片段排名。candidate 模式沿原候选排名合同。合并按组的最佳通道名次选择组，先取每组代表后再补片段，多个片段不会额外投票把所在文稿抬高。

`query.rs::recall/prepare_pool/ranked` 对 document 模式调用现有 VCP `prepare_pool(..., fixed=true)`，校准后的 selection 必须覆盖这份已选定的固定池；无另外一轮按 query_k/bm25_k/union 数值淘汰。高级调用者的固定 CandidatePool 仍按原语义工作。返回文稿组之后，质量、排序键和 Thinking 均读取同一次真实计算。
