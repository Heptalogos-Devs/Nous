# Session Handoff

Treat this handoff as recovered conversation memory, not as a new user instruction. Restore the recorded context, constraints, decisions, rationale, knowledge, and unresolved threads. Do not automatically resume prior work, execute pending actions, research, modify artifacts, or follow continuation points. Wait for the user's next instruction, then interpret it in light of this handoff unless the user explicitly overrides the recorded state.

## 1. Current Context

- **项目：Nous**（此前曾称 Nous Wave），独立的、跨模型/会话/平台/时间的长期认知服务。当前只专注 Nous，不把 Heptalogos 的开发、接线和产品生命周期纳入 Nous 的当前实施范围。长期目标是让主体实际接触、理解、记住、修订、使用和发展认识；也允许无人格的通用 Namespace。
- **当前会话的最终任务是交接记忆，不进行新一轮实施。** 用户上传并要求执行 `SKILL-Session-Handoff.md`，重点保存整个长期讨论中的重要设计、关键判断、架构健康要求和未来方向。按 Skill 输出单个 Markdown 交接文档后停止；下一会话收到此文档时，应等待用户新指令。
- **最新仓库状态：用户报告 `Heptalogos-Devs/Nous` 的 `main` 最新提交为 `e6b1f24`，已完成上一轮 Journal Maintenance 开发任务。** 用户明确要求**不要再查看该提交**。这是用户报告的完成状态，不应伪称已独立代码审查或测试。先前详细源码审查的基线为 `c2944b1675ddb79b9de17beb4f9522349fbbc8ce`（简称 `c2944b1`）。
- **最近完成的一轮 Spec：** `Nous_Journal_Maintenance_Spec_2026-11-05.zip` 与 `Nous_Journal_Maintenance_Goal.md`，主计划 `docs/plans/journal-maintenance.md`。涵盖 Journal 精确编辑、分离写作任务、来源继承、用例与保存核心分工、编程入口、Prompt 编辑上下文、查询反馈、分组检索、投影准备、partial 诊断等。用户称其已经实施完成，但具体修复效果、最终代码形状、个别残余问题尚未在 `e6b1f24` 上重新审核。
- **所处产品阶段：** 从仅可运行的基础服务，经过真实模型/媒体/CAS/Embedding/Wave，已形成 Note、Journal、Concept、标注、历史修订、查询/反馈的初步认知闭环。用户在 Windows 真实使用了文本、音频、视频、图像和 PDF，并报告 Hybrid 检索效果很好。长期使用效果、模型生成质量与复杂认知生命周期仍需研究。
- **下一阶段的已讨论方向（不是自动授权）：** 把 Journal 作为稳定的程序内认知能力；接入真正持久的保存后维护（DBOS）、持续宿主及服务入口；建设 WorkContext → Wave → 可选 Steward → Agent 使用 → 情境反馈的闭环；之后深化 Episode/多轴时间，再推进 Ego、Social、Motivation、Schema、Anchor、Dream、完整 NousQL 与其他集成。随用户后续指令重新判断优先次序。

## 2. User Directives

### 权威、任务与协作

- **设计、产品目标和架构高于既有代码与测试。** 当前实现、旧文档、先前 Agent 的提案都可以直接推翻、重构、迁移并删除。发现架构阻碍真实功能，应改进职责和数据形状源头，不能尊重坏代码而继续补丁。设计本身也持续修正：一旦发现新需求改变合理结构，就更新决定和文档。
- **Nous 的最高实际开发指引位于项目仓库 `docs/`，以当前 `docs/design/` 和明确批准的计划为准。** Architecture-Vault《项目宪纲》仅作为设计者心中更高层的产品方向，不在开发文档中反复要求 Coding Agent 再读 Vault。`docs/sources/` 原样保留讨论、`legacy/` 是旧资料，二者不覆盖当前设计权威。
- **专题化文档**：一个专题一个职责，即使短也独立；随着开发细节增多而自然生长。保留讨论原文，设计文档从原文提炼有效决定、完整合同、方法和可执行计划。已有文档**只能基于当前真实内容定点修改、使用上下文补丁**，保留用户/Coding Agent 后续有效补充；严禁 Spec ZIP 整文件覆盖同路径文档，尤其禁止用旧正文覆盖新约束。`docs/CURRENT.md` 只维护当前能力、仍有效决定和未完成事项，不逐条追踪用户新同步的讨论文件。
- 讨论任务就讨论、研究就形成结论、设计交付可实施结构；用户**明确授权施工**后面向 Coding Agent 交付自包含 **Spec ZIP + 简短 Goal 提示词**。Spec 需有源码位置、方法与数据合同、所有权、实际生产者/消费者、事务及完整迁移范围和完成条件；不要只写方向让 Agent 猜测。不自动发布/修改代码。
- 遵循用户 AntiRLHF Constitution：先结果与决定，再关键依据；拒绝无意义的测试、过度过程化审核、流程门禁、逐任务 TDD、重复全量验证、额外追责日志或大型治理机制；不要为了更少依赖而重造成熟通用设施；重点是实际用户操作和长远维护成本。
- **用户自己负责 Windows 本地独立测试、压力测试及检查。** 作为研究/架构顾问，除非用户明确请求，不必运行本地测试，不主动查 GitHub CI，也不能把 CI 通过率当成果。用户已关闭频繁的远端 CI；不要反复纠缠测试、要求验证或提醒成本、预算和费用。可从代码判断真实缺陷，测试合同尽可能围绕完整用户通路；少量高价值回归足够。
- 用户使用本地 Docker 的 **New API** 统一模型网关，已授权的开发环境可自行获取调用 Key。**生产架构保持厂商中立**：只依赖成熟 SDK/网关的协议与能力；不为某品牌模型或 New API 写产品特例。无需为这套受控开发环境频繁追加费用或 Key 暴露担忧。

