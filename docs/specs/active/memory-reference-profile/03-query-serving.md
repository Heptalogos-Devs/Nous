# Query & Serving

[返回文档目录](../../INDEX.md)

长期寻址、Query preparation、概念维护和 owner materialization 语义依据 [Architecture-Vault `5b96c63`](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/5b96c63da34b0a4c697b6961ae10ba6aa4de3ee1/docs/Nous-Wave/TARGET_DESIGN.md)。

## Owner

Runtime (crates/runtime) owns QueryPlan, lane budgets, fusion and result contracts. Retrieval (crates/retrieval) implements serving lanes and generations; Core performs model/resource host actions; Memory and Material validate and materialize their own final Authority.

## BoundQuery

Query 在 candidate generation 前绑定 Subject、exact object/revision、CurrentOnly/ExactHistorical policy、enabled lanes、lane budgets、hard constraints、accessibility、embedding space、fusion version、topology intent 与 rerank policy。候选集合不得反向修改这些定义。

普通 current recall 只投影 effective heads。TemporalFrame 在整树根固定 captured Subject CognitiveClock、AuthorityView(current/as_of) 与 RevisionView(current/history)。`$history` 可召回 eligible prior revisions；`$asof` 使用截点 Owner header/head、Tag meaning/canonicalization、relations 与 Material selection。五轴条件继续过滤返回对象。Current exact object 绑定 revision+epoch，漂移拒绝重绑；historical exact 使用 frozen past head/epoch，同时执行当前权限与 purge fence。

## 事件、角色见闻与数据库截面

Query 使用同一正式带域时间模型，区分事件发生范围、角色世界见闻截止点 `knowledgeAtWorldTime`、现实记录截点 `asRecordedAt` 及当前/历史认知解释。指定世界见闻点不隐式设置现实数据库 as-of；指定现实截点不把它解释为世界坐标。五轴条件继续是对应事实的谓词，不能用 formed/observed 数字范围冒充完整知情视图。未知或未映射的跨域比较不命中正约束，返回相应未知/不适用信息。

世界见闻只读取正式释放给该 Subject/角色的可见接触。晚实现且兼容既定历史的虚拟见闻可出现在其故事时点的查询；若同时指定早于实际入库的现实截点，则不可见。真实 QQ/网页/工具的实际 observed/recorded 不允许按这一规则回填。查询“现在如何理解过去”可采用当前合法解释并保留其形成/记录时间；它不把后来判断伪装成过去数据库已知。

用途、角色、世界视角与 EpistemicView 从已认证运行绑定取得，普通 Query 参数不授予权限。SourceRead 与可见/可知切面在索引候选、向量、rerank、摘要及正文预算前应用，继续由原 owner 与同一 request-scoped 视图校验；不新建一份时间/世界授权索引。隐藏导演内容和 execution_only 运行结果不是 Subject 召回语料，不能依赖末端过滤补救。

Projection 保留本次正式语义钟、适用的时间范围、实际接触来源及现实记录时间；时间敏感当前状态失效后重新选择，不作为永久 transcript 当前事实。世界、角色知识和普通 cognition 通过现有官方 Query/Projection/managed 接线组合，不在 Host 复制逐域检索。

正式 QueryModifiers 使用 knowledgeAtWorldTime（DomainStamp）与 asRecordedAt（真实 UTC）分别选择角色故事见闻和数据库记录历史，可同时指定。前者按同域 formed 选择旧 cognition revision、按角色 world observed 选择晚实现 Material，并使用该切面的历史 Serving corpus；未知映射不匹配。后者继续通过真实 Authority cutoff 排除晚入库内容。首个 WorldLife 消费者与受信角色视角由配套 Host/SDK 贯通，不能以隔离查询证明持续世界已部署。

## Result projection

根 ResultProjection 为非空去重集合。默认 Memory、Schema、Episode、Journal；Evidence/Resource 显式选择。`$return(cognition|domains)` 只在根定义，整树与 exact target 共享。候选 budget 前按 projection 过滤，不从子 scope 重新建立返回域。旧 domain-only target/directives 已删除，无 compatibility aliases。

