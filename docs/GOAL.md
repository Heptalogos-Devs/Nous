# Nous 当前实施目标

## 首要成果

交付架构、使用和流程都简单的独立 Nous。近期首先完成“真实输入 → 当前可用 → 有情境的认识形成与修订 → 检索/模型使用”这条产品主链。完整长期范围保留在后文；代码数量、机制名称、对象数量和局部可调用均不作为完成目标。

当前行为规范见 Architecture-Vault [输入输出与持续形成 Spec](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/docs/active-design-decisions/设计/Nous输入输出与持续形成Spec.md)，共享数据与模型合同见[原生认知设计](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/docs/active-design-decisions/设计/Nous原生认知与交互.md)。宪纲提供产品目的，具体结构由真实操作和职责决定。

## 当前设计决定

- CLI、MCP、HTTP、SDK 共用原生操作；接入能力可从工具逐步增加到真实事件和完整模型窗口。SDK 是访问手段，HTTP 和领域直接调用原生 owner。
- 作者、观察者、调用身份和模型协议角色各自表达。仅有模型请求时保存实际快照；发生过的消息、重放历史、背景和主体真正接触分别保存。普通自然语言始终合法。
- 原始资料/事件先可读可查并进入当前工作；长期形成读取有前文的完整片段及少量旧认识。程序管范围、身份、预算、来源和事务，模型提炼并比较语义。
- Memory 表达认识；Episode 组织原事件经过；Journal 用于主观回顾；Work 用于继续任务。一次输入按实际需要产生结果，空提案正常。
- 用共同 Material/Observation 接纳、Formation 情境形成及各领域采用替换重复接触和逐事件形成。复用已有 Authority、ContentStore、DBOS 和待处理接触合并，移除被替换路径，不增加总管所有输入的 Manager、事件层或调度器。
- 内部结构直接改定义、迁移全部消费者并删除旧接口、旧格式升级、别名和专属支撑。采用成熟实现的技术方法，不保护错误旧形状。

## 近期实施计划

### 统一事件与接触输入

负责位置：`sdk/python/src/nous_sdk/contracts/{runtime,contact,retrieval}.py`、`src/nous/material/observation.py`、`src/nous/imports/contact.py`、`src/nous/context/runtime.py`，以及 SDK hook 与现行 HTTP 入参。

产生一份共同事件/接触合同：真实内容或来源、稳定原事件身份、作者、观察者、会话/群体、已知发生时间/接触时间、回复/工具关联和实际背景。缺失信息保留未知；同一原事件的不同真实接触分别表达。共同 `record_contact_in(tx, ctx, input, materials, formation)` 用例在同一事务组织接纳与待形成登记，Material 不反向依赖 Formation。Runtime 使用它，删除按非 system 角色赋 contact 及直接重写 Observation 的分支。事件完整保存后当前 Work 和读取可以使用，后续处理通过正式引用衔接。

工具型 query 接收自然语言背景及独立引用/媒体参数，不要求模型先创建 Work 或填写机器上下文。深度 hook 保留完整原生 wire、tool/reasoning 和协议 ID；背景副本不成为新发生的交往。

### 把情境形成归回 Formation

负责位置：`cognition/formation.py`、`imports/{observation,observation_jobs,model_generation}.py`、`maintenance/{natural,tasks}.py`、应用组合根与 DBOS 注册。

将已有待处理接触合并、来源水位和旧认识候选组织迁到 Formation。离线 Import 继续管理文件/批次，交互形成不借 Catalog/Unit 复制状态。完整交互轮推进可处理水位；Formation 按 Namespace、观察者、thread 和可选 Work 合并来源，以默认30秒窗口登记一次低优先级整理，明确边界可立即flush。文本任务每批默认最多4096输入tokens并受真实模型预算限制，显式媒体使用自己的实际模态预算。worker区分新来源/背景，空结果也推进本次覆盖水位。

形成输入含原事件短键、作者、时间、必要前文、Work 与少量旧候选。采用一项有边界的语义任务输出陈述、主题、支持源键、原时间表达、旧候选键及重复/补充/变化/冲突关系。删除原始 UUID、完整生命周期快照和动作控制大 DTO。system 给充分标准与例子，user 给当次真实内容。

### 由记忆 owner 采用变化

负责位置：`cognition/{memory,formation,records}.py`、模型任务合同/模板与 Import 实际消费者。

Memory owner 根据语义结果及当前来源、认识归属、修订和采用策略，新增认识、增加支持、修订同一事项或保存冲突。共同记录能力完成精确版本和证据提交。保留旧原声明与原始历史；将队列接纳时钟与实际形成/采用时刻分开。基础记忆对照并入正常形成，原成长链只保留其高级领域用途。

### 直接从事件组织经过，工作摘要归 Work

负责位置：`cognition/episodes.py`、公开 Episode 合同与读取、`maintenance/tasks.py::summarize_work`、Context 的 Work/Steward 消费者。

Episode 输入可以是正式 Observation/实际工作事件，无需先生成 Memory；认识作为可选成员。来源重建、查询、时间/人物和原文展开共同迁移。Work 总结更新任务进展、决定及待解问题；显式 Journal 回顾读取对应经历与当时认识，不再借 Journal 充当 Work 更新。