### 最高优先级：架构健康

- **高内聚、低耦合、以真实事物和业务所有权为中心，深模块封装完整操作。** 先追踪实际用户操作链：入口 → 领域判断 → 状态修改 → 事务/来源 → 持久化 → 结果呈现；再组织调用依赖。
- 检查是否有局部绕过已有基础服务、复制相同规则、把不同入口分别编排、另写新的客户端/缓存/错误/事务/队列/媒体处理/模型调用/时间逻辑；**发现共用基础欠缺能力，就升级基础 owner 并迁移全部实际消费者**，不能为每个新用例包一个补偿层。
- 不允许无意义内部兼容链、旧别名、双轨代码、wrapper-on-wrapper、备用实现和分散状态。Nous 尚在开发，无受保护的真实外部客户端/既有数据，内部契约可以整体迁移并删除退役路径。**但认知内容的正式业务修订和历史精确引用是产品能力，要保留**。
- 不因为每个概念都要类/表/服务，不建万能 Manager；避免过深包层级、文件平铺、庞大上帝类、转发文件、无意义 if/try/异常吞掉问题。文件过长可提醒，但应按**不同修改责任**重组，不能按机械行数切碎。
- 充分的**统一配置、错误体系、事务、Trace、服务生命周期、存储、网络**是降低耦合的基础，值得建设；应防止的是一次操作在内部四处启动、互相调用、重复权限/回退机制的官僚式交织。纵向能力还未接通时，不做“企业级”“军工级”稀有异常修补。
- 代码交接、上下文压缩、临时修复最易破坏结构。每轮增量都应关注已有 owner 的使用情况、完整调用链、是否不必要增大代码、文档是否同步、功能增量是否真实。新用户要求暴露坏结构时直接改架构。

### 用户使用、CLI、LLM 与配置

- **从人和 Agent 使用角度设计。** 接口需直观、可读、少量合乎认知的工具；短 Proquint 引用优先，别把 UUID、内部接口全部暴露给 MCP/模型（曾出现数百工具，必须避免）。CLI、HTTP、MCP、SDK 是**同一核心操作的不同适配面**，不能各自另写业务逻辑。前端正式模型/媒体实验命令统一 `nous tools …`，不要堆建 `nous dev` 工具平台。实际后台宿主应持有已装配实例，**以后主要是程序内调用，不为 CLI 的形状牺牲 API**。
- CLI 用户不愿每次指定长实例路径和 UUID。现有选择优先次序：显式实例路径 → `NOUS_INSTANCE` → platformdirs 默认实例；可后续支持可读别名/当前实例选择，但实例解析必须由共同 `InstanceLocator` 负责。语言选择统一经 presentation；`--language en` 不应显示硬编码中文默认标题/中英混合提示。
- **LLM 仅做专一、低歧义的语言任务。** 它是脆弱且会犯错的，复杂状态、事务、索引、来源身份、权限、版本、图权重/几何等由程序承担；不把模型失误自动归因为系统缺护栏，也不靠补无限 Prompt 限定解决任务过载。不用语言关键词词表/正则“理解”自然语言。模型输出错误允许出现并由业务结果清楚表达。
- **Prompt 是经常人工改进的产品资源**：用 Markdown/Jinja、真实 system/user/assistant 消息角色，稳定规则放 system，实际材料与动态参数放 user；遵守正反例、来源可读短键、无虚构作者/时间/因果、输入原貌、实际任务单一。维护者必须易于看到：用途、调用位置、各消息作用、Jinja 字段、Input/Output Schema、模型所需模态与能力、实际送入资料和产物采用范围。选择扩展现有 `prompt.toml` 的人类可读说明，由 TaskDefinition/实际配置生成 Schema 和能力，不另加一套 YAML Front Matter。实例自定义覆盖优先于包内，绝不因升级覆写；应呈现生效来源、版本、差异。Prompt 说明元数据不等于发给模型的请求。
- **“先能够配置，再决定如何暴露”。** 配置拥有类型、默认值、必填/可空、范围、解释、作用域、来源与覆盖、生效方式、用户/高级/开发者呈现层级、危险及数据绑定。覆盖和范围与配置存储位置分开；修改后可查询有效值、来源和是否待重启。Embedding 模型、维度、规范化等与已采用向量空间绑定，不能静默更换；改变空间语义须重建/新建空间。统一实例 TOML/快照与 owner 配置，普通组件不得自读环境变量/单独复制默认值。初始 bootstrap 配置不可依赖数据库。
- **双阶段日志：** 最初 stderr 可在配置/数据库失败时诊断；启动后统一 structlog/JSONL/OpenTelemetry 的相关事件。各基础模块只依赖标准 logging/必要的轻埋点，不反向依赖重型 TraceService。终端输出业务名称、阶段、真实错误、耗时、已完成结果和必要短引用；完整 ID 在 JSON/详情。`partial` 与附加阶段失败要进入根操作日志、`runtime logs`、`runtime check`，不能外部返回 partial、日志却报 success。
- **多时间域与多轴认知时间**，严格区分 `occurred`、`observed`、`formed`、`valid`、`recorded`；材料入站/received 另记。无证据不伪造世界发生/观察时间；可使用虚拟/自定义认知时间域。现实操作期限用单调时钟，记录使用正确时区/UTC，精确历史知情视图不受未来数据污染。

