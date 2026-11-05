# 创建、编辑、重写与跨篇反思

## 本轮四项使用目的

| 入口 | 允许材料与输出 | 采用效果 |
|---|---|---|
| `writing.journalize` | Notes、原材料及其表示；围绕新主题成篇 | 新 Journal；没有目标 revision |
| `writing.propose_edit` | 精确基准及选区、修改要求、可选新材料 | 局部编辑提案；应用到同一 Journal/Note |
| `writing.rewrite` | 精确 Journal 基准、明确重写目标、可选新材料 | 全文重写提案；应用到同一 Journal |
| `writing.reflect` | 至少一份精确 Journal，可加材料；明确新理解主题 | 新 Journal，保留原文引用 |

journalize 不接受已知 Journal 正文作为再次压缩的主输入，也不接受 target_revision。RecordRef/RevisionRef 经共同来源解析；Artifact/ContentSelection 对应已采用 Journal 正文时同样识别其身份并返回 `cognition.writing_intent_required`，提供 rewrite/reflect 的准确操作。检查基于现存身份绑定，不用正文分类器、不遍历任意引用祖先做“来源污染”判定。Note 引用了 Journal 的事实可以按其自身用途使用；显式导入而没有认知身份的原件仍按声明处理。

删除旧 journalize target 参数、基准注入、专用 CLI 分支和文档例子，迁移实际消费者，不保留内部别名。现有 tools model run 仍是通用任务试运行，但结果不会自动取得正式文稿身份。

## 任务与模板

- `journal-draft`：保留写作、主体、来源和 no_material 规则，只用于新篇整理。
- `journal-edit`：纯文本和结构化 edits；语义限定在允许范围，使用小型 Schema。
- `journal-rewrite`：全文重新组织；是否压缩由明确要求给出，缺省保留内容完整性。实际减少内容在 diff 中可见。
- `journal-reflect`：针对新主题形成新理解，允许在本次明确目标内作语言综合；历史叙述与新的解释在表达中清楚归属，不复制源文充当第三份事实。

这些定义属于 `cognition/writing/tasks.py`，由组合根提供给共同 TaskCatalog/PromptCatalog；models executor 不识别 Journal 任务名称。

新篇整理、重写和反思都输出 Markdown，使用当前 source 链接规范。journal-draft 与 journal-reflect 无内容可成篇时沿任务的精确 no_material 解释；journal-rewrite 必须返回完整正文，完全未变化由采用时的等价检查返回 unchanged；journal-edit 无需修改时返回空 edits。不要要求正文模型同时决定长期 Concept、真值、事务或索引。

## 公共操作

```text
journalize(sources, topic?, purpose=record, author?, viewpoint?, times?,
           adoption=default, after=default) -> WritingOutcome
propose_edit(base, instruction, scopes?, blocks=(), sources=(), feedback_refs=(), feedback_mappings={}, intent=edit,
             adoption=draft, after=default) -> WritingOutcome
rewrite(base, instruction, sources=(), intent=edit, adoption=draft, after=default) -> WritingOutcome
reflect(journals, topic, sources=(), author?, viewpoint?,
        adoption=draft, after=default) -> WritingOutcome
read_draft(draft_ref, sources=false) -> DraftView
RecordOperations.adopt(draft_ref, override?: DraftOverride, after=default) -> RecordOutcome
```

所有操作固定 Namespace、配置、实际输入及语言任务。新写作可采用现有 cognition.journalize.adoption；新增编辑/重写/反思缺省返回提案，调用者可以显式 save，无普遍审批。原件的多模态准备继续由同一 Generation/Input/Media 通路完成。

## 成功提案的保存

将现有 DraftDescriptor 泛化为这四种成功写作产物所需的有限判别值，复用同一 drafts 表和读取采用路径：

```text
WritingDraft:
  ref, namespace, intent: new | edit | rewrite | reflection
  processing_result_ref, body_artifact
  base_revision?                 # edit/rewrite 必有，new/reflection 没有
  normalized_edits?              # edit 才有
  citation_context, selected_sources, author/viewpoint/purpose/times
  feedback_refs[]
  adopted_revision?
```

它只在成功计算及解释后出现，不保存 running/lease/retry 队列状态。edit 的实际模型提案保存为 proposal，由共同算法组装的完整 Markdown、规范 edits 和 diff 分别保存；parameters.program_outputs 标明程序解释产物，原始 SDK 响应仍保存为 response。完整 BASE 仅在程序解释时读取，其 revision_base 血缘用 program_input_keys 明确记录，不重复发送“全文＋目标块”。普通用户读取默认显示差异和完整正文入口，不显示庞大内部 DTO。

ProducedTask 与 DraftDescriptor 在一个短事务中采用；正式正文采用是后续独立事务。失败恢复沿现有成功计算保留机制，不为了 SQL 重试重新调用模型。DraftStore 不自行选择模型或更换基准。

## 采用和并发

RecordOperations.adopt 读取提案的冻结上下文，使用同一文稿提交核心。事务内一起写新 revision、提案已采用关联和有关反馈处理结果。重复采用返回原 adopted revision；存在更新的 head 时清楚报告 current，但不能以重试为由再应用一次。

修改提案不是拿新 head 换掉旧 base。遇到冲突保留原提案，调用者读取新基准后重新准备。复制提案为新篇必须显式执行 fork，且只适用于完整正文；局部 edits 不被隐式转换成新篇。

人工在编辑器修改生成稿时，以 `DraftOverride(markdown, source_changes=preserve)` 明确提交整篇替换。原生成稿及 ProcessingResult 保持可读，最终修订记录实际采用的是人工覆盖正文；不继续声称只应用了模型的局部 edits。无需为编辑器每次保存建立草稿历史系统。

同一提案已经采用后，普通重试返回原采用结果；重试另带不同 override 时返回 `cognition.draft_already_adopted` 及原采用引用，不能静默忽略新文本。需要继续改文就使用已采用 revision 的正式编辑操作。

## 本轮不实现

后台自动反思、自动反馈分类、自动修订所有依赖文稿、Concept 自动合并、Dream 调度不在本轮。当前四项操作足以让未来调用方明确表达意图，并保护 Journal 不被无意反复压缩。

## 采用的来源与代次

空 edits 或全文字节相同且有效绑定不变时返回 unchanged；调用者明确新增 current_basis 时，即使文字不变，仍保留实际成功路线的依据并按 bindings 变化形成修订。普通未被引用的新材料保留在 ProcessingResult 中，不自动加入编辑稿的引用目录。intent=correction 是明确采用语义，调用既有依据失效；意见的类别和措辞不决定该值。

手工编辑与 DraftOverride 的目录变更使用 [SourceChanges](exact-editing.md#来源映射继承)。adopted_as 区分 generated 与 override，原模型稿和差异保持可读。旧意见应用到新基准的显式 feedback_mappings 见 [反馈处理](feedback-processing.md#显式旧版意见映射)。

written_revisions 持久 committed_generation。重复采用、历史读取与后处理都读取该精确版本值，不以当前 Namespace 代次替换。
