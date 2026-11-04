# Wave 集合、概念与冻结资产

## 集合输入

集合清单是操作者明确提供的检索数据，不是新的认知权威。JSON 使用以下对象：

```text
CollectionManifest:
  name, space?
  documents[]: {key, input, derived_from?, evidence_group?, times?}
  concepts[]: {key, label, description, aliases?}
  source_sequences[]: {document, concepts[], ordered}
  candidates[]: {key, content, document, concepts?, paths?, metadata?}
  support_groups[]: {key, required_sources[], basis_refs[], kind, origin_ref}
  associations[]: {key, source, target, relation, directed,
                   strength, confidence, polarity, support_group}
```

input 为 file/text/artifact 之一；file 相对清单所在目录。content 指向一个 document 及精确 selector。所有 selector 直接使用内容 owner 的 SourceSelection：全文为 `{}`，行范围为 `{start_line,end_line}`，字节范围为 `{start_byte,end_byte}`，页范围为 `{pages:[...]}`，媒体时段为 `{start_ms,end_ms}`，图像选区为 frame/crop。concepts 顺序保留重复出现：例如 A→B→A；unordered 只提供共现，不能排序后假造方向。paths 保存有明确正文范围的局部概念路径，可缺省；单来源整文可继承对应 source_sequence，多个路径不拼接成一条跨主题曲线。投影合同见 [Journal 投影](journal-projection.md)。

文本可按全文、行或字节选区读取。PDF 候选通过媒体准备提取选定页的文字，候选保存实际文本 Artifact 和原 PDF 页选区，来源根仍归原 PDF。图片和音视频需先通过媒体解释或转写得到文本 Artifact，再将文本纳入集合。

documents.derived_from 是外部清单明确声明的派生关系，引用已列出的 document。它与已存 ResultStore 来源一起构成当前集合的血缘，保存声明来源，不伪装成 Nous 当年执行过的生成。循环或不存在的引用失败。

## 来源与事实单位

原件身份、源事实单位和候选分别保存。终端文稿拥有一个来源根；显式 evidence_group 可把确知属于同一事实来源的多份接纳合并为一个独立支持组。未声明时不凭 digest 合并不同观察。所有派生文本沿 derived_from / ResultStore 血缘取得根集合。

终端原始来源的 source_sequences 提供 source 事实质量。派生 Journal 可以通过带类型、原位置和 SupportGroup 的关系贡献参与结构投影；不能因为多一次派生增加独立来源。两个根共同产生的一项解释保持一个共同依据组。具体规则见 [关系与支持](evidence-projection.md)。

unordered 来源对每个不同 Concept 对形成同质量双向共现，贡献仍属于原根；它不提供方向/序列路径。原上游的 position/window 衰减仅用于 ordered=true。候选 memberships 与 ordered curve 分开：前者可支持概念匹配和直接锚，后者才支撑依赖序位的曲线奖励。没有 curve 时该分量标记不可用，不按 key 排序补造。候选绑定 annotation_ref 时，concepts 缺省为其精确范围完整覆盖的全部 occurrences 的唯一概念，独立于 paths 是否为空或包含这些出现项；显式 concepts 须包含这些归属。未绑定标注时，concepts 缺省为各 paths 中的唯一概念，整文候选可继承其 source_sequence 的成员集合。局部候选只使用实际选区完整覆盖的出现项。

source_sequences 缺省不会触发自动抽取。已有概念及向量仍支持 Native 的概念接触、融合与直接锚；没有事实边时传播与顺序奖励保持相应的空状态。缺少实际向量或概念能力时报告能力缺项。不得用候选全部两两相连或统一 dummy Tag 伪造可用 VCP。

## 三种局部身份

`curve_document_id` 标识候选曲线所属文稿，`sequence_id` 标识一份原始来源上的出现序列，`evidence_root_id` 标识独立支持组。三者分别映射到局部整数。共享一个 evidence_group 的两个来源序列不能串接成一条新时间顺序；它们的贡献在独立性计算时归同组。Thinking 去重同一文稿的规则使用 curve_document，排除自证使用全部 evidence_root，不能互换。

## 概念向量

每个 Concept 的实际编码文本固定为 label、description、aliases 的明确拼接模板，并将模板版本纳入构建配置。默认 `label + "
" + description`，非空 aliases 另起一行，重复 label 可按确定规则去重。调用 Embedding owner 的 document 用途，所有向量位于同一正式空间。

原始 label/aliases 供字面焦点识别；完整描述用于语义向量。算法不可把描述截成第一个词，名称变动不能重解释旧快照。查询临时 cue 使用相同描述编码约定，并具有 query/document 编码用途记录。

## 数值冻结输入