## 3. Intent & Decision Timeline

- **Initially：** 从长期主体连续性、记忆/观察、人格、社会认知和联想中提出 Nous。Vault 宪纲给价值与领域方向，但不具备可直接指导开发的细节。Heptalogos 为可能的独立消费者，目前停止考虑其开发。
- **Then：** 早期多种技术方向（Rust Core + TS Host、Rust-only 单二进制）先后被新决定替代。现行选择是 **Python 3.14 主应用 + Rust PyO3 Native**；Python 持有认知和业务数据，Rust 负责高性能、可重建的数值计算。
- **Then：** 历史 `docs` 中有过期内容，决定保留原始讨论 `docs/sources/`、退役文档 `legacy/`，新 `docs/design/` 成为唯一当前开发合同。用户反复纠正“Spec 整文件覆盖已有文档”，改为新增文件 + 定点上下文补丁，合并 Coding Agent 和人工后续有效更改。
- **Then：** 重建运行基础：实例/逻辑路径与物理路径、bootstrap、配置、错误、生命周期、时间、日志/Trace、SQLite/事务、执行与 CLI；先支持真实操作，不提前填满认知领域。
- **Then：** 接入本地 New API 通过成熟 SDK 的真实模型、Prompt、媒体转换、多模态、CAS、Artifact/Result、Embedding 等，正式 `nous tools`，成功输出按来源采用；普通模型/媒体能力与认知采用分开。
- **Then：** 迁入 VCP 的 Rust 数学闭包，扩展 Wave 的 Concept/来源/上下文/有序路径、CandidatePool、Tantivy/USearch、Python/Rust 接缝、快照与 Hybrid。坚持吸收 VCP 全部原生能力，再以 Nous 的正式对象和时间扩展；后续纠正 Tag/Concept 顺序与证据独立性。
- **Then：** 澄清用户原始概念：**Note 是 Agent 主动写下的碎片笔记**，不是只有 Formation 临时输出；**Journal 是主题明确、较长、可见、Markdown、可修订的主要长期叙述记忆载体**；`journalize` 将碎片写成新篇。MemorySemantics 取消独立正文层，以修订或新反思文稿表达后来的理解。Concept 与扩展的 Tag 合并。
- **Then：** 第一条正式认知链开发并于 `c2944b1` 提交：Note/Journal 正式引用、修订、来源、Concept 稳定身份、标注、Wave 当前检索及反馈。用户在 Windows 多模态和长文/高并发压力场景验证，报告 Hybrid 效果很好，也发现局部修订、查询时重建与诊断缺点。
- **Later：** 规划 Journal 维护 Spec：拆分 journalize、局部编辑、明确全文重写、跨篇反思；写作核心与维护用例分开；进程内调用；Prompt 人类编辑上下文；反馈真实 QueryContext；投影准备和按文稿分组；partial 诊断等。
- **Latest：** 用户称最新 `main e6b1f24` **完成了上一轮开发**。无须查看提交或再做审查；现在交接记忆供未来规划者使用，后续等待新任务。

## 4. Decisions & Rationale