## Historical Serving 与 Concept plane

Memory、Material 各自投影 Owner historical semantics；Persistence 只读取 canonical chronology/immutable rows。mutable header/lexical metadata 保存在既有 Authority 数据库的 chronology，认知正文与 Tag revisions 不复制。ServingRecord 的 typed view descriptor 记录 current/historical、snapshot digest、as_of、revision view；historical generations 不发布到 serving_current，按 effective-state digest/config/space 复用，并由同一 reader、grace 和显式 reclamation 保护。

共享 lexical/dense/concept/Native/VCP builders 在 candidate generation 前选择历史 corpus。Future dense documents、Tag meanings、attachments、Associations、Schema links 与 Entity bindings 不进入历史资产。Material 的历史 effective derivations 用于 Episode interpretation fragments。当前权限撤销/purge 不能由 as-of 绕过。

Tag 的 canonical normalized semantic representation/digest 与可选 embedding 为一套共享 Concept asset，别名不改变 semantic digest。无向量 attachment postings 独立支持 TagDirect；dense/Native/VCP 复用向量。QueryActivation 统一所有 explicit/exact/context 与可选 semantic/model seeds；默认 enrichment off，bare text lexical+dense 是一等路径。Graph diffusion 仅由 `$explore` 显式授权，不由 effort 或 Tag absence 隐式开启。

## Lanes and fusion

当前 baseline lanes 为 exact、entity、lexical、dense、temporal、runtime，以及适用时的 SchemaDirect、TagDirect。每个 lane 返回 exact revision candidate、deterministic rank、generation/watermark 和 diagnostics；lane provider 不能产生 global score。

- Entity/Temporal/Runtime 必须从 Authority/Runtime typed structure 生成 bounded candidates，不得 application-side arbitrary first-N。Temporal lane 只接受 occurred、observed、valid、formed、recorded typed axes；unknown 不匹配已知时间约束，多轴是 hard intersection。
- Core `TimePredicate` 与 canonical Proto 区分 Point 和 Range。`at` 对 instant 要求精确相等，对 interval 要求点位于左闭右开范围内；`from/to/within` 与 instant 作包含判定，与 interval 作非空重叠判定。Range 必须满足 start < end，省略端点表示无界。冻结 CognitiveClock、绝对时间和相对时间保留 Timestamp 完整精度；点不转换成微小区间。Persistence 的 SQL 编码在 PostgreSQL 微秒存储格上保持这些比较的含义，owner 最终验证使用原始 predicate。
- Lexical relevance 只来自 Lexical Serving hit；不得以 DB substring admission 或 fallback rank 补造 hit。
- Dense 绑定单一 `EmbeddingSpaceSignature`，不同 space 不混合分数。
- Hard constraint 未定义时在 binding 阶段明确拒绝；不得 silent ignore。
- 同一 revision 的多 view 先在 lane 内聚合，再参与跨 lane fusion。

Runtime 持有 QueryPlan、lane/result contracts、budget 和固定 fusion semantics；Retrieval 只实现 lexical/dense/topology serving mechanics。唯一 production fusion owner 由 Runtime 当前 contract 提供，不能在 Memory service 与 Retrieval 之间复制。

## Request observation

Retrieval 在同一 bound leaf 内先准备 `PreparedQuerySignals`：query text、一次 query embedding 与只读 lexical/dense base hits。Dense 的兼容 generation 共用这份 embedding，不各自调用 provider；`text_embedding = FORBIDDEN` 时不准备 embedding。缺失 generation/provider 和调用失败保留原 lane 状态，由 Runtime 的 RequirementStrength 决定 Partial/Degraded。