```text
FrozenAssets:
  identity: {snapshot_id, space_signature, build_config_signature}
  candidates: refs, documents, text, vectors[N,D]
  concepts: keys, labels, aliases, vectors[M,D]
  source_sequences: sequence_ids, root_groups, indptr, concept_rows, positions, ordered_flags
  candidate_memberships: indptr, concept_rows
  candidate_paths: candidate_indptr, path_step_indptr, concept_rows, positions, block_keys
  candidate_roots: indptr, source_rows
  association_edges: from_rows, to_rows, type_ids, direction, strength, confidence, polarity
  contribution_groups: group_ids, required_root_indptr, root_rows, kinds, origin_refs
  association_support: edge_group_rows
  source_metadata / candidate_metadata
```

数组按行位置对齐，metadata 只含小型描述；向量与大图不使用 JSON。Python 维护精确引用到行号的可重开映射，Native 使用局部整数行号。VCP 内部正 ID 从 row+1 映射，不能丢掉外部第 0 行。

vectors 为 C-contiguous float32，稀疏指针与索引为 uint64，权重/累计数值按算法使用 float64。所有维度、范围、重复索引策略和有限值在一次边界校验后形成不变量，热循环不逐次重复全资产检查。

## 公开引用和形状

`WaveSnapshotRef = instance_id + snapshot_id + address(wav:Proquint)`；`WaveCandidateRef = WaveSnapshotRef + candidate_key`。候选的内容引用仍是 ArtifactRef 与 selector，不给每个候选重建一套文件身份。

清单所有 key 在各自集合内唯一，引用必须存在；association.strength/confidence 为有限的 0..1，polarity 为 positive/negative，support_group 明确；source/interpretation 的 required_roots 非空，想象类没有证据时保持其性质。ordered occurrence 保留重复出现与实际序位；同一根的重复出现可以影响其数值质量，但始终只有一个独立支持身份。实现若将 occurrence 压成集合，必须保留无序标记并不运行依赖序位的曲线分量。

Context 的默认权重和 Native 索引配置在各负责专题中定义；此处不保存重复默认值。

## 元数据与时间

times 复用现行 TemporalValue 与时间域定义，只在调用者确实提供的轴上存在。Artifact 的实际 received/formed/recorded 来自内容 owner；清单可提供事件发生/适用等外部声明并标记来源。selector、独立 evidence group 与语义时间不混成一个字符串。

候选类型 metadata 可以是 article、note、journal、schema 等展示值，Native 不据此枚举全部未来认知类型。返回内容仍由原 owner 解析。

## 序列与接口迁移

本轮完整迁移 curve→paths 和关联支持组，清单、冻结数组、Rust、配置及示例一起更新，不保留旧字段解码。源出现序列不再超过 100 项整段消失；完整计算或明确的工作量失败见 engine。精确来源从 ResultStore 查询，可搜索文本由 MediaPreparer.text_view 提供，assets 模块不自行转换 PDF。

source 根集合与联合推导组分别保存；不要在 roots() 中用 derived_from 覆盖已存在的精确生产血缘。外部补充声明和实际 ResultStore 血缘若冲突，报告具体冲突，采用规则由清单/领域明确决定。

## 清单中的用户引用

required_sources 使用本清单的 documents key；冻结时沿真实来源解析为组内共同 required_roots，不将其展开成多个支持组。basis_refs 与 origin_ref 使用 `{document, selector}` 的精确声明，程序转换为 ContentSelection。用户无需先知道数据库 UUID 或 Native 行号。

候选手工 paths 的每项为 `{key, label?, steps:[{concept, selection}]}`，selection 属于该候选的实际正文版本。各次出现有自己的顺序位置，即使 concept 重复也不合并；paths 缺省且没有可继承源路径时只保留 memberships。使用 annotation_ref 时由正式标注结果提供上述位置，不能同时手工提供另一套 paths。

当前 Python 冻结与持久引用由 `wave/assets.py` 和 `wave/snapshots.py` 负责。候选路径通过 candidate→paths→steps 两级偏移传入；精确选区、块和短说明保留在 Python 映射中。关联的 edge→group 数组与 group→roots CSR 独立存放，同一组的共同前提只冻结一次；关联 metadata 保存 key、relation 与 group 的 key/kind。

AnnotationSet 使用 `models.annotations.read_annotation` 读取，正文和 parser profile 由共同 TextView 校验。`annotation_concepts` 可将标注 local key 显式映射到清单 Concept key；缺省按同一 key 查找。未知 key 返回错误，不按 label 合并。快照固定标注 Artifact 引用及其解析后的精确路径。

文稿范围与候选范围都使用同一 SourceSelection；候选、支持依据和原位置通过文稿范围的 `restrict(selected)` 组合，坐标继续相对于原 Artifact。继承完整来源序列使用共同 `is_whole` 判定。Wave 不定义第二套坐标类型或转换规则。
