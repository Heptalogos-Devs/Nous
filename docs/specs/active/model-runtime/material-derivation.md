# Material Derivation

[返回当前产品合同](../../INDEX.md)

## Owners

TypeScript Core orchestrates external model calls and operation snapshots. The Material owner validates and persists exact input graphs, representations, regions and provenance.

## Identity 与 provenance

Artifact、ObservationOccurrence、SourceRegion、DerivedRepresentation 与 DerivedRegion 各有独立 identity。相同 Artifact 的重复观察保留独立 occurrence identity。

DerivedRepresentation 保存 ordered exact inputs、strategy、representation kind、ProducerSignature、revision、payload 与 quality。Inputs 可引用 SourceRegion、DerivedRepresentation 或 DerivedRegion，必须属于同一 Subject，不得重复、自引用或形成 cycle。已提交 input set 不可修改。Supersedes 表示显式 refinement lineage，不属于 derivation inputs。

成功 derivation identity 由 Subject、ordered input digest、representation kind、producer signature、strategy 和 supersedes 组成。同一请求重试复用成功表示；改变 lineage 或 inputs 会得到新的 identity。

## Selected text 与 Serving

Material 对 Occurrence、Artifact、SourceRegion、DerivedRepresentation 和 DerivedRegion 提供有界文本视图。视图包含 exact reference、实际读取的 UTF-8 byte selection、总 byte 数、所选文本的 BLAKE3 content identity，以及历史 Authority snapshot digest。显式坐标必须位于合法 UTF-8 边界；读取预算截断只保留完整字符。精确读取、Serving 文档与 embedding preparation 共用这一读取实现，Region 的索引正文只包含所选片段。

普通 Serving 使用有效 representation；superseded representation 仍可精确回读，并按 captured historical view 决定过去的有效文档。Material 拥有 Artifact／外部来源 lineage 与多来源派生根，Memory 组合 cognition basis；Serving 消费相同的 current/as-of provenance contribution。Meaningful Use 可以支持关联，但不会凭空成为独立事实来源。

Embedding discovery 先选择至多 256 个 candidate refs，再通过共同文档合同读取正文，并批量判定兼容缓存缺口。返回的 continuation 绑定 Subject、当前内容 watermark 或历史 view digest；空 needs page 仍可能有 continuation。提交仅回读指定 exact reference，核对文本 content identity 和 view，并在 Authority fence 内发布缓存。Serving build、discovery 和 commit 使用相同正文格式。模型 batch admission 与文档候选 page 是不同预算；宿主遵守调用者的总提交上限。

材料与 Resource 的其他读取边界见 [Cognitive IO](cognitive-io-and-resources.md)。

## Supported derivation

- Text-like source 经严格 UTF-8 解码形成 ExtractedText，不调用模型重写原文。
- Image 可形成 ImageDescription、StructuredInterpretation，或按 describe_then_structure 先提交描述再结构化。
- Audio 默认使用 direct 多模态理解形成 AudioDescription。audio.input_mode 为 transcription 时使用独立 speech_transcription role 形成 Transcript；该模式不支持 direct_structured，describe_then_structure 可继续结构化 Transcript。两种 mode 之间不静默切换。
- Video 默认以原始 Artifact bytes 通过 gateway chat 的 video_url input 形成 SceneDescription。显式 frames mode 使用 FFmpeg 作有界抽帧，可选提取音轨并调用 speech_transcription。每张帧图片紧邻其 index/timestamp label；处理记录包含 FFmpeg version、采样策略、时间戳、source duration 与 truncation，frame envelope identity 进入 preprocessing digest。Frames 的 audio 可用性取决于实际提供的 Transcript；结构化音频支持可引用该 exact representation，第二阶段沿用相同可用性。
- direct_structured 使用 material_direct_structuring role；describe_then_structure 先持久化 description，再以 exact DerivedRegion segments 作为第二阶段输入。

素材策略为 description_only、direct_structured 与 describe_then_structure。结构化结果使用唯一的 Zod schema owner；schema digest 进入 ProducerSignature 与 derivation identity。DerivedRepresentation 可保存 structured JSON payload 和 deterministic text projection。每个结构化 support 都映射到输入 DAG 中可读回的 exact reference。

description segmentation 以 UTF-8 byte coordinates 建立稳定 DerivedRegion。structured model 只返回 invocation-local basis keys；Core 映射成 exact DerivedRegion，Material/Kernel 验证归属和输入图。第二阶段失败时保留已提交的 description，并返回该表示及显式 degradation。

结构化解释由 Core 按 visual/audio/source_text 分别固定 evidence_access，值为 original、representation 或 unavailable。原始帧可以具有 visual original，而同次提供的 Transcript 只有 audio representation；该音频通道的 coverage 不得为 observed、observation basis 不得为 direct。仅提供描述时，其可用通道均降为 representation；严格解码的原始文本仍具有 source_text original。未提供或未分析某通道不是该通道的观察，也不证明静音。Owner 拒绝违反这一边界的输出，不将它静默改成成功。文本投影、typed payload 的 evidence_access 与 representation quality.input_access 保存实际成功调用的访问方式；已有不可变表示保持其原始生产条件。

Video MIME 本身不授予音频证据能力；direct mode 每次 attempt 根据该冻结 route 的 audio_input 能力与实际发送的媒体建立访问方式，不能借用首选 profile 的能力评价 fallback。不可执行的 route 在预算 admission 和 HTTP 前过滤；模型取得同一实际访问声明。frames mode 的音频通道由实际提供的 Transcript 决定。第二阶段从已提交 description 的 quality 取得通道，再转为 representation，不重新读取首选模型能力。

语义校验与文本投影共用 Material owning implementation identity；其内容摘要及实际依赖版本进入 workflow reuse 与 ProducerSignature，修改实现不依赖手工版本常量。源码启动和 Portable build 使用同一 owning 文件摘要，Portable 将值嵌入 bundle。结构化生成合同校验在固定 routes 内完成，无效输出可按明确 fallback 继续；领域提交拒绝不发起另一轮生成。所有 route 失败仍保留成功前序 description 和 degradation。

description segments 使用 `material.description_segment_bytes` 的稳定 UTF-8 范围，默认 2,048 bytes，并优先在换行处分段。region coordinates 保存 segmentation policy digest。Text segmentation 输入上限为 1 MiB，最多 512 个 DerivedRegion。Video frames mode 使用 FFmpeg 作有界抽帧；实验观测归 [Research](../../../research/README.md)。

## Memory formation

`formFromObservation` 使用调用方提供的稳定 operation_id。初次执行固定 model/profile/Prompt/config snapshot，并将语义输入绑定到 operation identity；相同 operation 与相同输入重放原 outcome，相同 operation 携带不同输入返回 conflict。

调用方可指定 exact representation_id。未指定时，Core 按 Artifact modality 和配置选择可用 representation：text 用 ExtractedText，image 用 ImageDescription，audio 用 AudioDescription 或 Transcript，video 用 SceneDescription。没有可用 textual representation 的 binary input 返回 `material_representation_required`。

Aboutness 支持 explicit、select_from_resolved_mentions 与 none。select 模式使用 Kernel 已解析的 mention candidate keys，由 Model 选择子集；显式引用由 owner 验证。Actor identity 单独保存在 Observation。

形成结果引用实际使用的 occurrence 与 DerivedRepresentation。Material read/materialize 可按 SourceRegion、DerivedRegion 或 representation 精确回读来源链。普通 recall 不重新解释媒体。
