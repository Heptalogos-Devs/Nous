# Model Runtime 当前参考

[返回文档目录](../INDEX.md)

Core 启动配置用 `ModelRole → RolePolicy → ordered ExecutionProfile → ModelProfile → GatewayProfile` materialize 标准协议 clients。凭据只从 gateway 的 `credential_env` 读取。Kernel 保存经过规范化的 ProducerSignature 与不可变输入，不调用外部模型。

协议名称为 `openai-chat`、`openai-responses`、`openai-embeddings`、`openai-audio-transcription`、`rerank-v1`。SDK generation、embedding 和 transcription 使用显式 endpoint/model；rerank 使用有界 HTTP adapter。SDK retries 为 0，远程 destination 需要无 credential/query 的 HTTPS；literal loopback 可用 HTTP。

RolePolicy 持有 1..4 个有序 execution routes、Prompt 与 optional|required requirement；ExecutionProfile 持有模型引用、reasoning、temperature、top_p、输出 token 上限、timeout 与有界 provider options。reasoning 接受 provider-default/none/minimal/low/medium/high/xhigh，并按声明能力校验。角色具备完整可执行配置时报告 READY；未完整配置为 NOT_CONFIGURED，本地 prerequisite 缺失为 UNAVAILABLE。HTTP、timeout 和输出校验失败由当次 operation 返回。

Prompt 从仓库 `prompts/` 的 UTF-8 Markdown 加载；custom path 须落在允许 root，每份最多 128 KiB。logical id、内容 digest、role config digest 与实际 model/protocol 进入 producer。Prompt asset 缺失或无效时对应 role 不可执行。

SDK generation 使用独立 `instructions` 与 user material；provider adapter 决定实际 system/developer 映射。Prompt 按任务、来源忠实性、selector 目录和输出合同组织。结构化验证约束输出形状与目录引用，不能把模型生成的姓名、日期或概括自动当作事实正确。Query enrichment 的 novel concepts 是短检索概念／问题，未消解的词义保持未消解。

## Material 与 formation

`client.model.deriveMaterial({ subjectId, sourceRegionId, strategy, target, supersedes })` 返回实际 committed `representations[]`、selected representation 和 degradation。Configuration 的 `material.strategy` 默认 `description_only`，还接受 `direct_structured`、`describe_then_structure`。

- Text-like source：验证 UTF-8 后提交 ExtractedText，不调用模型重写。
- Image：`material_description` 生成 rich ImageDescription；direct structured 使用独立 `material_direct_structuring` model/Prompt/参数。两阶段使用 description 的 exact ref 作为 structuring input。
- Audio：`input_mode=direct` 默认发送多模态 `input_audio` 并形成 AudioDescription；direct_structured 与两阶段使用对应的 material roles。`input_mode=transcription` 使用 speech_transcription 形成 Transcript；该模式不支持 direct_structured，describe_then_structure 可继续结构化 Transcript。
- Video：默认将原始 Artifact 通过 gateway chat 的 video_url 扩展发送；frames 是已实现的显式 FFmpeg 模式，使用有界抽帧与可选音轨转写。观测情况见 [Research](../research/README.md)。

结构化角色通过 `model/schemas/contracts.ts` 唯一选择对应 Zod owner；Material 合同由 `model/schemas/material-interpretation.ts` 拥有，SDK 与 raw strict 请求共用同一派生 JSON Schema，outputSchemaDigest 进入 producer 与 derivation identity。自由描述不使用结构化 envelope；结构化表示保存一等 `structuredPayload` 和保留 basis/uncertainty 的 deterministic text projection。

两阶段先提交描述，再由 Material owner 生成 description_segment DerivedRegion（UTF-8 byte span）。第二模型只输出 invocation-local basis keys；提交前映射成 payload 中的 stable `basis_refs` refs，Kernel 核验它们属于输入图。summary 和内容项都保存 stable basis refs；basis、certainty 与 evidence_channel 分开；证据通道独立于所陈述的 event/state/action，original source text 与视觉 embedded text 分开。字段可通过 `client.material.derivedRegion` 与 `client.material.materialize` 精确回读，支持链可回溯原始 SourceRegion/Artifact。

Structuring 失败保留已提交的 description。普通 recall 不重新解释媒体。显式 `supersedes` 是更新 lineage；同一 lineage/input/producer/strategy request 复用成功结果。

`client.model.formFromObservation({ operationId, subjectId, occurrenceId, representationId?, aboutnessMode?, explicitAboutness? })` 使用指定表示，或按实际媒体策略选择最新描述/转写。operationId 由调用方稳定提供；同 ID/input 重放 outcome，不受后续配置变化影响，不同 input conflict。未解释的 binary source 返回 `material_representation_required`。Aboutness 支持 explicit、select_from_resolved_mentions（默认）与 none；select 模式只使用 Kernel 提供的 resolved mention candidate keys，Actor identity 单独保存于 Observation。

Memory revision 的 `producerSignatureId` 可用 `client.material.producer` 读取。Material 的表示、inputs、DerivedRegion、SourceRegion、Artifact 都有同 Subject 的 public read path。`nous trace` 只经 official Client 展开这条链，保存的 producer 不包含 token。