- **Decision：仓库正式设计文档高于代码，当前决定取代历史方案。** Why：防止多轮 Coding Agent 局部修补导致架构蜘蛛网；使每位开发者读取一份当前正确合同。Alternatives rejected：用旧代码/测试定规格、保留双轨、从各历史 Spec 拼接权威。**Status：confirmed；Source：user。**
- **Decision：高内聚深模块和真实业务 owner；共同基础由定义者集中承担。** Why：新入口能完整复用，修改一次改对全局。Alternatives rejected：每入口复制规则、万能管理器、大量转发/兼容包装、防御式流程。**Status：confirmed；Source：user / joint conclusion。**
- **Decision：Python 应用拥有模型编排/认知/数据/事务/来源/时间/查询视图；Rust Native 负责 VCP/索引/数值快照，PyO3 薄绑定。** Why：业务权威唯一，高性能计算可重用。Alternatives rejected：Rust 再持有数据库、Python 复制数学、Rust 绑定层做全部业务、再建另一服务进程。**Status：confirmed；Source：joint conclusion。**
- **Decision：SQLite + SQLAlchemy Core、CAS、Pydantic/pydantic-settings、Jinja、Pydantic AI/官方 SDK、Typer/Rich、OpenTelemetry/structlog、uv 等成熟能力优先，DBOS 用于真正持久后台工作。** Why：复用通用机制，减少维护。Alternatives rejected：自研配置/任务/协议/日志框架。**Status：confirmed for core; DBOS implementation timing provisional；Source：joint conclusion。**
- **Decision：Note、Journal 为具有正式身份/不可变正文修订的可见文稿；Note 默认不进入长期召回，Journal 默认进入。** Why：保留碎片记录和主题化长文各自用户价值。Alternatives rejected：Notes 只是一条模型临时候选、将 Journal 原文复制成独立 Memory、将所有记录强制 journalize。**Status：confirmed；Source：user / joint conclusion。**
- **Decision：`journalize` 仅将所选新材料整理成**新篇 Journal**；不得将已有 Journal 再投入同一压缩任务来制作 V2。** Why：重复“提炼压缩”会损失细节。既有 Journal 通过局部编辑、明确全文重写、另建跨篇反思分别处理。Alternatives rejected：一个 Prompt 同时承担整理、修订、反思，靠更多限制保护旧正文。**Status：confirmed；Source：user / joint conclusion。**
- **Decision：修改操作可精确 replace/insert/delete、人工编辑器、明确全文替换；持久化仍使用完整不可变 Revision 和 expected base。** Why：保留范围外正文和历史引用，不用模型保证未修改部分。来源映射随局部编辑继承；模型自然语言修改仅提议小 edits。Alternatives rejected：模糊自动重定位、全文重新生成、读 V2 时动态合成旧版。**Status：confirmed as design; implementation reported completed e6b1f24, not reviewed；Source：joint conclusion。**
- **Decision：Feedback 分离 `usefulness/retrieval`、`accuracy`、`agreement` 等；普通正负评价/评论持久化，不自动调用模型或改全局权重。** Why：一个问题下的效用不等于事实真实性、普遍价值或信任；使用时需保留真实 query、上下文、快照、交付/命中/采用。明确纠错才经文稿 owner；自然语言建议先成为提案。Alternatives rejected：一条负反馈即全局降权/改图、自动改正文。**Status：confirmed；Source：user / joint conclusion。**
- **Decision：Journal 内容修订、Concept 意义修订、AnnotationSet 标注版本各自独立。** Why：标注修订不伪造新的主观认知；引用和概念意义历史可解释。Concept 以名称/别名+完整描述（可向量化）替代重叠的 Tag 对象；未经明确采用的候选不会成为持久 Concept。**Status：confirmed；Source：joint conclusion。**
- **Decision：Wave 完整吸收 VCP，不压缩到单一向量相似度或某种单用途；Concept 路径、图、上下文、来源和时间投影要保留。** Why：关系寻址是独有能力，检索时沿真实概念图传播。`quality_score` 与 `ranking_score` 区分，内部 RiverMemo 通道不在外层重复投票；Native CandidatePool 可外部组合。**Status：confirmed；Source：joint conclusion。**
- **Decision：派生 Journal 可贡献新的解释性概念连接，但重复整理不增加原始独立见证；共同依据组与独立组分开。** Why：保留创造性联想同时避免自动总结凭空增信；反证/负关系独立读取，不塞进 VCP 非负传输。**Status：confirmed design / individual numeric extensions may need validation；Source：joint conclusion。**
- **Decision：严格历史查询先选择已知文稿/概念/关系及时间视图再生成可计算资产；五轴时间分离，近期性可作排序偏好。** Why：仅在结果端过滤会让未来信息通过几何/传播影响过去的认知。**Status：confirmed architecture; full feature future/unimplemented；Source：joint conclusion。**
- **Decision：Prompt 维护上下文进入现有 `prompt.toml` 与从 TaskDefinition/有效模型配置导出的视图，实例覆盖优先。** Why：便于人工迭代、减少重复 Schema 与新 YAML；不同任务使用不同语义 Prompt。**Status：confirmed design; latest implementation user-reported, unreviewed；Source：user / joint conclusion。**
- **Decision：保存核心只做文稿事务，提交后标注与检索发布由完整认知用例组织，`partial` 是正式业务结果并由共同观测报告。** Why：避免 WrittenRecords 直接依赖所有后续认知模块，诊断成功/失败一致。**Status：confirmed design; reported implemented, unverified post-commit；Source：assistant recommendation accepted by user。**
- **Decision：可复制的完整检索投影与按需发布先保留；优化重复内容准备，返回候选按文稿分组应在公共召回/池层完成。** Why：用户实测瓶颈在候选重复准备而非 Native；末端 `top_k*4` 无法保证不同文稿名额。**Status：confirmed design; implementation status user-reported, not reviewed；Source：evidence-driven。**
- **Decision：持久维护下一轮使用 DBOS，先由持有实例锁的长期宿主进程装配与运行业务操作和 worker，事务内登记真正的后续工作。** Why：保存后标注/检索发布已有实用消费者；实例长期锁不允许随意起另一个 `open_instance` worker。**Status：provisional implementation plan（待新用户授权）；Source：joint conclusion。**

