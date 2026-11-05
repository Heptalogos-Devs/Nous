# 关系结构、共同依据与独立支持

## 设计决定

派生 Journal 可以贡献新的概念组织与解释性关联。反复摘要、重排文字和复制关系不会产生新的独立来源。内容血缘、具体主张的依据和检索传输权重分别表达。

SupportGroup、精确排除和解释性关系继续沿此合同；本轮由正式 Journal 的当前标注补齐同一输入，见 [结构贡献](../cognition/structure-contributions.md)。清单实验也使用同一投影，不建设通用概率推理系统。

## 支持组

```text
SupportGroup:
  id                          # 明确来源/同一关系推导的稳定组键
  required_roots[]            # 这一组共同需要的原始来源
  basis_refs[]                # 可读取的精确证据位置
  kind: source | interpretation | hypothesis | imagination
  origin_ref                  # 这条解释首次出现的位置

RelationContribution:
  source, target, relation, direction
  support_group
  strength, confidence
  inherited_from?             # 复制/总结已有关系时指向原贡献
```

一个组内部的 roots 是共同前提；多个组才表达不同支持途径。A 与 B 共同启发“取消类似提交”，写成一个 {A,B} 组，不能均分成两个独立见证。明确来自两次独立经历的相同认识，可以有 {A}、{B} 两个组。

组的独立性来自来源声明与真实行为，不凭内容相似、作者相同或 SHA 自动判定。未知共同来源保持 unknown；强见证资格要求已知的、符合查询范围的依据。

## 重复与数值质量

同一贡献经过再总结保留原 support_group/origin_ref。对同一边、关系和规范化支持组的重复声明取最大已声明质量；真实原始 source sequence 内的多个 occurrence 继续按原 VCP 的顺序质量累计，但只保留一个来源组身份。

解释性贡献的初始质量为：

\[
a_{e,g}=gain_{relation}\,strength\,confidence.
\]

同一 required_roots 集合上、同一种解释关系的重复生成先合并（不靠每次新 UUID 区分独立性）。按边保留 `max a`，不因多生成十篇摘要而累加十次。不同组的可用质量可以相加，但独立见证计数另算。

原始质量与解释性质量在传输校准之前合并：

\[
W_e = W_e^{source}+\lambda\sum_g\max a_{e,g},\qquad \lambda\ge0.
\]

然后继续原 VCP 对数压缩、枢纽校正与行出流预算。不要向归一化 CSR 直接加权。`interpretation_gain` 缺省 0，本轮实验可明确开启；无解释输入时保持原始路径。lambda 是检索配置，不是事实真实性概率。

hypothesis 与 imagination 保留内容及关系性质，可按明确探索视图参与；普通事实查询缺省不把它们加入 source 事实质量。Dream 原文仍可被检索，无需证明后才允许保存。

## 排除候选自证

设候选根集合为 C，边贡献组 g 的共同根为 R_g、质量为 m_g，原始独立支持比例定义为：

\[
I(e,C)=\frac{\sum_{g:R_g\cap C=\varnothing}m_g}{\sum_g m_g}.
\]

没有已知质量时独立支持比例返回 unavailable；算法需要中性数值时单独标记其数值政策，不展示为已知支持。只要共同前提与候选自身来源重叠，该组不作为完全独立支持；这是本轮选定的保守定义，避免把联合推导拆开后留下伪独立的半份质量。

当前 VCP `independent_fraction_excluding` 带 0.15 数值底值。保留原公式的数值权重时，明确分成 `independent_support_fraction=I` 与 `support_weight=max(0.15,I)`；底值只作算法衰减，不展示成“至少有 15% 独立证据”，也不得用于 Wormhole 见证准入。未知来源不能默认为支持比例 1。

## Wormhole 资格

强路线修正继续要求上游对应的端点、额外共享概念、查询焦点和真实路线条件；新增组结构只修正来源资格。两份见证必须来自满足条件且根集合互不重叠的实际 source 组，并按候选排除其全部根。

interpretation 贡献可以建立检索通路，但本身不满足“另两份独立原始材料”的强见证要求。一次查询传播和模型生成的重复结果不新增 witness。单原件/单路径正常输入继续沿上游公式运行。

## 负关系

contradicts/refutes 不取绝对值塞进非负传输图。保留关系类型、方向、精确来源，并通过预建负关联邻接与 Concept→候选倒排读取有关反证。来源与关系由 Python 物化，Rust 只返回有关边/行。

## 本轮落点

把 Associations 中“evidence roots 均分质量”迁为显式 support groups，清单、Python 冻结输入、Rust provenance 与示例一起迁移，不保留旧形状兼容。原 source sequence 自动形成 singleton source 组；派生序列只有明确支持与类型后才贡献结构，不能仅凭 derived_from 展开所有根并宣称各根支持全部关系。

理论参考：W3C PROV 区分生成/使用/派生；provenance semiring 研究区分联合依赖和替代推导。本设计采用这种区分，不引入符号代数执行器，也不声称该质量公式是上游 VCP 原公式。

## 清单和内部值的对应

外部 required_sources 指向 documents key，内部 required_roots 才是冻结后的支持根组。basis_refs、origin_ref 可由清单中的 document/selector 提供；正式领域以后直接提供精确内容引用。这个转换发生在 Wave assets owner，一次完成，Native 只接收组号、根集合及有类型贡献。

显式 source 组与原始 source sequence 共享强见证规则：每份见证保留其完整 required_roots；两份见证的根集合必须互不重叠，且均不与候选根相交。相同共同根的重复 source 组会合并，interpretation 组只参与明确开启的传输质量。概念邻域的 source_witness_groups 返回这些实际共同前提组，便于检查派生材料没有增加独立见证。