`retrieval.cognitive.profile` 是 Developer/SubjectOverrideAllowed 的 typed fixed registry，使用 Live / QueryPolicy。四个 ID 为 `baseline-rrf`、`nous-node-potential-v1`、`vcp-dtsc-v9.2.1-adapter-v1`、`vcp-rivermemo-v3.1-adapter-v1`。Profile 在 BoundQuery/QueryPlan 中冻结；baseline 不产生 topology lane。当前 native topology readout 默认绑定 `nous-node-potential-v1`，机制为已冻结的 `experimental-node-potential-v1`。`QueryObservation` 以 query ID、bound time、topology generation、profile/config subset digest、source seeds 与五轴时间约束标识本次观测，只持有一份 QueryRiver。候选排名由 readout 计算，不写回 observation；普通 diagnostics 只报告 profile、seed/node/edge 数、实际最大 hop、完整性和 discarded mass。

Wave 的普通边必须支付 `normal_edge_cost`，预算不足时停止；合流状态合并能量和全部 origin，provenance 输出有稳定顺序。这些约束由实际传播执行，不依赖诊断层推断。当前默认数值行为由 frozen golden 保护。Profile 在 query 开始时冻结。DTSC 与 RiverMemo 共用同一 VCP immutable asset，asset digest 绑定 Authority watermark、embedding space/producer 与实际 asset policy；readout profile 不进入该 digest。Native graph 使用独立 asset contract。prepare 搜索 current 与 retired compatible artifacts，切回相同内容可重新 promote。VCP profile 的 embedding requirement 与 Dense 共用一次 preparation；FORBIDDEN 保持无调用。两个 VCP reference kernel/adapters 已接入 Authority-fenced query lane。

## Final authority

Core `QueryConstraints.matches_common` 根据 owner 提供的 typed `QueryFacts` 判定 authority、entity、来源 include/exclude、cognitive role、formation mode、modality 与 epistemic class；缺失事实不能满足显式条件。各 owner 继续决定权限、生命周期、时间轴与可访问性，不重述公共筛选规则。

Final validator 按绑定 view 批量读取 selected head、epoch、lifecycle、role/mode、aboutness、temporal、provenance/source class、authority/modality/epistemic 和 accessibility。Serving/lane prefilter 是优化，不是 Authority；不得产生每 candidate 一次 SQL 的 N+1 路径。

已标记 stale 的 generation 可以提供 candidate source，但必须标记 degradation，并在返回前丢弃 stale revision、suppressed、purged 或 constraint 不再匹配的 candidate。Unavailable required lane 产生明确 Partial；optional lane 产生 Degraded。Query 诊断区分 budget 不足、projection unavailable、Authority 不存在和主体未知。

Episode/Journal 使用 current lifecycle 与适用的 hard constraints，不参与 Memory/Schema 的年龄衰减。结果保留 exact revision、object epoch、有界正文和精确支持引用。

## Serving

物理 Serving family 只有 lexical、dense、topology 与 concept；Exact lane 直接使用 frozen exact bindings / owner SQL，不建立 postings 资产。Generation 回收在显式 Serving refresh / maintenance refresh 执行；reader、research pin 与 grace 保护实际 current/history 资产，query 不扫描整个 Serving 目录。

每类 projection 使用 immutable generation，绑定 authority watermark、producer/build identity、configuration digest、artifact checksum 和 vector-space identity。Authority 提交只使 generation 失效；重建生成新的 generation id，但语义结果必须仍指向同一 Authority identity/revision。watermark race 不得发布过期快照为 current。

Desired watermark 按资产实际依赖统一计算，状态、prepare、build stamp、promotion 和 publication 共用它。Native topology 使用空 space，只依赖真实 topology invalidation；VCP 使用 `vcp:<embedding space hash>`，聚合 topology、lexical 与所用 dense/concept space 的 invalidation。Text-only 变化不会刷新 native，Episode/Journal 的组织和生命周期改变会推进 topology。Publication 在已有 family/space 锁和 Subject 行锁内核对 desired 并安装 current，避免核对后 Authority 提交的竞态；不增加查询恢复流程。

复用包含artifact可读性/checksum验证。Serving owner拒绝缺失或损坏generation后，Persistence在同一family/space publication锁下标记该精确generation failed、仅删除仍指向它的current指针。相同watermark/config的重建不能再次复用失效current并删除新artifact。并发已发布的另一个current不被清除；Authority认知不随cache失效改写。failed artifact同样受reader/grace保护并进入回收。

