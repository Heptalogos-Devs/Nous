# Structured Material 与 External Resource

[返回文档目录](../../INDEX.md)

## Owner

Core owns model and Resource host calls; Material owns Artifact/Observation/DerivedRepresentation identity, selected text and source lineage; Memory and Runtime own their respective query and lifecycle semantics. Material 的精确文本／Serving／embedding 共同合同见 [Material Derivation](material-derivation.md#selected-text-与-serving)。

## 持久操作

共享合同位于 [workflow envelope Proto](../../../../proto/nous/wave/kernel/v1alpha1/workflow_envelope.proto)。Core 在同一 Kernel client 下按 Subject、owner 和 operation key 串行执行完整准备、生成和提交路径；等待者随后读取已保存结果。调用者取消只取消自己的等待或执行，不取消其他调用者。

Private operation store 保存不可变 semantic digest、snapshot、proposal、outcome 和有界 execution telemetry；read/start/save 使用同一 OperationIdentity，没有 child lease、busy token 或 release RPC。相同身份不同输入返回 conflict，已完成或 purged 记录不可重写。snapshot 将领域 payload、认知形成时间与 maintenance trigger snapshot 分开。保存 proposal 后重试只继续 owner 提交，不重新调用模型。

引用依赖来自 canonical Proto 的 typed refs，opaque 用户 JSON 和普通正文不产生依赖。Core 在 canonical mutation 前登记稳定 operation ID；领域 owner 在同一 Authority 事务发布实际结果依赖。Material derivation 直接携带 operation identity，selected Resource Observation 通过注册的 request ID 发布依赖，覆盖提交已成功但 Core 尚未保存 outcome 的窗口。Purge 清空保存内容并留下 terminal marker，迟到 save 不可恢复内容。Telemetry 保留最早 64 次 attempt 和省略计数；缺失 usage 保持未知。Fresh schema 直接定义 envelope、dependencies 和 mutation operations；不提供旧记录转换。

## Schema 与来源

Core 的 `model/schemas/material-interpretation.ts` 是唯一 model-facing Zod owner，供 inferred type、local parse、AI SDK Output.object、raw strict JSON Schema 使用。根与嵌套对象 strict、字段 required、可缺省 scalar 为 null；资源数量/byte/time/support/modality 约束在 Material validator 校验。Schema 定义 JSON 结构，Prompt 描述忠实度与任务。

输出包含带 basis_refs 的 summary、coverage(visual/audio/embedded_text/source_text)、observations、mentions、embedded_text、source_text、speech、interpretations、uncertainties。Observation 的 basis 为 direct/inferred，certainty 为 clear/uncertain，两个轴分别表示推导依据和认识确定性；evidence_channel 为 visual/audio/source_text，独立于 observation kind。转写直接陈述的 state/action 使用 audio evidence，不声称观察到视觉事件。source_text 表示原始 text-like source；embedded_text 只表示视觉媒体中的可见文字，description/transcript 的文本传输不会制造原始 text source modality。direct 与 two-stage 使用同一 schema。direct 项必须有 support；audio-only 不产生 visual direct observation；image-only 不制造 speech；未知 support key 失败。未完成/refusal/非法输出不能 commit。

规范 provider schema 的 digest 进入 ProducerSignature 和 derivation workflow identity。DerivedRepresentation 保存 structured payload 与 deterministic text projection；text projection 保留 uncertainty/basis，算法 identity 进入 preprocessing digest。payload_text、payload_json、payload_artifact 至少包含一项。

Text structuring envelope 的 evidence_kind 为 original_text 或 committed_representation；evidence_text 是该 invocation 实际可用正文。第二阶段仅访问已提交文本和其 segment catalog，不能由文本传输推断新的 source modality。

description_only 提交自由描述。direct_structured 给模型 invocation-local source catalog。describe_then_structure 第一阶段成功立即提交；Material 对已提交文本做 deterministic UTF-8 byte segmentation，保存 description_segment DerivedRegion(ordinal/start/end/digest)，将 D001 等 keys 提交第二模型。模型只输出 keys；本地映射成稳定引用后保存字段 basis_refs。第二阶段失败保留 description、selected 指向 description、显式 degradation。graph 回到原始 Artifact，shared roots 去重，unknown dependency 不成为独立证据。

## Resource 与续接

`$current` 是 scoped current-authority 约束，随 expression 进入冻结 leaf action；父级 required 不能被子级 none/prefer 放宽。Model rerank 在 bound query 含正文本意图且配置了 rerank role 时运行；Resource action 独立 finalize，因此 rerank 未运行时仍可完成 Resource 续接。

Selected materialization 的内容上限为 1 MiB UTF-8；query records 的 aggregate bound 为 2 MiB。workflow JSON 上限为 8 MiB，private ModelMaterial transport 额外保留 64 KiB envelope。Artifact 上传上限由 `object_store.max_upload_bytes` 单独控制。

本地材料通过 Artifact/SourceRegion/DerivedRepresentation 与原生 Serving 处理；宿主在启动时显式提供 ExternalResourceAdapter。普通 Core 默认没有外部 provider。LocalDocuments 研究宿主复用相同的生产 Core、公共 Resource descriptor 和 selected materialization 路径。

Kernel Resource descriptor 保存 adapter_kind/provider_profile/provider_locator，网络与 credential 留宿主。Core adapter 负责 describe/search/materialize/checkVersion/checkAccess；宿主供给的 provider identity 不允许重复。StableExternalRef 保存 provider/profile digest/resource/entry/version(nullable)/content digest/source locator/retrieved time/access descriptor；无 provider revision 时重新读取 content digest 检查变化。

Resource queries use one immutable prepare → host action → finalize ticket. Kernel 在 provider call 前固定 Resource action IDs、绑定 descriptor、query intent、limits、materialize/current-authority、budgets、ConfigSnapshot。Finalize 校验 Subject/action/resource/provider identity、一次提交、数量和 current version/access。准备后 Resource action IDs 与 lane/query budgets 保持固定；provider records 保留 provider rank，与 cognitive raw scores 分开。

Public QueryResponse 将 resource_records 与 cognitive hits 分开；resource_actions 保存未执行项。Adapter 未配置或调用失败时返回显式 degradation，local hits 仍可返回。Core 在 finally 中 release ticket，并将 cancellation 传递给网络请求。Official Client 可管理 Resource、执行 query、读取 records，并 materialize/admit selected record。

Kernel 将单条 record 限制为 1 MiB、每次 query 的总序列化 record 限制为 2 MiB。Authority policies 为 none/prefer/required；query results 保留各 record 的版本与访问判断。

Resource query returns candidates as records. Selecting a record runs bounded materialize → Artifact/SourceRegion → distinct ObservationOccurrence; `external_object_ref` preserves stable external identity, and each observation retains its occurrence identity. Memory formation then uses that Observation. 接纳前核验实际 adapter/profile identity、当前版本与访问。首次 materialize 使用同次读取返回的版本和访问判断；恢复已保存但尚未接纳的 proposal 时重新检查版本和访问。已成功接纳的 operation 回放复用原快照；外部版本变化或权限撤销不清除该已接纳材料，其清除仍由 Material 管理操作决定。
