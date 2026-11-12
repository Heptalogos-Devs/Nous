# 文稿投影的批量准备与日常重建

投影单元保留不可变快照的一致视图，并复用未变化文稿的解析、语义表示与候选布局。

## 一次准备的调用链

```text
CognitiveRetrieval.directory_in(ReadTx)
  → 轻量资格、精确修订、活动标注与来源签名
  → 匹配计算单元，仅展开变化组和必要来源节点
ProjectionBuilder
  → 文稿引用、候选布局、概念出现/路径、证据及 owner
Wave.build_product(CorpusView, ResolvedBuildProfile)
  → 按实际正文、标题、标注、来源与候选布局查找不可变投影单元
  → 只准备变化的正文/实际媒体表示
  → 在同一个 TextView 上生成选区和语义投影
  → 向量 owner 批量 ensure/read（semantic 才需要）
  → 冻结数组 → 复用完整签名相同的 Native 组件，或完整构建变化组件
```

Cognition 不先逐候选读正文，然后让 Wave 再读；Wave 不经字符串文件协议取得知识集合。ArtifactStore 提供共同范围与结构能力，application.preparation.TextPreparation 组织跨内容/媒体的批量准备，Wave 消费其结果。

## CorpusView 候选声明

显式 Candidate 与正式文稿的布局声明经同一个 _prepare_candidates 展开为 Candidate/PreparedText，再进入同一冻结构建通路。

```text
CandidateSpec = Candidate(kind="exact") | DocumentCandidates(kind="document")
DocumentCandidates:
  document
  group_key                     # 正式 exact record revision 或调用方稳定组键
  whole: bool
  local_layout: CompleteBlocks(target_bytes) | None
  annotation: 已由 owner 读取的概念出现与路径
```

`CorpusView.candidates` 使用这两个带 kind 的输入类型。`DocumentCandidates` 由 document、group_key、whole、local_layout 及共同 concepts/occurrences/paths/annotation_ref 组成；显式 `Candidate` 提供 key/content/document，group_key 缺省使用 document。修订、来源和复核状态保存在快照的组上下文中。变化的文稿通过一个 TextPreparation 会话批量准备，再展开整篇与 complete_chunks；局部片段使用 PreparedText.slice。引用定义、块间空白与尾部保留在覆盖中。语义文本保留原文的正式坐标映射，供词项片段与向量共同使用。

清单导入可以给显式 Candidate；正式认知提供 DocumentCandidates。冻结前统一展开为当前 Native 需要的文本、向量、选区、group_row、membership 和路径数组。不要让 Rust 解析 Journal Markdown 或访问认知数据库。

正式内联标注使用 `CandidateOccurrence(key, concept, selections)`，独立于 paths 保留。Wave memberships 取片段完整覆盖的 occurrence；paths 在未覆盖 occurrence 处切断，保留原始序位。没有路径的合法 occurrence 仍可贡献本地 Concept 归属。

AnnotationOperations 提供 `read_many_in(ReadTx, exact_revisions)`，包括活动标注、Concept 意义版本、每次 occurrence 的**多个真实选区**以及路径。CognitiveRetrieval 删除直接组装 concept_schema 的 SQL。一个 occurrence 跨 B1 和 B3 时不能以 min/max 把 B2 也当成概念位置。

## 准备结果复用

复用 [共同内容准备](../content/reading-and-preparation.md)及[统一选区](../content/selection-and-text.md)。每组的计算签名包含精确修订、正文、标题、标注、来源、解析与候选布局；纳入复核或探索组的选择属于目录资格，不改变共同组的计算签名。未变单元直接复用正文布局、语义文本和路径映射。文稿单元持有本篇的正文、来源 roots、候选向量引用和局部图声明；祖先各有独立节点，多组按精确文档键共享，整体概念、关系、支持和概念向量由图单元持有。变化的文稿通过 ArtifactStore 归一化范围与准备结构。

正文与布局另按实际 Artifact 选区、解析配置、布局参数和当前分段规则标识。超长普通段落形成有界、UTF-8 安全的局部选区，长代码块和列表保持完整。标题或活动标注变化时，复用单元中的正文和精确候选选区；按新标注重新计算覆盖与路径，按新标题更新词项和标题相关向量输入。正文读取量与整组复用量分别记录。

向量仍由 EmbeddingOperations.ensure 选择规范行；短 query 使用 encode_query。Concept 的实际编码文字通过同一确定性文本表示缓存复用，不每次重建都重新 ingest 一份完全相同的概念文稿身份。

构建没有候选或没有新向量时不发模型请求。lexical 不触发向量；需要坐标映射的语义文本投影由内容 owner 正式采用。

## 快照新鲜度保持原语义

先比较 Namespace generation 与实际 profile 的已发布代次，匹配时直接使用；不要在热查询也先捕获全部正文和标注。

落后时在短事务捕获 G，后续解析和计算在事务外。只有当前仍为 G 才切换相应 current 指针；构建中出现 G+1，本次查询使用已捕获 G，不追赶循环、不把旧结果冒充 G+1。旧快照和精确旧引用可继续使用。

没有做完的当次构建由查询明确报错，已保存正文不回滚。这个规则不因性能优化放宽成旧快照加新文本的双层投影。

## 可解释的耗时

构建摘要记录 content_prepare、embedding、native_build，以及文稿数、唯一正文数、候选数、读取量、解析数和复用单元数；认知捕获与提交发布分别记录耗时。

这些计数来自各负责者完成的真实工作，供日常故障排查；不新增 benchmark 平台或专用结果数据库。诊断不要为了计数重新扫描输入。
