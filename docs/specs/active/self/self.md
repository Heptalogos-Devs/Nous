# Self

[返回合同目录](../../INDEX.md)

## Owner 与引用

`crates/self` 拥有 SelfFacet、NarrativeThread、修订、来源与生命周期。Runtime 继续拥有 Session/WorkContext/UseEvent，Core 拥有模型与公共宿主，Subject owner 拥有身份、能力供给和 Seed 来源。

SelfFacet 是可独立修订的事项：kind、完整 statement、scope、stance、exceptions、epistemic class、RevisionBasis、valid time 和 producer。kind 为 identity、role、capability_belief、limitation、tendency、value、preference 或 domain_knowledge；stance 为 adopted、tentative、aspirational 或 attributed。scope 保留自然语言说明，并可按现有 purpose、Entity、WorkContext、conversation 约束。purpose 的正式值为 world_authoring、subject_deliberation、professional_execution、subject_rendering、narrative_review、reflection、maintenance，与 Host 实际使用合同一致。

NarrativeThread 保存 theme、interpretation、exact cognition sources 与 basis。叙事重解释修订原 thread；它不重写历史来源。两类对象分别使用 `self_facet`/`narrative_thread` 和对应 `_revision` 引用，不与 Memory 共用对象族。

## 供给和来源

`SubjectCapabilities.self_cognition` 显式供给 Self，默认不启用。`subjects.provisionCapabilities` 只追加请求的能力，保存既有 Subject 和认知，不通过克隆更换身份。没有 Seed 不生成默认人格、传记或自我对象。

Seed 保存原文、版本和 adoption，接受原有声明格式和 `text/markdown`；Markdown/frontmatter 保持来源地位。Self 的 SeedBasis 指向已保存版本，以 `$` 或 `line:start-end` 定位真实源文档；locator 越界和 foreign Subject 来源被拒绝。Seed 不创建伪称真实接触的 Occurrence；设定和自我叙事使用明确的 narrative/simulated 等认识地位，不能冒充 observed 外部经历。

EvidenceRef 由 Material 核验实际 Occurrence 与 locator；cognition dependency 使用 exact revision。Self 的来源/叙事内容不授予工具或管理权限。

## 官方操作

Canonical `SelfService` 由官方 Client 的 `self` namespace 暴露：

- `adopt` 创建 facet 或 narrative 对象及初始 revision；
- `revise` 要求 exact target、当前 object epoch、内容和实际变化理由，同对象创建新 revision；无正文变化时保存操作结果而不制造新修订；
- `addEvidence` 为 exact revision 增加 support、counterexample 或 boundary_case，不因依据增加而全文重写；
- `get`、`getRevision`、`list`、`listRevisions` 提供当前、精确及有界历史视图；管理页包含不可用/历史项和完整追加 evidence，100 是单页上限，不是领域总量上限；
- `select` 是独立的有界当前候选读取，接收 sources、显式 targets、检索排名和实际条目/正文依据字节预算；返回 candidateScope 与明确省略，不冒充完整管理视图；
- `setLifecycle` 执行 suppression/restore、withdraw/reaccept；
- `purge` 通过正式授权操作清除对象、修订和所属 Runtime 引用，保留必要幂等及 tombstone 语义。

旧 purpose 的存储表示通过一次性迁移更新，当前读取与写入不保留旧值别名。已保存反思结果分开保留原输出与 typed preparation；恢复和修复以原输出计算 action identity，以迁移后的 preparation 构造请求。原 prompt、输出、producer、operation id 与对象修订身份保持不变。Self owner 回执摘要在核对原 canonical request 后更新，仍按同一操作重放原结果。

操作使用现有 mutation envelope、owner lock 和 receipt。同 operation id、同请求重放既有结果；输入改变冲突。修订同时核验精确 head 与 epoch；历史读取保留真实旧内容。Self 修订与生命周期变化使依赖的当前解释进入 revalidation_required，不静默改写正文。

普通“记住/忘记”、称赞、纠正和不赞同是交谈或来源；显式 Self 操作才修改领域状态。主体自己的选择、遵守受托工作与采用价值分别解释。

## 自动认知与查询贡献

