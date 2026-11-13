# Rust 检索核心与调用接缝

## 真实核心

`native/retrieval` 拥有 Engine、Snapshot、资产编排、候选通道、融合、近重复折叠、反证邻接和 Thinking 组织。`native/vcp` 提供固定上游数值机制及明确的 Nous 扩展。`native/python` 只转换输入/输出、释放解释器和持有 Rust 对象，不定义检索业务。

实现位置：engine.rs 负责有界池和实际工作生命周期；snapshot.rs 负责冻结数组校验、数学资产、磁盘构建与重开；query.rs 负责观测、候选规划、固定池读出、反证和 Thinking；bundles.rs 定义批量数组值，context.rs 负责上下文聚合与分段。已有 lexical/、vector/ 保持深模块。某项变大时按职责拆分，不机械按 VCP 名词创建空 service。

Rust 核心无 pyo3/numpy 依赖；Python bundle 转成核心自有结构后调用。矩阵只在边界取得一次自有内存或明确的只读映射，query 不重传常驻大矩阵。使用现有 Rayon、Tantivy、USearch、serde，不另造调度器或 ANN。

## 接口

```text
Engine.build(FrozenAssets, BuildOptions, component_path, cancel) -> BuiltSnapshot
Engine.open(published_path, expected_identity, cancel) -> Snapshot
Snapshot.execute_program(program, query_signals, allowed, cancel) -> NativeQueryResult
Snapshot.observe(QuerySignals, ObserveOptions, cancel) -> Observation
Snapshot.think(Observation, Stages, ThinkingOptions, cancel) -> ThinkingResult
```

查询程序由类型化算子连接而成。Native 在同一快照借用内执行数值子图；相同的确定性节点共享执行结果，RiverMemo 和 DTSC 的读出消费图中传入的候选与观测。Python 完成原文映射、可选评价和预算选择。算子端口和配置见 [Wave 程序](program.md)。

CandidateSet 绑定 snapshot/space；条目保存 row、lane 原始分/名次和来源位。CalibratedPool 同时绑定其观测、候选和校准参照。外部提供候选时，它们必须对应当前快照已声明的正文和向量；新增候选需要新快照。重复或未知 row 返回明确错误。空 allowed 是空集合，None 才是全部。

Observation 保留实际查询向量、核心概念、临时 cue、河网、双场与实际 observe 参数。readout 不要求调用者重新传一份 core_rows，不能因漏传而失去直接锚；也不以 readout 的默认配置重算观测。

## 通道规划只有一个定义

程序显式连接 query/denoised/local/transfer/BM25/direct-anchor 等召回节点，各路使用自己的预算，预算为零时不执行该路计算。候选联合的预算和最终材料组数分别指定。

候选合并保留各路名次，按既有 union 规则和明确上限选择，之后一次读出。VCP 原生 select_superset 与外层规划合并为一项合同，不在两处做不同截断。达到上限报告实际规模与截断位置。

直接锚从同次观测的明确 core/lexical/seed 集合通过倒排取得；能被数值 readout 认可的直接锚必须有相应召回入口，不能仅扫描用户 core_rows。局部场候选也保持独立召回。

RiverMemo 已包含内部分数和条件 RRF；hybrid 不把同一组内部 lane 再投一次票。质量分、排序键、最终排名保持不同字段。

## 稀疏输入与长序列

图的节点集合包含已声明 Concept，即使该节点无边。空传输图保留概念向量、core/ghost 融合与直接接触；传播/拓扑奖励据实为零，EPA 无可用基时明确 unavailable。不得伪造边、零向量或全库 Tag。

移除 geometry、fact matrix、provenance 三处 100 occurrence 静默跳过。完整有序序列按当前 VCP 的实际位置公式处理；超过 100 时也纳入全部 occurrence，保留重复但不产生自环。无序序列用对称共现。

默认完整计算；需要计算限制时使用显式可配置的 pair_work_limit，超限在发布前返回工作量和具体序列，不静默截去尾部或整段。可以按明确的语义路径拆分输入，但系统不每 100 项自动断开并遗漏跨界关系。<=100 的既有原始输入保持同一数值规则和累计顺序。

## Thinking、DTSC 与错误

Thinking 使用同次完整前驱记录。请求需要记录时，在 observe 规划中明确 record_transitions 与有界记录额度；配置禁用时返回 `thinking.records_disabled`。不能暗改 0 为 usize::MAX。达到额度标 truncated，Thinking 可用状态说明缺少路线；取消、非法参数和内部错误仍传播。

DTSC 是独立读出算子，接收同次观测和明确候选集合，保留自己的几何、低支持行为及分数单位。

NativeError 使用 code/message/details，错误在了解含义的模块构造；不把 details 固定为空，也不靠匹配字符串决定 code。预期无结果正常返回空 hits；数值失败不能伪装为没有记忆。

## 快照和执行

build 在稳定组件目录内关闭 writer 和构建期 reader，再原子写入完成清单并返回 BuiltSnapshot 的身份和文件信息。Wave 以完整冻结输入、行映射、空间和构建配置的签名共享不可变组件；签名变化时重新构建词项索引、ANN 与 VCP 数学资产。open 校验身份与资产，不重新 Embedding 或重建数学资产。

所有 CPU 工作使用同一个有界 Engine pool，外层 Execution.blocking 传入取消 Token 并等待真正退出。Tantivy/USearch 的内部线程纳入同一资源目标。反证邻接、概念到候选倒排和必要范数在快照构建/打开时准备，查询不对每个 hit 重新扫描全部候选及全部关系。

Python QueryInput 支持文字、上下文、概念线索，以及显式 VectorRef(space_definition, purpose=query, artifact,row)。通过 Embedding owner 读取已有向量，可离线执行；Raw 数组的高级调用同样要求空间身份和用途。

## 候选校准与原数值次序

VCP 校准保留“授权池内归一化→通道联合选择→曲线读出”的计算次序、BM25 参照和原公式。`vcp.calibrate` 在明确的候选总体上计算一次校准；RiverMemo 读出消费对应的 CalibratedPool，不再次召回或重算参照。

不能在截断后重算参照导致普通 search 与同池 readout 的质量变化。外部 lane 分数标明来源与单位，无法与已知 lane 合并的分数只作独立诊断/融合输入，不冒充 BM25 或余弦。
## 数值范围与取消

长序列完整两两计算保持原位置公式，取消检查放在有界的 pair 批次内，不能只在整段结束后响应。共享 pair 迭代责任覆盖几何、事实和 provenance，三处不得对同一序列采用不同静默截断。该共用迭代仅组织位置对，数值权重仍由各自数学定义计算。
