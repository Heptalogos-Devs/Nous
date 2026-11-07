# Journal 维护的用户入口

## 产品命令与完整操作

CLI 调用绑定 Namespace 的同一应用能力。文件/标准输入/编辑器只负责取得输入；业务采用、来源和后处理属于应用操作。

将示例中引号内的标识符替换为实际返回的完整标识符并保留引号；标有 `@N` 的参数使用所需修订的完整标识符。

```text
nous note add --text "..."
nous journal create --file note.md
nous journal edit "jrn(…)"                      # 编辑器，人工修改
nous journal patch "jrn(…)@3" --changes edits.json
nous journal replace "jrn(…)@3" --file complete.md --reason "..."
nous journal revise "jrn(…)@3" --instruction "只修正这一段"
nous journal rewrite "jrn(…)@3" --instruction "重新组织全文"
nous journal reflect "jrn(…A)@2" "jrn(…B)@1" --topic "共同认识"
nous journal diff "jrn(…)@2" "jrn(…)@3"
nous journal history "jrn(…)"
nous journal draft read "drf(…)"
nous journal draft adopt "drf(…)"
nous journal draft recover "res(…)"
nous journalize --source "note(…)" --source "art(…)" --topic "新篇主题"
```

`revise` 作为自然语言提出修改的正式命令；原来 revise 的整篇文件替换迁到 replace，更新现有文档和所有调用方，不保留两套歧义含义。用户也可直接以代码调用明确 API，不需构造临时 JSON。

patch/edit/replace 不发语言请求。rewrite 是明确全文操作，journalize 不再接受已有 Journal 或 target。无需让每项人工编辑走提案审批；需要提案的自然语言操作可 --save 直接接续采用。

生成后的解释或草稿保存失败时，错误包含 writing_request_ref。使用 `journal draft recover` 传入该结果引用，按原基准、来源和采用选择继续。

`--changes` 文件是 TextChange 数组，每项通过 `action` 指定 `replace_quote`、`insert_before`、`insert_after` 或 `replace_selection`；`scope` 或 `selection` 使用读取到的精确正文选区或块引用。字段与坐标见[精确编辑](../cognition/exact-editing.md)。

root command 参数解析错误继续遵守统一 JSON/非零退出/debug 合同。--show-request 写 stderr，stdout 保持正文或 JSON；部分完成按现有返回合同呈现并给诊断引用。

## 反馈使用

```text
nous feedback record "jrn(…)@3" --category accuracy --value negative --comment "..."
nous feedback list --target "jrn(…)" --unresolved
nous feedback read "fbk(…)"
nous journal revise "jrn(…)@3" --feedback "fbk(…)"
nous feedback resolve "fbk(…)" --action not_applied --note "..."
```

query result 文件是正式 QueryResult 的序列化，`feedback record --query-result` 反序列化同一类型。提交意见不触发更正，显式修订采用后反馈才标为已处理。每个 complete command 只有一个根 operation。

## 标注完整性

note/journal annotate 直接调用 `AnnotationOperations.annotate(...)`；`--draft-only` 使用其正式选项。CLI 不自行 propose 再 adopt、不从查询结果里抽 SQL identity、不补根 Trace。人工 proposal、known Concept 与 bindings 仍由领域方法验证。

## 语言和标题

所有新增认知命令使用现有 `presentation` 翻译入口，支持 --language。删除各命令中拼写“中文 / English”的常量串，不另建一套翻译文件或按操作分别处理 locale。

业务层 title 未提供且正文没有 H1 时存 `None`；默认界面按文稿 kind 显示本地化“未命名笔记/Untitled note”“未命名日记/Untitled journal”。作者写下的正文、标题、评论和来源名不自动翻译。显式 H1/标题继续原样保存；标题字段可空的变更覆盖 schema、值类型、读取、列表、查询元数据和模板输入消费者。

JSON 中保留 title=null 和 kind，不能把当前终端语言写入持久数据。外部展示可根据相同消息键取得 fallback label。模型输入未命名材料时可使用局部来源键与实际类型，不把 CLI locale 伪装为作者元数据。

## 可读结果

普通读取先显示正文、精确版本、必要来源入口；修改提案默认显示有意义差异；采用后显示新版本及标注/检索的实际状态。查询按文稿显示片段；详细评分和参数通过 --explain 或 --json 读取。

多媒体来源显示类型、选区、可打开原件及已有表示；不能打印 base64 或 UTF-8 解码异常替代来源。运行中模型错误仍给业务原因和操作引用，不用长 UUID 列表代替说明。

## 已保存但未完成的进程退出

明确要求或已启用的维护发生失败时，CLI 先输出包含已保存引用和阶段错误的正常结果，退出码为 3。正常 defer/pending 返回 0；主操作失败沿现有失败退出码。`--json` 同样输出 RecordOutcome，不在 JSON 前后插入说明。宿主调用直接处理类型化 status，无 CLI 退出语义。

这个区别让自动调用者不会把已保存误判为未保存而重做 journalize，也不会忽略维护失败。重新尝试时调用标注/refresh 或提案采用的正式操作，不能重做已经提交的正文。

## patch 文件与来源变更

`--changes` 接收 TextChange 列表。JSON 必须显式写 action：replace_quote、insert_before、insert_after 或 replace_selection；模型编辑提案的 replace 是另一项专一输出合同。Python 构造器可以省略固定 action，导出时自动带出正确值：

```python
import json
from pathlib import Path
from nous.content import ReplaceQuote

change = ReplaceQuote(scope=view.body_selection, before="原句", after="新句")
Path("edits.json").write_text(
    json.dumps([change.model_dump(mode="json")], ensure_ascii=False), encoding="utf-8"
)
```

patch/replace 的 `--sources` 接收 SourceChanges JSON（preserve、add、remove）。缺省保留旧目录；add 只列新增 SourceBinding，remove 列旧键。replace --source 等逻辑来源在同一操作内分配不与旧目录冲突的 N 槽；--clear-sources 明确清空目录，正文仍必须通过真实引用校验。

`journal draft adopt --file complete.md [--sources source-changes.json]` 是显式全篇覆盖。来源变更须伴随完整 Markdown；草稿读取默认显示生成差异，--body 读取原生成全文，已采用的人工覆盖另标 adopted_as=override。

revise/rewrite 的 --intent correction 明确按更正采用；省略时是普通 edit，不按意见词汇推断更正。

## 将旧版意见明确用于新基准

revise、patch、replace 的 `--feedback-mappings mapping.json` 接收相同 JSON 对象：键是 --feedback 指定的反馈引用，值是适用于这份新基准的非空说明。没有映射时，旧版意见不会被静默当作当前版意见。自然语言提案把该对象保存在 DraftContext.feedback_mappings；采用后 resolution.mapping_note 保留说明。跨文稿和整体查询意见仍不接受这种采用。

```text
nous journal revise "jrn(…)@2" --feedback "fbk(…)" --feedback-mappings mapping.json --instruction "只更正仍有问题的日期" --save
nous journal patch "jrn(…)@2" --changes edits.json --feedback "fbk(…)" --feedback-mappings mapping.json
```

标题来源 title_source 为 user、heading 或 none。显式用户标题在正文修改后保留；heading 来源随新 H1 修改或删除而更新，none 在界面按 kind 本地化显示默认名称。读取与历史 JSON 保留这项来源以及真正的 title 值。
