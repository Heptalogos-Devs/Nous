# 语言任务合同与结果解释

## 任务负责的内容

每个语言任务定义自己的自然语言目的、纯输入类型、输出类型、来源键校验和结果解释。summarize、describe-media、extract-information、select-passages、custom-reading 继续可用。转写有自己的标准端点输入和纯文本输出。

使用一个静态的 TaskDefinition 目录，由组合根或包内显式注册；无需动态插件平台。

```text
TaskDefinition:
  id
  input_type
  output_type | json_schema
  validate_output(value, source_groups) -> validated_value
  outputs(value, source_reader) -> named OutputContent
```

具体方法名可符合现有代码风格，但相同输出类型不得手写成三套 Schema。Pydantic 类型负责结构；自定义 JSON Schema 使用 jsonschema。模型执行器接收这些明确能力，不出现按 contract_id 分支的业务判断。

## 来源校验与原文选段

信息提取的 source_keys 必须指向本次输入组。选段任务返回组键/块键，由任务采用者调用共同内容读取能力取回原文。重复返回键按请求顺序保留一次，未知键报具体错误。模型不重述已选段落，也不决定数据库操作。

Markdown 块由 markdown-it 提供语法位置；文本读取能力维护精确原件行范围。代码块、列表和普通段落各保留语义，向量化使用实际文字而非行号装饰。块属于不可变 Artifact 的一个选区，无需跨所有文档编辑追踪永久段落 ID。

## 输出和采用

语言阶段返回实际输出、原生响应必要部分、实际请求/模型/模板/Schema 标识及用量。任务定义把其转换为有名产物，例如 output、passages、response；ResultStore 统一采用。

失败输出只作为本次可诊断内容或错误引用，不能自动写成成功任务产物。修复次数默认零；启用时仍由共同尝试策略计数。

## 新增 Wave 输入任务

本轮不以自动抽 Tag 作为构建前提。操作者可以在集合清单中显式给出 Concept 与出现顺序。需要辅助产生概念时，可新增专一的“提出概念描述及正文位置”任务，输出仍是待使用清单；不得让模型编写数值向量、图权重、权限、存储 ID 或 Native 控制参数。

后续 journalize/Steward 直接提供任务定义并消费共同操作，不修改协议执行器。没有实际消费者的任务不预建。
