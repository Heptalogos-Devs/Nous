# Nous Python 重构进展

## 总体情况

Nous 当前以 Python 应用和同进程 Native 计算运行。Namespace/Subject、材料与来源、认知修订、形成与维护、查询与图、Context、模型资源及 A-Memorix 公开入口共用原生所有者。根旧 Rust 应用和专属 crates 已移除，保留的原生算法位于 `native/`。

本轮已实际恢复启动和真实模型调用，接通来源与对象的删除恢复、完整 Namespace 备份、人物画像、Ego 种子、Social、Symbols、Motivation、检索调优和宿主描述投递。隔离 MaiBot 的 MemoryService、Records 管理路由和原有工作台消费均有实际结果。Windows 发行包已在中文空格目录以标准 CPython 3.14.4 独立安装，HTTP、可选 RPC、CLI 和 MCP 实际读取通过；Linux/macOS 的发行安装和正式公开文本基准仍需对应运行。下文历史记录保留各自原配置与数据条件。
## 项目要完成什么

[项目宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)将 Nous 定位为独立的长期认知基础设施与人格中心：保存材料、经历、记忆、证据、来源、时间和修订；提供 NousQL、多方式检索、概念与关系、形成与维护；进一步支持自我、社会认知、语言、动机和工作认知。普通应用可以只使用记忆与检索，无须先建立人格。Nous 还要完整承接 A-Memorix 已有的实际工具、管理和消费接口。

Heptalogos 是基于 MaiBot 的下一代持续主体产品，拥有世界、生活、时间、注意、选择、行动和外部效果，正式依赖 Nous 提供认知。MaiBot 的消息、平台、插件、模型资源与管理体验是主体产品的重要基础。本次重构重点是 Nous 及其真实消费者接入；MaiBot 接入取得了进展，完整 Heptalogos 世界与主体运行仍待建设。

[开发目标与计划](../GOAL.md)明确完整功能范围，并将主应用改为 Python：权威数据库、事务、认知规则、模型与持久任务归 Python；VCP、原生索引和 NousQL 纯语法留在同进程 Rust。

## 架构如何变化

起点是旧分支 `codex/context-infrastructure-sdk` 的 `2458932646d6b2968db38d1e0a97103f93cd2cad` 交接。其文件接纳、导入、来源生命周期和 Bundle 有未完成工作，也保有真实资料和模型历史。迁移保留原源码作为参考，按业务所有者及其消费者重组，旧工作区数据未被当作新实例的默认数据库。

现在的应用由 `src/nous` 组装：Python 拥有 Namespace、Subject、材料、认知、来源、查询计划、上下文、导入、配置、模型与任务等业务责任；HTTP、MCP、CLI 与 A-Memorix 协议进入相同领域操作。公开 Pydantic 合同由 Python SDK 维护。`native/` 提供 PyO3 边界、VCP 计算、Tantivy/Jieba 中文索引、USearch 向量检索及 NousQL 语法解析；原生计算接收授权后的数据和索引快照，业务数据库、凭据、模型网络与正式领域状态由 Python 管理。

SQLAlchemy Core 和 aiosqlite 使用 SQLite 保存权威状态。DBOS 与业务表共用同一 SQLite 文件，业务结果和后续任务接纳由同一事务提交；网络、模型和耗时媒体处理在短事务外执行。模型由 Pydantic AI 和官方 SDK 承担提供方协议，应用管理角色、Prompt、调用身份、来源、结果与费用；出站网络策略和客户端生命周期集中到 NetworkService。uv workspace 管理服务、Python SDK 和 native 扩展三个发行单元，服务使用标准 CPython 3.14，SDK 保留 Python 3.11 及以上范围。具体设计见[架构](design/architecture.md)、[事务与作业](design/transactions.md)、[原生边界](design/native-boundary.md)和[公共基础能力](design/platform.md)。

Rust 计算实现集中在 `native/`，构建与发行使用其独立 workspace；产品能力由当前 Python 领域模块提供。

## 从交接到现在的主要工作

