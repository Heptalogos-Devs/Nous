# Nous 开发目标与计划

建设可独立安装、配置和管理的长期认知服务：支持普通智能体的记忆与知识工作，也作为人工主体的认知与人格中心。完整产品需求由[项目宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)定义。本文件规定产品范围、架构要求和交付目标。

Heptalogos 以 MaiBot 为产品基础，负责主体的世界、生活、时间、注意、选择、行动与外部效果，正式依赖 Nous 的认知接口。Nous 自己保持独立部署和通用应用价值；两者共享逻辑主体身份，各自拥有正式状态和生命周期。

## 完整产品范围

- **材料、来源与时间**：接纳文本、文件、图片、音视频和外部资料，区分原件、精确区域、派生表示、主体实际接触与导入。记录证据、认知身份及修订、来源支持和反证、五轴时间、精度与历史知情截点；来源撤回、恢复和清除贯穿全部派生数据。
- **认知及成长**：完整提供 Memory、MemorySemantics、Concept/Tag、Entity、Association、Episode、Journal、Schema、Fact 和各自生命周期。Ego 包含多文档 Seed、facet、NarrativeThread、DomainKnowledge、例外与设定来源；Social 包含人物、跨平台身份、有方向的关系、信任领域与群体范围；LanguageConvention 保存语言的听众与语用；Motivation 包含 Desire、Concern、DesiredCondition、组合条件、持续区间和 unknown。完成自然形成、认知修订、使用反馈、离线整理和持续维护。
- **查询与算法**：完整 NousQL、14 类返回域、精确引用、名称绑定、五轴过滤和历史；中文词项、语义、Hybrid、VCP、图、媒体与来源展开共同使用授权快照。吸收 VCP 的 AGS、EPA、Pyramid、Sense、双场、RiverMemo、DTSC 和 Thinking，并完成实际图资产、候选、直接锚点、查询线索、诊断与效果评估。
- **多模态**：三种解释策略 `description_only`、`direct_structured` 和 `describe_then_structure`；二段形成同时使用原媒体或区域及真实描述。提供图像、音视频、GIF、文档与区域派生，保留输入、producer 和谱系；原生视觉向量、描述向量及图文联合检索采用真实空间与调用身份。
- **当前认知与工作连续性**：Active Cognition、ResidentSet、WorkContext、Managed Context 和 Steward 按用途、听众、预算与来源编译输入。区分准备、实际输入接纳、模型接触、真实使用与结果反馈；覆盖模型切换、工具续轮、压缩、恢复及独立执行者。
- **A-Memorix 与 MaiBot**：完整承接[上游工具、管理动作和实际协议](../research/a-memorix-contracts.json)，包括文本、摘要、图片、检索模式、图、来源、Episode、画像、Fact、运行管理、导入、Bundle、调优、修正、删除和恢复。依照实际客户端需要支持独立 RPC；通过真实 MaiBot MemoryService、工具与管理界面消费同一认知权威。
- **产品与基础服务**：完善实例及跨平台安装、配置、模型资源与预算、Prompt、网络、权威事务与内容、索引、时间、持久执行、身份与密钥、能力扩展、媒体、可观察性、数据生命周期、接口与客户端、国际化。CLI、HTTP、MCP 和 Python SDK 提供领域操作；工作台源码暂时保留，后续按真实用户需要决定。提供备份、恢复、主体迁移、可理解状态及 Windows、Ubuntu、macOS 的发行与运行体验。

## 架构与实现

一份 Namespace 拥有独立认知状态；`mode`、`subject_key`、`enabled_features` 表达主体能力与跨产品身份。领域操作拥有业务语义，共同记录能力拥有认知身份、修订、证据、时间、生命周期和读取。组合根构造明确依赖并管理资源生命周期。

Python 主应用拥有认知规则、SQLite 权威数据和事务、来源授权、模型与网络、配置、Prompt、作业及产品入口；同进程 Rust native 负责 VCP、Tantivy/Jieba、USearch 和 NousQL 纯语法。PyO3/maturin 使用批量数组和 Rust 拥有的快照，计算释放 GIL 并在线程执行；native 不访问业务数据库、Provider 或凭据。权威对象及其消费者通过清晰的领域 owner 共享规则，派生索引可以重建。

采用标准 CPython 3.14、uv workspace、Basedpyright strict、Ruff；SDK 保持 Python 3.11+，native 提供 `.pyi` 和 `py.typed`。SQLAlchemy Core、aiosqlite 和 DBOS 共享 SQLite 文件，业务结果和后续入队在同一事务提交。模型生成与 Provider 协议采用 Pydantic AI、官方 SDK 和成熟实现；Jinja2、FastAPI、Typer/Rich 与官方 MCP SDK 分别承担模板和入口机制。项目负责领域语义、实际调用及费用、授权与完整用户路径。各项设计见 [Python/native 架构](python-native-plan/README.md)。

原件和大字节保存于内容存储，权威状态与元数据保存于 SQLite。Wave 维护可重建的固定索引代际，各检索通道产生候选并融合。自然语言按原文查询，可选 QueryContext 提供真实情境，NousQL 解析显式查询语法。

可编辑 TOML 是业务配置的正式来源。模型执行统一绑定 Provider、Model、Preset、Role，编译有角色的消息并执行目的明确的局部任务。任务恢复复用保存的输入、结果和业务待办。

优先采用成熟依赖，保留 Nous 的领域模型与完整原生操作。A-Memorix 的公共协议由仅依赖正式 SDK 的独立 `adapters/amemorix` 发行包提供。保留真实资料、来源、历史及模型结果。

## 实施依赖

以完整用户链组织工作：初始化实例和 Namespace → 接纳真实资料与来源 → 形成并采纳认知 → 实际 embedding 和索引 → 查询、展开来源 → 重启后继续。Catalog、Unit、Output、文件成功、模型调用与任务保持各自真实身份和完成条件。

先统一原生职责、Namespace 和真实数据，再贯通模型与媒体、Wave、长期领域、主动上下文和后台成长。SDK、HTTP、CLI、MCP 随完整操作接通；独立 A-Memorix 适配器通过正式 SDK 提供上游协议，随后进行真实使用、小规模公开测评和安装交付。

## 交付要求

交付可独立安装、配置和持续使用的 Nous 源码、安装产物、用户指南及完整 SDK 示例。使用者能够接纳和查询真实文字与媒体，读取来源、修订认识、恢复任务，并在连续模型及工具调用中自动取得人格、活跃记忆与当前工作。

独立 A-Memorix 适配器连接本地或远程 Nous，提供实际工具、管理、导入导出及 RPC。SDK 单独安装即可支撑这一外部消费者。

使用真实资料及上游公开数据进行小规模测评，保留实际输入、配置、结果、耗时、用量和具体问题。修正阻断使用和导致错误结果的问题，保留原始资料与模型结果。

Heptalogos 本轮只交付 Host API、主体绑定和模型调用接缝的交接资料；主体内核另行开发。
