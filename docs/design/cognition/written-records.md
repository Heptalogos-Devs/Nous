# 正式 Note 与 Journal 文稿

## 产品行为

Note 是主动记录的碎片。保存后立即可读，默认 recall=false；`--remember` 或对应配置让它直接进入长期召回。整理不会删除笔记，也不把它标为已消费而阻止再次使用。

Journal 是主题明确、自包含的长期 Markdown 文稿，可以直接手写，也可以从草稿采用。它直接记录经历、知识、反思或探索，表达理解与感受，并保留实际主体及观点归属；不以介绍输入材料和写作任务的旁白替代内容。默认 active、recall=true，不按日期强制分册。

两者共用不可变修订保存。Note 的编辑也保留旧版，使已经采用它的 Journal 仍能读回当时的笔记。原件属于 Artifact，接触记录以后由 Observation 提供；记录一个 Note 不制造虚构的外部接触事件。

## 公共身份

默认 Namespace 为 `default`，初始化时建立。Namespace 有稳定 ID 和可读唯一名称；提供 create/list/rename/select 所需操作。它划分认知数据及检索范围，本轮不提供账户、ACL 或远程多租户承诺。原件存储仍是实例级，显式在某域采用原件是正式来源关系。选择默认域时保存稳定 Namespace ID，名称仍用于输入和显示。改名保留同一身份；有效配置中的高优先级覆盖仍选择旧名称时，先返回覆盖来源与可操作错误，域名保持可用。

短地址使用已有 Proquint：`note:…`、`jrn:…`、`con:…`；草稿使用 `drf:…`。`jrn:…@3` 固定第三个正文修订；没有 @ 表示当前。内部 revision_id 保持稳定，ordinal 单调递增且不重用。旧快照和来源绑定只使用精确版本。

Note 和 Journal 的 kind 不原地互换。将 Note 整理为 Journal 创建新身份，并保存来源。公开引用包含实例及 Namespace，入口选择域与引用域不符返回明确错误。

## 操作合同

```text
record_note(namespace, body, sources=[], author=?, times=?, remember=default) -> SavedRecord
create_journal(namespace, body, title=?, purpose=record, viewpoint=?, sources=[]) -> SavedRecord
read(ref, revision=current, selection=?, sources=false) -> RecordView
revise(ref, expected_revision, body, title?, sources?, change_kind, reason?) -> SavedRecord
history(ref) -> ordered RevisionSummary[]
archive(ref, expected_revision) / restore(ref, expected_revision) -> SavedRecord
set_recall(note_ref, expected_revision, enabled) -> SavedRecord
```

body 是 UTF-8 Markdown 或已存在的 text Artifact；接受文本输入、文件或引用由共同内容操作转换。手工创建正文为空时返回有用错误。title 缺省从首个 H1 提取，无 H1 时使用可读“未命名笔记/日记”；标题不充当唯一身份。

revise 的整个 metadata 形成新 revision；不把旧时刻伪写成新记录时刻。未提供 sources 表示保留旧绑定，显式空数组表示移除；被正文实际引用的 key 必须仍可解析。更改作者/目的/视角也保留修订历史。

archive/restore、recall 修改只改变可见成员状态，不重写正文，但在短事务中推进 Namespace generation。expected_revision 防止用户依据过期正文进行该操作。旧内容精确读取继续可用。公开永久删除不在本轮范围，避免把数据清理误当归档。

## 保存与并发

先解析正文和引用、发布新字节；一个 WriteTx 核对当前 revision，登记 Artifact、revision、source bindings，更新 head，推进 namespace generation。冲突回滚这些正式记录，返回 current_revision 和可保留的编辑稿/草稿引用；不合并、不自动调用模型。

先核对 expected_revision；只有基准有效且同一正文与所有 metadata 均未变化时返回 unchanged，不制造新 revision/generation。比较由当前正式值完成，不拿正文相似度替代等价。

SavedRecord 包含 stable_ref、revision_ref、title、namespace、generation、saved=true 和 retrieval 状态。保存操作缺省不调用 Embedding 或构建索引；想立即查询时可要求 `--refresh`，或用 `--refresh-mode lexical` 明确词项刷新。进程内操作使用同名 refresh/refresh_mode 参数；共同 complete_save 在提交后组织标注与刷新，保存与接续结果分别呈现。

## 视角和时间

author/viewpoint 描述实际作者或主体，模型执行者从 ProcessingResult 读取；换模型不改变认知身份。可保存调用者提供的本地标识与可读名称，不为此创建 Social 或用户系统。

recorded 为事务的真实 UTC，revision 顺序消除同刻歧义。formed 可由调用者给认知时间域；未给则记录系统生成时刻及其域，不猜虚拟时间。occurred/observed/valid 只保存有实际声明的值；媒体页/秒不能映射为这些时间。

SavedRecord 的附加阶段使用 `annotation: not_requested|ready|failed`、`retrieval: not_in_scope|pending|ready|failed` 和具体 error；没有后台任务时不显示 running。主保存失败是 error，已保存而明确附加步骤失败是 partial，并附已保存引用。
