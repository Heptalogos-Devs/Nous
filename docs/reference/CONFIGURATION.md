# Configuration Service 当前参考

[返回 Reference](README.md)

操作者配置位于 `ConfigurationRoot/nous.toml`，声明 `config_revision = 2`。配置使用自然 TOML section；完整示例见 [nous.toml](examples/nous.toml)。[bootstrap.toml](examples/bootstrap.toml) 只定位独立运行目录。

## Catalog 与所有权

Kernel Configuration Service 统一持有 Rust 与 Core owner 注册的目录、部署值、持久覆盖、immutable snapshot 和 mutation receipt。Rust 类型由 `schemars` 发布 Draft 2020-12 JSON Schema；Core 的 Zod 4 owner schema 通过原生 `z.toJSONSchema()` 发布。Kernel 使用 `jsonschema` 校验 schema 和值，在正常服务启动前完成目录。

每个 descriptor 包含 path、owner、title、description、category、JSON Schema、reference default、exposure、scope、storage、apply mode、semantic effect、unit、sensitivity 和可选 reference profile。动态 profile map 是一个结构化配置值，TOML traversal 到达拥有该路径的 descriptor 时消费整个值。

Overrideable 值按 reference default → deployment TOML → persisted system override → persisted Subject override 解析。Deployment-only 值只使用 reference default 与 deployment TOML，管理 API 拒绝持久覆盖。SystemOnly 与 SubjectOverrideAllowed 声明作用域；暴露等级只控制呈现，不授予或限制管理权限。Core 的认证边界保护管理 API。

| 暴露等级 | 设置范围 |
| --- | --- |
| Standard | gateway/model setup、material strategy、maintenance 开关与 host 启动设置 |
| Advanced | role、media、Resource、consumer、Episode/Journal/consolidation 策略 |
| Developer | retrieval/accessibility/topology 算法、worker opportunity、retry 和运行预算 |

## 启动与快照

Core 先解析 `config_revision`、`host` 和 `database` bootstrap，通过私有 JSON bundle 交付 Core descriptors 与 deployment document。Kernel finalize Catalog 并加载持久覆盖后，Core 从 active snapshot 构造 model/resource/consumer runtime。凭据值只从 SecretRoot/environment 加载；Catalog 保存 credential reference。

Live 修改作用于后续 operation；在途 operation 保留固定快照。RestartProcess 修改 desired snapshot 并返回 restart effect，active snapshot 在重启前不变。Core model/gateway/role/media/resource/consumer、`material.inputs` 与 `core_execution` 结构使用 RestartProcess。`core_execution` 拥有 Kernel RPC、maintenance RPC、工作、cleanup、need acknowledgement 和回应等待的 typed opportunity policy 、HTTP body byte budget 和 managed context track 上限（默认 256）、Query embedding cache entries（默认 128）及 rerank candidate 上限（默认 64）；host startup/shutdown timeout 和 runtime download timeout（默认 300000 ms）是 deployment-only bootstrap 参数。NewSubjectsOnly 更新供给默认，已有 Subject 保存已采用的 typed capability set。ServingRebuild 返回 owner rebuild effect；Serving 在下一次需要该 family 的 prepare 或显式 refresh 中构建并原子替换 generation。Projection status 按 family/space 返回 generation ID、Authority watermark、实际配置 digest 和当前所需 digest；Authority 或配置落后时为 STALE。

Query、Authority formation 和 Serving build 使用固定 snapshot；影响输出语义的 key subset digest 包含对应 schema 与 reference profile identity。retrieval ranking/budgets、Memory accessibility、topology wave、EPA basis 和 longitudinal 参数的参考族位于 `config/reference/` 的当前 JSON，owner 从目录快照解析 typed policy。

Artifact 上传预算为 `object_store.max_upload_bytes`，归 Material owner；Core multipart receiver 和 official Client 读取同一 active limit。Description segmentation 使用 `material.description_segment_bytes`，默认 2048 UTF-8 bytes，范围 4..65536；1 MiB input 与 512 region 是代码拥有的 hard safety ceilings。

## 管理 API、Client 与 CLI

Canonical `ConfigurationService` 提供 list/describe/get 与 system/Subject set/clear。Schema 和配置值使用 protobuf JSON Value；结构化值保持结构。所有公开写入携带 desired view 返回的 `configuration_revision` 作为 `expected_revision`；Kernel 在同一数据库事务内比较全局修订，过期写入或清除返回 conflict，不改变配置或修订。Mutation identity 包含 operation ID、expected revision 和规范输入；同 ID 不同输入 conflict。已完成请求先重放冻结回执，即使后续修改已推进修订。SystemService 提供系统状态与 projection 状态。

