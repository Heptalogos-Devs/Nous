# 功能、数据流与内部能力地图

[返回文档目录](../INDEX.md)

本文记录全库整改目标和当前功能入口、数据流与责任归属。公共场景用受控 provider 验证模型线路；用户给定 Ark Gateway 的真实文本形成、向量提交与查询也已通过隔离实例验证，模型 Rerank 暂不启用。模型输出与检索质量仍由实际研究评估，连通验证不能替代质量结论。长期认知语义仍由 Architecture-Vault 的 Target Design 和已接受决定拥有。

## 整改目标

1. 从真实任务出发，沿官方 CLI、MCP、Client、Core、Kernel、owner 到存储和返回结果核实功能；区分已贯通、后端已有但入口缺失、实际缺失。
2. 删除不能直接服务任务的登记、准备、核对、转换、重复读取、中间状态和收尾流程；改完整流程，不只消除代码克隆。
3. 按实际能力划分内部职责，各能力拥有自己的数据、规则和完整操作；调用方不接触其 SQL、文件布局和内部处理顺序。
4. 一个概念保留一个正式数据结构和操作合同；修正源头并迁移全部当前生产者和消费者，删除临时包装、旁路传参和重新推断。
5. 复用现有 owner、标准库和成熟依赖，收拢事务、引用、数据库、文件、配置与错误机制；不增设通用框架。
6. 删除无实际用途的版本核对、owner 标记、子租约、恢复链和状态流转。保留已产生的数据和模型结果，不承诺从任意中断点完美恢复。
7. 算法与策略先具备配置能力，再决定暴露层级；状态、错误和已有功能接通官方入口。
8. 删除无继续价值的脚手架、过时或被覆盖的测试和脚本；缓存、临时生成数据有明确位置和清理入口，不扩张验收矩阵。
9. 代码按职责集中，测试与生产代码分开，脚本按用途组织；当前文档直接解释实际功能、配置和错误，历史保留在 Git。
10. 保留算法研究、Hybrid/VCP、真实模型、打包发布、多系统和路径布局能力；明显降低生产代码量、机制数量、转换次数和修改牵涉面。没有兼容需求，统一后删除旧路径。
11. 优先使用真实功能端到端场景；仅为必要的脆弱算法、并发、解析或 Authority 边界保留独立测试，不新增解剖式脚手架测试。

## 系统数据流

~~~mermaid
flowchart TD
  U[用户 / Agent] --> CLI[官方 CLI / stdio MCP]
  CLI --> Client[官方 TypeScript Client]
  Client --> Core[Core: 公共 API、模型和外部 Resource]
  Core --> Kernel[Kernel: 进程组合与协议转换]
  Kernel --> Owners[Subject / Material / Memory / Runtime / Configuration]
  Owners --> DB[Persistence: PostgreSQL、事务、回执]
  Owners --> Blob[ObjectStore: 不可变材料]
  Owners --> Serving[Retrieval: lexical / dense / concept / topology]
  Core --> Model[配置指定的 Gateway 与模型]
  Core --> Resource[宿主提供的外部资源适配器]
  Serving --> Owners
  Owners --> Kernel
  Kernel --> Core
  Core --> Client
~~~

模型和外部资源执行在 Authority 事务之外。模型输出是提案；所属 owner 决定持久提交。Serving 可重建，返回前由 owner 核验来源、生命周期、主体和精确修订。

## 实际功能地图

入口列说明当前完整执行路径；最后一列记录已拆除的多余步骤与仍存在的功能边界。没有独立公开操作的能力直接标明，不用局部测试替代系统可用性。

