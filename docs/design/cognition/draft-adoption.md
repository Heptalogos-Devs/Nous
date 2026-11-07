# 成功写作草稿与正式采用

本轮成功草稿统一承载 new/edit/rewrite/reflection 意图，具体值和操作见 [写作意图](writing-intents.md)。journalize 只负责新篇；编辑和重写才具有 base_revision。

## 一项专一的写作操作

journalize 将调用者选择的材料围绕一个主题整理为一篇 Markdown。第一版一调用一篇；多主题由调用者明确分成多次，输入超出共同 joint 能力时返回范围建议，不在底层逐文件回答再当成综合文章。

`journal-draft` 只负责正文；`annotate-concepts` 只负责正文标注。正文形成不等待 Concept 解析。无新增内容时，模型仅输出 `!NO_MATERIAL!`；静态写作任务只在 trim 后精确匹配该标记时解释为 `no_material`。普通空响应或纯空白是输出错误。

无材料结果仍保存真实 ProcessingResult（包括原始标记输出及请求），返回其 stage_results，不创建 DraftDescriptor 或 Journal。标记混在正常正文中或写成其他形式时不作无材料解释。输入列表本身为空时可直接返回 no_material，不发生模型请求。

```text
writing.journalize(sources, topic?, purpose, viewpoint?,
           adoption=draft|save) -> WritingOutcome
RecordOperations.adopt(draft_ref) -> RecordOutcome
read_draft(draft_ref) -> DraftView
```

缺省 adoption=draft；配置或一次调用可以设 save。人工 create 直接保存，不走模型和审批。草稿读出可直接编辑正文后采用，保留原模型稿与实际编辑依据。

## 冻结范围

来源在开始时固定精确 revision/selection。new/reflection 没有修改目标；edit/rewrite 固定模型实际看到的 base_revision、原来源映射与允许范围。来源顺序、key、usage、作者、时间、写作参数、实际配置与 Prompt 资源一起固定，不在采用时用 latest 替换基准。

程序负责关系、版本、事务、索引状态。Prompt 只包含任务目标、主题、必要视角、真实材料与可读引用键。

## 共同 produce 与草稿提交

使用 GenerationOperations.produce 执行共同模型链。save_produced 统一解释写作输出并返回 WritingOutcome：显式无材料标记只采用 ProcessingResult；正常正文由 ResultStore.prepare_outputs 发布字节，一个 WriteTx 调用 ResultStore.adopt_in 并写 DraftDescriptor。DraftDescriptor 保存 namespace、ProcessingResult、body_artifact、source bindings、intent、可选 base_revision/normalized_edits、purpose、viewpoint、times 和可选 adopted_revision。

DraftDescriptor 是用户可反复查看和采用的草稿，不是任务 running/lease/receipt 系统。它只在成功处理之后存在。普通 tools model run 也使用同一 produce + ResultStore 采用；不保留两套语言执行。

写作请求先由 DraftStore 保存 WritingRequest，固定原始选择、精确 base、来源、反馈映射、采用选择和 AfterWrite。语言响应把该请求作为执行依赖。解释、链接规范化、草稿保存或正式采用失败时返回 writing_request_ref；重开实例后可通过 writing.recover(ref) 或 journal draft recover <ref> 继续。草稿提交后独立采用正式正文，重复恢复返回同一修订。

来源键错误的成功正文同样保存为可读草稿，正式采用再执行来源校验。已知裸键的确定性整理遵循[正文引用](citations.md)；未修正的来源错误会带草稿引用返回，使用者可用完整 Markdown 覆盖修订后采用。

## 正式采用

RecordOperations 的采用事务读取 DraftDescriptor：new/reflection 调同一 create_in；edit/rewrite 核对冻结 base，应用已经解释的正文与来源，调同一 append_in。草稿采用关联、有关反馈处理与 namespace generation 同事务提交。局部编辑的范围外正文由程序保留。

同一草稿重复采用，返回已采用 revision 和当前状态，不再创建第二篇。用户确实想复制为另一篇时必须显式 `fork`；普通重试不具有复制语义。这是草稿业务键，不扩张为所有操作的幂等请求平台。

并发基准冲突时返回 current_revision、draft_ref、base_revision。用户读取差异后基于新的精确正文重新准备修改；旧提案不直接换一个基准再采用，不自动 merge 或重跑 LLM。

## 采用和检索结果

`adoption=save` 先保存 DraftDescriptor，再保存 Journal；返回两者引用。可显式要求标注或刷新，结果分别表达正文提交、标注和检索。RecordOperations 组织 AfterWrite；后两者失败保留正文并通过共同操作报告 partial，日志与响应保持一致。

模型草稿、正式作者观点与后续提案分开保留；生成者信息从 ProcessingResult 读取。多媒体先经显式媒体操作得到可读表示；journalize 可消费这些表示，仍记录原件与区域。

采用人工全篇输入使用 `DraftOverride(markdown, source_changes?)`，source_changes 为 SourceChanges；保存的 adopted_as 为 override，读取草稿继续呈现原生成稿，正式采用正文通过 adopted_revision 读取。已经采用的提案再给不同覆盖内容返回 cognition.draft_already_adopted。
