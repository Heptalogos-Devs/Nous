# 认知文稿的所有权与基础接入

## 分工

cognition 管 Note/Journal 的身份、版本、采用、概念绑定、反馈和知识范围；content 管实际字节、正文结构、处理结果和生成血缘；models 管语言任务与请求；Wave 管可重建检索资产；Native 管数值。程序在正式采用时作决定，LLM 提供文字或小型语义提案。

Note 和 Journal 都需要不可变正文版本、来源及精确引用。共享一个有限的 WrittenRecord 保存机制，避免复制并发检查、引用采用和版本读取。产品值有明确 kind，Note 的默认召回规则与 Journal 的用途分别定义。这个共同 owner 只覆盖两类文稿，不抽象为所有未来认知对象的万能基类。

## 源码位置

| 位置 | 完整职责 |
|---|---|
| cognition/writing/reading.py、records.py | 精确读取、来源、历史、事务内保存；核心不依赖模型、标注或检索 |
| cognition/writing/operations.py | 完整记录/编辑/采用用例及 AfterWrite；程序和 CLI 共用 |
| cognition/writing/generation.py、drafts.py、tasks.py | 创建型 journalize、编辑建议、重写、反思和成功草稿的统一采用 |
| cognition/concepts.py | 概念身份、意义版本、发现与明确复用 |
| cognition/annotation/ | 领域计划、语言提案、事务参与采用、绑定与活动版本 |
| cognition/retrieval/ | capture、projection、operations 分别负责一致读取、语义投影和查询发布；正式分组在 Wave 完成 |
| cognition/feedback.py | 反馈目标与评价；显式修改进入文稿 owner |
| cognition/scope.py | Namespace、generation 及同事务推进 |
| cognition/basis.py | 明确依据更正与待复核关系 |
| cognition/contracts.py、schema.py | 共享公开值和纯数据库定义；复杂后按真实 owner 拆分 |

若某文件已包含两个独立修改原因，就按责任拆分；不按固定行数切出转发文件。避免 cognition/domain/application/infrastructure 四层平行目录。输入输出值可集中；复杂算法放已有 Wave，不塞进 records。

程序内入口与完整迁移见 [维护架构](maintenance-architecture.md)。保存与后处理职责分开，公共用例仍一次完成有意义的操作，不增加逐方法转发层。

## 公共基础怎样接入

所有操作进入 Execution.operation。配置快照从操作取得，时间使用 Clock/TimeService；DB 只用 Database/read/write；长模型/解码/构建在短事务外；跨 owner 使用 *_in(tx)。字节由 CAS 发布，ArtifactStore.register_in 与认知记录同事务登记。ResultStore.prepare_outputs/adopt_in 保存模型产物。

共同纯表在 bootstrap 显式收集；schema 不 import executor/service。没有每个领域单独的 Session、重试、UUID 转可读标识符、环境变量、文件目录和错误格式。

## 任务语义的方向

TaskDefinition 提供输入准备、输出校验和输出解释。普通语言请求携带完整 SelectedInputs，每条路线规划实际表示；concept annotation 的单正文块结构由其任务处理器定义。TaskCatalog 由组合根用显式列表提供给模型操作，已有响应的解释只使用实际交付与保存的语义合同。

正文标注值由 content.annotations 提供，供任务和 cognition/Wave 共用；模型提案与正式 Concept binding 分开。块校验由共同标注值负责。

## 完整计算与采用

GenerationOperations.produce 完成当前共同 prepare/route/execute/validate/outputs，返回 ProducedTask（实际输入、结果、参数、出处及有名输出），本身不持有业务事务。公共 run 通过 ResultStore 保存它；Journalize 复用同一 produce，并将处理结果与草稿描述一起采用。它不是第二个模型执行器。

ResultStore 负责采用通用处理结果；模型操作把 ProducedTask 转成可采用输出，调用方无需重拼 SDK response。

## 阶段与检索

Generation/Media 通过 common.operations.add_error_context/record_completed 补充领域阶段；完整用例通过 report_outcome 报告部分完成，日志与 runtime check 消费同一终态；policy.error_report 保存实际 next_action。媒体策略由 MediaAnalysis 根据配置和操作参数解释，模型参数交已有协议依赖处理。

CognitiveRetrieval 与 Wave 共享类型化 CorpusView 构建。清单文件仅为导入入口，禁止从认知表临时生成 JSON 文件作为服务协议。各操作不从整个 Runtime 获取内部依赖；Runtime 只做明确装配及公共句柄汇集。

## ProducedTask 的精确交付

```text
ProducedTask:
  operation/task, producer
  actual_inputs: ordered InputGroup[]
  outputs: named OutputContent
  request, parameters, coverage
  completed_stage_refs
  selected_inputs, dependencies
```

`produce` 包含任务输出校验和解释，`prepare_adoption` 准备可复用的已发布输出，`adopt_in` 参加调用方事务。普通 run 采用正式 ProcessingResult；journalize 在同一结果采用事务中保存 DraftContext 与草稿。写作请求另有正式恢复引用，固定原基准、来源与采用选择。网络尝试、流、转写和实际交付各使用同一负责能力；出错保留已完成阶段，成功输出的保存重试沿用已完成结果。