2026 年 10 月 28 日的 `412767e` 建立 Python 应用和原生认知计算入口；`223718d` 接入持久上下文与公开 Python 消费者；`1d561cc` 增加中英文工作台和实例恢复流程；`93eb347` 修正跨平台 wheel 验证中的问题。10 月 29 日的 `d81748d` 扩展 Python 认知领域、派生媒体及持久维护，形成后续功能接续的基础。

此后 `a5de9f5` 推进查询、模型合同和 Bundle 来源，`c2eaf44` 修复真实媒体输出及每向量来源，`274b010` 接通范围内名称绑定和正式图查询，`06c332b` 集中出站网络与 VCP 配置，`b5b49c4` 接通图管理与可续接文本导入，`84f4282` 增加新输入接纳授权和可编辑 Prompt 的实际合同预览。

10 月 30 日，`f5aa2b1` 增加持久向量投影和可配置媒体准备，`31f6c78` 修复 VCP 读出丢失词法和直接锚点信号，`ae33be4` 接入来源支持的认知与持久 Managed Context，`68fc3ae` 完善 Episode 摘要和集合分页的 CLI 展示。之后继续开发 VCP 意图、Fact、Context 工具、修订状态和模型路由，并保存为 WIP 检查点 `67e09cd`。

## 已真实走通的使用链

### 真实资料、形成与续接

完整开发讨论原文进入材料和导入目录，冻结为三个 Unit；真实模型形成了三个输出，并记录一项完整文件成功事实。继续导入时复用了此前成功的输出，只形成剩余单位。DBOS 作业的成功、UnitOutput 和文件成功是分别记录的事实。证据见 `temp/review/python-local/discussion-continue-command.json`、`discussion-continue-job.json`。

这一链路贯通了资料接纳、原文保存、导入目录、形成、认知采纳与持久作业。导入支持按同一范围、来源及形成配置复用已成功结果；可单独继续失败的向量工作。文件去重、同一操作重放和内容向量复用使用各自的业务身份。

### 图片、向量、来源和重启

真实 `maimai-v2.png` 原件进入服务，模型先产生自由描述，再将原媒体和已保存描述共同送入结构化形成。图像、描述与结构化结果参与检索，来源可追溯到原件和派生表示。原生视觉、文本和图文联合向量都经过真实模型调用；重启后相同查询命中保持，已完成的描述和模型结果得到复用。证据见 `actual-media-embeddings.json`、`actual-joint-query.json`、`caption-reuse-summary.json` 和 `actual-restart-summary.json`。

持久向量投影对指定 Memory 的精确修订调用真实 embedding，保存请求、空间、结果和实际调用身份。重启后同一回执与调用继续有效，模型调用集合没有增加，查询和来源仍可使用。对应记录为 `codex-vector-projection-command.json`、`codex-vector-projection-job.json` 和 `codex-vector-projection-after-restart.json`。本次使用的开发环境是本地 NewAPI 网关、Doubao Seed 2.0 mini 和 2048 维视觉 embedding；这些结果针对实际配置的提供方与实例。

Native Subject Bundle 与 A-Memorix 可移植包均经过真实导出和接收。A-Memorix 包由实际上游 reader 读取，NPZ 向量坐标和空间得到核对；一次接收记录包含 9 条认知和 10 个向量。见 `actual-native-media-bundle.json`、`native-media-receiver.json`、`actual-amem-upstream-read.json` 和 `actual-amem-roundtrip-summary.json`。

### 动态媒体和来源范围

真实上游 GIF 原件有 189 帧、总时长 13,510 毫秒。服务实际生成两秒裁片、指定时间与像素区域的帧，并保留派生表示到原件和选区的谱系。仅获准区域的读取可取得相应派生结果，读取整份原件返回 `source_unavailable`。见 `actual-gif-original.json`、`actual-gif-derived-clip.json`、`actual-gif-derived-frames.json`、`actual-gif-sdk-summary.json`。

新材料的接纳权与凭据允许读取的既有 Source 范围分别处理；调用者可以在授权内提交新输入，既有来源的硬上限仍然生效。实际范围内上传、图查询和上下文更新见 `actual-scoped-upload.json`、`actual-scoped-graph-result.json`、`input-admission-context-update.json`；`source-scope-summary.json` 记录了精确 Memory 可读、整份原件不可读的结果。

### 图、查询与 VCP

