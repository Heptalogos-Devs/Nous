# VCP 输入、计算与 Nous 映射研究

## 真实材料与采用决定

Nous 源码基准 `fdb39e433126abe94d3231292eabb0c857772ec2`；旧 Native 来源记录固定到 VCP `d1b40c863273cbbba082fb7657671424768c4a53`。本轮上游实际读取为 `d5c019ebbac1f317c2938d655d2ce916e41b9164`。对照显示两者之间存在算法更新，不能以旧目录已有 Rust 实现为完整采用证明。

本轮选择：复用旧 Nous 纯计算解耦与多根来源扩展，更新到当前 VCP 数值闭包；产品语义采用现行 Nous 内容、时间、配置与领域边界。本文件记录研究所得；正文的新 input/API 是本轮设计决定，不声称上游原本就具有 Nous 的语义。

## 两层输入必须一起接入

| 上游真实输入 | Nous 对应 | 必须保留 |
|---|---|---|
| file_tags 的 file_id/tag_id/position | 独立来源上的 Concept 有序出现 | 顺序、重复、源贡献，不由排序造顺序 |
| tags 的原始向量/名称 | 描述型 Concept 向量与 label/aliases | 正式空间、实际描述、稳定键 |
| chunks + file_id + chunk vector | 候选内容/选区、curve_document 与向量 | 召回单位与来源事实单位分开 |
| queryVector/queryText | effective_query 与实际查询文本 | raw/context/effective 各自身份 |
| coreTags | 明确核心 Concept | 显式身份与普通相关线索区别 |
| ghostTags 及 packed vectors | 临时 cue 描述与对应向量 | 顺序、is_core、图外融合语义 |
| ContextBridge/ContextVectorManager | 调用者提供 context roles/order/content | 衰减聚合、分段及来源，不带会话全局单例 |
| QueryObservation | Native 单次计算观测 | 共用 Sense、双场、向量池及版本 |
| CandidateInput 多路原始得分 | 类型化候选通道 | BM25/向量/锚/时间数值不得混单位 |
| file provenance | 候选 root set 与边贡献 | 多来源排除，派生不重复投票 |
| Thinking 的 stages/diary_name | 具名阶段与候选集合 | 真前驱、顺序、closure、完整 transitions |

原始 file_id 同时涉及曲线文档与证据排除；Nous 明确拆成 curve_document 和 evidence_roots。否则一个 A/B 总结会错误地排除一个根、又把另一个根当独立支持。

## 计算源码采用表

| 上游路径 | 本轮处理 |
|---|---|
| rust-vexus-lite/src/memo_geometry.rs | AGS 局部与全局几何的纯计算迁入 native/vcp/src/geometry |
| memo_artifact_builder.rs | ordered facts、transport 与 provenance 数值迁入；SQL/发布替换为冻结输入 |
| memo_pipeline.rs | 当前 conservative-topology-v2.2 的 EPA/Pyramid/gates/fusion/fields 迁入 |
| memo_sensing.rs、memo_projection.rs | 真实前驱传播、动态已有边路线、有限场投影迁入 |
| rivermemo_topology_v3.rs、rivermemo_wormhole.rs | 当前读出、候选曲线/Ω/条件 RRF 及 quality/ranking 分离 |
| memo_thinking.rs | 同次观测上的阶段路线，剥离 DB 与 diary 目录读取 |
| ResultDeduplicator.js | 批量近重复数值思路迁 Rust，来源身份规则按 Nous 调整 |
| Plugin/RAGDiaryPlugin/ContextVectorManager.js | 角色聚合/分段能力；不拷贝模糊向量缓存与语义符号删除策略 |
| legacy/native/vcp/baseline/dtsc | 保留有明确作用的 Nous DTSC 模式，其他退役运行壳删除 |

这是数值采用范围，不是将 VCP 的全部插件、运维和交互系统装入 Nous。开发需沿函数依赖读取所需 closure，不能只移植表中入口却漏掉其内部 helper/配置/诊断。

## 当前分数合同优先于较早概述

[Topology V3 文档](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/docs/RIVERMEMO_TOPOLOGY_V3.md)描述完整主干；[RRF score contract](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/docs/RIVERMEMO_RRF_SCORE_CONTRACT.md)与实际 wormhole 源码进一步分离 quality 与 ranking。最终数组顺序是排名，score 可以不单调。这个更新必须合入 Python呈现和混合排序。

更早的 [MEMORY_SYSTEM.md](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/docs/MEMORY_SYSTEM.md)仍描述旧 V3.7/SVD 阶段，只用于理解历史意图。不要用它覆盖当前 Rust 实现，也不要把文档中明确尚未实现的 PSR 等提案列为已实现 VCP 能力。

## 三个不可混同的名称

QueryObservation 是瞬时计算状态；Nous Observation 是真实接触。VCP anchor_gain/direct anchor 是数值机制；Nous Anchor 是长期关联中心。上游 diary 分组是候选管理边界；Nous Namespace 的授权/认知语义另有 owner。

## 新扩展的依据与限制

描述型 Concept、多来源、typed Association、可读引用和多轴时间来自当前 Nous 设计；具体上下文混合默认值、显式关系质量投影和反证读取方式是本轮工程选择。它们需要真实资料研究效果，不能从算法名称推导效果已经提升。

纯计算与索引接口保留足够输入，无需先实现 Note/Journal/Schema 等领域。当前清单的人工声明与实际模型生成产物分别记录，不将二者混成自动认知事实。

## 主要原始依据

- [统一查询输入与管线](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/rust-vexus-lite/src/memo_pipeline.rs)
- [有序事实与来源贡献](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/rust-vexus-lite/src/memo_artifact_builder.rs)
- [当前读出与独立比例](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/rust-vexus-lite/src/rivermemo_topology_v3.rs)
- [条件排序代码](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/rust-vexus-lite/src/rivermemo_wormhole.rs)
- [Thinking](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/rust-vexus-lite/src/memo_thinking.rs)
- [ContextBridge](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/docs/CONTEXT_BRIDGE.md)
- [上下文实现](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/Plugin/RAGDiaryPlugin/ContextVectorManager.js)
- [现有 Nous Native 来源](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/legacy/native/vcp/UPSTREAM.md)
- [PyO3 并行与 detach](https://pyo3.rs/main/parallelism)

VCP 移植源码按其现有 CC BY-NC-SA 4.0 保留许可和实际来源，Nous 原有独立代码保留自己的声明。实现时处理实际复制/发行文件，不建立重复核验所有无关依赖的许可流程。