### 迁完整操作和删除支撑

负责位置：`imports/service.py` 及离线文件/媒体处理、`application.py`、SDK、HTTP、CLI、MCP、配置、必要回归与当前文档。

取消跨文件共享整个 self 的函数绑定式拆分，围绕实际共同修改的离线导入操作组织深模块；通用语义形成迁回 Formation。组合根按明确依赖构造，不通过 app 全局、后设字段和回调弥补循环。迁移生产者、消费者、动态注册与现行指南，再移除旧逐事件形成、大控制提案、重复接触和仅服务旧合同的测试脚手架。

## 沿真实使用检查下一项结果

- 工具调用：真实文字/媒体导入后可查询、取得短引用、展开原件与来源、明确纠正；仅提供自然背景也能使用。
- 持续交互：用本轮真实目标变化或模型配置变化，在必要前文中形成认识；当前 Work 立即反映新要求，随后查询当前理解和原历史。长期目标后移与永久取消分别表达，不强制每条消息新增 Memory。
- 深度接入：首次模型调用、工具续轮、压缩后接续都用真实输入，新的信息自动参与下一轮；真实来源与摘要可展开。
- 经历用途：尚无 Memory 的原始事件也能组织 Episode；Work 总结的结果留在 Work，回顾目的才形成 Journal。

每个完整能力片段沿实际入口运行一次并保存有用进展，不执行强制红绿循环、逐字段测试或反复全量审查。阶段提交推送当前 WIP，运行端反馈继续指导修改；CI 只承担其覆盖的检查。

## 后续恢复的能力

自由 Seed 资料及版本保留，但暂停扩展其独立解释、MCP 管理入口和旧维护控制分支。主链明确后，用同一来源/形成/采用机制完成自我、设定记忆、社会和动机初始化；角色来源、创作设定与真实经历各自保留。Ego/Social/Motivation、反思、Dreaming、Wave 高级算法与媒体能力继续按完整范围交付。

Nous 原生完整稳定后，再开展独立 SDK 发行、SDK-only A-Memorix 适配、小规模公开评价和跨平台交付。Heptalogos 本轮只交接口与接线资料。

## 完整产品目标

建设可独立安装、配置和管理的长期认知服务：支持普通智能体的记忆与知识工作，也作为人工主体的认知与人格中心。完整产品需求由[项目宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)定义。本文件规定产品范围、架构要求和交付目标。

Heptalogos 以 MaiBot 为产品基础，负责主体的世界、生活、时间、注意、选择、行动与外部效果，正式依赖 Nous 的认知接口。Nous 自己保持独立部署和通用应用价值；两者共享逻辑主体身份，各自拥有正式状态和生命周期。

### 完整产品范围