Official Client 使用 `client.configuration.list / describe / get / setSystem / clearSystem / setSubject / clearSubject`；写入参数要求 `expectedRevision: bigint`。CLI 默认显示 Standard，`--advanced` 包含 Advanced，`--developer` 包含全部目录；`--subject <id>` 选择 Subject scope，`--desired` 读取 desired view。`set/clear` 必须显式提供 `--expected-revision`，重试使用原 operation ID、原 expected revision 和原值。

```sh
nous config list
nous config list --developer
nous config describe maintenance.enabled
nous config get runtime.resident_limit --desired
nous config get maintenance.enabled --desired
nous config set maintenance.enabled true --expected-revision <configurationRevision>
nous config set maintenance.enabled false --subject <id> --expected-revision <configurationRevision>
nous config clear maintenance.enabled --subject <id> --expected-revision <configurationRevision>
nous config get models --desired
nous config describe models
nous config check --home <instance>
```

Set 的值统一使用 JSON 语法：boolean、number、带引号 string、array 或 object。CLI 类型、范围、单位、默认值、来源与应用模式来自目录。

`nous init` 与首次 `nous serve` exclusive-create 最小配置；已有文件保持原样。`config check` 复用 production bootstrap/catalog validation path，检查版本、schema、未知路径、descriptor overlap、Prompt 和显式 executable reference；无需凭据或 provider 调用。

## Models 与 Prompts

配置进入快照前由 owning type 规范化：Rust 使用注册类型的反序列化/校验/序列化，Core 使用 Zod owner，并向 Kernel 交付完整规范值。`config get`、执行消费者和 digest 使用同一值；省略结构默认字段与显式填写相同默认值具有同一执行身份。Core 的部署文件和公开 override 入口使用同一 normalizer，Kernel 继续唯一拥有覆盖顺序和 active/desired 状态。

Catalog identity 包含展示文案和 exposure；执行 identity 排除这些展示元数据，并保留有效值及约束。subset digest 将相关 paths 作为排序、去重的集合；schema 注释只在配置 schema 位置排除，模型实际 Prompt/输出合同中的说明继续参与其模型身份。

`models` 是 gateway/model/execution/role 引用图的原子配置值；修改时提交完整候选图，owner 校验引用与协议关系、填入实际 execution 默认值后才进入 desired snapshot。`models.gateway_profiles` 指定 endpoint、credential environment variable、enabled state 和 request timeout。Remote endpoint 使用 HTTPS；literal loopback 可使用 HTTP。凭据从 SecretRoot 的 dotenv 文件或进程环境读取，进程环境优先；凭据不进入公开输出。

`models.model_profiles` 描述 gateway、标准 protocol、model identifier、能力和可选 revision。Embedding profile 同时声明 dimension、weights revision、task、input representation、preprocessing identity/revision、normalization 与 output semantics，组成 EmbeddingSpaceSignature。`models.execution_profiles` 拥有 model profile、reasoning、sampling、output token limit、timeout 和 provider options；`models.roles` 选择有序 execution routes、Prompt 和 `optional | required` requirement。不存在的显式引用和非法协议组合是配置错误；disabled gateway、缺少凭据和未填写模型 identifier 是运行时能力状态。

当前 role 包括 projection steward、Memory formation、material description/structuring/direct structuring、query embedding/rerank、speech transcription、episode segmentation、Journal synthesis 和 Memory consolidation。Supported protocol 是 `openai-chat`、`openai-responses`、`openai-embeddings`、`openai-audio-transcription` 与 `rerank-v1`。

Prompt 默认来自 ProgramRoot/prompts；配置可使用 `config-prompts/` 前缀引用 ConfigurationRoot/prompts 下的文件。Prompt 必须是 UTF-8、位于所属 root 内且不超过 128 KiB。Producer identity 包含 Prompt 与 role config digest；token 不进入 ProducerSignature。

## Media 与 External Resource

`audio.input_mode` 为 `direct`（默认）或 `transcription`。Direct 将原始 Artifact bytes 作为 chat 的 `input_audio` 发送；transcription 需要 speech_transcription role。

独立字段具有真实叶路径，例如 `video.input_mode`、`video.max_frames`、`audio.max_source_bytes`、`material.inputs.formation_source_max_bytes`、`core_execution.opportunity.work_timeout_ms` 与 `consumer_state.receipt_limit`。`config describe video.max_frames` 直接返回该字段的范围与默认值；`config set video.max_frames 4 --expected-revision <configurationRevision>` 只覆盖该字段，其他字段保留各自来源。Catalog 不同时注册这些字段的 parent。媒体、Material 输入和策略影响 Authority formation；consumer composition 和 rerank candidate budget 标注 Query policy，其余基础设施预算标注 operational。当前这些 Core 参数仍实际使用 RestartProcess，按回执重启后生效。

