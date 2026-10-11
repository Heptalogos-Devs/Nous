# Gateway、Model 与 Prompt

[返回当前产品合同](../../INDEX.md)

## Owner

TypeScript Core owns external model invocation and client materialization. Rust Kernel owns Subject Authority, query/serving semantics and validation of persisted producer/material identity.

## Profiles 与 roles

Catalog 的 `models` 由同一模型 owner 持有完整 gateway/model/execution/role 图，作为一个原子 policy。部署 TOML 使用 `models.gateway_profiles`、`models.model_profiles`、`models.execution_profiles` 和 `models.roles` 分组；动态 profile 名称是图内键。owner 在冻结前校验显式引用、协议/能力、reasoning 和 controls，填入 protocol/gateway-dependent output tokens 与 timeout，旧的 downstream default resolver 已删除。

GatewayProfile 描述 endpoint、credential environment variable、enabled state 与 request timeout。Remote endpoint 使用 HTTPS；literal loopback 可使用 HTTP。Endpoint 不包含 userinfo、query 或 fragment。Token 只从配置引用的环境变量读取，不进入 ProducerSignature 或公开输出。

ModelProfile 描述 Gateway、standard protocol、model identifier、declared capabilities 和可选 model revision。Supported protocols:

```text
openai-chat
openai-responses
openai-embeddings
openai-audio-transcription
rerank-v1
```

Capabilities 包括 text、image_input、audio_input、video_input、structured_output、embedding、speech_transcription 与 rerank。ModelRole 是领域语义职责；RolePolicy 绑定 1..4 个有序 ExecutionProfile、Prompt 与 `optional | required` requirement。ExecutionProfile 持有 model profile、reasoning、sampling、max_output_tokens、timeout 与 16 KiB provider options，ModelProfile 只声明资源身份、协议、能力与支持的 reasoning levels。角色包括 projection_steward、memory_formation、material_description、material_structuring、material_direct_structuring、query_embedding、query_rerank、speech_transcription、episode_segmentation、journal_synthesis 与 memory_consolidation。全局 temperature 或 max-token override 不存在。

角色只有在其 profile、gateway、credential reference、Prompt 和本地 prerequisites 足以发起调用时才为 READY。未完整配置为 NOT_CONFIGURED；缺少本地 prerequisite 为 UNAVAILABLE。HTTP、timeout、cancellation 与输出校验结果由对应 operation 返回。optional degradation 与有序基础设施回退 由该 operation 定义；required role 不可用时 operation 失败。

Embedding profile 显式声明 dimension、weights revision、task、input representation、preprocessing identity/revision、normalization 与 output semantics。Core 据此形成 EmbeddingSpaceSignature；Kernel 持久化 producer identity 并以 embedding-space identity 隔离 Serving generations。

Model profiles 与 Prompt 在 Core 启动时解析；配置变更在 Core restart 后生效。SDK automatic retry 关闭；有序 execution fallback 处理基础设施、JSON/schema 和 generation input/output 合同错误，领域 Authority 的语义拒绝不触发另一轮生成。固定 snapshot 使用唯一 nous.model.execution format，只保存该角色的有序 routes、依赖资源、规范 controls、Prompt、provider implementation digest 与 output schema digest；无关 profile 或 media policy 不改变该角色身份。当前进程在 admission 前拒绝其他实现或输出合同的执行 snapshot。每个 outbound 调用计入 maintenance 预算。保存 proposal 后基础设施重试不得重新生成。

资源 schemas/types 由 profiles owner 提供；invocations 持有冻结 routes、admission 和 attempt 记录；protocols 负责 SDK/HTTP 调用且不依赖 ModelRole；Material interpretation owner 构造媒体输入并解释生成输出。每次调用先依据实际 physical input 过滤 route，再发送实际通道访问声明并校验输出。Source startup 与 Portable assembly 对 owning implementation 文件及实际依赖版本计算相同摘要，bundle 内嵌该值，不在 Portable 读取源码。Catalog 呈现身份与这些执行身份保持独立。

Attempt 区分 succeeded、failed、skipped 与 unknown；输入或资源不满足的 route 为 skipped，不发出请求。取消保留已经发生的请求与已知 usage；已传输而响应不可确认的请求记录 unknown，不记成成功或零成本。Provider response 与错误内容只在本地合同检查，公开失败分类不携带 credential 或原始 response body。

