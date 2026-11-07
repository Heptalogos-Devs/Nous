# 语言任务合同与结果解释

## 任务负责的内容

每个语言任务定义自己的自然语言目的、纯输入类型、输出类型、来源键校验和结果解释。summarize、describe-media、extract-information、select-passages、custom-reading 继续可用。转写有自己的标准端点输入和纯文本输出。

使用一个静态 TaskCatalog，由组合根显式提供各任务定义。专有输入准备、输出校验与产物解释由该任务负责；通用 InputPlan 不按任务名字分支。无需动态插件平台。

```text
TaskDefinition:
  id
  input_mode: joint | independent_batches
  text_input_mode: lossless | structured
  prepare_sources(originals, artifacts) -> MediaPlan[] | None
  validate_input(ReadingInput)
  output_type | caller_schema
  validate(value, ReadingInput) -> validated_value
  outputs(value, ReadingInput, artifacts, structured, producer) -> named OutputContent
```

具体方法名可符合现有代码风格，但相同输出类型不得手写成三套 Schema。Pydantic 类型负责结构；自定义 JSON Schema 使用 jsonschema。模型执行器接收这些明确能力，不出现按 contract_id 分支的业务判断。

TaskDefinition.interpretation_contract() 固定恢复解释所需的输出 Schema、来源规则和有名程序产物。恢复只读取保存的成功输出与实际输入，按静态任务目录解释；人工帮助、类型所在路径和描述展示不进入解释合同。Schema 在各层 schema 节点去掉 title、description、examples 等说明字段；properties 下同名业务字段仍保留，类型、范围、必填及其他约束继续参与身份比较。

## 来源校验与原文选段

信息提取的 source_keys 必须指向本次输入组。选段任务返回组键/块键，由任务采用者调用共同内容读取能力取回原文。重复返回键按请求顺序保留一次，未知键报具体错误。模型不重述已选段落，也不决定数据库操作。

Markdown 块由 markdown-it 提供语法位置；文本读取能力维护精确原件行范围。代码块、列表和普通段落各保留语义，向量化使用实际文字而非行号装饰。块属于不可变 Artifact 的一个选区，无需跨所有文档编辑追踪永久段落 ID。

## 输出和采用

TaskDefinition 的 `text_input_mode` 声明 lossless 或 structured；普通全文输入按连续 UTF-8 范围保留定义行、CRLF、间隙与尾部，结构化呈现仍保留块外原文。

共同 GenerationOperations.produce 在协议完整响应和结构化 validate 成功后，先通过 ResultStore 保存正式可读的原始成功响应，再调用任务 outputs 解释有名产物，返回 ProducedTask 的实际输出、输入、请求/模型/模板/Schema 标识及用量。validate 参与既有结构化修复预算；outputs 的确定性解释失败使用已保存响应恢复。公共 run 和认知草稿共用这一条计算路径；ResultStore.prepare_outputs/adopt_in 继续统一采用，允许调用方在同一短事务登记自己的正式关系。

结构化校验未通过的响应留在本次尝试诊断中；已经通过校验、但 outputs 或后续采用失败的响应拥有正式 model.language_response 引用。按原 producer_key 重入 produce 或调用 recover_response(response_ref, snapshot) 可重新解释，见[成功计算恢复](durable-computation.md)。修复次数默认零；启用时仍由共同尝试策略计数。

## 新增 Wave 输入任务

本轮不以自动抽 Tag 作为构建前提。操作者可以在集合清单中显式给出 Concept 与出现顺序。需要辅助产生概念时，可新增专一的“提出概念描述及正文位置”任务，输出仍是待使用清单；不得让模型编写数值向量、图权重、权限、存储 ID 或 Native 控制参数。

后续 journalize/Steward 直接提供任务定义并消费共同操作，不修改协议执行器。没有实际消费者的任务不预建。

## 正文任务

`journal-draft` 使用 joint 输入，返回普通 Markdown 草稿；认知 `cognition/annotation/task.py` 定义的 ConceptAnnotationTask（合同 ID `concept-annotation`）使用一份共同 TextView，返回概念、出现项和路径，具体合同见 [概念标注](../cognition/concept-annotations.md)。任务 owner 构造局部块键、校验来源并采用普通结果，通用 executor 不识别任务专名。

写作任务使用 `journal-draft`、`journal-edit`、`journal-rewrite` 和 `journal-reflect` 资源，各自的输入输出合同见[写作意图](../cognition/writing-intents.md)。`annotate-concepts` 要求一份完整正文，输入组使用共同 TextView 的 B 键。模型返回 AnnotationProposal；采用时加入正文、parser profile、内容修订与 producer，形成普通 AnnotationSet JSON 产物。`content.annotations.read_annotation` 读取产物、补入实际 ResultRef 并核对正文块键，供 Wave 等消费者复用。

Journal owner 使用这些语言任务，并通过 [草稿采用](../cognition/draft-adoption.md)和正式文稿操作完成状态。模型任务不修改 Journal/Concept 表；正文与标注产物继续可以作为普通 tools 结果独立使用。
