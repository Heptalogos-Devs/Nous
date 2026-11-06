# 文稿投影的批量准备与日常重建

## 已知使用问题

用户在 c2944b1 的独立检查报告：501 篇短文稿、约 144 KB 正文，保存 0.029 秒，紧随其后的 lexical 查询 23.64 秒；未修改时热查询中位数 0.116 秒；Native 构建与搜索约 0.15 秒。分段结果指向逐候选正文和选区准备。此处作为用户报告保存，不是本包重新测量的结果。

本轮保留完整不可变快照和一致视图。优先消除重复 SQL、CAS 读取、Markdown 解析和相同投影。索引算法替换、增量几何和后台队列不是这项延迟的前置修复。

## 一次准备的调用链

```text
CognitiveRetrieval.capture(ReadTx)
  → exact revisions + 批量标注/Concept + 来源关系 + generation
ProjectionBuilder
  → 文稿引用、候选布局、概念出现/路径、证据及 owner
Wave.build_view(CorpusView)
  → Artifact 元数据批量读取
  → 每个正文/实际媒体表示准备一次
  → 在同一个 TextView 上生成选区和语义投影
  → 向量 owner 批量 ensure/read（semantic 才需要）
  → 冻结数组 → 原 Native build → 发布/打开一次
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
  metadata                      # 标题、kind、revision、来源摘要
```

`CorpusView.candidates` 使用这两个带 kind 的输入类型。`DocumentCandidates` 由 document、group_key、whole、local_layout 及共同 concepts/occurrences/paths/annotation_ref/metadata 组成；显式 `Candidate` 继续提供 key/content/document，group_key 缺省使用 document。`wave/assets.py` 的 _prepare_candidates 先调用一个 TextPreparation 会话的 prepare_many，再展开整篇与 complete_chunks：有 whole 请求时只准备一次全文，只有局部请求时合并实际重叠/相邻选区而不强制读取整件，局部片段使用 PreparedText.slice。引用定义、块间空白与尾部保留在覆盖中。没有语义空间时只用内存投影词项，不为各片段采用语义 Artifact；需要 embedding 时才调用 semantic_content 并沿 EmbeddingOperations 批量 ensure/read。

清单导入可以给显式 Candidate；正式认知提供 DocumentCandidates。冻结前统一展开为当前 Native 需要的文本、向量、选区、group_row、membership 和路径数组。不要让 Rust 解析 Journal Markdown 或访问认知数据库。

正式内联标注使用 `CandidateOccurrence(key, concept, selections)`，独立于 paths 保留。Wave memberships 取片段完整覆盖的 occurrence；paths 在未覆盖 occurrence 处切断，保留原始序位。没有路径的合法 occurrence 仍可贡献本地 Concept 归属。

AnnotationOperations 提供 `read_many_in(ReadTx, exact_revisions)`，包括活动标注、Concept 意义版本、每次 occurrence 的**多个真实选区**以及路径。CognitiveRetrieval 删除直接组装 concept_schema 的 SQL。一个 occurrence 跨 B1 和 B3 时不能以 min/max 把 B2 也当成概念位置。

## 准备结果复用

复用 [共同内容准备](../content/reading-and-preparation.md)及[统一选区](../content/selection-and-text.md)。出现项与路径范围通过 ArtifactStore.normalize_selections 转为同一字节坐标后比较，不为判定局部覆盖重新读取全文。在同一次 build 中以 exact body 和转换策略建立一个有界准备会话；一份正文的 whole/section/block 共用解析、元数据与源图。只在当前 selected view 中访问材料，缓存命中不绕过引用和 Namespace 校验。

向量仍由 EmbeddingOperations.ensure 选择规范行；短 query 使用 encode_query。Concept 的实际编码文字通过同一确定性文本表示缓存复用，不每次重建都重新 ingest 一份完全相同的概念文稿身份。

构建没有候选或没有新向量时不发虚构请求。lexical 不触发向量，也不采用仅为了向量准备的临时产物。

## 快照新鲜度保持原语义

先比较 Namespace generation 与实际 profile 的已发布代次，匹配时直接使用；不要在热查询也先捕获全部正文和标注。

落后时在短事务捕获 G，后续解析和计算在事务外。只有当前仍为 G 才切换相应 current 指针；构建中出现 G+1，本次查询使用已捕获 G，不追赶循环、不把旧结果冒充 G+1。旧快照和精确旧引用可继续使用。

没有做完的当次构建由查询明确报错，已保存正文不回滚。这个规则不因性能优化放宽成旧快照加新文本的双层投影。

## 可解释的耗时

现有 build root 增加阶段摘要：capture、content_prepare、embedding、native_build、publish；记录文稿数、唯一正文数、候选数、实际读 bytes、解析数、cache 命中、模型提交批数和各段耗时。日志给出阶段名称与处理规模，不输出每个哈希。

这些计数来自各负责者完成的真实工作，供日常故障排查；不新增 benchmark 平台或专用结果数据库。诊断不要为了计数重新扫描输入。

## 完成条件

通过正式保存→当前查询路径再次观察用户的 501 文稿情境。要求反映结构变化：一次 build 的解析/读取随唯一正文数量增长，不随 whole+各片段重复增长；unchanged 的正文结构可复用；热查询不再做全视图准备。记录实际耗时与改善，不预填机器无关的毫秒承诺。完整快照、来源和排名语义必须同时保留。