实际 Subject 的图资产导出 36 个节点、11 条有向关联；另有三条关系的历史精确端点不在当前可物化图中，按精确修订规则省略。此前生产图只取 Concept，漏掉真实 Entity 和关系，导致图为空；图构造已改为读取正式 Concept、Entity、Association 和实际端点及来源。见 `actual-production-graph-query.json`。

VCP 之前把 BM25 结果放进候选池，最终读出却被纯语义相似度覆盖。`31f6c78` 让授权候选的词法与语义信号按统一尺度融合，并保留直接锚点。真实开发事实查询据此排到首位。

Windows 上发布原生索引代际时，Tantivy 的 mmap 和 watcher 仍持有文件，目录重命名因此失败。原生发布现在先同步快照并释放文件及冻结索引句柄，再执行目录发布；平台层统一拥有阻塞线程和取消收尾。重建后实际 Hybrid 查询、来源图和模型重排已运行，原有索引没有通过清空来掩盖这一故障。

VCP 请求意图进一步支持精确核心实体、具名 concept cue、`#` 概念和 `$explore`，实现已保存到 WIP 检查点。实际请求以 Nous Entity 为核心、使用两条真实 cue，并返回 Thinking ready 和指向正式认知对象的路线；关闭详细 diagnostics 后正式 VCP 结果仍可用。见 `actual-vcp-intent-command.json`、`actual-vcp-intent-result.json`。这是一条真实查询结果，完整算法质量、所有查询组合和规模表现仍需继续验证。

### Episode、人物画像和 Fact

真实 `project-discussion` 的三条 Memory 经模型形成三个 Episode。MaiBot 的 list、get 和来源读取可取得这些经历；同一操作重放使用同一 job，没有新增形成调用。CLI 人类可读和 JSON 两种输出均返回三条记录。见 `actual-episode-rebuild-submission.json`、`actual-episode-source.json`、`actual-maibot-episode-get.json`、`actual-maibot-episode-rebuild-replay.json`、`episode-cli-structured.json`。

真实 MaiBot `MemoryService` 经 A-Memorix adapter 使用 Native 人物画像：查询、证据、人工别名和摘要覆盖、删除覆盖以及实际模型刷新均已运行。人工控制在刷新后保留，Native 和 A-Memorix portable 导入分别保留相应内容与证据。相关记录包括 `actual-maibot-profile-control-*`、`actual-profile-auto-vector-job.json` 和 `actual-profile-controls-roundtrip-summary.json`。

Fact 实现已通过实际 MaiBot 创建、跨 key 更新、撤回、恢复、读取和列表，并完成 Native 与 A-Memorix 纯事实包往返；代码已保存到 WIP 检查点。Native 保存两个事实身份、五条修订；portable 包包含两条 claim、五条 evidence、五条 transition，保留状态、时间和来源。跨 key 更新按上游语义执行 retract 加 assert。见 `actual-maibot-fact-*` 和 `actual-fact-roundtrip-summary.json`。该组历史记录取得于统一可见性修复前；当前原生目录已统一状态和来源过滤，断续有效期、撤回及画像联动另按当前合同验证。

### MaiBot 模型调用与上下文

真实 MaiBot 克隆使用 `ConfigManager`、`ChatManager`、`LLMServiceClient` 和 `OpenaiClient`，在模型调用前取得非空 Nous 认知，按实际完整 provider 输入测量并接纳上下文，调用结束后记录真实模型结果。重建执行者后，已接纳的 track 与游标能够继续。模型的真实 `memory_source` 工具经过两轮调用并保留原有宿主历史，成功读取已接纳认知的来源后产生一条 actual-use 事件。

接入过程中修复了反馈丢失实际 Provider 和模型身份的问题；结果包含真实 provider、模型、response 和 token 使用量。相关补丁在 [managed-context.patch](../../integrations/maibot/managed-context.patch)，记录见 `actual-maibot-managed-resumed.json`、`maibot-managed-metadata-fixed-result.json`、`actual-maibot-tool-use-events.json`。MCP 的 `query_cognition` 也已实际返回可读正文、短引用和完整 `structuredContent`，见 `actual-mcp-readable-query.json`。

以上实际记录均位于 `temp/review/python-local/`。运行实例、原件、付费调用账本与历史保存在本地实例及该目录中。

