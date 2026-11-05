# 精确编辑、完整修订与引用继承

## 三类正式修改

1. 调用者已给定文本：程序直接应用编辑或全文替换，不调用模型。
2. 自然语言要求：专一任务提出局部 edits，程序组装新正文，形成可读提案；显式 apply 或采用策略才提交。
3. 明确全文重写：独立写作任务输出完整正文，不经过 journalize，不缺省要求压缩。

最终都保存完整不可变 revision。旧版和旧选区保持可读；`expected_revision` 是操作开始看到的版本。历史业务版本不是内部 API 兼容承诺。

## 共同文本编辑能力

在 `content/edits.py` 提供纯编辑算法，通过现有 ArtifactStore/TextView 取得精确原文。它不调用模型、不写认知表。

```text
EditScope = BlockRef | ContentSelection
ReplaceQuote(scope, before: str, after: str)
InsertBefore(scope, anchor: str, text: str)
InsertAfter(scope, anchor: str, text: str)
ReplaceSelection(selection: ContentSelection, expected_text: str, replacement: str)
TextChange = 上述判别联合
PreparedTextEdit(body_ref, base_digest, changes[ResolvedEdit], utf8_bytes, diff)
ResolvedEdit(start_byte, end_byte, expected_bytes, replacement_bytes)
```

TextChange 的 JSON 使用 `action` 判别：`replace_quote`、`insert_before`、`insert_after` 或 `replace_selection`。程序内直接构造上述类型时，action 由类型提供。

quote/anchor 必须在所给精确 scope 内唯一匹配。使用字面匹配，不用正则、语义模糊匹配、语言词表或自动最近位置。可以先缩小到块，再选块内原句；重复匹配返回匹配范围让调用方选择。调用者已经有精确选区时可用 ReplaceSelection。

所有变更都以同一 base 正文的 UTF-8 半开区间为坐标。校验完整码点边界、同一 body/profile、原文相等和区间不重叠，然后按逆序应用。多个插入落在同一点，或插入点落在另一个替换区间及其边界时返回明确冲突，让调用者合为一次编辑；不靠排序猜意图。删除用空 replacement。没有变更返回 unchanged。

范围之外字节保持原样，包括换行、空白、代码和引用。读取时不默认转换 CRLF 或 Unicode 规范形式。`difflib` 只生成展示差异，不作为模糊应用 patch 的引擎。整篇替换是独立的明确输入，不被拼装成若干猜测编辑。

## 认知写入合同

```text
RecordOperations.apply(
  base: RecordRevisionRef,
  changes: sequence[TextChange],
  reason?: str,
  intent: edit | correction = edit,
  sources: SourceChanges = preserve,
  feedback_refs: sequence[FeedbackRef] = (),
  after: AfterWrite = default
) -> RecordOutcome

RecordOperations.replace(
  base, markdown, reason?, intent=edit, sources=preserve,
  feedback_refs=(), after=default
) -> RecordOutcome

RecordOperations.diff(left_revision, right_revision) -> RevisionDiff
```

create/replace/apply 的语义验证进入同一 WrittenRecords；patch 不另造正文表、来源表、失效路径和版本检查。缺省所有旧绑定保留；新增引用必须绑定到本次固定的实际来源；显式移除引用键时，正文不能还有该键。`intent=correction` 使用现行依据失效 owner，普通编辑保持原语义。程序不根据 reason 内容判断 intent。

提交前取得 base，发布新字节；短事务再次核对 current 等于 base，登记 Artifact、revision、source bindings、生成依据及头指针。事务内关联已处理反馈与已采用提案，按真实影响推进 generation。并发冲突返回原 base、实际 current、编辑内容/提案引用；不自动 rebase，不用新 head 冒充模型基准。

完全相同的正文、元数据和绑定返回 unchanged。未发生正文变化但调用者显式解决反馈时，可以单独保存处理结论，不强造新版本。

## 来源映射继承

`SourceChanges(preserve=True, add=(), remove=())` 负责目录合并：省略时沿用全部旧绑定；add 仅列新增 SourceBinding；remove 列确切旧键；preserve=False 明确清空旧目录再加入 add。新增重复键、未知移除键和移除后正文仍残留的真实 source 链接都会失败。调用者不需要为增加或移除一项来源重新提供整个旧目录。手工 apply/replace 和 DraftOverride.source_changes 使用同一值与合并能力。

`CitationContext` 在准备修改时固定：base_revision、原 source-key→精确来源映射、本次新来源、允许编辑范围。旧正文中的 key 原样可解释，不让模型重新为未改动内容编号。

给新材料分配与旧键不冲突的局部根键（N1、N2…）；共同 InputPlan 保留显式根键，媒体分片可以追加已有 C/P 等子键。使用实际成功路线最终提供的键与表示，不能使用未提交的备用路线映射。输入键能力是共同 InputSpec 的正式字段，所有消费者使用同一个解析位置。

基准正文以 revision_base 输入，保存编辑血缘；它不增加独立事实见证。未改动部分沿用原 bindings。新材料加入引用目录，实际引用与明确 current_basis 各自保存；读过的材料始终保留在 ProcessingResult 中，不用引用数量推断独立支持。

