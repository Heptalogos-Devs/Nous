# Prompt 编辑上下文与实际调用说明

## 资源说明沿用 TOML

扩展现有 prompt.toml，不引入第二份 YAML Front Matter 或新的模板语言。

`documentation` 和 `message.description` 供维护者阅读，绝不被自动加入模型消息。
每份 `prompt.toml` 都应在自身说明中提供足以编写相邻 system 和 user 模板的上下文，
不要求编写者先查看其他生成文件。

`input_notes` 说明可用模板变量的形状、本任务如何使用它们、来源键和媒体附件如何进入请求；
给出一组假设变量及其渲染后的完整 user 消息，清楚标出独立附件。
`output_notes` 说明模型返回的内容、字段和实际用途，并给出与该输入对应的完整输出示例；
结构化任务补充字段类型、引用约束和空结果，自定义 Schema 示例同时展示调用方提供的 Schema。
示例与真实任务合同保持一致，不把示例或维护说明发送给模型。

例如，[局部编辑资源](../../../src/nous/models/resources/journal-edit/prompt.toml)
展示了可编辑目标、只读上下文、新增材料，以及返回的 JSON 编辑提案。
保留用户已写的 Markdown/XML 规则、消息角色分工、静态正反例和实例覆盖。

`append_sources` 在 user 消息中附加实际材料及已知的用途、范围说明。来源的准确表示与说明可通过 task preview 查看，选区、原件时长或页数等取自实际输入规划；维护说明本身不进入模型消息。

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

`edit` 在编辑目录同时生成只供人阅读的 `CONTEXT.md`，按输入、输出、消息及已配置模型组织说明，并列出实际路线的能力、格式、预设参数和实例执行限制。input-contract.json 来自 ReadingInput 的真实字段与 template_values，output-schema.json 来自 TaskDefinition 的实际输出类型；custom-reading 明确标记由调用者提供 Schema。实际媒体和逐次参数使用 preview 查看。无编辑器时 --from-dir 沿当前导入能力；有实例覆盖时清楚显示包内/实例两个来源。恢复包内版本是明确用户动作。

## 实际请求指纹

区分 `execution_signature` 与 `documentation_signature`。现有 `version`/`template_version` 保存执行签名；CompiledPrompt 同时提供两个具名签名。前者包含实际有序 role、模板及 literal 依赖内容、append_sources 和实际任务输入输出合同；后者包含维护说明。只改人类说明不能使实际请求缓存失效或伪造新语言计算；真实规则或 Schema 改变必须影响执行指纹。

沿现有冻结操作：根操作固定所需 Prompt 与任务定义，fallback 和后续阶段使用这份资源。不会边生成边读取被人修改的文件。实例覆盖与包内默认不同是正常情况；报告差异，不自动合并正文。

包内资源新增字段使用同一个 PromptManifest；迁移现有解析器、导入、show/edit/validate、CompiledPrompt.to_dict 和 producer。移除旧的人造 `ReadingInput-v1` 标签，以真实输入合同及语义字段形成指纹，不增加 v2/v3 兼容分支。

## LLM 职责

保留现有 Agent 约束：系统规则在 system，真实主题、主体和材料在 user；模型只完成当前语言任务。不向模型发送部署凭据、SQL、Native 参数、事务决策。不会为了防止可能的语言错误不断增加后台审核或自动重写。

journal-edit、journal-rewrite、journal-reflect 与 draft/annotation 任务分别表达不同目标。各任务的资源和正式合同由 PromptCatalog 与 TaskDefinition 共同提供；用户可维护实例模板和说明。

写作 TaskDefinition 由 cognition/writing/tasks.py 提供，组合根 task_catalog() 显式并入共同 TaskCatalog；Prompt 管理和 Runtime 使用同一目录。required_roles 使用编译器的真实输入校验，journal-edit 要求 edit_target，journal-rewrite 要求 revision_base；没有相应请求输入时 describe 只展示该要求。