## 十六项基础服务

下表说明各基础服务的实际 owner 和接线。已实现的操作以相应源码为准；列出的真实运行只覆盖具体场景，剩余目标继续按[开发目标与计划](../GOAL.md)完成。

| 服务                                                | 当前 owner 与已接主路径                                                                                                                                    | 尚需完成或确认                                                                                         |
| --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Instance / Platform：实例与跨平台 | `platform/paths`、实例锁和 bootstrap 统一默认目录与应用生命周期；三包发行、Windows 独立安装、HTTP 和可选 RPC 已接。 | Linux/macOS 仍需在对应平台实际构建、安装和服务管理。 |
| Configuration：配置 | 类型化配置拥有实例、Namespace、Subject 的有效值、来源和历史；显式应用/回滚使用真实 commit 和 previous_value，模型、网络、检索、媒体、成长消费同一配置。 | 配置修改继续通过正式 configuration 操作。 |
| ModelResources / ModelCalls：模型资源与调用 | 统一 RoleRegistry、冻结调用目录、物理 attempt、账本与用量；安全回退、Retry-After、结果未知暂停、已存响应恢复和整目录同空间投影已接。 | 未确认的调用保持未知状态；提供方 alias 与实际型号映射由空间所有者统一。 |
| Prompts：提示词 | 随包提示词、实例修订、Jinja 渲染、合同和实际请求预览共用消费者定义；Seed 与 Symbols 的输出读取、恢复使用原物理响应。 | 新提示词只作用于后续请求，历史响应保持原事实。 |
| Contexts / ModelInput：上下文与模型输入 | 准备、预算准入、原生请求、游标、居民、工作态和反馈已有 owner；真实 Host 重启重放复用同 call，MaiBot 使用正式 managed binding。 | Entity 有效字段变化会拒绝旧准备；保存的 Motivation 评价按同一精确条件修订供给。 |
| Network：网络 | NetworkService 统一 HTTPX 客户端、策略、代理、CA、TLS、超时及关闭；模型与资源读取使用实际客户端。 | 新提供方按实际协议接入。 |
| Authority / Objects / Indexes：权威存储、内容与索引 | SQLite/SQLAlchemy、ContentStore 和 Native 索引各有所有者；领域写入与 DBOS 入队同库提交，修订、内容引用及派生失效共用权限和生命周期。 | 质量与规模表现需要有效数据和实际负载验证。 |
| Clock / Scheduler：时间与调度 | 五轴时间、时区、有效区间、历史截点和 DBOS schedule 已接；Fact 不同适用窗口独立保存，冲突仅按重叠区间判断。 | 世界日历和时间推进归 Heptalogos。 |
| Tasks / Execution：持久执行 | 导入、描述形成、向量、Episode、画像、调优、成长和反馈使用 DBOS；取消、重试和批量处理复用同一执行所有者。 | 副本的未完成工作暂停，保留来源和恢复状态。 |
| Identity / Secrets：身份与密钥 | Namespace/Subject 提供分页、能力配置、启停、回收站、恢复、稳定宿主绑定；凭据轮换、撤销、到期和 Source 上限共用可信入口。 | 清除身份保留退役记录，恢复保留原身份。 |
| Capabilities / Extensions：能力与扩展 | 服务支持能力、主体配置能力和启用能力有正式 typed 合同；角色、配置和领域处理器由应用显式注册。 | 宿主扩展通过公共 SDK 与协议消费。 |
| Content / Media：内容与媒体 | 原件、区域、派生、Resource、导入和接触共用 Material/Content 所有者；HTML/PDF、图音视频和 GIF 处理已接。描述补偿由原生有租约 outbox 的 claim/complete 管理。 | 投递只使用已保存描述与真实宿主消息身份。 |
| Observability：可观察性 | 结构化日志、OpenTelemetry、原生 Subject 诊断和模型账本关联请求、作业、Source、attempt 和用量；HTTP/CLI/MCP/SDK共用诊断。 | 未知结果及历史托管调用分别呈现。 |
| DataLifecycle：数据生命周期 | 来源/对象预览、操作记录、回收站、恢复、永久清除、Subject 迁移及 Namespace ZIP/分块备份已接；outbox 与原生精确引用随副本重绑，失效租约清除。 | 语义共享包、Namespace 备份和安装备份保留各自范围。 |
| Interfaces / Clients：接口与客户端 | CLI、HTTP、MCP、SDK 与可选 A-Memorix gRPC 消费同一原生业务；真实 MaiBot Records、画像、图、调优和上下文入口已有使用结果。 | 原有工作台只维护现有读取与操作合同。 |
| Locale：国际化 | Fluent 与客户端 locale 支持中英文；模型实际解释消费调用者语言，Symbols 恢复使用原响应及当前合同。 | 地区与语言按真实调用者参数处理。 |

