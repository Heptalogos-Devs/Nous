# 专一语言任务与小输出合同

## 目的

本轮用简单、可复用的真实语言任务完成预实验。任务会留作产品初始工具模板；它们不创建认知对象，不推断对象生命周期，不承担整个系统的规划。

TaskDefinition 由任务 owner 提供 ID、输入类型、输出类型/校验方式、需要的模型能力和默认 Prompt。Prompt 引用该合同，不自行发明一份不一致的程序 Schema。

## 初始任务

| contract_id | 输入 | 输出 | 实际用途 |
|---|---|---|---|
| reading-summary | question/instructions、ordered sources | Markdown 字符串 | 主题整理、文本或资料摘要 |
| media-description | question、ordered sources | Markdown 字符串 | 描述实际图像/声音/页面/采样画面 |
| information-extraction | question、ordered sources、可选 description source | ExtractedInformation | 明确目的的信息提取 |
| passage-selection | question、带局部段落键的文字 sources | PassageSelection | 从已给出的候选原文挑选段落 |
| transcription | 已准备音频、可选 language/hint | 带实际返回信息的转写文本 | 标准转写端点输入输出 |
| custom-reading | ReadingInput 与使用者明确提供的 JSON Schema | 已校验 JSON | 用户定义的单一信息处理任务 |

`ExtractedInformation` 仅含 `title: str` 与 `items: list[Item]`；Item 为 `text: str`、`source_keys: list[str]`。缺少内容时 items 可空，不编造补足。来源键只引用本次实际输入，归属和不确定性写在自然语言 text 中。

`PassageSelection` 仅含 `keys: list[str]`，例如 `S1:P2`。程序检查键存在，从保存的 source map 读取原文，按返回顺序去重后交付。模型不重写段落，也不负责数据库引用转换。

这些形状使用一份 Pydantic 类型产生 Schema、进行验证与序列化。数字不是可信度万能字段；本轮不要求模型给每句话打分。

## 输入组织

生成类任务共用 `ReadingInput(question: str = "", instructions: str = "", sources: tuple[PreparedSource, ...])`，具体任务可以对它进一步约束。空问题/附加说明在类型中有明确默认，StrictUndefined 不用来逼使用者填写无关字段。

PreparedSource 含局部 key、可读名称、可选精确文本、可选媒体 part、Artifact/Selection 对应关系。发给模型的文本只展示 key、名称、必要时间/位置和正文；UUID、blob 摘要、数据库状态与配置对象留在程序。

全局问题/instructions 可以为空。实际资料是唯一任务材料来源，模板中的示例明确是示例。用于摘要的开发对话由用户或本次接纳提供；不存在的前文保持空。

## 段落选择的范围

使用 markdown-it-py 的 block/source map 组织局部段落，按原文行/字节位置取回准确内容。列表和代码块保留完整语义块，不按每一个空行盲切。键映射固定到本次不可变 Artifact；跨版本段落跟踪不在本轮。

候选由使用者显式给出。这里没有 Wave 候选检索、Steward 常驻工作集或任何记忆强化行为。后续真正 Steward 可以复用选择任务及原文回取能力。

## 自定义结构化任务

`custom-reading` 合同允许通过 `model run --schema PATH` 接收明确 JSON Schema；已登记的 extraction/selection 等任务继续使用自己的固定输出合同，不能用该选项无声替换。custom-reading 的任务配置明确选择 native 或 tool 输出模式。先用标准 jsonschema 检查 Schema，模型响应再由同一 Schema 校验。只支持本地解析的引用，不为 Schema 自动下载外网内容。Pydantic AI 的 StructuredDict 本身不保证本地值校验，因此不能以拿到一个 dict 就宣称结果正确。

常用任务继续使用 Pydantic 类型；通用 Schema 是用户主动选择的额外入口。结果错误给出实际问题及原输出读取方式，不默认增设自我审查/反思轮。

依据：[Pydantic AI 输出](https://pydantic.dev/docs/ai/core-concepts/output/)、[markdown-it-py source map](https://markdown-it-py.readthedocs.io/en/latest/api/markdown_it.token.html)。