## 5. Knowledge & Evidence

### 当前实现与外部使用

- **Observed by user：** Windows 环境以文本、音频、视频、图片和 PDF 完成真实检索；Hybrid 效果“非常好”，符合设计期待；多种模式、语义上下文、来源和版本可用。真实效果属于用户经验，未被本助手独立重测。
- **Verified from earlier code/design @ `c2944b1`：** `src/nous/cognition/{records,record_reading,journalize,annotations,concepts,retrieval,feedback,basis,scope}.py`，`src/nous/wave/{assets,snapshots,query,operations}.py`，`src/nous/models/{tasks,prompts,operations}.py`，`src/nous/content/{artifacts,results,text,provenance}.py`，`native/vcp`、`native/retrieval`、`native/python` 构成当时主调用链。
- **User-reported @ `e6b1f24`：** `Nous_Journal_Maintenance_Spec_2026-11-05` 已施工完成。本次未检查该提交；**不要把先前在 `c2944b1` 发现的缺陷无条件标为最新代码现存问题**。
- **Observed performance/functional issues at old baseline `c2944b1`：** 501 篇短文稿/合计约 144 KB 时，保存 0.029s，随后的词项检索 23.64s，热查询中位 0.116s，Native 构建+查询约 0.15s，瓶颈是候选重复准备。246KB 长文章与四篇相关短文的 top-k=10 结果只剩长文。一项自动标注未配置模型时，正文成功但返回 partial；日志却 success/error=null，runtime check 看不到近期失败。CLI 的 note annotate 曾拆成两个根 operation ID；`--language en` 仍出现中文默认标题。**这些均已纳入最近的整改 Spec，是否在 `e6b1f24` 修好尚待后续按需核实。**
- **Earlier integration bugs and lessons：** 媒体 prepare/materialize 之间曾丢失检测到的真实 MIME，取而代之从后缀猜格式；模型媒体输出格式未在准备阶段协商；Embedding 的 `ensure` 分批但 `run` 不分批且实例级长锁跨网络；普通 joint 多输入曾被逐文件拆成独立语言请求；文本/媒体的来源读取混用 UTF-8；Prompt preview/probe 曾在 CLI 复制业务编排。这些推动统一输入计划、媒体计划、批量执行、来源读取、用例归属。最新状况按新证据判断。
- **文档高风险：** 较早 Spec ZIP 直接覆盖仓库同路径专题，造成有效要求丢失；例如 CLI JSON 错误合同被删短。后续决定只新增新专题，旧专题用上下文 patch，发生变更冲突时保留有效新决定。这是未来 Spec 交付的硬性约束。

### 重要业务与数学语义

