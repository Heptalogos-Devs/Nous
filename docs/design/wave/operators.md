# Wave 算子

程序使用带类型的端口连接共同数值阶段。每次执行固定快照、空间和允许返回的候选；行号只在快照借用内使用。

| 算子 | 主要输入 → 输出 | 职责 |
|---|---|---|
| `vcp.observe` | 查询信号、快照 → 观测和 enhanced/local/transfer 向量 | 一次 VCP 传播与场计算 |
| `ann.search` | 向量、快照、范围 → CandidateSet | 已有向量索引召回；候选组内上限和搜索扩展可分别配置 |
| `lexical.search` | 问题词项、快照、范围 → CandidateSet | Tantivy 词项召回 |
| `context.recall` | 检索阶段情境、快照、范围 → CandidateSet | 按父材料质量独立召回情境词项候选 |
| `concept.direct` | 观测、快照、范围 → CandidateSet | 直接种子到候选的倒排召回 |
| `candidates.union` | CandidateSet[] → CandidateSet | 精确身份联合、lane 事实与组公平预算 |
| `vcp.calibrate` | 观测、CandidateSet → CalibratedPool | 固定校准总体与 BM25 参照 |
| `river.readout` | 观测、CalibratedPool → RankedSet | RiverMemo 质量与排序键 |
| `dtsc.readout` | 观测、CandidateSet → RankedSet | DTSC 独立读出 |
| `rank.from_lane` | CandidateSet → RankedSet | 将指定 lane 的原始名次转成排名 |
| `rank.cosine` | 查询向量、CandidateSet → RankedSet | 在同一候选集计算真实余弦，不进行额外召回 |
| `rank.context` | RankedSet、情境候选 → RankedSet | 将情境候选按其实际贡献合入排名 |
| `rank.fuse` | RankedSet[] → RankedSet | 带权 RRF；同一输入节点不重复投票 |
| `vcp.think` | 观测、阶段输入 → 路线 | 复用同次观测生成显式探索路线 |
| `passages.locate` | 排名、问题、快照 → PassageSet | 对入选组内的实际局部单元评分 |
| `passages.evaluate` | PassageSet、问题 → PassageSet | 共同模型评价按 key 添加自身尺度的判断 |
| `passages.select` | PassageSet、交付预算 → EvidenceSelection | 按相关性、可用预算及实际来源范围选择 |
| `delivery.read` | EvidenceSelection、材料范围 → EvidenceDelivery | 返回精确原文和组级事实 |

RRF 使用 `Σ weight/(k+rank)`；文档分组时同一路最多对同一组贡献一次。共享观测的多个下游算子不重复运行观测。候选联合只合并同一快照中的精确行，同一路重复条目保留较优名次。校准在明确候选总体上执行一次，改变总体时需要新的校准节点。

RiverMemo 的 quality_score、ranking_score 和顺序保留各自单位；DTSC、BM25、余弦和 RRF 的分值不混写到质量字段。`based_on` 记录分数来源，关联算法的多个数值通道不作为独立事实支持。

局部单元的词项和向量分别使用该单元的真实输入。局部 RRF 保留原值，并以理论最优名次作明确参照；所有入选组的局部单元共同竞争交付预算。预算选择计算实际来源范围的成本，不以向量分数代替正文或来源。