Episode/Journal 的文本 projection 支持 exact、lexical、dense；显式 topology projection 使用其真实 cognition members、sources 和 exact supports，具体边合同见下文。

Episode canonical text 包含 title、boundary explanation、experience time，以及前 16 个成员中 occurrence 的有界文本片段。文本 Artifact 读取至多 2 KiB 的完整 UTF-8 前缀；媒体 occurrence 选择同 Artifact 的 ready coverage 派生文本，按 created time 与精确 representation ID 确定顺序，每段同样限制为 2 KiB。Serving 和查询正文共用该成员输入；描述更新使文本 projection 失效，不修改 Episode Authority。查询 evidence 保留所用派生描述的 exact ref 与 interpretation role，独立根仍由来源 lineage 决定。

Journal canonical text 包含 title、narrative 和按序 points；查询正文最多 64 KiB，截断保持完整 UTF-8 字符。

Native Wave 与 VCP 由显式检索 profile 启用；默认 Reference Query 不因 effort 自动开启 topology。EPA/Residual 数值研究保留，但独立 residual sensing 尚未接入生产查询，required 请求明确失败。Dense 不再构建未被查询消费的 EPA basis。实验结果不改变 baseline fusion、Authority eligibility 或 lifecycle。

## Serving 复用与回收

Kernel 固定 request-scoped immutable Serving view；并发 profile 切换不替换在途视图。BoundQuery 的 activation view 持有同一 Arc 查询视图，retained validation ticket 沿用它；finalize/release/expiry 释放最后一个持有者后，Retrieval 的弱 reader 注册不再保护该 generation。QueryExecution 不保存另一份 read lease，Runtime 不依赖 concrete Retrieval。

Retired collection 由显式 Serving refresh 和维护触发，普通 query prepare 不扫描整个资产目录。`serving.retired_grace_seconds` 为 Developer/SystemOnly/Live，默认 300 秒，范围 0..604800。当前 generation、active query readers/tickets 和显式 research pins 不回收；collection 只跳过被 active reader/ticket 引用的 generation；无关 retired generation 仍可回收。过 grace 的 unpinned retired artifact 删除物理目录与对应 catalog row，不保留 reclaimed audit/tombstone。该 pass 同时清理 owner root 下超过 grace 的未引用 UUID generation 和 orphan staging。配置/watermark 不兼容时不复用；重复并发 publication 复用已发布的相同内容身份并删除多余目录。

## Prepared Query 与 Query Representation

Exact target 是 scope 的候选集合边界，不能和 Runtime/lexical/dense/Tag/Schema/topology discovery 混合扩展。Runtime 仍按冻结绑定、projection、hard constraints 和 owner 最终验证读取；整个 expression 的有效 leaf scopes 都是 exact 时，Kernel 直接调用语义 owner，不准备 Serving generation、query embedding、concept enrichment 或 model rerank。对象 target 在 current/as-of view 绑定一个 head；history view 允许旧 immutable revision，不改变对象 exact read 为历史枚举。混合 typed tree 的非 exact scopes 保留各自 discovery 语义。

公开 `CognitionService.PrepareQuery` 与官方 Client `cognition.prepareQuery` 编译 NousQL 或 typed expression、解析 exact selectors、同一 repeatable-read transaction 捕获 Session/foreground WorkContext，并返回既有 `bound_query` 字段语义的 JSON inspection；不执行 retrieval、Serving build 或 provider。Inspection 包含 resolved CognitiveQuery、representation/version/SHA256/source refs/truncation flags、current refs、topology seeds、exact bindings、profile 与 ConfigSnapshot digest。

正式 query 必须包含非空 TextCue。自然语言代词和短 follow-up 是合法意图，不做多语言闭合启发式；只拒绝 `<UNRESOLVED:...>` machine placeholder。NousQL 为必需的未加引号 Unicode 意图与可选 $/@/# 语法岛，见 Agent 手册。Identity selectors 仍由 Directory 唯一解析。

