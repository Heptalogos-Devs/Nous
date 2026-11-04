# Rust 检索核心与调用接缝

## 真实核心

`native/retrieval` 拥有 Engine、Snapshot、资产编排、候选通道、融合、近重复折叠、反证邻接和 Thinking 组织。`native/vcp` 提供固定上游数值机制及明确的 Nous 扩展。`native/python` 只转换输入/输出、释放解释器和持有 Rust 对象，不定义检索业务。

实现位置：engine.rs 负责有界池和实际工作生命周期；snapshot.rs 负责冻结数组校验、数学资产、磁盘构建与重开；query.rs 负责观测、候选规划、固定池读出、反证和 Thinking；bundles.rs 定义批量数组值，context.rs 负责上下文聚合与分段。已有 lexical/、vector/ 保持深模块。某项变大时按职责拆分，不机械按 VCP 名词创建空 service。

Rust 核心无 pyo3/numpy 依赖；Python bundle 转成核心自有结构后调用。矩阵只在边界取得一次自有内存或明确的只读映射，query 不重传常驻大矩阵。使用现有 Rayon、Tantivy、USearch、serde，不另造调度器或 ANN。

## 接口

```text
Engine.build(FrozenAssets, BuildOptions, staging_path, cancel) -> BuiltSnapshot
Engine.open(published_path, expected_identity, cancel) -> Snapshot
Snapshot.observe(QuerySignals, ObserveOptions, cancel) -> Observation
Snapshot.recall(observation_or_query, RetrievalPlan, allowed, cancel) -> CandidatePool
Snapshot.readout(Observation, CandidatePool, ReadoutOptions, cancel) -> QueryResult
Snapshot.think(Observation, Stages, ThinkingOptions, cancel) -> ThinkingResult
Snapshot.search(QueryRequest, cancel) -> QueryResult
```

`search` 是上述操作的完整组合，最多一次 observe；readout 消费给定池，不再 ANN/BM25。公开 Python binding 同时提供 search 与高级 observe/readout/think。无需额外进程、HTTP 内核或内部命令总线。

CandidatePool 绑定 snapshot/space；条目保存 row、lane 原始分/名次和来源位。外部提供时间/实体等候选时，它们必须对应当前快照已声明的正文和向量；新增候选需要新快照，不临时伪造行号。每个 row 保存合并后的 lane；重复或未知 row 返回明确错误。空 allowed 是空集合，None 才是全部。

Observation 保留实际查询向量、核心概念、临时 cue、河网、双场与实际 observe 参数。readout 不要求调用者重新传一份 core_rows，不能因漏传而失去直接锚；也不以 readout 的默认配置重算观测。

## 通道规划只有一个定义

RetrievalPlan 持有每路 enabled、k 和合并上限。query/denoised/local/transfer/BM25/direct-anchor 各按自己的 k 获取候选，k=0 不执行对应工作。移除现有独立 candidate_budget；使用各 lane K 与唯一 maxUnionCandidates，参数目录、帮助和消费者一起迁移。

候选合并保留各路名次，按既有 union 规则和明确上限选择，之后一次读出。VCP 原生 select_superset 与外层规划合并为一项合同，不在两处做不同截断。达到上限报告实际规模与截断位置。

直接锚从同次观测的明确 core/lexical/seed 集合通过倒排取得；能被数值 readout 认可的直接锚必须有相应召回入口，不能仅扫描用户 core_rows。局部场候选也保持独立召回。

RiverMemo 已包含内部分数和条件 RRF；hybrid 不把同一组内部 lane 再投一次票。质量分、排序键、最终排名保持不同字段。

## 稀疏输入与长序列

图的节点集合包含已声明 Concept，即使该节点无边。空传输图保留概念向量、core/ghost 融合与直接接触；传播/拓扑奖励据实为零，EPA 无可用基时明确 unavailable。不得伪造边、零向量或全库 Tag。

移除 geometry、fact matrix、provenance 三处 100 occurrence 静默跳过。完整有序序列按当前 VCP 的实际位置公式处理；超过 100 时也纳入全部 occurrence，保留重复但不产生自环。无序序列用对称共现。

默认完整计算；需要计算限制时使用显式可配置的 pair_work_limit，超限在发布前返回工作量和具体序列，不静默截去尾部或整段。可以按明确的语义路径拆分输入，但系统不每 100 项自动断开并遗漏跨界关系。<=100 的既有原始输入保持同一数值规则和累计顺序。

## Thinking、DTSC 与错误

Thinking 使用同次完整前驱记录。请求需要记录时，在 observe 规划中明确 record_transitions 与有界记录额度；配置禁用时返回 `thinking.records_disabled`。不能暗改 0 为 usize::MAX。达到额度标 truncated，Thinking 可用状态说明缺少路线；取消、非法参数和内部错误仍传播。

DTSC 保留独立模式与原数学合同。转换时类型化传递 EPA/Pyramid 几何，删除 unwrap_or_default；算法本身定义的低支持行为继续保留。

NativeError 使用 code/message/details，错误在了解含义的模块构造；不把 details 固定为空，也不靠匹配字符串决定 code。预期无结果正常返回空 hits；数值失败不能伪装为没有记忆。

## 快照和执行

build 结束时所有 staged 文件和 writer 已关闭，只返回 BuiltSnapshot 的身份、规模及文件信息。Python 发布目录后 open 一次并登记数据库。open 校验必要身份与资产，不重新 Embedding 或重建数学资产。

所有 CPU 工作使用同一个有界 Engine pool，外层 Execution.blocking 传入取消 Token 并等待真正退出。Tantivy/USearch 的内部线程纳入同一资源目标。反证邻接、概念到候选倒排和必要范数在快照构建/打开时准备，查询不对每个 hit 重新扫描全部候选及全部关系。

Python QueryInput 支持文字、上下文、概念线索，以及显式 VectorRef(space_definition, purpose=query, artifact,row)。通过 Embedding owner 读取已有向量，可离线执行；Raw 数组的高级调用同样要求空间身份和用途。

## 候选校准与原数值次序

迁出 VCP select_superset 时保留其“授权池内归一化→通道联合选择→曲线读出”的计算次序、BM25 参照和原公式。普通 recall 在完整召回池上完成一次校准/选择，CandidatePool 保留需要的参照；readout 不再重做选择或截断。高级调用提供的最终池经同一校准规则准备，候选集合由调用者明确给定。

不能在截断后重算参照导致普通 search 与同池 readout 的质量变化。外部 lane 分数标明来源与单位，无法与已知 lane 合并的分数只作独立诊断/融合输入，不冒充 BM25 或余弦。
## 数值范围与取消

长序列完整两两计算保持原位置公式，取消检查放在有界的 pair 批次内，不能只在整段结束后响应。共享 pair 迭代责任覆盖几何、事实和 provenance，三处不得对同一序列采用不同静默截断。该共用迭代仅组织位置对，数值权重仍由各自数学定义计算。