## Prompt registry

PromptRegistry 从 ProgramRoot/prompts 读取默认 Prompt；配置中的 `config-prompts/` 路径引用 ConfigurationRoot/prompts。文件必须位于对应 root、是非空 UTF-8 文本且不超过 128 KiB。Prompt 内容 digest 与 logical id 进入 ProducerSignature；Prompt 资产缺失或无效时该角色不可执行。Prompt 不支持 remote URL 或通用模板语言。

各角色 Prompt 只包含该任务的指令。来源文本、媒体、Evidence 与 candidate documents 以独立 input content 传入，并保持 untrusted input 身份，不能成为 system instruction。

## Structured Contract Registry

Core 的 `model/schemas/contracts.ts` 按 role 唯一绑定 model-facing Zod owner，拥有 `projection.steward`、`memory.formation`、`material.interpretation`、`episode.partition`、`journal.synthesis` 与 `memory.consolidation` 合同。两种 material structuring role 共享同一合同。Production generation 不接受调用方提供另一 schema；SDK 与 direct-media raw response format 使用相同 provider name 和由 Zod 派生的 draft-7 JSON Schema，其 canonical digest 随 producer 保存。

Schema title/description 说明字段语义；任务策略由 Prompt 拥有。Formation 的 title 与 Steward 的 summary 用 required nullable scalar 表达缺省状态。`inspect:model-contracts` 从同一 registry 和 PromptRegistry 导出实际合同、Prompt 与无敏感信息的角色身份，生成文件只保存在 ignored research data。检查命令及 research gateway trace 见 [开发脚本](../../../../scripts/README.md#模型合同与-trace-检查)。

## 输入语义与 Steward

Formation 使用 evidenceText、resolvedEntityCandidates、aboutnessMode envelope。证据、source dates 和作者观点保持其来源归属。Steward 只取得 policy-filtered segment id/role/text，合同为 task-agnostic faithful compression；没有 task intent 时不声称按 current consumer 任务选择相关性。Memory contribution 在 Kernel owner materialize，并携带 evidence 与实际 revision，受文本预算和领域可访问性规则约束。

## Producer identity

ProducerSignature 标识实际 adapter/protocol、operation、model identifier/revision、Prompt logical id/digest、实际成功 execution、ModelRole、ModelProfile、inference controls digest、RolePolicy digest、strategy 与 frozen configuration digest。Formation、Material 和维护使用模型 owner 的同一 metadata 映射，implementation 来自实际成功 adapter 的 owning code identity，不另写 SDK 版本常量。Credential、token 与 provider response body 不属于 producer identity。

模型输出形成派生表示或带来源的候选提案；Material 与认知领域 owner 验证并提交，由领域 Authority 持有认知身份与修订。

Core 的 formation、derivation 与 embedding preparation 共用 `core_execution.opportunity` 的 work budget；各 route timeout 限制单次 provider attempt，不能重置外层工作机会。ModelWorkflow private reservation 的 lease 由同一执行机会传入；取消后的 telemetry/save/release 使用剩余 cleanup budget。官方 Client 从 active Configuration 派生响应等待，较短 caller deadline 保留。维护机会与父 claim 语义见 [纵向认知](../cognitive-runtime/longitudinal-cognition.md)。

音频、视频和结构化 Material 的输入模式与提交语义见 [Material Derivation](material-derivation.md)；rerank 的候选、预算与 Authority revalidation 见 [Query/Rerank](../retrieval/rerank.md)。

## Query concept role 与历史 embedding

`query_concept_enrichment` 是独立 structured role，使用 `prompts/query/concept-enrichment.md` 与严格 catalog-key/novel-text schema。它不复用 formation 或 concept-maintenance prompt，不创建 Tag，不用无支持的 inferred concept 充当事实证据。Existing match/query embedding 与 dense/Native/VCP 共享 Concept asset；reference default enrichment off，capability forbidden 不调用 role。

历史 Query 执行可在 embedding 许可下通过同一 prepared token 发现、生成、提交截点 Owner-selected 文本的 cache miss，继续使用既有 embedding_materials/content digest。提交不得把 current Tag description 当作旧文本验证；有界 batch 不足或模型不可用按 required/optional 合同报告。Query prepare 只检查 captured representation/config/capabilities，不调用 provider。