同一 producer read 也覆盖 Episode、Journal、Schema、Tag revisions 与 AssociationEvidence；historical revisions／revoked association 的实际引用仍可追溯，跨 Subject 不开放未引用的签名。最新角色模型与 reasoning 部署建议、真实上下文和延迟观测见 [Cognitive Model & Evolution Research](../research/cognitive-model-evolution-2026-10-08.md)。Embedding 的 max_batch_size 要按实际 endpoint 配置；本轮 Doubao endpoint 明确拒绝超过 10 条的输入，配置为 10 后完整 preparation 成功。

使用 `corepack pnpm inspect:model-contracts --all` 导出实际 provider schema、Prompt 与 digest；通过 research gateway 的 `--trace-root` 和 `inspect:model-trace` 检查真实 New API wire attempt。命令详见 [开发脚本](../../scripts/README.md#模型合同与-trace-检查)。

完整配置与语义合同见 [Gateway/model/Prompt Spec](../specs/active/model-runtime/gateway-model-and-prompts.md)、[Material Spec](../specs/active/model-runtime/material-derivation.md)。


## 纵向维护角色

`episode_segmentation`、`journal_synthesis`、`memory_consolidation` 使用 text + structured_output profile 和 canonical Zod schema。Core 的 `client.cognition.grantMaintenance({ subjectId, maxOperations, maxModelCalls, maxElapsedMs })` 执行有界机会，Rust owner 验证并提交模型 proposal。Core 在 claim 前只允许可执行角色对应的 maintenance kinds；未配置角色不取得 lease 或增加 attempt count。临时 provider 故障使用 Configuration Service 的有界指数退避。角色配置、Prompt 和超时沿用现有 ModelProfile/ExecutionProfile/RolePolicy 机制。数据身份与重试语义见 [纵向认知合同](../specs/active/cognitive-runtime/longitudinal-cognition.md)。

强模型的长程维护实测可达 235–264 秒。一次 grant 的 elapsed budget 由全部调用共享；先前调用耗时后，下一调用的可用时间会变短。研究 runner 默认每次 grant 一个模型调用；Host 可按实际角色延迟安排后续机会。Node Client 默认 transport timeout 为 30 秒，长程调用须同时给足 API 与 transport 时间，例如：

```ts
await client.cognition.grantMaintenance(
  { subjectId, maxOperations: 16, maxModelCalls: 1, maxElapsedMs: 300_000 },
  { timeoutMs: 330_000 },
);
```

## 输入与投影

Formation envelope 的 `evidenceText` 是原始来源或已提交表示正文，`resolvedEntityCandidates` 是可选择实体目录，`aboutnessMode` 由 owner 决定。作者观点保留作者归属，观察者身份不自动成为 aboutness。Material text structuring 使用 `evidence_text` 和 `evidence_kind` 区分 original_text 与 committed_representation；后者不提供原始媒体访问。

Projection Steward 接收经 consumer policy 筛选的 id/role/text，执行无任务上下文的忠实压缩。Kernel contribution owner 经现有 ContextResolver materialize Memory 内容，遵守请求文本预算、可访问性与 lifecycle，保存实际 revision 和 evidence；Core 在模型前执行 consumer policy。Steward 不承担未提供 query/Focus 的任务相关性判断。


`concept_maintenance` 是独立 structured role，默认 Prompt 为 `prompts/memory/concept-maintenance.md`。Role READY 时才进入 maintenance allowed kinds；generation 沿同一固定 role/config/Prompt snapshot、provider-call reservation、durable proposal/receipt 与 lease/retry 路径。模型输入使用局部 cognition/tag/entity/association/basis keys，禁止自由 UUID 或 catalog 外 refs；single noisy occurrence、单纯词法重叠和 exposure 不证明长期 concept。`no_change` 单独输出，新增 Tag 与后置 attachment 在同一 proposal 中表达。


## Query concept role 与历史 embedding

`query_concept_enrichment` 是独立 structured role，使用 `prompts/query/concept-enrichment.md` 与严格 catalog-key/novel-text schema。它不复用 formation 或 concept-maintenance prompt，不创建 Tag，不用无支持的 inferred concept 充当事实证据。Existing match/query embedding 与 dense/Native/VCP 共享 Concept asset；reference default enrichment off，capability forbidden 不调用 role。

历史 Query 执行可在 embedding 许可下通过同一 prepared token 发现、生成、提交截点 Owner-selected 文本的 cache miss，继续使用既有 embedding_materials/content digest。提交不得把 current Tag description 当作旧文本验证；有界 batch 不足或模型不可用按 required/optional 合同报告。Query prepare 只检查 captured representation/config/capabilities，不调用 provider。

Execution snapshot 冻结完整 RolePolicy、routes、ExecutionProfiles、ModelProfiles、Prompt 内容和结构化合同。只允许有界 transport/timeout/HTTP/schema 回退；owner 的语义、catalog、ownership、provenance/lifecycle 拒绝不回退。保存 proposal 后重试复用原 proposal。每次实际 maintenance outbound 调用单独计入 grant；ProducerSignature 保存实际成功 route、模型、角色、controls/policy/Prompt/contract digests；workflow receipt 保存最多 64 个累积 attempts、failure class、usage、latency 与省略计数。Embedding 所有备用 route 必须共享完整 space signature，缓存、提交与 Serving 按实际 producer 分区。