QueryRequest 支持 `work_context_id` 和 `situation`（consumer、current refs、current objects、object descriptions）。显式 context 覆盖 foreground selection，须为同 Subject 的 open WorkContext。Runtime 在 preparation 冻结单一 QueryContextSnapshot（ResidentSet、WorkContext context_text/cognition/entity/tag anchors 与 request situation），保留来源；Runtime lane 和 topology 使用冻结 refs，执行时不重新读取另一个 Session/context。Embedding 只消费有界 current descriptors，不复制全部 ResidentSet 或 Session transcript。

Representation 顺序固定：Intent、Temporal orientation、Entities、Concepts、Schemas、Current cognition、Resources、Current objects、Current work、Consumer/task。Entity 使用 display name/必要 aliases，Tag 使用 label/description/kind，cognition 使用 bounded owner text；opaque identity 留在 exact/source refs，不当语义正文。缺失 descriptor 和超界截断显式报告；total budget 保持 UTF-8 完整字符。

`retrieval.query.representation` 是 typed Developer/SubjectOverrideAllowed/Live policy：配置只有 `total_chars=8192` 与 `max_context_items=16`；durable context_text 上限 64 KiB，total_chars 默认 8192、可配置到 32768；Current work 不再固定 1024 上限，必需 intent 超界拒绝而不截断。预算按 Intent → 显式时间/selector/semantic cue → WorkContext Entity/Tag → Current work purpose/questions/text → pinned cognition → ResidentSet/旁路背景分配，随后按固定 section 顺序渲染。同一 descriptor 取其最高输入优先级；截断同时报告 section 和受影响的输入来源类别。`sha256` 对实际 representation text 计算。

Core 查询先取得一次冻结 BoundQuery 的 bounded preparation token，再为完整 representation 生成最多一份 embedding，Kernel 直接消费该 token，保持 ConfigSnapshot 与 context 一致。Runtime 的 QueryReservation 将不可变 BoundQuery 与 infrastructure QueryLease 分开；Core 从同一执行机会传入剩余 lease，激活后的 preparation 和 execution/validation ticket 继承该原始截止时间，不重新开始计时，也不另读一个 Query lease 默认值。Tickets 共用 query slots，single-use、Subject-bound，并在 failure/finalize/release/expiry 清理；最终 validation 在使用和反馈发布前仍须有效。Dense、VCP sensing 与 expression leaves 显式共享同一 typed request embedding；受控 provider fallback 在 request 内复用成功或失败结果，lexical leaf 仍使用该 leaf intent；All/Any 的集合语义保持不变。Rerank 接收同一完整 representation 和原 validated candidates。

Core 的意图编译、prepared Query、embedding/concept/rerank/Resource 调用与 projection 共用 `core_execution.opportunity` work clock；最后 release 使用其有界 cleanup。官方 Client 的 Query、recall、prepareQuery、projection 与 managedContext 从 active Configuration 派生响应等待，较短 caller deadline/signal 保留。单个 provider timeout 与候选/算法预算各自保持 owning contract，不能重置该外层机会。

生产 `text_only_compatibility` flag 已删除。研究的 text-only 输入是普通无 context TextCue；轨迹输入显式携带 WorkContext/Session，按 v2 entry contract 分开报告。

QueryRequest 的 typed `capabilities` 传递 text embedding、multimodal interpretation、residual sensing 与 rerank requirement。`rerank: "forbidden"` 明确关闭 model rerank，适用于 deterministic algorithm/Agent wiring run；`text_embedding: "forbidden"`（Client 为 `textEmbedding`）禁止 embedding provider。Required 需求的失败不静默回退。

内部算法比较使用 `BoundQuery::for_profile` 从同一 prepared/context snapshot 生成只读 readout plan。Configuration 的 `query_override` 只允许 Live QueryPolicy，记录 OperationOverride 来源并计算独立 digest；不改变 active/desired/persisted 配置。Serving 根据对应 asset contract 复用 shared artifacts。