| 任务 | 官方入口与实际路径 | 数据与责任归属 | 简化后的流程与功能边界 |
| --- | --- | --- | --- |
| 开发启动、状态、停止 | dev / launcher serve → Core → private Kernel → PostgreSQL → discovery → Client status | Launcher/Core 拥有进程；RuntimeLocations 拥有路径；Kernel 组合 owner | dev 与包共用启动；一个 JSON bootstrap；普通 serve 使用已安装 runtime；监督 Kernel 退出，收尾完整执行，最后释放实例锁 |
| 配置与模型接入 | config describe/get/set/clear/check → Core normalizer → Configuration owner → active/desired snapshot → 模型角色 | Schema 属于参数 owner；Configuration 拥有覆盖顺序、快照和作用域 | 入库时规范化，读快照不重复解析；删除人为 config revision；未配置角色直接报告 NOT_CONFIGURED；凭据在 SecretRoot |
| Subject、Seed | subject create/use/list、seed/adopt-seed → Subject → seed Artifact | Subject 拥有身份、capability 和 adoption；ObjectStore 保存不可变 Seed | owner 提供完整批量 view，Seed 接通 CLI；主体删除/分支尚未实现 |
| Session | session open/show/list/close → Runtime Session → 返回选中会话 | Runtime 拥有会话、revision 和 foreground WorkContext；CLI 只保存选择 | 直接返回 owner SessionView，不在 transport 重读或查询 foreground；列表支持分页 |
| 观察和材料上传 | observe text/file → Client streaming upload / RecordObservation → Material → Runtime admission | Material 拥有 Artifact、Occurrence、SourceRegion、mentions、来源 lineage；Runtime 拥有实际接触 | read/list/producer/derived graph 由 Material 提供完整 view，共享解码与批量读取 |
| 材料读取与来源核对 | read/show/trace → Material read/materialize → 文本范围或二进制元数据；read --output → Client download | Material 拥有选择、范围和 lineage；ObjectStore 拥有流式字节 | preview 有界；Artifact 全量流式写入新文件，失败删除部分输出，不受 preview 上限约束 |
| 派生与媒体 | derive → Core role/routes/预处理 → Material commit → representation/regions | Core 拥有 provider、FFmpeg；Material 拥有 immutable DAG 和 ProducerSignature | 保留三策略、图像、音频直接/转写、视频直接/frames；已有 outcome 直接返回，缓存 proposal 再提交，不为 direct replay 下载原字节 |
| Memory 形成、修订与生命周期 | form、memory form/revise/history/lifecycle → Memory | Memory 拥有对象、不可变 revision、basis、aboutness、依赖和生命周期 | 全部管理操作接 CLI，真正分页；共享批量 hydration；purge 一次事务提交，删除 checkpoint 与中间状态 |
| CognitiveSchema | schema create/revise/evidence/split/merge/lifecycle、maintenance consolidation → Memory Schema | Schema 能力拥有适用范围、边界、支持链和 lineage | 现有公共操作全部接 CLI；独立 list/history 尚缺；保持独立语义模型 |
| Tag 与 Association | tag create/revise/merge/split/attach、association create/revoke/neighborhood → Memory concepts | Tag 拥有语义资产和 canonical identity；Association 拥有 typed endpoints/basis/polarity | SQL 与完整 read/search 属于 owner；修订历史和独立 Association read/list 尚缺 |
| WorkContext 与 ResidentSet | context create/list/set/pin/unpin/clear/foreground/pause/resume/end → Runtime | Runtime 拥有任务文本、exact anchors、revision、生命周期和接纳 | 正式文本、问题、约束、resume 与 budget 字段接 CLI；保留真实 expected revision，transport 不访问 Runtime 表 |
| Episode 与 Journal | episode create/revise/history/link/lifecycle、journal reads/lifecycle；maintenance 自动组织 | Memory 拥有 exact organization、成员/来源、修订与 lifecycle；Runtime 拥有实际 experience | 现有 Client 操作全部接 CLI；四家族共用 Runtime exact-ref 清理，领域规则分别拥有 |
| 有意义使用与维护 | use → Runtime receipt/feedback → MaintenanceNeed；maintenance grant → Core → owner actions | Runtime 拥有使用事件和待做事项；Core 拥有有限 host 预算；Memory 拥有提交 | grant 按 Subject 串行，选择 pending need，完成核对 trigger revision；没有数据库运行租约/token；保留调用与时间预算 |
| Query 与历史切面 | query/prepare/inspect → NousQL → Runtime preparation/context → lanes/fusion/rerank → owner final validation → result:N | Runtime 拥有意图、时间切面、prepared context；Retrieval 拥有投影；各 owner 拥有 materialization | 一个显式 embedding，复用 binding 与打开的 family；stale exact binding 被拒绝；仅显式 refresh/maintenance 扫描回收资产 |
| Hybrid、native、VCP | 同一 prepared query → 显式配置的检索 profile → lexical/dense/shared concept/topology | Retrieval mechanisms/reference/vcp 拥有数值算法；owner 提供可重建输入 | 四类 profile、数值 reference、真实 read pin/grace 保留；死的 shadow policy/rank、旧 family 兼容与源码哈希升级核对删除，制品结构、参数、向量空间和实际内容校验保留 |
| 外部 Resource | resource put/get/list/remove/materialize；query Resource continuation → adapter → Material Observation | 宿主 adapter 拥有外部访问；Runtime 拥有单次 query ticket；Material 拥有已接触来源 | Registry 接 CLI；stock Core 无 provider adapter，只能管理描述符并明确报告未接入；提供 adapter 的宿主可查询和接纳，初次复用 version/access，恢复未接纳 proposal 才重查 |
| Context projection | projection build/managed → Core steward → Runtime exact source refs | Core 拥有有界表达；Runtime 拥有 context/使用接纳 | 现有能力接 CLI；需要 operator consumer policy 和 projection_steward；不能用缓存充当 cognition Authority |
| Windows Portable 与多布局 | build/runtime pack/notices → assemble → installed launcher → verify/restart/relocate | Release 拥有 payload；RuntimePack 拥有 acquisition/inventory；Locations 拥有共置/分离 roots | 只加载目标平台 catalog，不重复 acquisition 已完成的校验；保留显式 release verify；Linux source-less 尚未交付 |
| 研究和本地数据维护 | research gateway/media/longitudinal/contracts/trace；clean:local → data/temp、data/cache | Research 拥有真实语料、数值 oracle、模型观察；开发工具拥有本地生成路径 | 真实研究保留；跨系统显式清理，受控停止临时 PostgreSQL；longitudinal 公共场景运行一次；删长度 gate 与重复脚手架 |