## 认知对象与成长能力

这些对象共享精确 `CognitiveRef`、证据、来源、时间和修订，但各领域拥有自己的业务规则。下列“已有”包括代码及公开合同；有实际案例的项目单独说明。

| 对象或能力                                  | 已有内容                                                                                                        | 仍需完成的目标                                                                                      |
| ------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Memory 与 MemorySemantics | 正文、媒体、认识立场、接触与形成、精确来源和修订共用 Native Memory；真实文本、视觉向量及 Host 续接已运行。 | 使用质量与规模按真实场景继续测量。 |
| Concept / Tag | 稳定概念、别名、成员、吸积、合并拆分与 lineage 已纳入原生查询和成长，VCP 消费同一授权图。 | 正式质量比较仍需公开基准。 |
| Entity / Person | 身份、属性、平台认识和来源描述独立保留；生成画像具有自己的证据，当前读取过滤失活部分。原著副本免费刷新、撤回、清除和旧格式迁移已验证。 | 真实长期人物认识效果由后续使用观察。 |
| Association | 独立有向关系与证据、权重及立场进入 Native 图；与 Memory 共用 Retention 状态，九项前提保护实际删除计划。 | 复杂图查询与长期维护效果按实际负载验证。 |
| Episode | 轨道、成员、层级、局部重分段、迁移状态和来源处理进入 Episode owner；重建和批处理使用同一 Execution。 | 扩大真实经历使用场景。 |
| Journal | 时期、知情截点、视角和 Episode 引用由原生合同与采纳校验管理，进入成长与查询。 | 真实日记与长期叙事使用仍需扩展。 |
| Schema | 命题、范围、边界、支持与反例、立场和 lineage 由原生形成、修订与提案处理。 | 反例和使用效果按实际材料继续验证。 |
| FactClaim | 人物/会话范围、适用区间、冲突、撤回/恢复和画像联动已接；不同有效窗口独立，当前目录不混入旧已撤回修订。 | 更大真实数据规模的行为与效果继续观测。 |
| EgoFacet、NarrativeThread、DomainKnowledge | 种子材料、提案读取/编辑/独立分组采用、自我侧面、叙事和知识边界已有原生路径；原第三回六项认识与四项Ego实际导出。 | 人格的长期成长质量仍需真实连续使用。 |
| SocialRelation 与平台身份 | 关系、维度、信任、情境和社会群体有原生合同；身份假说、链接和撤销保留来源证据，Context按实际对象和关系门槛供给。 | 跨平台真实主体使用场景继续扩展。 |
| LanguageConvention | Symbols统一目录、选择、组合解释、媒体依赖、语言和保留响应恢复；语义解释保存为局部工件，原始付费状态保持。 | 更广泛真实社会语境下的理解质量继续观察。 |
| Desire、Concern 与条件 | 达到、保持、避免与停止使用区间/三值评价；真实语义判断、显式保存、精确条件历史、coverage/gaps和Context读取已运行。 | 持续评价效果依赖后续真实事件和证据。 |
| 成长与维护 | 接触、形成、定时维护、提案分组、审阅采用、反馈及失败恢复接到原生owner；失败动作保留并可人工编辑。 | 长期形成与遗忘效果需要连续使用验证。 |

## A-Memorix 工具和动作

[上游公开工具及动作](../../research/a-memorix-contracts.json)由 Python 适配器映射到原生 owner，CLI `amemorix`、HTTP、MCP 与 SDK 使用同一分发。当前主路径如下：

