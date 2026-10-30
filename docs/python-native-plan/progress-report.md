# Nous Python 重构进展

## 总体情况

Nous 已经从原 Rust 应用的暂停交接，发展出一套以 Python 为应用主体、在同一进程中使用 Rust 原生计算的认知服务。真实开发讨论、图片与 GIF、模型形成与向量、查询和来源、重启续用、MaiBot 画像和上下文等路径已经实际运行。当前成果是一个有真实用户链路的开发中系统；完整认知产品、A-Memorix 全部既有能力、产品管理和发行仍有大量工作。

本报告记录截至 2026-10-30 的观察，以当时 `codex/python-native-rewrite-cloud` 上的实现代码检查点 `67e09cd` 为观察点；此前的正式功能节点为 `68fc3ae`。VCP 请求意图、Fact、Context Source 工具回执、统一修订状态、SDK/MCP 表达和模型路由已保存到 WIP 检查点。运行中的本地服务加载的是此前的实例代码，新模型服务尚未完成应用接线，检查点源码不能直接启动。各真实结果对应实际运行时使用的代码，修订状态与模型路由仍待最终集成和验证。

## 项目要完成什么

[项目宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)将 Nous 定位为独立的长期认知基础设施与人格中心：保存材料、经历、记忆、证据、来源、时间和修订；提供 NousQL、多方式检索、概念与关系、形成与维护；进一步支持自我、社会认知、语言、动机和工作认知。普通应用可以只使用记忆与检索，无须先建立人格。Nous 还要完整承接 A-Memorix 已有的实际工具、管理和消费接口。

Heptalogos 是基于 MaiBot 的下一代持续主体产品，拥有世界、生活、时间、注意、选择、行动和外部效果，正式依赖 Nous 提供认知。MaiBot 的消息、平台、插件、模型资源与管理体验是主体产品的重要基础。本次重构重点是 Nous 及其真实消费者接入；MaiBot 接入取得了进展，完整 Heptalogos 世界与主体运行仍待建设。

[开发目标与计划](../GOAL.md)明确完整功能范围，并将主应用改为 Python：权威数据库、事务、认知规则、模型与持久任务归 Python；VCP、原生索引和 NousQL 纯语法留在同进程 Rust。

## 架构如何变化

起点是旧分支 `codex/context-infrastructure-sdk` 的 `2458932646d6b2968db38d1e0a97103f93cd2cad` 交接。其文件接纳、导入、来源生命周期和 Bundle 有未完成工作，也保有真实资料和模型历史。迁移保留原源码作为参考，按业务所有者及其消费者重组，旧工作区数据未被当作新实例的默认数据库。

现在的应用由 `src/nous` 组装：Python 拥有 Namespace、Subject、材料、认知、来源、查询计划、上下文、导入、配置、模型与任务等业务责任；HTTP、MCP、CLI 与 A-Memorix 协议进入相同领域操作。公开 Pydantic 合同由 Python SDK 维护。`native/` 提供 PyO3 边界、VCP 计算、Tantivy/Jieba 中文索引、USearch 向量检索及 NousQL 语法解析；原生计算接收授权后的数据和索引快照，业务数据库、凭据、模型网络与正式领域状态由 Python 管理。

SQLAlchemy Core 和 aiosqlite 使用 SQLite 保存权威状态。DBOS 与业务表共用同一 SQLite 文件，业务结果和后续任务接纳由同一事务提交；网络、模型和耗时媒体处理在短事务外执行。模型由 Pydantic AI 和官方 SDK 承担提供方协议，应用管理角色、Prompt、调用身份、来源、结果与费用；出站网络策略和客户端生命周期集中到 NetworkService。uv workspace 管理服务、Python SDK 和 native 扩展三个发行单元，服务使用标准 CPython 3.14，SDK 保留 Python 3.11 及以上范围。具体设计见[架构](design/architecture.md)、[事务与作业](design/transactions.md)、[原生边界](design/native-boundary.md)和[公共基础能力](design/platform.md)。

旧 `crates/` 仍在仓库中供研究。后续以当前 Python 模块实现产品能力，只按实际需要吸收算法思路或必要纯计算。

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

