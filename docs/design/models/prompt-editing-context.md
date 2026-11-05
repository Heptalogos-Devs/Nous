# Prompt 编辑上下文与实际调用说明

## 资源说明沿用 TOML

扩展现有 prompt.toml，不引入第二份 YAML Front Matter 或新的模板语言。

```toml
contract_id = "journal-edit"

[documentation]
title = "局部修订 Journal"
purpose = "根据明确意见提出局部编辑，程序保留范围外原文。"
consumers = ["writing.propose_edit"]
input_notes = "T 键为允许修改范围；其它正文是只读上下文；N 键为新增材料。"
output_notes = "返回 edits；程序解释为完整修订与差异。"

[[messages]]
role = "system"
template = "system.md.j2"
description = "稳定的任务、编辑语义与固定假设例子。"

[[messages]]
role = "user"
template = "user.md.j2"
append_sources = true
description = "本次修改要求、目标块、来源表与必要上下文。"
```

documentation 和 message.description 供维护者阅读，绝不被自动加入模型消息。保留用户已写的 Markdown/XML 规则、消息角色分工、静态正反例和实例覆盖。新增上下文说明不能覆盖已有提示词正文。

## 从实际合同取得技术信息

TaskDefinition 提供输入类型、模板变量说明、输出类型/Schema、纯任务所需能力、有名输出的意义，以及输出是否声明为来源的可读表示。Compiler 保留现行 question/instructions/sources；若任务确需新增变量，由共同输入类型一次声明并接入，不在模板说明里先写一个执行时没有的字段。

生成的 PromptContext 使用三种来源：

1. Prompt TOML：功能、使用者、每个 role/template 的目的和编辑提醒。
2. TaskDefinition/Compiler：实际 Jinja 变量名、类型、缺省/必填、哪些在 user 动态区出现；实际输出 JSON Schema/引用规则。Schema 从真实 Pydantic 类型或现有 jsonschema 合同导出，不人工维护一份相同 JSON。
3. 有效 task 配置与 InputPlan：实际模型、输出模式、格式与模态、前后语言阶段、原件及已有表示的角色。未提供本次输入时，只显示静态需要与可能模态，不宣称已经准备具体媒体或检查窗口。

模型能力由实际输入表示和任务输出要求共同决定。媒体描述可能是图片/音频/视频，journal-edit 是文本＋结构化输出；描述后结构化阶段可能仍带原媒体。不能仅根据 Prompt 名称猜模型能力，也不能在说明中承诺当前协议不支持的消息角色。

## 公共方法和产品命令

```text
PromptCatalog.describe(prompt_id, *, task_assignment?, snapshot?) -> PromptContext
PromptCatalog.diff(prompt_id, left="package", right="instance") -> dict
PromptCatalog.restore_package(prompt_id) -> PromptResource
```

真实请求预览继续走共同 prepare/compile；describe 不发模型请求、不借诊断自动转写。命令为 `nous tools prompt describe <id> [--task <task>]`、`show`、`edit`、`diff`、`restore`。

`edit` 在编辑目录同时生成只供人阅读的 `CONTEXT.md`，包含角色/模板用途、实际变量和 Schema 的相对查看入口及来源说明；input-contract.json 来自 ReadingInput 的真实 dataclass 字段与 template_values，output-schema.json 来自 TaskDefinition 的实际输出类型，custom-reading 明确标记由调用者提供 Schema；它由 describe 生成，可再次生成，不是第二份权威合同，也不进入请求。无编辑器时 --from-dir 沿当前导入能力；有实例覆盖时清楚显示包内/实例两个来源。恢复包内版本是明确用户动作，不在升级时自动覆盖。

## 实际请求指纹

区分 `execution_signature` 与 `documentation_signature`。现有 `version`/`template_version` 保存执行签名；CompiledPrompt 同时提供两个具名签名。前者包含实际有序 role、模板及 literal 依赖内容、append_sources 和实际任务输入输出合同；后者包含维护说明。只改人类说明不能使实际请求缓存失效或伪造新语言计算；真实规则或 Schema 改变必须影响执行指纹。

沿现有冻结操作：根操作固定所需 Prompt 与任务定义，fallback 和后续阶段使用这份资源。不会边生成边读取被人修改的文件。实例覆盖与包内默认不同是正常情况；报告差异，不自动合并正文。

包内资源新增字段使用同一个 PromptManifest；迁移现有解析器、导入、show/edit/validate、CompiledPrompt.to_dict 和 producer。移除旧的人造 `ReadingInput-v1` 标签，以真实输入合同及语义字段形成指纹，不增加 v2/v3 兼容分支。

## LLM 职责

保留现有 Agent 约束：系统规则在 system，真实主题、主体和材料在 user；模型只完成当前语言任务。不向模型发送部署凭据、SQL、Native 参数、事务决策。不会为了防止可能的语言错误不断增加后台审核或自动重写。

journal-edit、journal-rewrite、journal-reflect 与 draft/annotation 任务分别表达不同目标。各任务的资源和正式合同由 PromptCatalog 与 TaskDefinition 共同提供；用户可维护实例模板和说明。

写作 TaskDefinition 由 cognition/writing/tasks.py 提供，组合根 task_catalog() 显式并入共同 TaskCatalog；Prompt 管理和 Runtime 使用同一目录。required_roles 使用编译器的真实输入校验，journal-edit 要求 edit_target，journal-rewrite 要求 revision_base；没有相应请求输入时 describe 只展示该要求。
