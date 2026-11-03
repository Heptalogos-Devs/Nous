# Prompt 资源与确定编译

## 资源责任

PromptCatalog 列出、读取、编辑和选择模板；PromptCompiler 在纯输入上编译角色消息；任务 owner 定义任务的自然语言目标与小输出合同。两者不连模型、不查数据库、不在模板中进行媒体解析或检索。

模板是普通 Markdown/Jinja。包内资源使用 importlib.resources 与 Jinja loader；实例覆盖放 config/prompts/<id>/。Namespace 尚未成为本轮产品前置，不为模板覆盖单独创建认知域。

## 文件结构

一个资源目录包含 `prompt.toml`、明确列出的消息模板及被引用的 include 文件。例如：

```toml
contract_id = "media-description"
[[messages]]
role = "system"
template = "system.md.j2"
[[messages]]
role = "user"
template = "user.md.j2"
append_sources = true
```

messages 的数组顺序就是实际消息顺序。role 使用成熟库可保留的 system/user/assistant/tool 等含义；system/developer 的协议差异由依赖处理并在预览中展示实际采用的角色。无法保留的组合在准备阶段报出，不能压平为一个 user 字符串。

`append_sources=true` 是一个固定媒体/资料槽：在当前消息的模板文本之后，按输入顺序展开已经准备的来源标题、文字和媒体 parts。它不查询新来源、不选择材料。初版不增加通用模板编排 DSL。

## 一次编译使用一份资源快照

get/compile 一次读取 manifest 与模板依赖的实际内容，形成资源版本；使用 DictLoader/已冻结 loader 渲染这份内容。literal include/extends 形成依赖集合；本轮拒绝动态文件名 include，避免执行时无声加载版本之外的模板。

版本由实际模板内容、manifest 和任务输入/输出合同版本共同确定。修改 include 也会改变版本。已准备请求继续使用原编译结果，不读回磁盘再次渲染。fallback 更换模型只调整协议适配与可用参数，任务正文/来源不因此切到新版本。

使用 Jinja Environment、StrictUndefined 和内置 bytecode cache。Markdown 默认不进行 HTML 转义；资料只作为值插入一次，不当作新 Jinja 程序再次解析。模板由本机可信操作者编辑，无需建设在线模板沙箱平台。

## 内容结构

system：任务目标、适用标准、短例子和输出含义。user：当前问题、真正的资料与局部来源键。任务限定来自需要，不灌入所有配置、预算和内部对象。

编译值返回有序的消息、媒体位置、模板版本、Schema、局部来源映射与估计输入信息。二进制媒体由成熟库的内容类型携带；用户预览显示类型、尺寸、选区和地址，不打印 base64。

## 预览与执行

`prompt preview` 使用正式编译器，完成模板与输入合同检查，不执行模型请求。需要确定性媒体准备时可以执行，并在结果中说明；不会因此自动运行音频转写或媒体描述。

`model run --show-request` 先准备与该次运行完全相同的请求，在 stderr 展示，然后执行该进程内 PreparedGeneration，不二次构造消息。这个选项明确表示“运行并展示请求”；纯预览只用 prompt preview。单独在另一命令中 preview 后再 run，会重新准备当时的输入；各自版本明确，不提供跨命令隐含冻结承诺。

`prompt validate` 检查 manifest、合同引用、模板语法和变量名称；没有实际输入时不宣称数据依赖分支都已成功渲染。实际 preview/run 使用真实输入完成 StrictUndefined 检查。

`prompt show/edit/validate` 是产品管理操作。edit 通过系统选定编辑器编辑实例副本，返回后校验并报告；无编辑器环境可用 `--from-dir` 导入整份资源。未配置模型也可使用。

## 结果与回看

ResultStore 保存已经采用请求的语义快照及来源。SDK 对请求的实际 wire 编码由依赖完成；预览和记录反映提交给该依赖的相同角色、内容、模式和参数。需要排查协议时再展开实际传输诊断，不把诊断数据当作第二份请求权威。

正式模板及其输入、输出合同位于[模型资源目录](../../../src/nous/models/resources/)，实例可按需覆盖其中的模板。

依据：[Jinja API](https://jinja.palletsprojects.com/en/stable/api/)、[Pydantic AI messages](https://pydantic.dev/docs/ai/api/pydantic-ai/messages/)。
