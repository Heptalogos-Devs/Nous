# Journal 正文与多路径检索投影

## 资产来源

Journal 当前正文版本、概念标注版本及 Concept 意义版本由认知 owner 固定，Wave 读取这份不可变视图。当前尚无正式 Journal 时，正文 Artifact 与人工/模型标注的 ProcessingResult 提供相同输入。

```text
正文与版本 → TextView 块 → Concept occurrences / paths
        → 候选选区与向量 → FrozenAssets → Native
```

正文向量、概念向量、来源组和路径分别保持身份。候选可以是整文、一个章节或若干完整块；向量块不创建新的 Journal 身份。

## Concept 出现与路径

一个 occurrence 指定 concept key、精确正文块/选区及实际顺序。同一个 Concept 可以多次出现。路径是一组 occurrence 的有序列表，具有自己的正文覆盖与用途说明。不同路径之间没有默认边。

无序 memberships 只用于概念接触。使用 annotation_ref 时，memberships 从候选精确范围完整覆盖的全部 occurrences 取得，包含未参加路径的出现项；每个出现项引用的全部块都须落在该范围内。paths 为空仍保留这些归属及直接锚召回，原出现项的数组次序不生成候选路径或源图边。显式 candidate.concepts 可以补充归属，并须包含覆盖出现项和路径中的概念。给定路径与候选选区，取其实际覆盖的连续子路径；被选区排除的中间步骤不能在裁剪后伪造相邻边。需要跨一项的路径仍按原算法 Span-2 条件计算。

单个主题 Journal 可以有“经过”“解释”两条路径。对多条路径不执行扁平 concat，不对笛卡尔积建事实边。

## 多路径的数值读出

本轮采用可直接实现的、偏保守的聚合：对同一候选 c 的每条合法局部路径 p，按现有 RiverMemo 计算 `(P_p,G_p)`，选择使 `P_p+G_p` 最大的一条路径作为本次拓扑贡献：

\[
p^*=\arg\max_p(P_p(c)+G_p(c)),\quad
S_0(c)=clip(B(c)+A(c)+P_{p^*}(c)+G_{p^*}(c),0,1).
\]

不分别从两条不相关路径取最大 P 和最大 G 后拼成一条虚构路径。B 按实际候选正文计算一次；A 按同一候选的原 Direct Anchor 规则计算一次（包含上游已有去重与门控）；原 P+G 奖励上限继续执行。仅一条路径时完全退化为原定义。

返回 chosen_path、各可用路径诊断及覆盖。多个路径不产生多份独立候选投票。条件 Wormhole RRF 与 Thinking 的路线落地绑定实际合格路径，不能跨路径串接一条不存在的 continuation。

这是 Nous 的多路径扩展。它避免路径越多简单加分，但最大值仍可能受标注数量影响，实验需观察人工分支与重复分支的结果；不预先宣称最优相关度。

## Native 形状

候选 bundle 增加 candidate→paths、path→occurrences 的两级稀疏偏移，occurrence 保留 concept row、position、对应正文局部键。大数组批量传递，短说明留 metadata。原 `curve` 内部字段与清单示例一起迁为 paths，不保留自动拼接兼容。

图来源路径和候选读出路径仍分开。解释性 Journal 路径可通过明确的 RelationContribution 参与 [关系投影](evidence-projection.md)；没有支持声明的标注只供候选读出，不自动成为源图事实。

## 返回正文与 Steward

Native 返回 candidate row、path row、分数与有关关系；Python 将其映射到原 JournalRevision/Artifact 的选区。Steward 后续只选择这些版本上的块，代码回取原文。默认检索交付可直接返回摘要性片段或全文引用，完整出处通过 read 展开。read 同时返回快照冻结的 concepts 与 paths，保持原文、无序归属和显式顺序的区分。

## 本轮清单入口

Candidate 可以显式选择手工 paths，或绑定其实际正文的 annotation_ref；二者互斥。Wave assets 通过公开结果/AnnotationSet 读取操作解析，不直接读取模型私表。标注绑定另一份正文时返回具体 mismatch。完整 Journal owner 以后产生同样的注解输入，当前不虚构 Journal 身份。

候选读出先按原规则计算一次正文基项和直接锚；每条路径分别计算成对 P/G，并按两者之和选一条。返回包含所有路径的可用贡献与最终选择。相同路径复制多次保留同样数值。Thinking 的每个候选模块绑定一条实际路径，重复 Concept occurrence 保留独立位置，不跨两条路径拼接前驱。

Wave 的显式行/字节 selector 保持调用者指定的原文范围，不自动扩成块，也不向既有 Embedding 编码拼接标题。需要完整块候选的调用方使用共同 BlockRef.content/SourceSelection，并可将该块 heading_path 保留到候选 metadata 供呈现。annotation_ref 是当前 Wave 的结构消费者：通过同一 TextView 的实际 block.selection 取得出现位置；Wave 不再解析或切分 Markdown。