Native seed preparation first uses closed exact/context/Entity/Tag/relation anchors that map into the current graph. When no anchor maps into that graph, already prepared ready/truncated lexical and dense lanes can supply weak seeds: at most four distinct graph members per lane, bounded by the graph neighbour budget, with configured `lexical_promoted`/`dense_promoted` weight divided by lane rank. Out-of-generation references and rank zero are excluded. The fallback consumes the shared query signal; it makes no additional embedding request and does not change Wave propagation. Direct graph anchors remain authoritative for explicitly associative queries. Seed origin and weight are included in readout; a promoted direct hit is not a multi-hop witness.

Native current/history topology 共用所选 Authority 视图的结构读取，不加载 Material 正文或从材料文本构造 cognition graph。Topology projection includes every current accepted/valid/unsuppressed Memory, Episode, Journal and CognitiveSchema revision when Memory capability is enabled. Existing Episode cognition members/exact supports, Journal sources, Memory dependencies and active supporting Schema evidence links supply `cognition_basis` edges in both traversal directions, with one shared exact structure identity. These are source/organization adjacency, not causal or independently learned associations. Historical, withdrawn or suppressed endpoints, revoked Schema evidence, counterexamples and contradiction supports do not create positive source edges. Explicit AssociationEvidence to inactive cognition is excluded from the current graph. Native/VCP assets bind their actual policy, vector space and content identities; incompatible assets rebuild while compatible shared vectors remain reusable. There is no source-code or dependency hash upgrade check.

Material owner validates and materializes Artifact/Occurrence/SourceRegion/DerivedRepresentation/DerivedRegion in bounded Subject-scoped batches, including rerank final validation. Runtime groups candidate refs by semantic owner and does not interpret Material lineage or time through Persistence. Occurrence times come from the actual Observation; SourceRegion and derived evidence follow their real source artifacts/representation roots. An eligible source row must jointly satisfy requested occurred/observed/recorded predicates; multiple origins cannot satisfy separate predicates by mixing rows. Recorded time belongs to the returned object, and interpretation formation time is distinct from source observation. Unknown time does not satisfy a requested temporal axis, and evidence has no inferred world-valid claim. Initial owner materialization and final validation both enforce these predicates and publish known freshness. Schema owner materialization also applies all five axes and follows exact source lineage, including Memory evidence/dependencies, for occurred/observed metadata.

## Host activity 与 Query feedback

Host query concept role 只能选择 bounded catalog key 或提出 ephemeral hypotheses；模型不能构造 Tag identity/写 Authority。Required/optional/forbidden 分别沿既有能力合同处理。Historical embedding discovery/commit 使用同一 prepared token 的 frozen Owner view，复用 existing material cache；query embedding 不因 lane/profile 再生成。

Runtime 保存有界 query feedback record：prepared/activation digest、bounded signals、实际返回的 exact revision refs、expiry，默认七天；不保存检索正文。ReportUse 的 query_id 必须同 Subject、未过期且属于 returned membership。稳定 duplicate 可在 record expiry 后重试，accepted UseEvent 不随 feedback 清理删除。Presented 不触发概念 review；refutation 不是 positive reinforcement。Maintenance planner 消费 bounded signals，但只有 Host grant 才可调用模型和提交 canonical Concept mutations。

Serving cache 不承担跨 build 的隐式升级或迁移。资产身份由当前 Authority view/watermark、实际 policy、vector space 与 artifact content checksum 表达；打开时执行真实格式/数值 validation。代码或依赖升级需要显式清理可重建 Serving cache；不扫描源码生成版本 marker。每类 durable generation identity 同时用于 diagnostics 与 read pin，不另分配 process-local generation counter。

Generation 文件位置由当前 ServingRoot + generation UUID 派生，不把绝对 operator 路径写入持久 catalog。显式 reclamation 在 current、research pin、grace 与 live read 检查后删除物理 cache 与对应 catalog row，不保留无 consumer 的 reclaimed tombstone。跨中断不提供独立 cache repair protocol；残留 owner staging/UUID 目录可由下次显式 cleanup 清理，认知 Authority 不受影响。
