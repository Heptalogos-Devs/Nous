# Nous Wave 当前状态

[返回文档目录](../INDEX.md)

## Available

- Memory-only Subject：Subject capability、Memory/CognitiveSchema/Tag/AssociationEvidence Authority、不可变 revision、provenance、幂等和 lifecycle。
- Material：Artifact、ObservationOccurrence、SourceRegion、DerivedRepresentation DAG、结构化 payload、DerivedRegion 与可追踪派生。
- Runtime：Session、ResidentSet、UseEvent、WorkContext durable checkpoint 和跨 Session continuity；Runtime 同事务捕获 Session Experience、持久化分段 cursor/draft 和 MaintenanceNeed；Memory owner 提供 Episode/Journal revision、history、provenance 与 lifecycle。
- 纵向认知：自动 interaction Episode、模型局部 N-to-M partition、有 formation basis 的 Subject Journal 和有界 host-granted Memory/Schema consolidation；CognitiveClock 支持内部确定性时间注入。
- Query/Serving：QueryExpr、exact read、lexical/dense/runtime lanes、固定 fusion、可选 model rerank、final Authority revalidation 和可重建 generation。
- Authority mutation：Persistence envelope 统一 owner/operation locks、receipt replay、transaction 与合并 projection invalidation；领域 owner 保持独立数据模型和 mutation/lifecycle 验证。
- Core 与官方 Client/CLI：ModelRole/RolePolicy/ExecutionProfile/ModelProfile/Gateway、Prompt identity、media derivation、宿主显式供给的 External Resource adapter、trace、流式材料上传与完整下载；Memory/Schema/Episode/Journal 管理、Seed adoption、Resource registry、projection 与 managed context 均可从 CLI/MCP 操作。
- [正式 OpenClaw 认知集成](../../packages/openclaw/README.md)：Client managed binding、固定版本原生入口、逐模型认知应用、RESET、原生压缩委派、独立子 Agent 与真实 Observation/ReportUse；Subject 可无 Seed。
- Windows x64 source-less portable package 的 assembly 与 runtime installation path。

当前 Agent 接口使用非空 Unicode NousQL 意图与可选语法岛；自然语言指代不需要显式补全。WorkContext 持久化自由文本和 cognition/Entity/Tag anchors，Runtime 同事务冻结 Session/ResidentSet context snapshot，所有 retrieval profiles 共用 representation 和 activation，history 排除未来 cognition/Tag。CLI 使用 citty，默认语义文本、opt-in owned JSON、exact result:N 续接和原请求 receipt 恢复。模型 routes/controls/Prompt 冻结；逐次 fallback 计费并保存实际成功 producer 和有界累积 telemetry。Formation basis、来源 lineage 与 epistemic relation 分离；结构合法的认知错误仍可演化和纠正。

## Experimental

Topology 是默认关闭的显式 lane，包含 weighted PCA/EPA、residual decomposition、bounded propagation 与 node-potential ranking。VCP DTSC/RiverMemo reference/adapters 保留；tracked 数值 fixtures 已压缩为区分性 goldens，完整 source outputs 在 ignored Research results。Prepared Query、稳定 LexicalRef、Identity ambiguity/candidate continuation、WorkContext/Situation Representation 和共享 query embedding 已实现。Material 与 Memory 各自负责所属对象的 final materialization 和五轴时间验证；generic Persistence reference_times 已删除。

`concept_maintenance` 只围绕 exact focus，提供局部来源、aboutness 相关 Memory、Tag 名称/别名与缓存相似候选及一跳关系。Core 保存 proposal，以稳定 action id 调用 Tag/Association owner mutation；已提交结果通过 owner receipt 重放，不保存逐项进度。consolidation 同样使用 canonical Memory/Schema API。Core 按 Subject 串行执行 maintenance grant；Runtime 选择 pending need，以 trigger revision 核对完成，运行中新增的触发不被旧结果覆盖。没有数据库运行租约、token 或 takeover。Tag merge/split 自身保持原子，支持一份明确 exact source；owner 不递归证明整个 relation graph。Accretion 按 center 按需派生，配置收敛为 enabled/generic_degree/recurrence_review，不形成 Authority truth。

长期语义 Authority 为 Architecture-Vault `5b96c63da34b0a4c697b6961ae10ba6aa4de3ee1`，查询合同见 [Query/Serving Spec](../specs/active/memory-reference-profile/03-query-serving.md) 与 [Agent 手册](../agent/NOUSQL.md)。当前支持根 ResultProjection、TemporalFrame/CognitiveClock、多轴过滤、as-of/history Owner/Serving views、共享 Tag semantic asset、QueryActivation/TagDirect、显式扩散、explicit formation Tag 和 Query→ReportUse→Concept Maintenance feedback。Current permission/purge fences、历史 asset reuse/read lease/reclamation、名称与媒体 interpretation 截点由 targeted integration tests 覆盖。

模型检索质量研究、范围及未测项见 [Research 观测](../research/observations.md)。Deterministic fake vectors 验证线路和合同，不证明语义排名质量或算法 winner。功能入口与模块责任见[功能地图](../architecture/function-and-ownership.md)。

真实 New API 验证已覆盖七个 Structured Output 角色、四类代表性媒体的三种派生策略、六篇个人博客的 formation/query，以及 Episode/Journal/consolidation proposal。稳定结论与剩余质量范围见 [Research 观测](../research/observations.md#2026-10-04-真实-new-api-验证)。大样本分段边界与 Schema 泛化质量仍未测量。

## Known Issues

- 真实模型可提出非法 selector 或无支持的 modality；owner 验证拒绝提交，两阶段保留已提交描述。当前代表性 case 的已发现问题已修复并重跑；NASA 低信号音频仅作为 research negative controls，不能泛化成产品语义缺陷。

## Deferred

Self、Social Cognition、Motivation、Heptalogos integration 与 Linux source-less portable。