共同 CommonMark 解析器解析 source 链接，代码块中的示例文本不当成正文引用。引用解析、编辑校验、检索去标记共用 `content/citations.py` 的正式结果，删除 records.py 中另起 MarkdownIt 的平行校验。

## 自然语言提案的输出

模型只读取允许修改的 T1/T2 范围、必要的只读上下文、意见及来源表。输出：

```json
{"edits":[{"target":"T1","action":"replace","before":"会议在周二举行。","after":"会议在周三举行。"}]}
```

action 为 replace / insert_before / insert_after；before 是该范围内的精确原句或锚，replace 的 after 可以为空。模型不写数据库 ID、字节偏移、事务状态、图权重。没有应改内容时返回空 edits。正文外解释可在提案说明字段简述，不进入保存正文。

任务解释器使用共同编辑算法校验并形成全文和 diff。没有要求全文改写时，模型只输出局部内容；未涉及正文的保留由程序实现。错误提案保留可读实际响应和具体合同错误，修复次数沿已有策略，不新增自动审核模型。

## 标注与时间

修改后旧标注继续绑定旧正文。新正文可以读取和普通检索；可选标注产生独立新标注。暂不为每次编辑自动模糊迁移所有位置。

recorded 由提交写入。排版和文字编辑默认保留原 formed；调用者声明新认识形成时显式给 formed。occurred/observed/valid 沿原字段继承或显式修改，不用系统时刻补造。编辑同一片段不改写过去的来源文稿。

## 来源槽与不连续来源的完整迁移

共同输入增加 `InputSpec.slot_key: str | None`，缺省由 planner 分配 S1/S2；`InputGroup.slot_key` 持有它，`InputGroup.key` 是本次实际显示键。同一逻辑来源的多个离散 InputSpec 可以共享 slot_key，planner 分配 F1/F2 子键并拒绝同槽不同角色；媒体分片、转写、任务特有分块和备用路线完整保留 slot_key；例如 key=N1:P3 仍明确属于 slot_key=N1。表示如何命名不决定来源归属，消费者不得 split 冒号来恢复关系。根键与所有实际组键的唯一性由共同 planner 检查。

SourceBinding 的正式目标收敛为 `selections: tuple[ContentSelection, ...]`，普通单来源也是长度一的同一形状。保留 key、owner_revision?、usage、name。多个不连续实际源片段分别保存，不能以最小起点和最大终点扩大。所有 targets 需要属于声明的精确认知正文时逐个核对。`CitationContext` 另保留调用者选定的范围；实际阅读覆盖以 ProducedTask.actual_inputs 及其 origins 为准，不把 requested 范围冒充 observed 覆盖。

持久来源使用 `revision_sources` 保存有序 source group，`revision_source_targets` 按组和 ordinal 保存每项 ContentSelection。迁移 `contracts/schema/reading/records/drafts/feedback/basis/capture/projection` 和 JSON 示例、CLI 反序列化；删除原单 ContentSelection 字段的读写分支。Content InputGroup 表同步持久 slot_key，ResultStore 读写使用新正式定义。已授权开发实例按当前表结构重建；无内部旧格式兼容。

写作提案冻结旧 SourceBinding 与新 slot 目录。完成后用实际组的 slot_key 取得源身份，以 origin 精确坐标构造新的 binding targets。一个 key 对应多 part 时读取多项 ContentReadView；向量和 source root 的独立性不因 part 数量增加。

输入资料包含既有 `source:S1` 的正文时，不对正文做字符串全局替换。旧稿自己的引用目录作为独立命名域交付；任务只可生成程序提供的允许键。未修改旧正文继续使用旧目录；必要的新增键由程序预分配为不冲突的 N 键。共同解析器处理链接 token，代码示例和原始引文不重写。

## 小型任务输入的具体准备

`propose_edit(scopes=None)` 使用基准 TextView 的完整根块作为 T1/T2 目标；给了 scopes 就只开放这些选区。一个目标的实际内容通过 InputSpec(role="edit_target", slot_key="T1", artifact=..., selection=...) 进入共同 InputPlan；必要的其它块使用只读 context 角色。重复正文不再以“全文＋全部块”发送两遍。

TaskDefinition 从 ReadingInput 中的 edit_target 分组读取程序保存的精确 ContentSelection。模型输出 target 只匹配这些键；outputs 阶段调用 `resolve_edits(body, target_map, proposal)` 和 `apply_resolved_edits`，生成 output（组装 Markdown）、edits（规范变更）与 diff（展示）三个有名产物。原始模型响应沿现有 ResultStore 请求/响应产物保存，不用另一个 LLM 重写结果。所需引用目录由 WritingOperations 传入既有 instructions/user 材料槽，正式编辑坐标保留在程序输入值，不写进 system 指令。

用户要求改动超出允许范围时只返回实际可应用部分或空 edits；不能通过静默扩大范围完成。调用者需要全文自由组织时使用 rewrite。
