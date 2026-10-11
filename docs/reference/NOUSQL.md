# NousQL 当前实现参考

[文档目录](../INDEX.md) · [自包含 Agent 手册](../agent/NOUSQL.md)

NousQL 表达检索意图，Core 编译成 typed QueryExpr，Kernel Runtime 绑定查询，Serving 提供候选，Memory/Material owner 最终验证和物化。长期语义依据 [Architecture-Vault `5b96c63d`](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/5b96c63da34b0a4c697b6961ae10ba6aa4de3ee1/docs/Nous-Wave/TARGET_DESIGN.md)。

## 文本与身份

Agent [强烈建议显式化能够可靠识别的指称](../agent/NOUSQL.md#prefer-explicit-referents-for-retrieval)：使用具体名称和关键词，帮助 lexical、embedding 与 Entity/Tag activation。不能消解时原始自然语言仍正常查询。WorkContext 的自由文本与真实 Entity/Tag descriptors 在同一冻结 representation 中参与 embedding；foreground 后可跨多次查询复用。

必需的未加引号 Unicode 自然语言意图是独立的 lexical+dense 查询路径，不要求 Entity、Tag、WorkContext、concept model 或扩散。`#concept` 是独立 semantic text cue，不解析 durable Tag，也不走 lexical-only 伪造。`@tag` 解析 durable semantic concept，直接召回 attachment；`@e`、`@schema`、`@r`、`@object` 是 typed cues，只有 `@ref` 是 exact read。名称歧义必须选择返回的 LexicalRef，不退化为向量猜测。

表达式为单一自然语言意图与可选 `$`、`@`、`#` 语法岛。删除 quoted root、selector-only、universe、布尔查询树和裸正负偏好语法；普通文本中的符号保持原意。字面标记用 `\$`、`\@`、`\#`、`\\` 转义。偏好使用 `$prefer("text")`、`$avoid("text")`、`$prefer(recent,recorded)`。

## 返回域

根 `$return(cognition)` 指 Memory、Schema、Episode、Journal，也是默认集合。可用 `$return(memory,schema)` 等非空、去重的集合；Evidence、Resource 必须显式选择。Projection 在 candidate budget 前生效，也限制 exact target。旧域指令已删除，没有兼容别名；返回域不能在子表达式重新定义。

## 时间合同

`$time` 支持 occurred、observed、valid、formed、recorded 五轴；各轴分别指来源事件、收到证据、主张有效期、认知/派生形成、canonical 记录时间。不同轴可以同 scope 共存且取 hard intersection；同轴重复拒绝。Absolute `at/from/to` 必须有 timezone；`at` 是 Point：匹配精确 instant 或包含该点的 interval；`from/to/within` 是非空半开 Range，匹配其中的 instant 或与之重叠的 interval。unknown 不匹配已知约束。`within` 不与其他窗口参数混用。

相对时间使用 query preparation 捕获的 Subject CognitiveClock。`$asof(timestamp)`/`$asof(ago=30d)` 选择 Authority 知识截点；`$history` 允许 eligible prior cognition revisions 作为独立 documents。二者可组合，与五轴过滤独立。默认 current view 只投影 effective heads。

`@ref` 将该 scope 的候选集合闭合为绑定的 exact targets，不执行相似检索、Runtime resident recall、概念 enrichment 或模型 rerank。整个查询只有 exact scopes 时直接访问语义 owner，不依赖 Serving 或模型。对象引用绑定 current/`$asof` view 的 head；`$history` 不将该 exact 对象展开成所有历史版本，指定旧版应使用不可变 revision 引用。Projection、时间和生命周期 hard constraints 仍生效。

Historical exact binding、名称/别名、Tag canonicalization、immutable descriptors、attachments、AssociationEvidence、Schema links、Entity bindings 和 Material interpretations 使用截点状态。当前权限撤销、purge 与物理缺失仍是硬约束。冻结 QueryContextSnapshot 中晚于截点的 cognition anchors 在 representation/activation 前剔除，当前问题和 purpose 保留。

## 直接召回、概念与扩散

Tag 是稳定身份与可修订 label/description/kind_hint 组成的 embeddable semantic concept。共享 Concept generation 持有 canonical text/digest、可选 vector 与一跳 attachment postings；dense、Native、VCP 使用同一资产。无向量 postings 独立可用，不要求 embedding 才能 direct Tag recall。

QueryActivation 统一 explicit Tags、exact/current cognition、Entity/Schema cues、可选 inferred Tags、novel hypotheses、provenance 和 query embedding。`retrieval.query.concept_enrichment` 为 off/existing/model，默认 off；existing 使用共享向量，model 使用独立严格结构化角色且只能选 catalog keys 或提供 ephemeral 文本，不创建 Authority Tag。

`$explore` 明确开启 bounded associative diffusion；没有它不启用 Native/VCP topology。Agent 不选择物理算法，profile 与数值政策由 Host Configuration 决定。缺少兼容资产时显式 unavailable/degraded，不用 future current assets 替代。

## 控制、上下文与能力

`$return`、`$asof`、`$history`、`$effort`、`$limit`、`$diagnostics`、`$explore`、`$materialize` 为 root-only。其他支持项包括 `$source`、`$modality`、`$cognitiveRole`、`$formationMode`、`$evidenceClass`、`$authority`、`$current`、`$exclude`。`$current(none|prefer|required)` 属于 Resource freshness constraint，不能被子 scope 放宽。

查询上限为 32 KiB、2048 tokens，`$limit` 为 1–2048。persona/relation/rel 认知仍 unavailable；此次不增加 Self/Social/Motivation。

Query 可显式提供 Session、WorkContext、Situation refs/objects/descriptions/consumer。正式 TextCue 必须非空；自然语言代词没有 closure gate。只拒绝明确的未解析 machine placeholder。独立研究的 text-only 输入保持单一原始 TextCue，不添加 context/exploration/enrichment。

CapabilityPolicy 包括 textEmbedding、multimodalInterpretation、residualSensing、rerank、queryConceptEnrichment。Forbidden 不调用对应 provider/model；required 不满足时显式失败或 partial，optional 才允许有诊断的 degradation。Host embedding/profile 与 query concept role 分离。

## Preparation 与 inspection

`client.cognition.prepareQuery` 与 Query 共用 compiler/Identity resolver；`nous query prepare|inspect` 显示 root projection、TemporalFrame、captured clock、representation、resolved sources、profile、configuration digest 和 capability requirements，不调用 provider。

正式 Query 使用 Subject-bound single-use preparation token。Historical embedding cache miss 由 Host 在该 token 的 frozen Owner view 中发现、生成、提交，继续复用 existing embedding_materials/content digest；有界 batch 未完成时按能力合同报告。Query embedding 在 activation/dense/VCP/leaves 间复用。Actual generation trace 来自 query 持有的 read view，不来自 serving_current。

Full diagnostics 显示 QueryActivation、TagDirect/扩散来源、concept generation、实际 query concept model calls、截点 view digest、truncation/degradation。Preparation/validation tickets 共用 bounded slots/lease，在失败、消费、release 或 expiry 时清理。

## 反馈与显式形成

Formation `explicit_tags`/CLI `form --tag` 在 Memory commit transaction 中校验 Subject/current Tag、canonicalize merge 并去重，不要求模型推断 Tag。Concept API 是 `client.concepts`，RebindEntity 由 Identity facade 路由到 Material owner。

Query 返回 query_id；ReportUse 可携带它并引用实际返回的 exact revision。Runtime 校验 Subject、membership、retention，并将 bounded activation/hypotheses 送入后续 Concept Maintenance。Presented 不触发 review；result_refuted 是负反馈，不是 positive support。Review/feedback 不授权模型活动或 Authority mutation；Host 必须授予有界维护执行。