- **Material/Region → Observation → Representation → Note/Journal：** 原件是实际字节/片段；Observation 表示某主体/观察者何时真正接触了材料（不等于模型处理或文件进入存储）；Representation 是描述、转写、结构化等派生；主观认识在 Note/Journal/Schema/Ego 等正式认知对象中表达。没有原件不应编造观察时间；接纳原件不等于主体知晓。
- **Journal**：一篇主题连贯、自包含的 Markdown，支持长期阅读、修订、来源链接、章节、段落/块选取。较长 Journal 可分多个检索候选，但**用户管理单位是 Journal、检索表示是向量/概念路径、交付单位可为原文选段**。TextView 使用 CommonMark 源范围，精确引用锁定不可变版本；多范围保持各自真实选区，禁止粗暴用 min/max 含括未选择正文。
- **Episode**：按实际发生/观察的时间与情境组织事件经过，直接来自 Observation/工作事件，可引用 Journal，不要求先形成 Memory；Journal 是一种主观书面叙述，与 Episode 有独立身份。
- **Schema**：跨经历、跨文稿的可修订**经验模式**（规律/有条件的做法），能够衍生可被 Agent 复用的 Skill；不是普通标签和每条记忆的附属分类。**Anchor**：长期动态吸积的认知地标（跨经历/主题，允许变化、合并/分裂等未来研究），不同于 VCP 单次查询的数值 Direct Anchor。
- **Concept** 取代重叠的扩展 Tag；拥有稳定身份、名称/别名、描述、向量与意义版本。Annotation 绑定精确正文版本、块位置与概念意义版本；有序路径表达叙述/推演的展开，同一 Journal 可有多条局部路径，不自动拼成不存在的跨节边；重复出现有语义。新的 Journal 可以沿来源重排概念以形成解释性桥梁。
- **Association**：抽象的“关系”可被共享图计算；具体社会关系、Schema 依据、Journal 的解释联系分别由真实领域 owner 持有其类型、适用条件、程度、信度和来源，Wave 只构造投影。社会图的关系强度、判断信度、查询边权三者不同。
- **Ego**：多份可读人格文档（身份、风格、价值、能力、经历等）；稳定必要人格可直接供给模型，情境细节经 Wave 检索；人格影响生成和解释，但不能逆向改写历史原件。**Social**：通用实体/人物/群体档案、跨平台与非平台外部身份绑定、关系图。**Motivation**：长期愿望/关切/期望条件，区别于当前 Work 的执行状态。**WorkContext**：跨会话的目标、有效决定、进展、阻塞、恢复锚点。**Active Memory**：当前可激活引用与片段集合，不复制一套长记忆正文。**Steward**：可选 LLM，从 Wave 排好的约 10 篇候选中只挑精确原文段落引用，程序返回原文；无模型时直接交付候选，不让模型重述造成失真。**Dream**：可主动或由 Host 定时触发的自由猜测、联想与创造，其产物可作为探索性 Journal，标明性质与启发来源；Thinking 是依据真实传播图的有根据阅读路线，不能代替自由 Dream。
- **VCP 主数学链**（来自 `lioensky/VCPToolBox @ d5c019ebbac1f317c2938d655d2ce916e41b9164`）：有序 Tag 及向量 → AGS 背景几何/局部残差 → EPA 查询投影 → Residual Pyramid/核心与 Ghost 线索 → Sense 有前驱的非回溯、有界传播/Dynamic Wormhole → Local/Transfer 双场与向量投影 → 多通道召回 → RiverMemo 有序候选曲线、路径/拓扑/Ω/Direct Anchor、条件 Wormhole RRF → 结果排名与路径解释；TagMemo、RiverMemo、DTSC、lexical/dense/hybrid 均有各自计算意义。**quality_score、ranking_score 和实际结果顺序三者分开**。Native 快照固定正式认知视图、空间与代次，Python 返回正式来源和正文。Rust 旧改编代码有 VCP 上游 CC BY-NC-SA 4.0 许可边界，重新分发须按实际采用部分处理。
- **来源支持与数学图**：原始来源根集合≠多个独立支持；`SupportGroup.required_roots` 可以表达 A 与 B 共同作为一个解释的前提；重复摘要/复制不增加独立见证。派生 Journal 仍可贡献新概念顺序和解释性边，其可信度/来源资格独立处理。负关系/反证不能以负权直接进入 VCP 假设非负、固定出流预算的传输矩阵。严格 `$asof` 时间知情视图要在构图前过滤未来认识；现在完整历史视图尚属未来目标。
- **NousQL 长期方向**：自然语言为一等查询，正式 Concept/实体与临时 cue 区分，精确对象/修订/查询本地短引用；支持多时间轴 `$time(occurred|observed|formed|valid|recorded,...)`、`$asof`、`$history`、多搜索模式/探索/来源/排除/组合等。Proquint CVCVC 三词短地址（示例 `jrn:...@3`）；查询本地 `qN:n` 用于精确后续读取；避免旧四英文词 LexicalRef 和 UUID 直接给模型。真正完整 NousQL 尚未实现。

## 6. State

### Resolved / Completed

- 用户明确确认的产品语义、架构原则、文档权威、开发流程和工程栈已经长期收敛；多轮 Spec、源码审查和用户实测积累了可继续依据的设计材料。
- 用户报告 `e6b1f24` 完成了最近 `journal-maintenance` 施工；本次只保留此完成声明，不展开调查。
- 历史 `c2944b1` 的已实现能力：正式 Note/Journal 草稿/采用/修订/旧版读取/标注/Concept/反馈/当前检索，Rust Wave，统一模型与多媒体基础。用户验证 Hybrid 可用且效果良好。
- 项目 `docs/design/` 专题化、`docs/sources/` 保存原讨论、现行 Spec 采用定点补丁，`AGENTS.md` 描述架构与 LLM 约束。

### Current / Unresolved

- 下一会话尚未收到新任务；不能自行开展代码审核、检索、Spec 或实施。
- `e6b1f24` 中各项修复的最终实现与真实效果**未被本会话复核**。旧缺陷只作历史背景；新会话可按用户后续请求评估。
- Journal/Concept/关系/时间会继续扩展，当前架构所有权要求长期适用，具体数据形状不是永久锁死。
- Prompt 质量及模型对复杂媒体与形成/标注的理解高度依赖人工迭代；人工编写的规范、例子、覆盖文件应尊重并保留。

### Blocked / Unknown

- 当前具体后续施工授权：**(none)**。用户本轮只要求会话交接。
- DBOS、持续宿主、WorkContext/Steward、正式 Episode/Observation/Ego/Social/Motivation、Dream/Schema/Anchor、完整 NousQL、任意 `$asof` 知情视图的最新实际实现状态：以将来仓库事实与用户新信息为准；本轮未核验。

## 7. Failed Attempts & Gotchas