`video.ffmpeg_executable` 默认显式 null，表示使用 RuntimeRoot 已安装 FFmpeg pack；string 表示 ConfigurationRoot 下的显式 executable。清除 override 回落到部署值或 reference default，显式 null 覆盖部署的 executable，空 string 非法。模型图中的空 profile map 是明确空集合，省略字段则由 owning graph 填入默认值。

`video.input_mode` 为 `direct`（默认）或 `frames`。Direct 将原始 Artifact bytes 作为 chat 的 `video_url` 发送；frames 使用有界 FFmpeg 抽帧与可选音轨转写。FFmpeg 来自显式 executable 或当前 RuntimeRoot 已安装 pack。两种 mode 都受来源字节上限约束；frames 另受时长、帧数、单帧、音频与进程时限约束。没有隐式模式回退。frames 的实验观测见 [Research](../research/README.md)。

外部 Resource adapter 由宿主在启动时显式提供，并按 `adapter_kind`/`provider_profile` 解析。普通安装默认没有外部 provider；直接输入 Nous 的材料由 Material 与原生 Serving 管理。Resource descriptor 和 selected reference 使用公共资源合同；研究宿主的 LocalDocuments 使用同一生产接纳路径。目录不包含供应商专属 `resource_profiles` 配置。

`consumers` 按 consumer id/revision 保存 Memory、Runtime、Resource contribution requirements 与 item/text budgets。Consumer policy 为一次调用限定各 owner 可贡献的内容和预算；领域 Authority 仍由对应 owner 持有。

## 纵向认知配置

这些 typed settings 由 Runtime/Memory registry 定义，支持 registry 声明的 scope override 与 apply mode；实际生效快照由 Configuration Service 读取。

| Setting | 默认值 | 单位/含义 |
| --- | --- | --- |
| `maintenance.concept` | 16 candidates / 4 suggestions | Developer、Live、AuthorityFormation：局部 Tag 候选和逐项 ordered suggestions；候选范围 1..32，建议范围 1..4 |
| `maintenance.concept_use_review_interval` | 3 | Advanced、Live、AuthorityFormation：meaningful use count 跨 interval 时排入 concept review，范围 1..128 |
| `maintenance.accretion` | enabled / generic_degree 32 / recurrence_review 3 | Developer、Live、AuthorityFormation：按需派生聚合，genericity 和 recurrence review；关闭时不计算 hints，基础 concept workflow 仍工作，无 provider 调用 |
| `maintenance.enabled` | true | host maintenance 开关 |
| `maintenance.poll_interval_seconds` | 30 | standalone loop 的基础设施秒 |
| `maintenance.worker_lease_seconds` | 120 | worker lease 的基础设施秒 |
| `maintenance.max_operations_per_grant` | 4 | 每次机会/standalone tick 的操作数上限 |
| `maintenance.experience_batch_size` | 256 | Developer：每次 Episode segmentation processing operation 的 ExperienceItem batch，范围 1..256 |
| `maintenance.member_text_max_bytes` | 2048 | Developer：每个 maintenance Experience member 的输入 byte 预算；共同 materialization safety ceiling 由实现持有 |
| `maintenance.terminal_retention_seconds` | 86400 | terminal need finish replay 的基础设施秒，范围 1..604800 |
| `maintenance.retry_initial_seconds` | 30 | transient retry 初始基础设施秒，范围 1..3600 |
| `maintenance.retry_max_seconds` | 3600 | transient retry 延迟上限秒，范围 1..86400 |
| `maintenance.retry_max_attempts` | 8 | 连续 transient failure 上限，范围 1..32；达到后 blocked |
| `maintenance.max_model_calls_per_tick` | 4 | standalone tick 全局模型调用预算，范围 1..32 |
| `maintenance.max_elapsed_ms_per_tick` | 60000 | standalone tick 全局 elapsed 毫秒预算，范围 1..900000 |
| `episode.context_switch_count` | 2 | 形成边界所需变化的 context dimensions，范围 1..4 |
| `episode.synopsis` | longitudinal | Developer：Episode member text synopsis 的 member/fragment/total byte 预算；同一 policy 用于 Query rendering 与 lexical/dense generation |
| `episode.soft_idle_seconds` | 300 | 认知秒 |
| `episode.hard_idle_seconds` | 1800 | 认知秒 |
| `episode.settle_delay_seconds` | 300 | semantic review 的认知秒 |
| `episode.max_neighbor_episodes` | 8 | 局部 repair 的 Episode 数 |
| `episode.max_neighbor_span_seconds` | 86400 | 局部 repair 的认知秒 |
| `journal.max_episode_count` | 12 | 一次 Journal synthesis 的 Episode 数 |
| `journal.max_span_seconds` | 86400 | Journal scope 的认知秒 |
| `consolidation.settle_delay_seconds` | 300 | 整合前的认知秒 |
| `consolidation.max_actions` | 8 | 一个 ordered model proposal 的 action 数；每项独立 owner mutation |
| `consolidation.context` | longitudinal | Developer：query cue/candidate text 字符预算、candidate/support/provenance/entity 数上限；运行开始解析一次 |

