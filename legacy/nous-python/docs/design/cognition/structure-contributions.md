# 正式文稿的结构贡献与来源资格

## 本轮补齐的接入

已有 VCP 模块、候选路径、支持组和解释性传输继续保留。正式 cognition 投影当前主要只从原始文稿生成 source_sequences；派生 Journal 的路径虽能作为候选曲线，却缺少对应解释性结构输入。本轮补齐这一接缝，不改变 VCP 公式。

文稿身份、局部候选、独立来源三者继续分离。当前版本及活动标注决定哪份结构可参与；旧版只通过精确旧快照或历史引用读取。反复总结与正文编辑不增加独立来源数。

## 投影规则

- 正式原始 Note/Journal 的已采用概念路径，按现行 source_sequences 规则生成有序来源输入，保留重复 occurrence 与路径边界。
- 派生 Journal 的已采用路径提供 `interpretation` 贡献。每条局部路径中相邻概念产生明确的 `narrative_next` 结构联系；这表示叙述展开，不声明现实因果或事实蕴含。Span-2 等候选读出继续由 VCP 自己处理，不在此额外创造一套关系。
- 依据为该派生内容明确引用的 source roots 联合集合时，一个组保存所有共同前提。只有明确有两个独立来源各自支持时才是两个组，不按根数均分成独立见证。
- 仅 inspiration 的反思保留可读正文及假设/想象性质。它可在明确探索视图使用，普通事实 source 图不把它当作新的原始支持。

解释性组固定 origin_ref 和 basis refs。相同概念边/关系类型/规范共同根组的重复派生取最大贡献；不同 UUID 或修订不会让同一推导重复投票。`inherited_from` 已知时保留，未知时不靠文字相似度臆断来源。

## 数值和配置

沿现行 [evidence-projection](../wave/evidence-projection.md) 的 source + interpretation 质量合并、对数压缩、枢纽校正与行预算。不要在归一化矩阵上直接加边，也不把负关系取绝对值加入传输。

`wave.associations.relation_gains.narrative_next` 缺省为 1，表示已采用有序路径的单位叙述关系质量；`interpretation_gain` 缺省继续为 0。正式投影现在能完整提供输入，开启该配置时必须实际使用，并在快照/profile 中记录。普通默认能力不能被描述成已经测得解释性增益。用同一批真实材料比较关闭/开启是本轮有限实验。

Wormhole 强见证继续只接受符合条件的独立 source 组；解释性联系可以参与明确开启的传输，不能伪装成额外原始文件见证。quality_score、ranking_score 和最终顺序维持原合同。

## 位置与封装

Annotation owner 返回 occurrences 的多个真实选区与有序路径。`cognition/retrieval/projection.py::interpretation_contributions` 将正式派生 Journal 的 paths 与真实 basis_refs 映射为一个共同前提 SupportGroup 和去重的 directed positive narrative_next；单位 strength/confidence 均为 1。原始文稿仍按已采用路径提供 SourceSequence；仅 inspiration 或没有已知支持根的文稿保留候选与原引用。该模块解释文稿用途、来源和结构贡献，形成完整 CorpusView；Wave 负责一次数值投影与去重。共享来源图展开应提升到现有 provenance/basis 能力，不能在投影内复制另一套递归读取及证据计数。

本轮不增加持久 Association 总表或“自动发现关系”模型。这里是已有正式文稿和标注到可重建资产的直接映射。后续 Social/Schema 的明确关系由各自 owner 提供同一支持组输入。