- **材料、来源与时间**：接纳文本、文件、图片、音视频和外部资料，区分原件、精确区域、派生表示、主体实际接触与导入。记录证据、认知身份及修订、来源支持和反证、五轴时间、精度与历史知情截点；来源撤回、恢复和清除贯穿全部派生数据。
- **认知及成长**：完整提供 Memory、MemorySemantics、Concept/Tag、Entity、Association、Episode、Journal、Schema、Fact 和各自生命周期。Ego 包含多文档 Seed、facet、NarrativeThread、DomainKnowledge、例外与设定来源；Social 包含人物、跨平台身份、有方向的关系、信任领域与群体范围；LanguageConvention 保存语言的听众与语用；Motivation 包含 Desire、Concern、DesiredCondition、组合条件、持续区间和 unknown。完成自然形成、认知修订、使用反馈、离线整理和持续维护。
- **查询与算法**：完整 NousQL、14 类返回域、精确引用、名称绑定、五轴过滤和历史；中文词项、语义、Hybrid、VCP、图、媒体与来源展开共同使用授权快照。吸收 VCP 的 AGS、EPA、Pyramid、Sense、双场、RiverMemo、DTSC 和 Thinking，并完成实际图资产、候选、直接锚点、查询线索、诊断与效果评估。
- **多模态**：三种解释策略 `description_only`、`direct_structured` 和 `describe_then_structure`；二段形成同时使用原媒体或区域及真实描述。提供图像、音视频、GIF、文档与区域派生，保留输入、producer 和谱系；原生视觉向量、描述向量及图文联合检索采用真实空间与调用身份。
- **当前认知与工作连续性**：ContextRuntime 拥有会话、当前居民及实际模型调用状态，按用途、听众、预算与来源编译输入；WorkContext 拥有可跨会话接续的任务目标、决定、进展和未解问题。Steward 由程序管理检索、预算、来源与压缩覆盖，模型承担独立的相关性判断、主题摘要等语言任务。区分准备、实际发送、模型接触、真实使用与结果反馈；覆盖模型切换、工具续轮、压缩、恢复及独立执行者。
- **A-Memorix 独立适配**：完整承接[上游工具、管理动作和实际协议](https://github.com/A-Dawn/A_memorix)，包括文本、摘要、图片、检索模式、图、来源、Episode、画像、Fact、运行管理、导入、Bundle、调优、修正、删除和恢复。独立发行包只通过正式 SDK 提供兼容协议和 RPC，并接通真实消费者。MaiBot 的宿主接线归 Heptalogos。
- **产品与基础服务**：完善实例及跨平台安装、配置、模型资源与预算、Prompt、网络、权威事务与内容、索引、时间、持久执行、身份与密钥、能力扩展、媒体、可观察性、数据生命周期、接口与客户端、国际化。CLI、HTTP、MCP 和 Python SDK 提供领域操作。提供备份、恢复、主体迁移、可理解状态及 Windows、Ubuntu、macOS 的发行与运行体验。

### 架构与实现

一份 Namespace 拥有独立认知状态；`mode`、`subject_key`、`enabled_features` 表达主体能力与跨产品身份。领域操作拥有业务语义，共同记录能力拥有认知身份、修订、证据、时间、生命周期和读取。组合根构造明确依赖并管理资源生命周期。

Python 主应用拥有认知规则、SQLite 权威数据和事务、来源授权、模型与网络、配置、Prompt、作业及产品入口；同进程 Rust native 负责 VCP、Tantivy/Jieba、USearch 和 NousQL 纯语法。PyO3/maturin 使用批量数组和 Rust 拥有的快照，计算释放 GIL 并在线程执行；native 不访问业务数据库、Provider 或凭据。权威对象及其消费者通过清晰的领域 owner 共享规则，派生索引可以重建。

采用标准 CPython 3.14、uv workspace、Basedpyright strict、Ruff；SDK 保持 Python 3.11+，native 提供 `.pyi` 和 `py.typed`。SQLAlchemy Core、aiosqlite 和 DBOS 共享 SQLite 文件，Authority 提供唯一的短写事务，完整业务操作在同一事务中提交状态与后续任务。外部事件、导入单元和作业使用各自稳定业务键处理重复投递。模型生成与 Provider 协议采用 Pydantic AI、官方 SDK 和成熟实现；Jinja2、FastAPI、Typer/Rich 与官方 MCP SDK 分别承担模板和入口机制。

原件和大字节保存于内容存储，权威状态与元数据保存于 SQLite。Wave 维护可重建的固定索引代际，各检索通道产生候选并融合。自然语言按原文查询，可选 QueryContext 提供真实情境，NousQL 解析显式查询语法。

材料、实际接触和长期认识分别持有身份。自动形成结合旧候选采用新 Memory、补充来源或修订同一事项，保留认识归属、适用范围和历史；原文按 Material 与 Region 读取。MemorySemantics 表达对已有 Memory 的独立再解释，并引用其精确修订及新证据。显式记忆保存用户指定的内容。模型执行有明确目的的语义任务，程序负责领域对象、身份、来源与采用；各任务结果保留各自的生成出处。

可编辑 TOML 保存期望配置，进程内保存校验后的有效快照。模型执行统一绑定 Provider、Model、Preset、Role，编译最终有角色的消息与媒体，执行目的明确的局部任务，返回输出、实际用量和精简的生成出处。采用结果的来源随正式派生表示保存；结果缓存与调用诊断可按容量和时间淘汰。并发、超时及单次资源限制由执行器承担，持久任务由 DBOS 接续。

优先采用成熟依赖，保留 Nous 的领域模型、真实算法与完整原生操作。A-Memorix 的公共协议由仅依赖正式 SDK 的独立 `adapters/amemorix` 发行包提供。原件、来源、正式认知及采用的模型结果具有独立于运行缓存和诊断的生命周期。

### 实施依赖

以完整用户链组织工作：初始化实例和 Namespace → 接纳真实资料与来源 → 形成并采纳认知 → 实际 embedding 和索引 → 查询、展开来源 → 重启后继续。导入保存真实原件、输入单元、采用结果和处理状态，支持部分完成及继续未完成工作。

沿资料接纳、认知形成、投影检索、模型实际应用和长期成长串起完整产品。职责、Namespace 与事务在对应操作的负责处统一，迁移同一契约的全部生产者、消费者与专属支撑；现有可用能力继续连接下一段操作。SDK、HTTP、CLI、MCP 同步消费原生操作；Nous 本身完整、稳定后，再进行独立适配与发行。

### 交付要求

交付可独立安装、配置和持续使用的 Nous 源码、安装产物、用户指南及完整 SDK 示例。使用者能够接纳和查询真实文字与媒体，读取来源、修订认识、恢复任务，并在连续模型及工具调用中自动取得人格、活跃记忆与当前工作。

独立 A-Memorix 适配器连接本地或远程 Nous，提供实际工具、管理、导入导出及 RPC。SDK 单独安装即可支撑这一外部消费者。

使用真实资料及上游公开数据进行小规模测评，保留实际输入、配置、结果、耗时、用量和具体问题。修正阻断使用和导致错误结果的问题，保留原始资料与模型结果。

Heptalogos 本轮只交付 Host API、主体绑定和模型调用接缝的交接资料；主体内核另行开发。
