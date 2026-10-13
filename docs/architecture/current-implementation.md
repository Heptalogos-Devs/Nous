# Nous Wave 当前实现架构

[返回文档目录](../INDEX.md)

本文描述当前代码的进程边界、语义 owner 和主要数据流。长期认知语义与目标架构见 [Architecture-Vault Target Design](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/docs/Nous-Wave/TARGET_DESIGN.md)。

## 进程边界

TypeScript Core 提供 Connect/HTTP API，启动和管理 private Rust Kernel，执行模型与外部 Resource 调用，并承载 WorkContext、Projection、Managed Context 和官方 Client 集成。Core 通过 authenticated loopback RPC 调用 Kernel；PostgreSQL 访问由 Kernel/Persistence 负责。

Kernel 组合 Subject、Configuration、Material、Memory、Runtime、Persistence 和 Retrieval owners。Authority 写入由对应领域 owner 验证并持久化；Serving generation 可从 Authority 重建。启动只建立 Authority 和宿主能力绑定；Serving 由请求的冻结计划按需准备，显式管理刷新才准备全部配置族。

## Rust owners

| Owner                  | 当前责任                                                                                                                                              |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/core`          | typed IDs、exact references、query DTO、时间与共享语义错误                                                                                            |
| `crates/protocol`      | 从 `proto/` 生成的 Rust wire bindings                                                                                                                 |
| `crates/configuration` | Rust/Core unified Catalog、JSON Schema validation、overrides、immutable snapshots、digest 与 capability provisioning                                  |
| `crates/persistence`   | PostgreSQL transaction/pool、fresh schema、MutationEnvelope、receipts、projection watermarks                                                          |
| `crates/subject`       | Subject identity、Memory capability、Cognitive Seed version/adoption；Artifact admission 调用 Material owner                                          |
| `crates/material`      | 共享 Artifact CAS admission、ObservationOccurrence、SourceRegion、DerivedRepresentation、DerivedRegion、批量 selected text 与 context materialization |
| `crates/memory`        | Memory、CognitiveSchema、Episode、Journal、Tag、AssociationEvidence、provenance 与 lifecycle                                                          |
| `crates/runtime`       | Session、ResidentSet、WorkContext、UseEvent、QueryPlan、lane/result contracts 与 fixed fusion                                                         |
| `crates/retrieval`     | lexical/dense/concept Serving、current/historical immutable generations、批量共享向量与显式 associative diffusion                                     |

## Transport 与 mutation

Kernel 在同一 authenticated loopback server 上承载 canonical generated `SubjectService`、`MaterialService`、`MemoryService`、`ConceptService`、`IdentityService`、`RuntimeService`、`ResourceRegistryService`、`ConfigurationService` 和 `SystemService`。Core 通过 generated ConnectRPC descriptor adapter 转发这些 owner 操作，保留调用方的 cancellation 与 deadline。官方 Client 的 `cognition` 和 `resources` namespace 组合相应 owner 与编排接口。Rust Query 和内部 readiness status 使用当前 typed shape；协议身份由 canonical Proto 声明，不再逐对象附加固定 API 版本。

Core 的 `CognitionService` 承载 Query、GrantMaintenance、Projection 和 Managed Context 编排；`ResourceService` 承载外部 Resource materialization；`ModelService` 执行 formation、derivation 和 embedding。`SystemService.GetStatus` 一次汇总 Core、Kernel 与当前 model runtime 的状态；Kernel 不可用时仍返回宿主状态和明确错误。

Model resource schemas 位于 `model/profiles.ts`，role 与聚合配置各有独立 owner。`model/invocations.ts` 固定执行 routes/snapshot 并记录实际 attempts；`model/protocols.ts` 处理不依赖 Role 的 SDK/HTTP 调用。`model/input.ts` 按实际输入和候选 route 判断可执行性，`model/interpretation.ts` 构造 Material 请求并按通道解释输出；`model/derivation.ts` 拥有媒体形成；`operation-results.ts` 完成既有结果读取、提案保存和 owner 提交结果记录。每次实际媒体访问及其输出校验使用同一通道描述，quality/provenance 保存实际 adapter、模型、Prompt、输出 schema 和推理参数；不读取源码生成升级版本核对。

私有 workflow services 按实际能力分组：`KernelQueryService` 负责认知时间读取及 query prepare/finalize/release；`KernelModelResultsService` 只保存实际 proposal/outcome、必要结果元数据与 telemetry；Core 按 operation identity 串行执行，内核不维护第二套模型租约；`KernelMaterialWorkflowService` 提供 derivation/embedding 输入与提交；`KernelMaintenanceService` 提供 pending needs 的 select/plan/finish，以及天然原子的 Episode partition 与 Journal 提交。Core 按 Subject 串行运行 grant，Runtime 使用 trigger revision 防止旧完成结果清除新增工作。consolidation/concept proposal 由 Core 逐项调用 canonical Memory/Schema/Tag/Association owner API；稳定 action identity 和 owner receipt 支持已提交结果的重放，不保存逐项检查点。`KernelProjectionService` 提供 contribution batch。`KernelConfigurationService` 提供 bootstrap/snapshot，`ArtifactStreamService` 提供 Artifact 上传/下载及 Material owner 解析后的选区流；Core 不再自行解释 source 的物理坐标。

Persistence 的 `MutationEnvelope` 持有 Subject/operation identity、可选 owner Subject lock、operation lock、canonical digest receipt、transaction 和 projection invalidation。owner 选择 invalidation families，envelope 为同一事务分配一个 Authority sequence，并在 commit 时发布合并的 watermarks 与 receipt。Replay 返回 receipt，由领域 owner 解码结果；未提交的事务回滚；四类 cognition purge 与 receipt 在同一事务提交，失败整体回滚。

Memory、Episode、CognitiveSchema 和 Journal 保留独立模型、typed tables、provenance 与 head/epoch fence；所属认知能力共用 lifecycle/purge 事务和 Runtime exact-ref 清理。Episode reaccept 的组织验证、Journal source revalidation 和各家族 projection invalidation 保持具体规则。Memory crate 的内部 read、mutation、lifecycle、partition 和 provenance 按能力组织，Kernel 不构造领域 basis catalog。Episode 创建/修订共用正式内容；Schema/Tag 返回选中修订与当前 head，mutation 直接返回完整视图，provenance、consolidation catalog 与 accretion 使用同一 exact support 批量读取；每次认知遍历在一个读取事务内选择归属、依据和 Episode 成员，Schema links 按当前或捕获切面选择。Memory、Schema 和 Episode 共用正式 RevisionBasis 的数据库解码；Query 与 Context 共用该结构的 evidence handle 操作。Episode/Journal Query 先完成 owner 可用性判断，再读取正文和请求需要的依据；片段正文在请求内按 exact ref 复用，每批最多 256 项并随批次释放，保留历史切面和每个 Episode 的原有顺序。Material 的维护输入同时捕获 Observation、已选表示和 payload metadata，Memory 只应用认知排序和文本预算。

Serving 校验实际配置、向量空间、Authority watermark、制品结构与内容校验和。没有源码树哈希、实现版本核对或跨构建缓存迁移；更改制品格式后显式清理可重建 cache。当前与历史代次共用 `retrieval/assets/` 中的构建、安装、替换与互斥规则。Native Wave 的配置声明与解析由 `retrieval/policy/wave.rs` 拥有，图构建由 `retrieval/mechanisms/graph.rs` 拥有；调整配置展示等级不会改变实际查询参数。

## 数据流

```text
Official Client / Host
        ↓
