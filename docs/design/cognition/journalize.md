# journalize：将相关碎片整理成篇

## 输入与目标

journalize 消费明确选定的 Notes、材料表示、已有 Journal 片段和工作内容，形成自然、自包含的 Journal；一次调用围绕一个明确主题形成一篇，多个主题由调用者分别选择材料调用。主题可以是经历、知识、研究、反思。正文直接表达内容和理解，保留人物、重要条件、观点与感受的归属，不写逐份介绍输入材料的任务旁白。

普通问候或无新增认识可以不成篇。Agent 主动调用与 Host 后台调用使用同一个操作，触发时机归调用者。初期不依赖定时服务或 DBOS。

## 操作

```text
journalize(source_refs, purpose, optional_topic, adoption_policy)
  -> drafts / adopted_journals + source links + completed refs
```

读取并固定来源 → 确定同一主题的输入 → 专一 Markdown 写作任务 → 保存草稿及目标基准 → 领域采用 → 可选概念标注。写作与标注分别有各自任务合同，不能让一个 LLM 同时负责主题选择、数据库状态、图运算和所有修订决定。

源材料以完整键和可读名称提供。正文使用 `[语义说明](source:S1:C1)` 普通 Markdown 链接，可见文字承载有意义的主体、内容或指代，机器键只用于回溯；程序按 [引用合同](citations.md)保存和解析精确绑定。没有局部引用时仍保留本次输入的实际生成血缘。

输入超过单请求已知限制时按主题明确分为不同文稿，或者由这个任务定义有意义的整理阶段；不能由底层自动逐文件独立处理后把结果当成综合 Journal。

## 采用

缺省可以返回草稿，自动采用由配置/本次调用显式决定。Note 直接长期保存仍是独立能力，不必经过 journalize。已有 Journal 的更新需要明确目标与 expected_revision；不同主题形成不同 Journal。

采用前通过 [Journal owner](journal.md) 核对来源、目标和时间。模型生成成功但数据库采用失败时保留可读取的 Draft Artifact，不重发模型。

## 人格与探索

写作时可选加入 Ego 的相关文档片段，作为当时视角并记录版本；不把整个 Ego 和所有配置一并塞入 Prompt。旧 Journal 保留当时表达，Ego 更新不回写旧正文。

Dream 可以用相同的文稿存储与采用能力保存 exploration 类型，允许自由猜想和创作。依据、启发材料与外部事实支持分别标明；无外部证据的创作本身可以保存，普通事实查询按用途解释。

## 本轮正式操作

复用普通 journal-draft 与 ResultStore，按 [草稿采用](draft-adoption.md)保存精确来源和目标 expected revision；正文正式采用、重试、冲突和可选标注均进入 [交付计划](../../plans/journal-delivery.md)。Note 继续保留供读取与再次整理，不维护独占消费状态。
