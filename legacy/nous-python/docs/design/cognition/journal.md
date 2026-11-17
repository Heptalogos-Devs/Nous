# Journal：主题明确的长期叙述记忆

## 产品意义

Journal 是主体或使用者能够阅读、写作、整理、修订和召回的 Markdown 文稿。它记录一段经历、一项理解、一段研究或一次反思。它是长期叙述记忆的主要载体，不要求按日历一天一篇，也不把所有材料压成短事实句。

Note 保存主动书写和系统明确摘录的碎片，可直接使用或按配置长期保留。journalize 将相关碎片整理成篇。Episode 组织实际事件经过与时间，Journal 表达有视角的理解；一篇 Journal 可引用多个 Episode，同一经历也可有多篇不同视角的回顾。

Memory 是能力统称，不再复制一份 Journal 正文到另一种 Memory 容器。MemorySemantics 不作为独立正文层。修订由 JournalRevision 处理，同时存在的不同解释各自成文并引用来源。

## 对象

```text
Journal:
  id, identifier, namespace, author/subject?
  current_revision

JournalRevision:
  journal_id, revision_id, parent_revision?
  title, body_artifact
  purpose: record | reflection | knowledge | exploration
  viewpoint/author
  times, source_refs, related_journals/episodes
  change_reason?
```

purpose 表达写作用途，不是固定文体模板；各类正文都可以有事实、观点和推测。关键认识需要更精细语义时，把关系或主张绑定到相应正文块，不给整篇文本强贴一个真假枚举。

body_artifact 是不可变 Markdown；Journal 保存认知身份及修订关系，不复制另一套字节存储。时间使用共同类型。记录一项主观感受本身可以是新经历；它引用的外部事件仍保留自己的来源。

## 身份与时间

公开可读标识符指向 Journal；精确引用固定 JournalRevision 与正文选区。读取“当前”得到 current_revision；已经交付给模型的引用固定当时版本。

recorded 由实际采用事务记录。formed 表达这次认识形成的坐标；subject 运行于虚拟时间时使用 Host 提供的相应域，未提供时保持明示的系统形成时间/未知，不猜世界时间。occurred、observed、valid 来自具体来源和主张，不用写作时间回填。

同一文稿可包含不同时间的事情，默认从引用的来源/局部声明解释；文章级时间只表达确实覆盖全篇的范围。读者要能区分“那时发生”“那时知道”和“后来回顾”。

## 正文结构与标注

复用 [TextView](../foundation/text-structure.md)。概念标注是绑定 body_revision 的派生结构，标注版本独立于正文版本；重新标注不会伪造一次新的主观记忆修订。

每条 Concept occurrence 指向真实正文位置；有序路径表达叙述/解释展开。局部路径互不自动串接，概念重复出现保留位置。文章不必先完成标注才能读、存、普通检索。

Concept 的完整意义版本固定在标注中。概念重命名/合并或意义修订由 Concept owner 完成，之后重做受影响投影，不回写旧 Journal 的历史解释。

## 正式操作目标

文稿 owner 完成 create/read/revise；journalize、feedback 和当前检索各自拥有完整操作，并复用该文稿采用能力。多入口只是适配。正文编辑和模型生成都进入同一个正式 revision 提交方法，职责见 [认知接入](integration.md)。

feedback 可以表达有用/无用、赞同/不同意、具体说明及可选修改。评价检索无用不自动重写正文；明确文本更正进入 revise；新观点需要新篇时建立 related relation。

## 正式接入

[文稿操作](written-records.md)、[引用](citations.md)、[草稿采用](draft-adoption.md)、[标注采用](annotation-adoption.md)与[当前检索](current-retrieval.md)定义本轮完整行为；[交付计划](../../plans/journal-delivery.md)安排实际开发。正文与标注分别版本化，正式认知身份由领域采用，普通 ProcessingResult 继续保留其处理产物身份。
