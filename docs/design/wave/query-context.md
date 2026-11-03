# 上下文、核心概念与临时线索

## 查询输入

```text
WaveQuery:
  snapshot
  text
  context[]: {key, role, sequence, content | text, weight?, time?}
  core_concepts[]
  cues[]: {label, description, is_core}
  candidate_filter?
  top_k, mode, include_trace, include_counterevidence
  thinking_stages?
```

自然语言 text 是完整的一等查询；不要求用户先取得概念 ID。context 的 role 可以是 user/assistant/work/ego/source 等可解释通道，属于检索相关性输入，不具有模型指令权限。sequence 决定顺序；不需要真实时钟才能衰减。重复包含当前 query 的历史项由明确 key/source 去重，不能按文字相似擅自删句。

core_concepts 解析为当前快照稳定 Concept。cues 可以是完整描述，调用 Embedding owner 按 Concept 的 document 编码规则取得 ghost 向量，不强制插入正式概念库。不在图内的 core/cue 按 VCP ghost 融合合同保留，不能编造边或持久节点。

## 上下文数值

实现一项明确、可关闭的 Nous 上下文 profile：按 role 分组，对每组最近 window 条归一化向量做权重 `w_i = explicit_weight_i × decay^age_i`，结果归一化；再按配置 role_weight 合成 context 向量。零权重组略过，零范数明确诊断。

explicit_weight 缺省为 1，必须为有限非负数；未知 role 保留名称并使用配置的 default_role_weight=1。默认 window=10、decay=0.75，role_weight 为 user=1、assistant=0.5、work=1、ego=0.25、source=1；这些是本轮起步设计值，全部可配置，效果由实际使用决定。没有 context 时 effective_query 与原始 query 一致。提供 context 时：

```text
effective_query = normalize((1-context_mix) * normalize(raw_query)
                            + context_mix * normalize(context))
```

默认 context_mix=0.2，可设 0。query/raw、角色聚合、effective_query 均可在诊断展开。权重影响搜索线索，不更改候选内容、证据根、事实边或认知置信度。

上下文与原查询按空间的 query 用途编码；持久 Concept 按 document 用途编码。Native 接收上下文矩阵与序位并完成数值聚合；Python 负责选择片段、空间校验与实际向量生成。上游 ContextVectorManager 的角色衰减与语义分段功能一并提供为可选 `segment` 操作：按相邻向量相似度阈值分段、返回归一化均值与原范围。禁止拷贝其模糊向量复用或默认去 Emoji 清洗；Nous 精确输入与社会符号保留。

## 到 VCP 的映射

VCP `queryVector` 使用本次 effective_query；它在该次 VCP 内部称 original query，随后还会产生 enhanced/local/transfer 向量。Nous raw_query 另存，避免与 enhanced 混淆或重复施加上下文增益。

`queryText` 使用实际查询文字及明确可见的字面 cue 名称，不塞全部历史文本干扰字面门控。已知 core_concepts 映射正节点 ID；临时 cues 按顺序携带 label/vector/is_core；ghost metadata 与矩阵行必须一一对应。

无词项 query 但提供 core/cue/context 是合法的明确检索操作：有 context 时用其非零归一化聚合作为 effective_query；只有 core/cue 时将其明确描述按 query 用途编码，使用这些 query 向量的非零归一化均值作为 effective_query（ghost 的 document 向量仍单独保留）。raw_query 保持缺失，并记录 query_origin=context 或 concepts。所有贡献互相抵消为零时返回明确输入错误，不补随机/全零向量。需要向量的模式若全部信号为空则返回输入错误。纯 lexical 不强制模型调用。

## 时间和过滤

候选过滤由 Python 对精确内容/来源组使用现行 TimeDomain 与 match_range 计算，再传允许行集合。多个时间条件必须命中同一声明/来源事实组；unknown 默认不作为精确 matches，显式设置可保留并标记。

“最近”需要对应域当前坐标；虚拟时间不从真实 UTC 猜测。完整 NousQL 的 `$asof` 及认知知情历史尚未接入，当前接口不冒充其全部语义。

本快照定义可参与推理的 evidence scope。candidate_filter 只限制返回对象。若调用者要求更窄的证据/隐私/历史视图，使用据该证据范围重建的快照；不能仅过滤最终结果并宣称隐藏材料没有影响几何与传播。Native 的输入合同支持具体来源子集构建，后续权限 owner 直接供给这些资产。


concept-only 查询的内部聚合键分别使用 core/cue 的稳定序号，重复 label 的临时 cue 保留各自贡献；ghost 的 label 与调用者顺序保持。Thinking 的公开 route 以 source、target、previous 返回快照 Concept key，null 保留，flow/hop/wormhole 保留数值含义。