Managed Projection 从已捕获的实际 usage 自动选择当前 SelfFacet，无需调用者拼装 refs。owner 在候选限额前以 SQL 核验生命周期、valid time、purpose、conversation、WorkContext 与已解析 Entity 条件。必需身份独立读取，不与普通条目竞争管理第一页；可选事项按既有 Query 提供的排名和共享 lexical generation 的相关 refs 优先选择，结构适用项仍保留有界候补，不能以一次关键词不匹配断言不适用。读取限额使用本次 projection 的条目预算；内部正文与依据读取上限为文本预算四倍（最少 16 KiB、最多 4 MiB），最终输出仍遵守原 projection 预算。已采用且适用的 identity 标为必需身份；其他适用认识保留完整条件作为情境指导。必需身份与 foreground WorkContext 在预算中先保留，无法完整容纳时终止准备；可选内容按语义单元整体省略。

用途和必需性由来源 owner 声明，Core 读取贡献的 domain，不根据一组不断增长的 wire-kind 字符串猜测。Self 不交给 Steward 自由改写；RESET/APPEND 继续复用 ContextCompiler。历史 Self 修订作为过去认识标记，不能冒充当前身份。

Self 使用现有 CognitiveContributor、共享 lexical/dense Serving、固定 fusion 和最终 owner materialization。Kernel 按 Subject 实际供给的域构成 owner 集合；Self-only 主体不要求 Memory 已启用。Query 支持 `self_facet`、`narrative_thread` 结果域，current/as-of 选择和当前 lifecycle/purge fence 分开核验。Self 的 Scope、epistemic class、依据和精确修订随命中返回。

直接依赖的 Memory/Schema/Episode/Journal 来源发生修订、失效、suppression 或 purge 时，既有依赖遍历同时使相关 Self 解释待重新核验。历史正文保留，当前解释不能继续作为可用认知注入。

## 模型形成与反思

`client.model.interpretSelf` 是显式授权的领域解释操作。输入指定同一 Subject 已采用的 Seed version，或实际 Occurrence 与可选文本 representation。Seed 使用 `self_formation`，观察反思使用 `self_reflection`；两种 role 都复用现有模型配置、严格结构输出、producer、执行 telemetry 和 model-results。没有配置模型角色时返回明确的 Self 领域失败，不创建默认人格。

模型从本批来源准备有界候选，不读取全域管理第一页。`interpretSelf.targets` 可显式指定需要核对的当前对象或 exact revision；`maxCandidates` 默认 32、上限 128。候选优先包括显式目标、来源的直接依赖/反向依赖、共享 lexical 检索结果与当前采用的身份；只补读一跳依赖，不拉 narrative 全历史。开放 scope 条件完整交给模型解释。来源检索文本最多前 2048 个 Unicode code point，该边界与实际候选/正文依据预算在 candidateScope 中声明。正文与追加 evidence 分别按选中 revision 批量读取，同一 repeatable-read 视图组合；管理页复用相同批量机制且不省略 evidence。

模型输入的 currentSelf 明确是候选目录，candidateScope 保留范围、预算和省略；未出现不表示 Subject 没有该事项。不能充分判断新建/修订时 keep 并指出需按 targets 补读的具体事项，不重新读取完整 Self。候选正文/依据预算沿用 formation_source_max_bytes 的四分之一，来源与最终输入继续受原预算约束。候选省略的 degradation 随已保存提案和正式响应保留。

模型按事项、类别和 scope 提议 adopt/revise/keep/evidence。引用只能选择调用内目录；Seed 行键绑定原始版本和 semantic path，观察绑定实际 EvidenceRef。名字与情境偏好分别形成可独立修订的事项。证据发生时间不等于认识的有效时间；持续采用的认识不能仅因选择发生在某时刻就被限制到该单点。新变化必须引用本次来源；当前 exact cognition 和原有依据可补充支持，目标不能依赖自身。NarrativeThread 引用已有 exact cognition revision，不把 Seed 故事伪造为 Occurrence。

付费的结构化提案、来源快照和实际 producer 在提交前保存。整个提案先校验目录选择，再逐项调用 Self owner；每项独立使用现有 receipt/CAS，没有跨领域总事务。同 operation id 重放已存结果，Core 重启不重新支付或重复采用；发生基础设施中断时复用已保存提案和 owner receipt 继续。无效提案也保留付费结果并报告失败。

