# Structured Material 与 External Resource

[返回文档目录](../../INDEX.md)

## Owner

Core owns model and Resource host calls; Material owns Artifact/Observation/DerivedRepresentation identity, selected text and source lineage; Memory and Runtime own their respective query and lifecycle semantics. Material 的精确文本／Serving／embedding 共同合同见 [Material Derivation](material-derivation.md#selected-text-与-serving)。

## Model output 与 owner commit

Core 在单次调用内组装类型化输入，执行所需模型或外部 Resource 调用，校验结果并直接构造 canonical owner request。Material、Memory、Runtime 或相应 owner 重新验证来源、身份、生命周期、权限和预期 revision/epoch，再提交 Authority。owner mutation receipt 与对应 Authority mutation 原子提交；相同 owner request 可以重放提交结果，不同 request 使用同一操作身份会冲突。

模型输入快照、提案、attempt telemetry 和 outcome 不作为可恢复 workflow 存储。已提交的 DerivedRepresentation、Observation、producer identity 与可重建 Serving/cache 仍由各自 owner 合同持有。CLI 的 bounded request/result cache 仅支持显式手动 retry；重试可重新生成或重新读取外部 Resource，不承诺自动崩溃续接。

## Schema 与来源

Core 的 `model/schemas/material-interpretation.ts` 统一定义 model-facing Zod schema、类型与 provider JSON Schema。AI SDK Output.object 使用该 schema 完成解析；raw media 入口直接解析。每条 route 随后执行一次领域语义校验与映射，成功结果直接交给消费者。根与嵌套对象 strict、字段 required、可缺省 scalar 为 null；资源数量/byte/time/support/modality 约束在 Material validator 校验。Schema 定义 JSON 结构，Prompt 描述忠实度与任务。

输出包含带 basis_refs 的 summary、coverage(visual/audio/embedded_text/source_text)、observations、mentions、embedded_text、source_text、speech、interpretations、uncertainties。Observation 的 basis 为 direct/inferred，certainty 为 clear/uncertain，两个轴分别表示推导依据和认识确定性；evidence_channel 为 visual/audio/source_text，独立于 observation kind。转写直接陈述的 state/action 使用 audio evidence，不声称观察到视觉事件。source_text 表示原始 text-like source；embedded_text 只表示视觉媒体中的可见文字，description/transcript 的文本传输不会制造原始 text source modality。direct 与 two-stage 使用同一 schema。direct 项必须有 support；audio-only 不产生 visual direct observation；image-only 不制造 speech；未知 support key 失败。未完成/refusal/非法输出不能 commit。

规范 provider schema 的 digest 进入 ProducerSignature 和 derivation identity。DerivedRepresentation 保存 structured payload 与 deterministic text projection；text projection 保留 uncertainty/basis，算法 identity 进入 preprocessing digest。payload_text、payload_json、payload_artifact 至少包含一项。

Text structuring envelope 的 evidence_kind 为 original_text 或 committed_representation；evidence_text 是该 invocation 实际可用正文。第二阶段仅访问已提交文本和其 segment catalog，不能由文本传输推断新的 source modality。

description_only 提交自由描述。direct_structured 给模型 invocation-local source catalog。describe_then_structure 第一阶段成功立即提交；Material 对已提交文本做 deterministic UTF-8 byte segmentation，保存 description_segment DerivedRegion(ordinal/start/end/digest)，将 D001 等 keys 提交第二模型。模型只输出 keys；本地映射成稳定引用后保存字段 basis_refs。第二阶段失败保留 description、selected 指向 description、显式 degradation。graph 回到原始 Artifact，shared roots 去重，unknown dependency 不成为独立证据。

## Resource 访问与 materialization

`$current` 是 scoped current-authority 约束，随 expression 进入冻结 leaf action；父级 required 不能被子级 none/prefer 放宽。Model rerank 需要正文本意图、可用 role 和至少两个候选。Kernel 统一筛选并限制候选数量，向 Core 发送有序引用、文本与 Resource actions；完整查询结果留在 Kernel。Core 按模型返回的候选索引映射回原引用。Resource 续接独立于可选 rerank；required rerank 未完成时暂停 Resource 调用。

Selected materialization 的内容上限为 1 MiB UTF-8；query records 的 aggregate bound 为 2 MiB。Artifact 上传上限由 `object_store.max_upload_bytes` 单独控制。

本地材料通过 Artifact/SourceRegion/DerivedRepresentation 与原生 Serving 处理；宿主在启动时显式提供 ExternalResourceAdapter。普通 Core 默认没有外部 provider。LocalDocuments 研究宿主复用相同的生产 Core、公共 Resource descriptor 和 selected materialization 路径。

Kernel Resource descriptor 保存 adapter_kind/provider_profile/provider_locator，网络与 credential 留宿主。Core adapter 负责 describe/search/materialize/checkVersion/checkAccess；宿主供给的 provider identity 不允许重复。StableExternalRef 保存 provider/profile digest/resource/entry/version(nullable)/content digest/source locator/retrieved time/access descriptor；无 provider revision 时重新读取 content digest 检查变化。

Resource actions 是 QueryRun 的宿主步骤。Kernel 在 provider call 前固定 Resource action IDs、绑定 descriptor、query intent、limits、materialize/current-authority、budgets 和 ConfigSnapshot；Core 在同一 QueryRun 的最终宿主阶段调用 adapters，再通过同一私有 Query 流提交结果。Kernel finalization 校验 Subject/action/resource/provider identity、单次提交、数量以及 current version/access。QueryRun 期间 action IDs 与 lane/query budgets 保持固定；provider records 保留 provider rank，与 cognitive raw scores 分开。

Public QueryResponse 将 resource_records 与 cognitive hits 分开；resource_actions 保存未执行项。Adapter 未配置或调用失败时返回显式 degradation，local hits 仍可返回。Core 将 cancellation 传递给网络请求，出错或取消时终止 Query 流，释放运行状态、Serving read lease 与并发名额。Official Client 可管理 Resource、执行 query、读取 records，并 materialize/admit selected record。

Kernel 将单条 record 限制为 1 MiB、每次 query 的总序列化 record 限制为 2 MiB。Authority policies 为 none/prefer/required；query results 保留各 record 的版本与访问判断。

Resource query 以 records 返回候选项。选中一项后执行有界 materialize → Artifact/SourceRegion → 独立 ObservationOccurrence；`external_object_ref` 保留稳定外部身份，每条 observation 保留自己的 occurrence identity，Memory formation 再引用该 Observation。每次接纳前核验当前 adapter/profile identity、外部版本和访问权限；手动重试可能再次读取资源。外部资源变化或权限撤销不会静默 purge 已接纳的本地 Material，其生命周期仍由 Material owner 管理。