Self、Social、Motivation 和 Heptalogos integration 是长期方向，当前未实现。本轮保持既有功能边界，不用占位 API 表示完成。

## 模型能力

| 能力 | Role | 输出提交 owner |
| --- | --- | --- |
| 形成与上下文表达 | memory_formation、projection_steward | Memory、Runtime |
| 纵向组织与认知演化 | episode_segmentation、journal_synthesis、memory_consolidation、concept_maintenance | Memory 的 Episode、Journal、Memory/Schema、Tag/Association |
| 材料理解与结构化 | material_description、material_structuring、material_direct_structuring、speech_transcription | Material |
| 查询模型工作 | query_embedding、query_concept_enrichment、query_rerank | 查询临时结果或可重建 Serving，不写为认知事实 |

ModelRole、ExecutionProfile、ModelProfile 和 Gateway 分别表达工作、调用控制、模型资源与连接。保留明确配置的有限 route fallback 和实际成功 ProducerSignature；删除重复解析和另一套流程租约。未配置或缺少凭据时状态必须直接说明。

## 内部能力与数据合同

| 内部能力 | 单一责任 owner | 调用方获得的操作 | 已移除的泄漏或重复 |
| --- | --- | --- | --- |
| 身份和正式 DTO | canonical Proto、core typed IDs/refs；Identity Directory 解析地址 | exact typed ref、明确歧义候选、公开 DTO | Client 的平行 Data/WeakMap 结构、重复 schema 恢复、CLI 再次查已装饰的地址 |
| 领域读取、列表和修改 | Subject / Material / Memory / Runtime 的能力子模块 | 完整 view/page/mutation result，数据处理在 owner 内 | Kernel transport SQL、ID 列表再逐项读取、调用方重建内部字段 |
| 事务与幂等 | Persistence 提供同一事务和 receipt；语义 owner 提供规则 | Owner mutation，原请求重放同一结果 | 无需求的多阶段数据库 checkpoint、临时 owner 状态；不删除真实并发和同 ID 不同输入冲突 |
| 模型操作保存 | Core 序列化同一 operation；Persistence 保存 snapshot/proposal/outcome | 读取或执行同一正式 operation，已有提案直接提交 | child lease/token/busy/release、父任务数据库租约、逐项 progress、重复 prepare/reserve、任意位置完美恢复链 |
| 来源和 purge | Material 拥有来源；Memory 各能力拥有 cognition；Runtime 拥有 runtime 清理 | 在 owner 事务内清除 exact refs 并留下无正文回执 | 四家族重复写 Runtime 表、嵌套逐事件循环、调用方推断 JSON 引用 |
| 配置 | 参数 owner 提供类型/默认值/校验；Configuration 保存解析结果 | 已规范化的 immutable typed snapshot 与清楚 apply mode | 每次读取再 normalize、重复 protobuf/JSON/domain 包装；不增加第二套配置服务 |
| Serving 资产 | Retrieval assets 拥有 build/open/install/reclaim | 准备后直接返回当前请求打开的 family assets | 发布后重查 DB/reopen/重建 snapshot、每次查询全目录扫回收；保留历史身份、read lease 和 purge fences |
| 文件和进程 | ObjectStore、RuntimePack、Locations、Core lifecycle | 流式字节、已安装 runtime、统一 serve/stop | 普通启动重复摘要检查、dev 另起一套启动、首个收尾错误阻断其他资源关闭 |
| CLI 消费者状态 | CLI 只保存选择、exact query results 和原 mutation 请求 | 同一命令的固定输入、官方 owner 返回结果 | 本地 terminal outcome Authority、多层 protobuf→Data→恢复→显示转换；不再解释领域状态 |
| 测试与生成数据 | 公共功能场景；owner 独特脆弱逻辑；workspacePaths | 单次真实公共路径检查，显式清理本地临时/缓存 | 重叠脚手架测试、重复运行、过时目录扫描；不删数值 oracle、真实语料和独特并发/解析风险 |

## 完整路径验证

`just check` 执行格式、协议、类型、lint、Rust checks、全部保留测试与五条公共 smoke；纵向认知场景随 Kernel integration suite 执行一次。公共场景启动真实 Core、Kernel 和 PostgreSQL，通过官方 Client 完成提交与返回；算法 goldens 和独特 Authority、时间、取消、并发风险由所属测试保护。真实模型的协议、输出质量以及 Windows native packaging 需要相应实际环境，不能从 Linux 模型替身场景推断。

旧实现随消费者同步删除，不保留 alias、shim 或双实现。功能边界与实际检验结果决定完成程度，不以固定问题数或验收记录数量作为目标。

相关入口：[当前实现架构](current-implementation.md)、[配置](../reference/CONFIGURATION.md)、[官方 CLI](../../apps/nous-cli/README.md)、[Client operations](../reference/CLIENT.md)、[检索合同](../specs/active/memory-reference-profile/03-query-serving.md)、[脚本](../../scripts/INDEX.md)。