Fact 实现已通过实际 MaiBot 创建、跨 key 更新、撤回、恢复、读取和列表，并完成 Native 与 A-Memorix 纯事实包往返；代码已保存到 WIP 检查点。Native 保存两个事实身份、五条修订；portable 包包含两条 claim、五条 evidence、五条 transition，保留状态、时间和来源。跨 key 更新按上游语义执行 retract 加 assert。见 `actual-maibot-fact-*` 和 `actual-fact-roundtrip-summary.json`。普通 NousQL 曾默认返回已撤回 Fact，统一可见性修复仍需在新代码加载后验证。

### MaiBot 模型调用与上下文

真实 MaiBot 克隆使用 `ConfigManager`、`ChatManager`、`LLMServiceClient` 和 `OpenaiClient`，在模型调用前取得非空 Nous 认知，按实际完整 provider 输入测量并接纳上下文，调用结束后记录真实模型结果。重建执行者后，已接纳的 track 与游标能够继续。模型的真实 `memory_source` 工具经过两轮调用并保留原有宿主历史，成功读取已接纳认知的来源后产生一条 actual-use 事件。

接入过程中修复了反馈丢失实际 Provider 和模型身份的问题；结果包含真实 provider、模型、response 和 token 使用量。相关补丁在 [managed-context.patch](../../integrations/maibot/managed-context.patch)，记录见 `actual-maibot-managed-resumed.json`、`maibot-managed-metadata-fixed-result.json`、`actual-maibot-tool-use-events.json`。MCP 的 `query_cognition` 也已实际返回可读正文、短引用和完整 `structuredContent`，见 `actual-mcp-readable-query.json`。

以上实际记录均位于 `temp/review/python-local/`。运行实例、原件、付费调用账本与历史保存在本地实例及该目录中。

## 十六项基础服务

下表说明各基础服务的实际 owner 和接线。已实现的操作以相应源码为准；列出的真实运行只覆盖具体场景，剩余目标继续按[开发目标与计划](../GOAL.md)完成。