`GrantMaintenance` 调用同时提供 operation/model-call/elapsed budgets；有效操作数还受当前 registry policy 限制。`maintenance.poll_interval_seconds`、`maintenance.worker_lease_seconds`、experience batch、member text、retention/retry/tick budget 设置均由 `cognitive-runtime` owner 注册，使用 Developer exposure、SystemOnly scope、Live apply mode 和 Operational semantic effect。其他以上设置允许 Subject override。retry 延迟为 `min(retry_max_seconds, retry_initial_seconds × 2^(连续失败次数−1))`。语义合同见 [纵向认知](../specs/active/cognitive-runtime/longitudinal-cognition.md)。

`material.inputs` 是 Developer 运行输入预算：`formation_source_max_bytes` 默认 32768，`derivation_source_max_bytes` 默认 1048576。超界 source 必须选择 bounded representation/region；预算不允许截断完整来源后仍声明完整支持。Memory formation workflow 保存首次采用的 source budget，retry 使用该快照。

`retrieval.query.default_result_limit` 默认 12，作用于未声明 result limit 的公开 Query。`core_execution.public_rpc_response_max_bytes` 默认 4194304；公开 RPC request 预算使用 `http_body_limit_bytes`。

`video.frame_end_margin_seconds` 默认 0.1；frame sampling 将最后一个采样点留在该 configured end margin 之前。

`retrieval.cognitive.profile` 使用 SubjectOverrideAllowed / Live / QueryPolicy，两个 Subject 可选择不同 profile；变更只影响后续 preparation，已形成的 in-flight query 保留自己的 snapshot 和 semantic representation；DTSC/RiverMemo 共享 VCP asset，不因 readout 切换重建。`serving.retired_grace_seconds` 默认 300，范围 0..604800，Developer/SystemOnly/Live；active readers、validation tickets、current artifacts 和 research pins 保护回收边界。运行时 expiry/release 后可在后续 query 机会清理 retired artifact，metadata 保留简短 audit。

`retrieval.query.representation` 控制完整 query embedding 的 typed character/descriptor/count budgets，Developer、SubjectOverrideAllowed、Live。配置只有 `total_chars=8192` 与 `max_context_items=16`。各 section 的 hard ceilings 由实现持有；预算优先满足 Intent、显式时间/selector descriptor、Entity/Tag、current exact refs、WorkContext 和其他 descriptor，输出 section 顺序固定。一次 prepare 固定 policy；inspection 返回实际 SHA256 与截断/缺失 descriptor 诊断。Preparation token 与 validation ticket 共用 `runtime.query_lease_slots`/query lease 的现有预算机制。


## Temporal、Concept activation 与 feedback

`retrieval.query.concept_enrichment` 为 `off`（reference default）、`existing`、`model`，SubjectOverrideAllowed/Live/QueryPolicy。Existing 使用共享 Concept vectors；model 加入独立严格结构化 query role。变化只影响后续 preparation，不改写 durable Tags。

`retrieval.query.concept_activation` 下的独立叶字段由 Runtime 注册为 Advanced/SubjectOverrideAllowed/Live/QueryPolicy：`max_activated_tags` 默认 8，控制合并后保留的 inferred Tags；`minimum_similarity` 默认 0.72，范围为余弦的 -1–1；`model_catalog_limit` 默认 32，控制给 query model 的冻结 catalog。两个数量预算为正整数。默认值只由当前 reference profile 定义；Retrieval 和 model readout 读取同一 prepared operation 的配置快照，在途查询不受后续修改影响。Provider 的结构化输出合同仍独立限制一次模型响应的合法形状。

`runtime.query_feedback_retention` 默认 604800 认知秒（七天）；bounded records 包含 query/activation digest、signals 与 returned exact refs，不保存正文。过期清理不删除已接受的 UseEvents。Historical artifact cache 复用 `serving.retired_grace_seconds` 与 read leases，不引入第二套 history DB/独立缓存政策。Semantic intervals 使用 captured Subject CognitiveClock，timeout/lease/retry 使用 infrastructure time。
