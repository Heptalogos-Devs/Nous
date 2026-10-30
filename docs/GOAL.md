# Nous Wave 开发目标与计划

建设可独立安装、配置和管理的长期认知服务：支持普通智能体的记忆与知识工作，也作为人工主体的认知与人格中心。完整产品需求由[项目宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)定义。本计划统一开发范围、有效技术决定和当前工作；[重构进展](python-native-plan/progress-report.md)记录本次重构的实际成果与证据。

Heptalogos 以 MaiBot 为产品基础，负责主体的世界、生活、时间、注意、选择、行动与外部效果，正式依赖 Nous 的认知接口。Nous 自己保持独立部署和通用应用价值；两者共享逻辑主体身份，各自拥有正式状态和生命周期。

## 完整产品范围

- **材料、来源与时间**：接纳文本、文件、图片、音视频和外部资料，区分原件、精确区域、派生表示、主体实际接触与导入。记录证据、认知身份及修订、来源支持和反证、五轴时间、精度与历史知情截点；来源撤回、恢复和清除贯穿全部派生数据。
- **认知及成长**：完整提供 Memory、MemorySemantics、Concept/Tag、Entity、Association、Episode、Journal、Schema、Fact 和各自生命周期。Self 包含多文档 Seed、facet、NarrativeThread、DomainKnowledge、例外与设定来源；Social 包含人物、跨平台身份、有方向的关系、信任领域与群体范围；LanguageConvention 保存语言的听众与语用；Motivation 包含 Desire、Concern、DesiredCondition、组合条件、持续区间和 unknown。完成自然形成、认知修订、使用反馈、离线整理和持续维护。
- **查询与算法**：完整 NousQL、14 类返回域、精确引用、名称绑定、五轴过滤和历史；中文词项、语义、Hybrid、VCP、图、媒体与来源展开共同使用授权快照。吸收 VCP 的 AGS、EPA、Pyramid、Sense、双场、RiverMemo、DTSC 和 Thinking，并完成实际图资产、候选、直接锚点、查询线索、诊断与效果评估。
- **多模态**：三种解释策略 `description_only`、`direct_structured` 和 `describe_then_structure`；二段形成同时使用原媒体或区域及真实描述。提供图像、音视频、GIF、文档与区域派生，保留输入、producer 和谱系；原生视觉向量、描述向量及图文联合检索采用真实空间与调用身份。
- **当前认知与工作连续性**：Active Cognition、ResidentSet、WorkContext、Managed Context 和 Steward 按用途、听众、预算与来源编译输入。区分准备、实际输入接纳、模型接触、真实使用与结果反馈；覆盖模型切换、工具续轮、压缩、恢复及独立执行者。
- **A-Memorix 与 MaiBot**：完整承接[上游工具、管理动作和实际协议](../research/a-memorix-contracts.json)，包括文本、摘要、图片、检索模式、图、来源、Episode、画像、Fact、运行管理、导入、Bundle、调优、修正、删除和恢复。依照实际客户端需要支持独立 RPC；通过真实 MaiBot MemoryService、工具与管理界面消费同一认知权威。
- **产品与基础服务**：完善实例及跨平台安装、配置、模型资源与预算、Prompt、网络、权威事务与内容、索引、时间、持久执行、身份与密钥、能力扩展、媒体、可观察性、数据生命周期、接口与客户端、国际化。CLI、HTTP、MCP 和 Python SDK 提供领域操作；工作台源码暂时保留，后续按真实用户需要决定。提供备份、恢复、主体迁移、可理解状态及 Windows、Ubuntu、macOS 的发行与运行体验。

## 架构与实现

Python 主应用拥有认知规则、SQLite 权威数据和事务、来源授权、模型与网络、配置、Prompt、作业及产品入口；同进程 Rust native 负责 VCP、Tantivy/Jieba、USearch 和 NousQL 纯语法。PyO3/maturin 使用批量数组和 Rust 拥有的快照，计算释放 GIL 并在线程执行；native 不访问业务数据库、Provider 或凭据。权威对象及其消费者通过清晰的领域 owner 共享规则，派生索引可以重建。

采用标准 CPython 3.14、uv workspace、Basedpyright strict、Ruff；SDK 保持 Python 3.11+，native 提供 `.pyi` 和 `py.typed`。SQLAlchemy Core、aiosqlite 和 DBOS 共享 SQLite 文件，业务结果和后续入队在同一事务提交。模型生成与 Provider 协议采用 Pydantic AI、官方 SDK 和成熟实现；Jinja2、FastAPI、Typer/Rich 与官方 MCP SDK 分别承担模板和入口机制。项目负责领域语义、实际调用及费用、授权与完整用户路径。各项设计见 [Python/native 架构](python-native-plan/README.md)。

在当前 Python 模块中完善实际能力，优先使用成熟依赖；旧实现仅供研究，可按实际需要吸收算法思路或必要的纯计算。A-Memorix 的专有公共合同由边界适配器提供，领域与数据库保持 Nous 自己的模型。公开客户端和真实持久数据按实际边界处理，保留来源和历史。

## 实施依赖

以完整用户链组织工作：初始化实例和主体 → 接纳真实资料与来源 → 形成并采纳认知 → 实际 embedding 和索引 → 查询、展开来源 → 重启后继续。Catalog、Unit、Output、文件成功、模型调用与任务保持各自真实身份和完成条件。

在已走通的基础路径上，完成统一修订状态与模型路由，再补齐 A-Memorix 管理动作、完整查询和认知领域，以及宿主每次调用的上下文和长期维护。各功能同时接通实际 SDK、CLI、HTTP、MCP、MaiBot 或管理界面消费者。继续完善产品管理和对应平台的构建、安装与运行。根据实际独立故障、关键业务行为与真实使用选择验证；已有成功的付费调用在恢复时复用。

## 当前工作

正式开发基线为 `main`。真实文本、图像、动态媒体、模型形成与向量、来源、重启、MaiBot 画像、Episode 和 Managed Context 已有实际使用结果。VCP 请求意图、Fact、Context Source 工具及 SDK/MCP 可读输出已有真实运行记录，相关实现已保存到 WIP 代码检查点 `67e09cd`；此前正式功能节点为 `68fc3ae`。

下一项先完成统一 `RevisionState` 的消费者接线和重新加载，实际验证 freeze、restore、Fact retract 及查询结果。模型 Role 多路线还需向应用注入 `RoleRegistry`，将向量投影的整个输入目录与分批统一到同一模型空间，并完成实际 Provider 路径。运行实例加载的是此前代码，检查点源码尚不能直接启动。随后继续未完成的 A-Memorix 消费路径、完整查询及各认知领域。

日常开发与启动见[项目 README](../README.md)，安装、服务管理与恢复见[安装说明](python-native-install.md)。开发缓存和实例使用 `temp/`；旧实例独立保留。