| 服务                                                | 当前 owner 与已接主路径                                                                                                                                    | 尚需完成或确认                                                                                         |
| --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Instance / Platform：实例与跨平台                   | `platform/instance`、`bootstrap.py` 管理实例路径、初始化和锁；CLI 提供服务安装及启动停止。三包安装和独立实例已运行。                                       | 新工作区的角色注册尚未接线；逐平台核实当前版本的安装、服务管理、升级与迁移，完成可交付发行。           |
| Configuration：配置                                 | `platform/configuration` 提供类型化配置、注册、实例/Namespace/Subject 有效值、来源、历史及管理入口；网络、检索、媒体、模型和上下文消费实际配置。           | 模型角色新注册表与配置 validator/schema 的应用组合仍在施工；继续完善配置生效和管理体验。               |
| ModelResources / ModelCalls：模型资源与调用         | `platform/models` 管理 Provider、Model、Route、Role、Prompt 输入快照、真实调用与用量。实际生成、embedding、媒体及重排已使用该服务，成功结果可恢复复用。    | 多路线、每次物理调用与安全回退在施工；需完成应用注入、整目录向量空间及实际 Provider 验证。             |
| Prompts：提示词                                     | `platform/prompts` 和 `resources/prompts` 提供随包资源、用户修订、激活、Jinja 渲染和预览；实际形成、媒体、画像、Episode、Steward、维护及重排使用各自合同。 | 继续覆盖全部模型职责，并让管理界面同步新路线预览；预览与实际请求必须保持相同输入及输出合同。           |
| Contexts / ModelInput：上下文与模型输入             | `context` 保存准备、接纳、游标、居民、工作上下文和反馈；SDK managed binding 与真实 MaiBot 模型输入和来源工具已运行。                                       | 统一新修订可见性，完成切模型、压缩、各类宿主工具续轮及子执行者路径，完善真实使用反馈。                 |
| Network：网络                                       | `platform/network` 集中 HTTPX 家族客户端、连接池、代理、CA、TLS、超时与生命周期；模型、向量及提供方请求使用配置快照。                                      | 继续核实各实际 Provider 和外部资源的协议适配及受限网络行为。                                           |
| Authority / Objects / Indexes：权威存储、内容与索引 | `platform/persistence`、`platform/content` 和 native 检索分别拥有 SQLite 事务、不可变内容和可重建索引；领域写入及 DBOS 入队同库提交。                      | 完成修订状态统一、各数据生命周期和大规模索引发布恢复；数据库是权威，索引及缓存需按权威状态失效或重建。 |
| Clock / Scheduler：时间与调度                       | `platform/time`、公开时间合同及 DBOS schedule 支持物理时间、认知时间、五轴查询、时区与定时任务；历史截点已有实际查询。                                     | 完整处理不确定精度、跨时间域和实际使用场景；世界日历和时间推进由 Heptalogos 负责。                     |
| Tasks / Execution：持久执行                         | `platform/execution` 使用 DBOS 队列和持久 workflow；导入、形成、向量、Episode 与维护有任务、等待和恢复路径。                                               | 补齐所有长期工作、取消与多执行者行为；向量新路线恢复必须保持既有实际调用意图。                         |
| Identity / Secrets：身份与密钥                      | `identity` 管理 Namespace、Subject、凭据、权限和来源约束；凭据轮换撤销及受限来源读取已有实现。模型凭据通过受保护引用读取。                                 | 继续覆盖所有消费者的授权、跨主体和迁移场景，完成完整身份和扩展权限体验。                               |
| Capabilities / Extensions：能力与扩展               | 应用组合注册配置、角色、处理器和 Subject 相关能力，领域通过正式入口供给。                                                                                  | 完整的第三方扩展发现、安装、启停、健康和生命周期仍是产品目标；目前的内部注册不应当作完整插件平台。     |
| Content / Media：内容与媒体                         | `platform/content`、`platform/media`、`material` 管理原件、区域、派生、谱系和 FFmpeg/Pillow 处理；图片、GIF、视频及音频路径有真实使用记录。                | 继续覆盖更多媒体和提供方、精确区域授权、资源限制与端到端消费。                                         |
| Observability：可观察性                             | `platform/observability`、结构化日志、OpenTelemetry 接线和模型账本关联请求、任务、调用、来源与实际用量；管理入口提供诊断。                                 | 完善跨活动追踪、运行状态和告警，以及费用和未知结果的完整操作者体验。                                   |
| DataLifecycle：数据生命周期                         | `data_management`、`imports`、材料和认知 owner 协作处理来源、导入、包、删除恢复与在线备份；Native/A 包及部分跨主体安装有实际结果。                         | 补齐完整级联、保留、清除、主体迁移及全部 A-Memorix 操作，确保各派生副本跟随权威生命周期。              |
| Interfaces / Clients：接口与客户端                  | `transports/http`、`cli`、`mcp` 和 `sdk/python` 共享合同；工作台与 MaiBot 补丁消费同一服务，MCP 可读文本及机器结构已实际返回。                             | 完整覆盖所有领域操作、A-Memorix 管理页面、按实际消费者需要的协议和一致错误；当前部分入口仍在开发。     |
| Locale：国际化                                      | `platform/localization`、Fluent 资源和客户端语言选项提供中英文文本；工作台和 CLI 已有本地化路径。                                                          | 补齐新增功能、错误、帮助和地区格式，持续核对全部管理路径。                                             |

## 认知对象与成长能力

这些对象共享精确 `CognitiveRef`、证据、来源、时间和修订，但各领域拥有自己的业务规则。下列“已有”包括代码及公开合同；有实际案例的项目单独说明。