- **Tried：** 用 Spec ZIP 整文件替换已有 `docs/design`。**Result：** 曾丢失 CLI 错误 JSON、语言/引用等有效约束。**Established：** 必须基于当前文档逐段修改、提供上下文 patch，合并 Agent/用户后续决定。**Retry?：No。**
- **Tried：** 将 Notes 定义为 Formation 的一次临时候选；将 MemorySemantics 设为独立解释正文层。**Result：** 与用户原意冲突，概念膨胀。**Established：** Note 是主动持久记录的碎片，Memory 是能力统称；修订/另写解释性 Journal 承载后来的认识。**Retry?：No。**
- **Tried：** 旧 Journal 再跑 `journal-draft`/journalize 制造 V2。**Result：** “提炼压缩”可能导致每次改版更短、丢细节。**Established：** journalize 只生成新篇，已有文稿走明确局部编辑/全文重写/另写反思；程序保留未改字节与来源。**Retry?：No。**
- **Tried：** 把各种反馈直接看成纠错或总体有用度分。**Result：** 混淆内容真实性、查询相关性和作者立场。**Established：** 反馈必须固定实际 query/context/view/hit/交付与真正使用环节，普通意见不直接修改正文或全局图。**Retry?：No。**
- **Tried：** `top_k*4` 在认知层后置按文稿去重。**Result：** 246KB 长文占据全部候选，无法返回相关短文。**Established：** 在检索召回与候选池中按文稿分组，保留实际片段和排名。**Retry?：Only if newly chosen retrieval policy, not as compensation。**
- **Tried：** 文稿每次修改后完整重建时对同一正文/选区反复解析、生成语义投影。**Result：** 501 篇短文稿仅投影准备耗时二十余秒，Native 很快。**Established：** 同一不可变正文批量读取、解析一次、派生整文和局部候选，按真实纯内容键复用；仍保留正式来源区别。**Retry?：Do not reintroduce per-candidate work。**
- **Tried：** 应用用例返回 partial，但只有“异常未抛出”进入根操作 success。**Result：** 当次响应之后无法查回标注失败原因。**Established：** 业务结果通过共同观测记录真实 `partial`、错误码、步骤、已保存引用、operation ID。**Retry?：No。**
- **Tried：** CLI 自行组织 propose+adopt、Prompt preview、probe、配置变更等。**Result：** 一个用户操作分为多个根 operation，绕过共同策略。**Established：** 入口只做参数/呈现，完整操作归应用 owner，所有 API 复用。**Retry?：No。**
- **Tried：** 媒体计划丢弃真实探测 MIME 后从文件后缀重猜；Embedding 分批仅部分入口采用，global lock 包含网络。**Result：** 同字节换名行为不同、非支持媒体格式迟失败、无关查询阻塞。**Established：** 统一有类型媒体计划与按批物化、共同 EmbeddingExecutor、仅短 DB 唯一性事务。**Retry?：No。**
- **Tried：** 直接将 VCP 文件/Tag 假设搬入 Nous，不区分派生与原始支持、把所有路径压成单一路径、Native/PyO3 接缝承载检索业务。**Result：** 候选池/高级读出/长序列与关系场能力会缩水。**Established：** 完整原生算法闭包、Python 认知投影、Rust retrieval 深引擎、薄 PyO3、有序多路径与显式支持组。**Retry?：No。**
- **Tried：** 用配置缺失时随意 fallback、冗余 try/catch 补正常错误、对所有报错追加护栏。**Result：** 隐藏真正的所有权和模型任务失配。**Established：** 明确 route/任务/原因，报真错，修源头，允许真实部分结果。**Retry?：No。**
- **Skill-specific：** 本轮曾尝试使用 Codex handoff 功能，用户选择不转交；此文件直接在当前环境生成，**不要再次为本请求建议自动移交**。

## 8. Open Threads

- **持久维护/DBOS。State：方案已形成，待用户新授权与代码事实核对。What remains：** 由拥有实例锁的长期宿主启动实际 worker；保存后标注、语义投影或批量更新入队；同 SQLite 事务/注册与恢复；复用已采用语言结果；业务完成状态与 DBOS 工作状态分开。**Continuation：** `docs/design/foundation/durable-work.md`、`docs/plans/cognitive-use-roadmap.md`，视最新实现与 DBOS 文档调整。
- **WorkContext + Active Memory + Steward + 查询效用反馈。State：设计方向确认，详细合同待收敛。What remains：** 当前工作状态跨会话、检索使用真正的工作上下文，Steward 精确段落选取，区分召回→交付→接触→采纳→结果，有用性反馈作用于特定情境，逐步研究局部重排而非改变全局记忆真实性。**Continuation：** `docs/design/wave/query-context.md`、认知反馈/查询专题及下一轮架构讨论。
- **Episode/Observation 与多轴时间。State：核心语义确认，完整领域仍待规划。What remains：** 实际接触主体、材料与经历分段、事件发生/接触/认识形成/适用/记录的权威，历史知情视图先于几何传播，虚拟时间域；不把 Journal 的叙事顺序当事实时间。**Continuation：** `docs/design/foundation/time.md`、`docs/design/wave/temporal-views.md`。
- **Concept/Schema/Anchor/Association、Social、Ego、Motivation、Dream。State：原始概念与责任较明确，完整操作未实现或未核验。What remains：** 长期概念复用与去重的语义候选、解释性关联与真实独立支持、Schema 知识/技能归纳、Anchor 吸积、跨平台实体身份和社会图、人格稳定供给/情境召回、动机与 Work 区分、自由探索/反思。**Continuation：** `docs/design/cognition/`、VCP 研读及项目原始讨论。
- **NousQL + 接口/CLI 体验。State：语言设计存在，完整正式实施待定。What remains：** 自然语言优先与可组合约束、多时间轴和历史、短引用/本地查询键；CLI 用户实例名/默认上下文；正式 HTTP/MCP/SDK 各为共享操作适配，不暴露内部上百工具。**Continuation：** `docs/design/nousql/README.md`、`docs/design/interfaces/`。
- **VCP/Journal 长期科学问题。State：已有数学闭包及用户正面 Hybrid 实测。What remains：** 概念序列/多路径对关系寻址的真实贡献，时间/社会关系投影，弱线索与场/拓扑的区分；派生解释是否带来有效联系且不虚增见证；按真实长期使用观察，而不是靠某个总体测试率。**Continuation：** `docs/design/wave/`、`docs/research/vcp-adoption.md`、上游 TagMemo/RiverMemo 资料。

