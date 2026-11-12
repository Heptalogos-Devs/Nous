# Wave 检索能力与 Native 边界

## 本轮交付

Wave 从明确的候选内容、Concept、来源和关系构建可重开的检索快照，支持词项、Dense、TagMemo、RiverMemo、混合排序、DTSC、语义近重复折叠和 Thinking。上下文、核心概念与临时线索进入同一个查询计划。结果可回读当前 Nous 的精确原文与来源。

完整 VCP 指当前采用版本中的检索数值闭包：几何、资产、查询观测、双场、候选曲线/拓扑/Ω、条件虫洞排名、Thinking 以及相关输入处理。上游的 Node 插件调度、日记文件管理、SQL 权威和兼容门面不迁入。

## Python 和 Rust

Python WaveOperations 负责接纳清单、解析正式引用、调用 Embedding、确定数据与时间范围、发布快照、结果读取。Rust 负责索引和密集/稀疏数值计算，不拥有认知对象，也不访问业务数据库。

```text
build(manifest, space) -> WaveSnapshotRef
query(snapshot, query) -> WaveResult
read(snapshot, candidate_ref) -> 精确内容及来源
inspect(snapshot) -> 能力、空间、来源规模、构建配置
```

这些是共同应用操作，CLI 仅适配。Native 自身的 observe/rank/think 用于计算复用，产品不逐项暴露内部方法为工具。

## 概念层次

Concept 是有名称、完整描述、别名和向量的检索线索。认知领域拥有正式 Concept 身份和意义修订；Wave 清单也可以提供构建所需的精确概念描述。

VCP Tag 对应 Concept 的计算节点；原来的短 Tag 名保留为 label/alias，向量可以来自完整描述。VCP file 是有序出现的事实单位，candidate/chunk 是召回单位；两者与 Nous Artifact/Journal 没有强制一一对应。

VCP QueryObservation 是一次数值计算的观测。它与 Nous 将来的 Observation 接触事实分开。VCP anchor_gain/direct_anchor 是数值机制；Nous Anchor 的长期中心语义由领域与清单关系提供。Thinking 返回有真实传播前驱的候选路线，不替代语言推理、自由 Dream 或执行工作流。

## 本轮扩展

实现描述型概念、按角色/顺序组织的上下文、多来源候选、typed relation 投影、反证关系读取、域内时间过滤和显式的研究模式。完整使用既有来源、向量与时间 owner；不引入新的通用 Cognition 表。

认知吸积的自动合并与拆分、长期反馈学习、全量 NousQL 和多用户授权属于后续能力。当前后台维护由 Runtime 持有普通任务。

## 本轮结构收敛与后续认知

当前结构和工作范围见 [架构重构计划](../../plans/architecture-rework.md)。检索核心位于 retrieval crate；PyO3 为薄边界。Note/Journal 的正式修订和活动标注进入当前视图；正文、多路径和支持组见[投影](journal-projection.md)，语义时间见[时间视图](temporal-views.md)。