| 对象或能力                                  | 已有内容                                                                                                        | 仍需完成的目标                                                                                      |
| ------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Memory 与 MemorySemantics                   | 保存正文、摘要、媒体、角色、形成方式、认识立场与精确来源；Memory 维护和语义形成有真实文本与媒体案例。           | 持续完善使用反馈、强度与遗忘策略、时间和完整消费者管理。                                            |
| Concept / Tag                               | 有稳定概念、别名、成员、合并拆分和 lineage，查询可绑定名称；实际 VCP 使用概念线索。                             | 完成语义吸积、合并拆分及成员变更的完整真实工作流。                                                  |
| Entity / Person                             | 有人物及其他实体、属性证据、平台身份、提及和画像人工控制；MaiBot 画像及真实刷新已运行。                         | 补跨平台身份解析、人物关系、完整证据纠正及管理消费者。                                              |
| Association                                 | 有有向端点、极性、证据、可选 weight 和认识立场；正式图管理与真实图资产已运行。                                  | 完成独立证据根、关系演化与更复杂图查询；历史端点保持精确身份。                                      |
| Episode                                     | 有轨道、成员、父级、边界说明、lineage 和重分段合同；三条 Memory 形成三个 Episode 并经 MaiBot 读取。             | 完成轨道层级、局部重分段与完整管理、持续形成。                                                      |
| Journal                                     | 有叙事、时期、知识截点、观点、Episode 引用与 lineage 合同，参与认知形成与成长。                                 | 验证写作时知情视角、日记生成与后续修订的真实链路。                                                  |
| Schema                                      | 有范围、命题、边界、支持、反例、lineage 和立场；成长路径能提出并采纳 Schema。                                   | 完整实现、验证反例、形成、修订及使用规则。                                                          |
| FactClaim                                   | 有人物/会话范围、值、状态、有效期、证据及修订；实际 CRUD 与两个格式往返已运行，代码保存在 WIP 检查点。          | 完成默认查询和各消费者统一可见性，以及完整事实纠正、冲突和画像联动。                                |
| EgoFacet、NarrativeThread、DomainKnowledge | 已有独立认知正文、成长提案和 Ego/Seed 操作；SDK 可采纳种子、读取 Ego 画像。                                   | 完整验证多文档种子进入记忆、设定来源、人格例外、叙事身份及知识范围的长期演化。                      |
| SocialRelation 与平台身份                   | 有带方向、关系维度、程度、信任领域、范围、群组和判断充分性的合同；存在身份链接和社会关系操作。                  | 完成真实多会话、跨平台对应、关系积累与独立证据路径。                                                |
| LanguageConvention                          | 有表达、意义、听众、语用和情境的认知合同及成长提案。                                                            | 接入真实群体语言形成、适用范围、修订与宿主消费。                                                    |
| Desire、Concern 与条件                      | 有愿望、关切、组合条件、achieve/cease/maintain/avoid、持续区间和 unknown 状态合同。                             | 完成条件评价、实际 coverage、来源与持续更新，并交由 Heptalogos 使用；认知本身不代主体创建外部承诺。 |
| 成长与维护                                  | 持久作业可以生成、审阅、采纳或拒绝提案，采纳检查原修订；定时计划已有入口，Schema/Concept 形成和采纳有实际例子。 | 补齐各领域独有策略、反思、离线整理、可观察结果和使用反馈，不把任务完成视为认知自动采纳。            |

## A-Memorix 工具和动作

对照[上游工具及动作清单](../../research/a-memorix-contracts.json)，当前 Python 适配器将已接操作转到同一材料、认知、任务和来源 owner。下表是当前源码覆盖；其中画像、Fact、Episode、图、包和上下文的实际运行证据见上文，其他动作仍需按真实上游消费者逐项完成。