TypeScript Core
        ↓ authenticated loopback RPC
Rust Kernel
  ├─ Subject / Material / Memory Authority
  ├─ Runtime query and use state
  ├─ PostgreSQL persistence
  └─ rebuildable lexical / dense / concept / topology Serving
```

Observation 写入 Material Authority；Memory 使用 occurrence-bound EvidenceRef 形成 revision。Authority commit 发布 projection invalidation/watermark。领域 Authority 持有认知对象与 revision；索引、向量和拓扑 generation 提供可重建 Serving，Session 与 UseEvent 等运行状态由 Runtime 持有。

Core 解析 model profiles 与 Prompts，调用 embedding、rerank、media derivation 和 Resource adapters；Kernel 验证并保存 ProducerSignature、exact material inputs、Authority revisions 与 query state。External Resource records 与本地 Artifact/Observation 保持不同身份；被选用的外部内容通过 Material owner 形成新的 Observation。

## 当前边界

当前可供给的认知能力组合为 Memory-only。Episode authority 归 Memory；Runtime 自动组织 Session Experience；Memory 持有 Episode/Journal Authority；Core 提供有界维护机会、模型 refinement、Journal synthesis 和 Memory/Schema consolidation。具体合同见 [纵向认知](../specs/active/cognitive-runtime/longitudinal-cognition.md)。

Windows x64 portable assembly 使用 LLVM-MinGW UCRT 生成 shipping Kernel，并携带所需私有运行时 DLL。独立运行目录、配置、数据和第三方 runtime 布局见 [Runtime Bundle Spec](../specs/active/deployment/runtime-bundle.md)。

## 认知恢复后的 owner 边界

Query source admission/continuation、automatic concept maintenance、derived Accretion 与逐项 owner mutation 的长期设计 Authority 位于 Architecture-Vault merge `2de60296bc80d790e9dd508bc6b3abd19c7d3236`。Prepared Query 的 semantic representation 由当前描述符和 exact refs 在 planning 前固定；readout profile 不改变 semantic input。Material/Memory 分别提供最终材料与时间验证，Runtime 组合 owner contributions；没有通用 persistence reference_times resolver。

Concept planner 围绕 focus 的 aboutness、来源、一跳关系和 bounded Tag candidates；不生成 Subject 全量 cognition catalog。proposal 不是超级事务，Core 保留每项 committed/no_change/invalid/stale/dependency outcome。Natural Tag merge/split、Episode partition 与 Journal mutation 继续由真实 owner 维护自身原子性。Accretion 按需提供来源、成员、recurrence、时间跨度、关联、使用和反证等 derived signals，review hints 在调用点计算。

WorkContext 的创建、修改、生命周期和 foreground 操作共用 Persistence MutationEnvelope 的事务与回执，不增加 Subject 串行或认知投影失效。文字意图不重验或重写未变化的锚点，锚点修改按原序批量写入；暂停/结束在同一事务中按 Session ID 锁序批量解绑。

Context projection 由 Runtime 统一捕获 Session 与 foreground、选择 consumer 允许的来源并分配预算。Runtime 直接使用捕获的 WorkContext 任务文本、约束、恢复条件和预算摘要，并从已有目录读取 Entity 名称与别名；未绑定的 Entity 仅提供引用。Memory/Schema/Episode/Journal/Tag、Material 与 Resource 各自提供实际内容和证据。各 owner 共用 `ContextSource`，已解析引用与真实内容修订标记分别表达；WorkContext 使用整数 revision，Tag 使用选中 revision UUID。Core 只执行 steward；空 optional 集合不调用模型。Observation 与 ReportUse 共用 Resident 接纳规则，完整列表批量读取。

Research 只使用 official public Client/CLI。小型 runner 不启动额外 Runtime、不管理 Serving installer/embedding store，输出在 ignored data。VCP reference/adapters 与有效 asset reuse/reclamation 保留；完整数值矩阵保存在 ignored Research results，CI 使用 compact discriminating goldens。

## Temporal 与 Semantic Concept 查询

根 ResultProjection 默认四类 cognition，Evidence/Resource 显式选择。TemporalFrame 捕获 Subject CognitiveClock、as-of AuthorityView 与 current/history RevisionView；相对窗口、名称解析、Query Representation 与 activation 共用该截点。Memory/Material 提供 historical owner projection，Persistence 读取既有 canonical chronology/immutable rows；Serving 在 candidate generation 前按 selected state 构建历史资产，不发布到 serving_current。Reader、grace 和显式回收与 current 资产共用；最终物化仍执行当前权限/purge fence。

Serving freshness 由 Persistence 的单一 family/space desired watermark 计算。Native topology 的空 space 依赖真实图结构；VCP 使用 `vcp:<embedding space hash>`，同时依赖 lexical、所用 dense/concept space 和 topology。状态、prepare、build stamp、promotion 与 publication 使用同一规则，text-only 变化不会使 native graph 失效；Episode/Journal 组织和生命周期变化必须推进 topology。Publication 在既有 family 锁和 Subject 行锁内核对 watermark 并安装 current，不用另外一套恢复状态。

ConceptService/client.concepts 承载 Tag/Association Authority；Identity facade 路由 Entity rebinding 到 Material owner。Tag canonical semantic text/digest、embedding 与 attachment postings 是共享资产。QueryActivation 分离 durable Tag 和 ephemeral inferred/novel concepts；TagDirect 不依赖 graph 或 embedding，扩散仅由 `$explore` 明确开启。Bare text lexical+dense 不要求概念模型。

Host 通过 prepared token 的 frozen view 准备历史 embedding cache miss，所有 lane 复用 query embedding。Query concept model 是独立可选 role，不形成 Authority。Runtime 以 bounded retention record 关联 query_id 与真正返回的 exact revisions；ReportUse 推动 review/planning，只有 bounded Host grant 才能运行维护模型并提交 owner mutation。