返回 `domain=self`、`status=applied/kept/partial/failed` 与逐项结果。目标版本冲突明确失败，不能覆盖新状态；模型解释失败不撤销已经创建的 Subject 或保存的 Seed。来源保存与领域解释是两个正式、可分别核对结果的操作。调用方应先保存来源，再显式解释所需领域。

## 共享来源遍历

`client.cognition.trace` 统一遍历 Self、Memory/Schema/Episode/Journal、实际 Material 来源及 exact Seed version。Kernel 在同一认知 read view 中组合各 owner 的节点与来源声明；Memory 不再拥有通用 trace，也不将 Self 放进自己的事实支持对象族。节点与正文共享预算，重复引用只返回一次，边界省略明确标记 truncated。

Self 的原始 basis、异常、追加证据、叙事 exact sources 与实际 producer 一并可追溯。Seed 节点保留原文、格式、provenance、artifact 和版本；`subjects.getCognitiveSeedVersion` 读取指定来源版本，不把后续 adoption 写到旧来源上。读取 current adoption 仍使用 `getCognitiveSeed`。Seed 原文不是实际 Occurrence，也不能由通用遍历变成独立的观察事实。Self producer 复用共享签名注册表，trace 返回 canonical signature。

## 原生历史清除边界

正式 managed binding 在模型发送前通过 Runtime 检查来源和 Session 历史。显式 purge/suppression/withdrawal 使已经接触该来源的历史待 reset，沿父 Session 关系传递；删除认知与清理 UseEvent 在同一 owner 事务中保留该屏障。未接触来源的独立 Session 不被一并清空。标记跨 Core/Gateway 重启保留，旧实际模型输出和压缩摘要不能因新 slice 移除来源就继续发送。详细原生合同见 [Managed OpenClaw](../cognitive-runtime/openclaw.md)。

## 原生运行验证

CLI/MCP 提供正式 owner 操作、来源解释与只读 soul；OpenClaw 自动贡献走同一输入路径。原生行为场景覆盖无 Seed 的实际名字/有情境偏好、有 Seed 的独立主体、公共原文工具结果、同事项反例修订和精确旧历史，以及换话题、模型、会话和 Core/Gateway 重启后的连续性。普通聊天的“记住/忘记”通过状态比较核验权限边界。运行要求与命令由 [插件包说明](../../../../packages/openclaw/README.md) 维护。

## 容量验证

`pnpm smoke:self-selection` 已接入 `smoke:self`，在隔离实例中通过正式 API 先创建 121 项容量背景（多数撤回，其余不匹配 purpose/conversation/WorkContext 或已过期），再建立第一页之后的当前身份。检查管理分页与完整 evidence、精确历史、自动身份/省略、有界来源/目标及一跳依赖候选和反思输入范围。真实配置与 native 运行方法见 [公共 smoke](../../../../scripts/smoke/README.md)，不修改原 Subject。

## 领域知识与本次认识投影

`kind=domain_knowledge` 使用正式 `domainKnowledge` 内容：稳定 `domainRef`、`familiarity`（unfamiliar/aware/conversational/practiced/specialized）、`canDo`、`limitations`、`experienceKinds` 和 exact Motivation `interestRefs`。熟悉度是本人认识，不能授予工具权限或代表模型能力；亲历、耳闻、阅读、学会和实际练习保留区别。更新时间使用 owner 形成/记录时间，支持与反例继续使用原 basis/evidence/exceptions。普通 facet 不携带这一内容。

领域概括仍是独立 Self revision，具体事实、学习材料和经历继续由 Memory/Material owner 保存。后台 worker 结果仅在正式释放并被主体接触后成为学习来源；一次读取不自动升格专业能力，查询和使用不自动追加支持。interestRefs 进入既有 exact-reference、来源权限、依赖失效与 purge 图。没有 Seed 或明确学习背景时不补造专业、课程或旧经历。

官方 Projection 的 `epistemicView` 组合已经过来源许可和预算选择的 Self、Memory、Social 与当前材料贡献，返回 familiarDomains、acceptedFactsAndReports、experiences、unknowns、executionAssistance、learningOpportunities 和 coverage。Memory 保留其实际 epistemicClass，未知/省略不推导永久无知；当前协助不冒充既有个人能力。该视图是当次派生输出，Host 不拼装第二套领域知识库。官方 Managed Context 将同一视图交给 provider 前 renderer，理解、是否参与、委派和表达共同使用它。