| 工具                                           | 已接范围                                                                                                                                                                    | 未完成或需要继续核实                                                                                                                                                           |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `search_memory`                                | search、time、hybrid、episode、aggregate 均有查询分派，支持人物、聊天、时间与来源过滤。                                                                                     | 逐模式核实上游返回语义、过滤边界和实际消费者。                                                                                                                                 |
| `ingest_text`、`ingest_summary`                | 接纳正文、来源、外部身份、参与者和时间，并按配置提交形成。                                                                                                                  | 覆盖上游各输入与错误语义，以及完整多模态和宿主接入。                                                                                                                           |
| `get_person_profile`、`memory_stats`           | 人物画像和授权范围内的诊断统计已有读取；画像有真实 MaiBot 消费。                                                                                                            | 完善统计字段、画像的全部上游条件与未知项。                                                                                                                                     |
| `maintain_memory`                              | reinforce、weaken、protect、unprotect、freeze、forget、restore、trash、purge、decay、recycle_bin 进入认知生命周期。                                                         | 完成状态统一后验证冻结、恢复及其他入口的共同效果。                                                                                                                             |
| `memory_graph_admin`                           | search、get_graph、node_detail、edge_detail、create_node、rename_node、delete_node、create_edge、update_edge_weight、delete_edge 已接。                                     | 继续核实所有参数、授权和管理 UI，完善历史端点与图资产消费。                                                                                                                    |
| `memory_source_admin`、`memory_delete_admin`   | 来源 list、delete、batch_delete；source 模式的 preview、execute、restore、purge、get_operation、list_operations 已接。                                                      | 非 source 删除模式、延迟级联及完整上游删除合同仍需对应 owner。                                                                                                                 |
| `memory_episode_admin`                         | query、list、get、status、rebuild、process_sources 已接；真实形成和重放已运行。                                                                                             | `discard_migration_backfill` 及完整迁移、局部重分段行为待完成。                                                                                                                |
| `memory_profile_admin`                         | list、query、evidence、get_aliases、set/delete_aliases、set/delete_override、correct_evidence 与显式刷新已有路径。                                                          | 上游 `status`、`process_pending` 不在当前输入合同中，需完成相关后台管理流程。                                                                                                  |
| `memory_fact_admin`                            | create、update、retract、restore、get、list 已有实现和真实操作，代码保存在 WIP 检查点。                                                                                     | 完成通用查询状态与其他消费者一致性，扩充上游场景。                                                                                                                             |
| `memory_runtime_admin`                         | get_config、list_vector_spaces，以及通过配置 owner 的 set_config 路径。                                                                                                     | 上游 self_check、refresh_self_check、recover_embedding、rebuild_all_vectors、paragraph_backfill_once、save、set_auto_save 等仍需接入实际 owner。                               |
| `memory_import_admin`                          | settings/get_settings/get_guide、list/get/chunks/get_chunks、cancel、retry_failed、create_upload/create_raw_scan/create_paste 已接；路径别名和解析由宿主 SDK 文件管理提供。 | 上游 LPMM 转换、OpenIE、MaiBot 数据迁移、temporal backfill 的独立创建动作与完整消费者仍待接续。                                                                                |
| `memory_bundle_admin`                          | export、import、inspect、list、resolve_file、uninstall 已接；原生及上游便携包已有真实往返。                                                                                 | 完善所有选择、冲突、卸载及更广泛历史资源场景。                                                                                                                                 |
| tuning、V5、correction、fuzzy modify、feedback | SDK 的 `invoke` 将 V5 的 reinforce、weaken、remember_forever、restore、forget、recycle_bin 等维护动作转入 `maintain_memory`；相关底层生命周期和认知修订有原生能力。         | V5 的 status 和服务端完整分发，以及 `memory_tuning_admin`、`memory_correction_admin`、`memory_fuzzy_modify_admin` 尚待实现；feedback 的 get/list/rollback 也需按上游入口落实。 |

独立 A-Memorix gRPC 中 Namespace、记忆、任务、认证和备份等 RPC 也在清单中；需要以实际客户端和协议要求完成支持。Python 当前的 HTTP、MCP、CLI 和 SDK 操作不应被当作这组 RPC 全部已经迁移。

## 现有能力和剩余范围

| 能力                        | 当前情况                                                                                                                            | 继续工作                                                                                                                                                                                                                                                                            |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 实例和基础服务              | Python 应用、实例目录、授权、配置、Prompt、网络、持久任务、备份恢复和多入口已有实现；部分真实链路已运行。                           | 完成各入口与管理界面的全功能接线、安装和真实使用。                                                                                                                                                                                                                                  |
| 材料、来源与导入            | 有原件、区域、派生、Observation、Catalog/Unit/Output、文件成功、来源管理及 Native/A 包；文本和媒体有真实往返。                      | 按真实消费者补齐全部格式、来源生命周期、删除恢复、包选择与共享资源行为。                                                                                                                                                                                                            |
| 查询和原生计算              | 中文词项、语义、Hybrid、VCP、NousQL 解析、名称绑定、时间/历史、来源展开和图操作已有；真实图与若干查询已运行。                       | 完成目标中的全部 14 类返回域、语言与历史语义、算法组合、质量和规模验证。精确引用和受限图必须持续使用同一权威状态。                                                                                                                                                                  |
| Memory、Episode、画像、Fact | Memory 与修订、Episode 形成和读取、画像人工控制与刷新、Fact 往返各有实际例子。Fact 及统一可见性代码已保存为 WIP，后者尚待重载验证。 | 完成 Episode 轨道、层级、局部重分段以及全部事实、画像和宿主管理行为，完成当前状态修复的真实验证。                                                                                                                                                                                   |
| 其他认知领域                | Journal、Schema、Concept、Association、Ego、Social、Language、Motivation 及成长维护已有领域合同和部分实现。                        | 按各领域补齐并验证多文档人格种子、叙事身份、知识范围、社会关系与信任、语言惯例、愿望及条件、图式反例和 lineage、反思、自然形成及真实使用反馈。                                                                                                                                      |
| 当前认知与工作连续性        | 持久准备、预算准入、模型接触、实际使用、结果反馈以及真实 MaiBot 例子已接通；Work/Resident/Steward 有相应实现。                      | 补齐模型切换、压缩、工具续轮、子执行者与不同宿主的完整路径，验证长期居民及持续反馈。                                                                                                                                                                                                |
| A-Memorix 消费者            | 基础写入、检索、维护、统计、画像以及图、来源、Episode、Fact、导入、Bundle、部分 runtime 管理已接入同一认知权威。                    | 顶层 `memory_tuning_admin`、`memory_v5_admin`、`memory_correction_admin`、`memory_fuzzy_modify_admin` 当前尚无服务端分发；runtime 的部分动作和某些删除模式也返回 capability unavailable。继续逐项接通[实际合同](../../research/a-memorix-contracts.json)、MaiBot 管理页和必要协议。 |
| 产品管理与发行              | 有中英文工作台、CLI、HTTP、MCP、SDK、安装脚本、服务管理及多平台 wheel 构建配置。                                                    | 扩充各领域管理体验；完成各平台实际安装与运行。Windows/macOS 的 runner 配置是构建路径，结果取决于对应 Actions 运行。                                                                                                                                                                 |
| Heptalogos                  | MaiBot 作为真实认知消费者已有补丁和实际模型调用。                                                                                   | 以 MaiBot 派生产品建设持续世界、生活、注意、自主行为、设备与效果，并完整继承插件和管理体验。                                                                                                                                                                                        |

