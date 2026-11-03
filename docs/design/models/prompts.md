# Prompt 资源与 Jinja 编译

## 目标

Prompt 是可阅读、可编辑、可预览的正式资源。任务以专一语言工作组织，输入提供真实材料，输出仅包含程序需要的结果。身份、事务、配置生效、资源调度与正式数据采用留在程序中。

PromptCatalog 管理资源；PromptCompiler 渲染确定的消息；各领域维护自己的任务说明、输入与输出合同。两者不承担模型网络请求。

## 资源形状

每个 Prompt 资源具有稳定名称、有序消息模板、所服务任务、实际版本摘要及说明。常见资源为同目录的 `system.md.j2` 和 `user.md.j2`；需要多消息时用显式清单声明顺序与 role，不依据文件系统遍历顺序猜测。

包内模板只读，通过 importlib.resources 加载。操作者覆盖放入实例 config 根的 `prompts/`，Namespace 覆盖由同一资源解析规则处理。精确采用版本由实际模板内容及依赖的 include 共同确定。

任务输入类型与输出类型由任务 owner 用 Pydantic 定义，模板资源引用这个合同。不得手写三份相同字段分别给本地验证、Provider Schema 和结果解析。编辑普通模板不能无声改变程序接受的输出字段。

## 编译

```text
PromptCatalog.get(task, scope) -> PromptResource
PromptCompiler.compile(resource, typed_input, output_contract) -> CompiledPrompt
PromptCatalog.preview(...) -> CompiledPromptView
```

CompiledPrompt 包含有序的角色消息、媒体 parts、实际使用的模板版本和 schema 信息。编译只消费传入的纯值，不查询数据库、不调用模型、不过程式选择检索结果。

使用 Jinja Environment、StrictUndefined、内建 loader 和 FileSystemBytecodeCache。必需变量缺失直接报出变量及模板位置；真正可选的变量在输入合同中显式定义。模板只由可信操作者编辑，不把用户上传正文当作可执行 Jinja 再次渲染。

模板输入的资料作为文本值插入一次。局部来源键如 J1:P2 由程序分配并绑定精确来源，原始正文中出现模板符号也保持正文。少量纯格式 filter 可以注册；网络、文件读取和业务动作不做成模板 filter。

## 消息角色与内容

任务指令使用适合提供方协议的 system/developer 位置；当前材料和问题使用 user；真实历史保持 assistant/tool 的配对与协议 ID。role 映射归 adapter，无法保持语义的协议组合应在准备阶段明确报出，不压平为一个 user 字符串。

Markdown 标题组织任务、材料、必要例子和输出说明。system 说明稳定标准；user 给本次内容、局部引用和目标。不要在模型输入里灌入完整业务 DTO、UUID、事务版本或所有配置限制。

媒体以独立、有顺序的 parts 进入请求，不把图片、视频等仅替换成不可访问的路径字符串。必要描述与原媒体能够共同进入后续任务。

## 典型任务的粒度

Steward 输入问题与已排序候选，返回段落键；程序取回原文。journalize 组织主题连贯的文章；程序负责正文保存与引用。视觉描述只描述实际内容；后续结构化任务读取描述及必要媒体。

模型无效输出应先检查任务是否过载、材料是否完整、模型是否支持所需能力。程序做类型及引用检查，不把每次失败都扩张成新的推理阶段。

## 预览、历史与缓存

预览显示实际有序消息、媒体位置、输出模式和估计输入量。执行使用同一份编译结果，不二次渲染。付费调用的采用者保存必要模板版本和生成出处；无需为每个文件编辑创建永久数据库历史系统。

模板缓存、模型前缀缓存和结果缓存分别见[缓存专题](caching.md)。模板明确修改后产生新版本；已准备请求保持原编译内容。

## 完成条件

模板可列出、读取、编辑、校验和预览；缺少变量在联网前失败；预览与实际请求对应；中文 Markdown、工具历史和媒体顺序保留；用户正文中的 Jinja 符号不被二次执行。

依据：[Jinja API](https://jinja.palletsprojects.com/en/stable/api/)、[OpenAI Prompt engineering](https://developers.openai.com/api/docs/guides/prompt-engineering)、[Reasoning best practices](https://developers.openai.com/api/docs/guides/reasoning-best-practices)。