- 检索、写入、摘要、人物读取、统计、图、Episode、Fact、画像管理、导入和 Bundle 已有正式分发；人物状态和待办处理进入原生执行服务。
- Memory 和 Association 共用 Retention 状态、保护、强度、冻结、遗忘和恢复；V5 status 与维护动作使用真实记录状态。
- 来源/对象管理支持 source、paragraph、entity、relation、mixed，提供预览、操作历史、恢复和清除；九项关系状态前提在原生写事务重新核对。
- 调优使用调用方提供的查询和标签、Optuna 与 ir-measures，显式 apply/rollback；纠错与模糊修改使用原生 plan/apply/rollback 和精确修订。
- runtime 的诊断、空间目录、进度、恢复、重建、回填和持久状态均消费原生 owner。原生配置修改使用 configuration；关闭每次事务持久化会被明确拒绝。
- LPMM、OpenIE、MaiBot 数据与时间回填进入原生导入所有者；主机文件传输由 SDK 的 transfer_root 约束。反馈的队列、状态与回滚使用原生 Feedback 和 Context。

可选 gRPC 已接 Auth、Namespace、Memory、Job、Backup 的发布协议，认证和 Source 范围由同一身份所有者检查。Namespace 备份明确声明 `nous-namespace-backup` ZIP 格式及 producer 版本，保存原生材料、作业、Context 和状态；跨 Namespace 副本重新分配身份，旧付费调用进入历史托管。

## 现有能力和剩余范围

当前核心能力已进入本轮发行安装阶段：正式 API、CLI、MCP、SDK 和可选 RPC 共用原生所有者；材料/认知/Source/模型/向量/作业的状态和权限已连接。人物的来源描述与自动画像分开保存，Source 失活过滤派生部分；原著第三回已形成六项实际认识，原实例与模型历史保留。

已运行的真实链路包括隔离 MaiBot 的写检、Records 详情与历史、画像和调优，原生 Host 的真实模型调用与重启重放，文学种子的逐组读取/人工编辑/采用，愿望评价的真实语义调用、保存及 Context 读取，以及真实媒体的描述、社会符号解释和向量投影。具体小样本保持各自原件、候选池和参数规模。

Windows 完整包已安装并读取；剩余为 Linux/macOS 对应平台验证及官方文本基准的检索和评分。官方 LongMemEval 六题共享282会话池已完成Lexical/Dense/Hybrid各6次：Recall-any/all@10均为1，MRR分别0.875000/0.861111/0.916667，nDCG@10分别0.859495/0.892701/0.904644。VCP首题缺图资产失败，其余5题未执行。实际规模、原延迟与调用统计见 `dist/nous-first-evaluation-20261031/README.md`。Heptalogos 的持续世界、生活和外部行动仍是独立产品工作；本轮维护既有工作台消费，不新增工作台功能。

## 当前施工与未决问题

修订写入和可选择条件由 Cognition 统一；原生目录、Graph、Context、Fact、Retention 与 Source 清除使用同一状态和权限。Entity 的原始描述和别名保留来源断言，自动人物摘要保存独立证据，旧存储修订按真实 cause 迁移。Context 会拒绝与当前有效 Entity 字段不一致的旧准备。

模型角色、多路线与整目录空间已经接线。生成请求保留实际合同、提供方响应格式与物理状态；只读恢复不会把失败调用改成成功。向量恢复固定已接纳目录、同一提供方实际模型与空间，成功物理批复用；已证实的 alias/actual 型号映射已由空间所有者统一，实际重开目录 complete=True，未重发模型调用。外部 MemeCap 原图检索的实际范围为 3 张图、候选池 3，原始 caption 的 Dense top1 为 3/3；记录见 `temp/public-benchmarks/memecap/image-retrieval-results.json`。

最终包采用标准 CPython 3.14，服务/SDK/Native 分开分发源码与 wheel；Windows 中文空格目录已独立安装全部三包及可选 gRPC，HTTP/SDK、CLI 和 MCP（139 个工具）读取现有 README 成功，RPC 读取 Namespace 成功，服务均正常关闭。官方基准消费上游原始数据与现成评分。Source N+1修复后同首题无profile的SDK测量为1.967892秒、HTTP span1.811654秒、238条SQL；50条exactref及顺序不变，模型账本37→37，原六题正式延迟保留。