表中的“已有实现”表示对应 owner 或入口存在；实际进度以当前代码与运行记录判断。完整范围和实施依赖见[开发目标与计划](../GOAL.md)。

## 当前施工与未决问题

### 认知修订状态与可见性

Fact 撤回后，普通 NousQL 默认结果曾仍包含该 Fact。问题涉及 Cognition、Retrieval、Graph、Context 和向量 Worker 对同一修订状态的判断。当前工作把最终 `RevisionState` 和可选择条件集中到共同 owner，并迁移消费者；写入最终 Record 后再发送通知，移除延后状态写入和外层覆盖。Context 消费者已迁移，存储侧代码已落盘。还需要完成组合、加载新代码，并通过实际 freeze、restore、Fact retract 等操作检验查询、上下文和派生工作的一致结果。

### 模型角色多路线

此前 Role 虽有 routes，实际只解析首条。当前工作正在建立冻结的调用计划和统一路由 owner：记录每一次真实物理调用及结果，父调用绑定真正成功的 Call；保存实际输入、来源和凭据范围；Pydantic AI 的底层 OpenAI 客户端显式关闭隐藏的 SDK 重试。明确未发送或特定可确认拒绝的情况可以切换路线；超时、断线等结果未知的调用保持未知并停止，避免误判后重复付费。既有成功或未知调用继续使用原意图，既有已接纳的单路线请求保留原事实。

角色 purpose 从平台硬编码集合转为应用注册；配置校验和 schema 使用同一个注册表。预览合同也已扩展为展示路线。定点类型检查和 Ruff 已通过，但尚未启动新应用或调用真实 Provider。[application.py](../../src/nous/application.py) 还需先创建、注册并注入必需的 `RoleRegistry`；因此当前工作区不能视为可重启完成。

向量 Worker 仍先按首选路线的 batch 大小切组，并分别循环文本和媒体。若各组回退到不同模型，整份投影可能混入不同语义空间。下一步要把完整逻辑输入目录、逐项来源、路线选择与物理分批交给模型 owner，固定整目录的同一空间；同时保留旧已接纳作业的物理调用意图，避免把恢复变成新付费请求。随后还需完成模型预览 UI、所有入口与真实集成验证。

### 后续依赖

先完成修订状态和模型路由的共同 owner、应用接线与向量目录契约，再在真实实例中验证修改和重启。随后沿 A-Memorix 未覆盖动作、MaiBot 实际管理消费者和完整查询合同补齐能力；认知领域与持续维护依照各自 owner 继续落地。按真实需要整理旧源码，并完成平台安装、管理和发行。真实原件、已发生的模型调用和费用、来源与历史应随这些工作继续保留。
