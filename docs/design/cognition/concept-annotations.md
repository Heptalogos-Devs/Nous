# 概念出现、路径与正文标注

## 设计

Concept 表达可维护的名称、说明和别名。标注表达“这份正文的这一处使用了这个概念”；Path 表达若干出现项的有序展开。标注不会改变正文，排序也不会自动改写认知。

```text
AnnotationSet:
  body_ref + parser_profile
  annotation_revision
  concept_bindings {local_key -> ConceptRevisionRef | proposed description}
  occurrences {local_key, concept_key, block/selection}
  paths {key, label, occurrence_keys[]}
  producer/result_ref
```

正式 Concept 接纳以后使用稳定意义版本；当前实验允许清单内完整概念定义。两者通过明确 binding 区分，无 alias 兼容旧身份链。

## 本轮模型任务

新增静态 TaskDefinition `concept-annotation` 与可配置任务 `annotate-concepts`。输入是带 B1/B2 局部块键的实际 Markdown 视图、可选现有概念短列表。模型只提出少量 label/description、它们对应的正文块，以及局部路径顺序。

模型不写数据库 ID、向量、图权重、独立来源数、权限、时间域、版本号。输出示例见 `docs/examples/journal-wave/annotation.json`。输入可人工标注，自动模型不是 Wave 构建前置。

程序将块键解析为精确范围，检查声明键存在、路径里的概念已声明，保留重复出现；未知块报任务合同错误。不能以更多 Prompt 限定替代不合适的任务拆分。

正文依据检查是语义任务，程序只保证引用真实存在。可由人编辑标注后采用；实验标注保存为普通 ProcessingResult，正文与标注都可以独立读取。模型提到的一个名称不自动创建长期 Concept。

## 语义原则

路径顺序可以是问题到解释、事件发展、对照或反思，未必是现实时间或因果。每一步需在正文中找到含义；没有路径就允许空集合，不按字母顺序凑链。

长 Journal 的不同章节可给不同路径。重复概念有不同 occurrence，不能先 set 去重再计算序位；memberships 可以另取唯一概念。新解释引用已有经历，其结构贡献及独立支持按 [关系投影](../wave/evidence-projection.md) 区分。

## 修订

标注版本固定正文和 parser_profile。重标注只产生新标注版本，正文 current 不必变化。正文修改后不自动移植所有位置；沿已确认不变的精确块复用可以由程序执行，但不得模糊匹配后声称位置仍精确。

## 本轮结果合同与输入呈现

模型输出只有 concepts、occurrences、paths，字段与 annotation.json 一致。occurrences.blocks 指向程序提供的块键；正式 AnnotationSet 的 body_ref、parser_profile、producer 与版本由程序补入。编译前通过该 TaskDefinition 的输入准备取得单份 TextView，每个 root block 成为 B1/B2 的来源组；不要让 CLI 或通用 Compiler猜块。

`journal-draft` 注册文本输出合同，仅返回 Markdown，不创建 Journal。两任务从共同 Compiler 可用的 question、instructions 和 append_sources 接收内容；本文不引入另一套模板变量 DSL。输出引用校验与采用均在任务 owner 中完成。