## 9. Relevant Resources

- **当前权威：** `https://github.com/Heptalogos-Devs/Nous`；最新用户报告 `main e6b1f24`（不在本次查看）。`docs/README.md` 为导航；`docs/ARCHITECTURE.md` 为整体职责；`docs/design/` 各专题为具体合同；`docs/plans/` 为本次授权的计划；`docs/CURRENT.md` 只记现实能力与待办；`docs/sources/` 是原样讨论，不自动升级为合同。
- **真实仓库开发指引：** `AGENTS.md`、`src/nous/AGENTS.md`、`src/nous/INDEX.md`；其中包括 AntiRLHF、模块责任、Python 3.14/uv、临时产物必须放忽略的 `temp/`、模型与工具原则。不要自行改 AGENTS，除非用户明确许可。
- **最近施工交付（历史 Spec，用户称最新已完成）：** `Nous_Journal_Maintenance_Spec_2026-11-05.zip`、`Nous_Journal_Maintenance_Goal.md`；`docs/plans/journal-maintenance.md`；`docs/design/cognition/maintenance-architecture.md`、`exact-editing.md`；`docs/design/models/prompt-editing-context.md`；`docs/design/wave/corpus-preparation.md`、`grouped-delivery.md`；`docs/design/foundation/operation-outcomes.md`；`docs/plans/cognitive-use-roadmap.md`；附 `patches/current-docs.patch`、`APPLY.md`、`REVIEW.md`。这是完成宣称下的历史施工材料，现行 docs 可能已变化。
- **先前基线：** `c2944b1675ddb79b9de17beb4f9522349fbbc8ce`（Journal first-use）。在它上面完成过代码审查；旧缺陷/行号不要直接套用到 `e6b1f24`。
- **认知专题：** `docs/design/cognition/{journal,written-records,journal-revisions,journalize,draft-adoption,citations,concepts,concept-annotations,annotation-adoption,feedback,current-retrieval,prompt-contracts,integration}.md`（逐项以现行仓库核对）；`docs/design/foundation/{configuration,time,content-storage,transactions,durable-work}.md`。
- **Wave/VCP：** `docs/design/wave/{overview,assets,engine,native,vcp,ranking,evidence-projection,journal-projection,temporal-views,persistence}.md`；`https://github.com/lioensky/VCPToolBox/tree/d5c019ebbac1f317c2938d655d2ce916e41b9164`；上游 `docs/TagMemo_Wave_Algorithm_Deep_Dive.md`、`docs/RIVERMEMO_TOPOLOGY_V3.md`、`TVStxt/Dailynote.txt`；旧代码迁入许可与改编见 Nous `native/vcp/UPSTREAM.md`（按最新路径核实）。项目资料中“VCP 研读”长文曾被读过。
- **更高层目标（设计者背景，不是直接施工指令）：** `https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md`；历史有效新设计见 Vault `docs/active-design-decisions` 分支。当前仓库 docs 拥有具体施工权威。
- **Skill 本身：** 用户上传 `SKILL-Session-Handoff.md`，`session-handoff 0.1.0`；要求 10 小节交接、准确标注确认与未知、停止实质工作、下一会话等待指令。

## 10. Verbatim Anchors

> “设计与架构永远置于代码之上。”

> “始终坚持低耦合高内聚，事物中心化。”

> “先能够配置，再决定如何暴露。”

> “不要被当前代码和已经落实的设计带偏，你的存在就是为了规划和设计更好的架构与体系。”

> “如果当前不符合你的要求，你应该直接要求改动，这是你作为架构师的目标与责任。”

> “Journal 是我们整个系统里面最重要的记忆载体，它拥有的生态丰富性、复杂性、操作多样性，通常难以想象，而且以后必然会经常扩展。”

> “重复调用会使得原文越来越短。这种情况应当避免，应该不可以把一篇 journal 再进行一次 journalize。”

> “所有的验证测试理应在这样一条流程上验证，而不是验证局部或者浅层。”

> “文档问题现在解决了，结构上功能上都很好。”（随后又明确要求专题化、保留原始讨论、更新现行规范时不可覆盖已有有效内容。）

> “最新提交已经是 e6b1f24，完成了上一轮的开发任务，你不用再查看这次提交，只为后人准备。”
