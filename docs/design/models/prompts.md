# Prompt 资源、快照与共同预览

编辑时的用途说明、真实变量/Schema、模型能力和资源差异见 [Prompt 编辑上下文](prompt-editing-context.md)。TOML 的 documentation 与 message.description 只供人阅读；执行指纹和说明指纹分别计算，下面的指令与冻结合同继续成立。

## 资源

PromptCatalog 管理包内只读与实例可编辑的 Markdown/Jinja 资源。manifest 明确 contract_id、有序 message role/template 和 source 槽；模板只渲染纯值，不查数据库、不准备媒体、不发模型请求。

TaskDefinition 管理输入输出类型及引用规则。system/user/assistant 是本轮模板直接支持的角色；工具历史由 SDK 原生消息值保留。若新增 developer/tool 模板资源必须连同真实协议 ID 和返回配对完整接入，不能文档写支持而编译器丢弃。没有需要时保持准确的当前角色集合。

## 指令设计

通用任务分工、输入准备、输出判断和提示词写法见 [LLM 使用与提示词设计](../../agents/llm-use.md)。

## 冻结范围

根操作开始时捕获其所有计划内任务资源，内容、manifest、literal include/extends 和输入输出合同共同确定版本。Jinja 使用 DictLoader 冻结本次实际内容、StrictUndefined、已配置编译缓存和 bytecode cache。后续阶段与 fallback 使用同一份已捕获资源。

输入正文只作为值插入一次，原文中的模板符号原样保留。动态文件名 include 本轮不支持。模板修改影响下一次普通根操作。已排队任务使用 GenerationOperations.freeze_language_task 发布的正式 FrozenLanguageTask；其中保留 manifest、实际模板及 include/import 内容，load_language_task 在当前 Runtime 核验资源绑定后恢复，既有任务继续使用接受时的模板。冻结保存与资源绑定详见[成功计算恢复](durable-computation.md)。

## 共同编译

同一 Compiler 供 preview、run、media 和后续认知任务使用。不要在 CLI 另建默认缓存参数的 Compiler。任务语义编译与协议适配分开：前者形成有序文本/媒体引用组，后者由成熟 SDK 提交实际媒体。

InputGroup 的文本和多个媒体 part 保持明确顺序；预览显示名称、类型、来源选区和已知尺寸，不打印 base64 或大矩阵。`append_sources` 是固定槽，没有过程式 DSL。

附加来源时，user 消息在材料旁以当前配置语言说明已知的来源用途和实际范围；具体范围来自媒体准备和来源组，不从文件名、编号或模型名称猜测。详见[输入规划](input-planning.md)。

## 预览

纯模板 preview 可以接收准备好的文字和确定性媒体；其结果明确为模板预览，未选择实际模型时不宣称窗口/协议已检查。task preview 使用同一 InputPlan、任务解析与表示选择，展示真正的请求组。joint 的多个小输入共同出现；明确分批任务分别显示批次。不能展示执行时不会发送的合并请求，也不能把联合问题预览成互不相关的文件。

`model run --show-request` 展示即将执行的 PreparedAttempt 后直接执行同一对象；没有审批停顿。preview 不发转写/描述。缺少必需语言表示时明确指出由 media analyze 或正式 transcription 操作产生，或者用户显式提交已有结果。

## 编辑

导入目录与普通编辑共享资源加载及校验。先直接读取传入目录的 manifest 与 literal 依赖并校验，再写入实例覆盖。已发布请求持有旧内容，不依赖磁盘资源在执行中不变。

包内模板、实例覆盖和 schema 参数形成单一实际请求说明。采用结果保存实际模板版本；缓存只是性能资产。

## 管理与预览的具体合同

`prompt show/edit/validate` 在没有模型配置时仍可使用。validate 检查 manifest、任务引用、Jinja 语法和已知变量；没有真实输入时不声称数据依赖分支已渲染。preview/run 使用实际输入完成 StrictUndefined 校验。

messages 数组保留顺序，append_sources 在对应消息后按 InputPlan 顺序加入来源；Markdown 默认不 HTML 转义，输入中的 Jinja 文字只作为值插入一次。动态 include 不进入隐式依赖。

普通 edit 使用所选编辑器编辑实例副本，返回后校验；无编辑器使用 --from-dir。编译请求在根操作内保持冻结。另一条命令 preview 后再 run 会重新准备当前资源，不提供跨命令隐含冻结。

show-request 写 stderr，stdout 保持结果/JSON/NDJSON。预览只展示媒体类型、实际范围、大小和短引用，不输出 base64。独立纯预览不发转写或描述请求。

## 资源目录与消息槽

实例资源位于 config/prompts/<id>/，与包内只读资源使用同一加载合同。prompt.toml 的 messages 数组决定消息次序；append_sources=true 在对应 user 消息文本后按实际顺序加入已经准备的资料组，不执行检索或选择新资料。

修改模板文字不改变 TaskDefinition 的程序输出类型；显式 custom-reading 的 --schema 按其合同使用。预览记录提交给成熟 SDK 的同一语义消息及实际参数；需要 wire 诊断时另展开，不把诊断字节当作第二份请求权威。
