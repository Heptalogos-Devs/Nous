# 冻结语言任务与成功计算的恢复

## 分工

模型执行器负责当前请求、路线尝试、流和协议响应；具体 TaskDefinition 负责输出解释；ResultStore 负责正式处理结果采用。DBOS 记录步骤完成值，不能成为认知或原始内容的另一份权威。

`ProducedTask` 是成功语言响应经 outputs 解释后的内存值；`FrozenLanguageTask` 与 `PublishedComputation` 提供正式内容引用，连接模型响应、任务解释和领域采用。

## 先校验可确定的输入

journalize/reflect/rewrite/edit 在发送请求前校验 Namespace、exact base、来源范围/归属、任务合同、采用方式以及调用者提供的认知时间域。`TimeService.validate` 验证已经提供的时间坐标；默认 formed 在实际形成时确定，recorded 由采用事务记录，不能把“提前校验”误实现为把所有时间固定成排队时刻。

成功响应先由 GenerationOperations 采用；outputs 失败携带 response_result_ref 和 producer_key。写作 _complete 从链接规范化开始捕获失败并保留 PendingWriting，complete_pending 继续使用已成功的 ProducedTask；已知时间域由写作用例在请求前验证。

## FrozenLanguageTask

冻结材料使用现有 Artifact/CAS 保存 JSON，包含：任务名称、当前任务合同描述、输入/输出 schema 与 Prompt 执行签名、Prompt manifest、system/user/assistant 实际模板文本及依赖 include/import 内容、顺序路线与模型资源定义、preset/参数、输出方式、任务预算和媒体准备政策。

只保存模板文件名或 config_version 不足以在重启后重现已接受任务。模板随实例覆盖变更后，既有 Job 继续用被冻结内容。freeze 不执行语言请求；可以在提交前准备，正文 revision 在事务里确定后放入 Job payload。

不得在冻结对象里保存原始密钥、运行中的 SDK 对象或全局 Runtime。凭据仍使用正式 credential_ref；当前凭据轮换可以生效。load_language_task 逐一核对当前完整 ModelConfig 与冻结定义，并核对 provider 的 endpoint、protocol、credential_ref；任一不一致返回 model.resource_binding。任务合同描述变化返回 model.task_contract_changed。Embedding 空间由其正式 owner 按维护配方另行核验。

恢复使用当前 Runtime 的连接、文件与真实执行资源。任务请求参数、模型路线、preset、媒体准备政策及依赖转写任务采用冻结值；当前安装的输入/输出、尝试与期限上限继续约束执行。不要将旧 Snapshot 设置为当前全局配置，也不要让每个 owner 自己重新解析 TOML。

## PublishedComputation

```python
class PublishedComputation(ContentValue):
    descriptor: ArtifactRef
    producer_key: str

# descriptor JSON 中的核心内容：
# operation, producer, actual_inputs, request,
# outputs[name] -> {blob/artifact ref, media_type, display_name},
# parameters, coverage, completed_stage_refs, frozen_task_ref
```

成功输出在 CAS 发布，并形成可以独立读取的描述；正文/响应不塞入 DBOS 步骤表。**描述及其稳定 `producer_key` 必须有可持久查询的关联**，使用既有 ResultStore 的 adopted processing result/有名输出以及共同内容引用机制扩展，不能只把 JSON 文件放进 CAS 后期待重启时扫描目录找回。描述保持 actual_inputs 的 slot、role、顺序、origins、精确选区以及真实请求路线；不得用最初计划覆盖实际备用路线消耗的输入。

GenerationOperations 的 `freeze_language_task(task, snapshot)` 发布 FrozenLanguageTask，`load_language_task(ref, snapshot)` 在当前资源绑定下恢复 FrozenTask。`produce`/`produce_prepared` 接受稳定 producer_key 与 frozen_task_ref；协议成功响应采用为 `model.language_response`，业务键为 `<producer_key>:response`。

`publish_computation(produced, producer_key, frozen_task_ref=...)` / `load_computation(ref)` 把 ProducedTask 与 PublishedComputation 互转，`model.published_computation` 的有名输出同时保留 descriptor 及所有实际输出字节。`adopt_produced(..., producer_key=...)` 在正式采用处复用同一唯一键合同。转换仅发生在持久边界，普通进程内调用仍用类型化对象。持久步骤返回 descriptor 的 JSON 引用。

ModelExecutor 在完整响应、JSON Schema 与 TaskDefinition.validate 均通过后返回 GenerationResult。validate 继续参与既有结构化修复预算；GenerationOperations 在此后、TaskDefinition.outputs 之前正式采用 model.language_response。其 response 有名输出保存原始协议响应，computation 有名输出保存已验证输出、实际 ReadingInput、请求/模型信息与 coverage；输入组保留 slot、role、origins 和精确选区。

outputs 失败的错误包含 response_result_ref 与 `<producer_key>:response`。调用 `ResultStore.find_producer(key)` 或 `get(response_ref)` 可取得正式结果，`read_view(response_ref, details=True)` 展开原始响应。恢复解释有两个入口：以原 producer_key 重入 produce/produce_prepared，或调用 `GenerationOperations.recover_response(response_ref, snapshot)`。两者读取已保存响应，再调用同一 outputs owner，不发送新的模型请求。已保存 frozen_task_ref 时恢复会加载冻结配方并核对当前资源绑定；普通非持久调用的响应采用当前同合同任务，输出 schema 改变时明确拒绝。解释、链接规范化与后续采用各自失败均保留已完成引用。

外部请求已经返回但原始响应还没有首次被本地采用时，崩溃仍可能重新调用，这是实际最窄的不可避免窗口。持久化原始响应后，下游任何失败都不能因没有 DBOS checkpoint 而静默重发模型。调用前的时间域、来源、任务与配置约束由写作用例预先校验；出错时不消耗一次已经成功的模型计算。

## 采用幂等

ResultStore.adopt/adopt_in 的正式采用接受可选 producer_key，数据库唯一约束负责同一业务产物只采用一次。键由持久 workflow 和阶段身份给出，不使用临时 operation_id。重复采用前核对操作、producer、实际有序输入、输出 blob/类型/名称、request、parameters 和 coverage 的采用签名；不一致返回 content.producer_conflict。

已采用结果返回相同 ResultRef。DraftStore 的 result_id 唯一关联继续复用；Annotation adoption_key 继续复用；业务 revision 采用仍核验固定基准。CAS 写入可以重试，已发布相同字节复用，不让 SQL 重试重新调用模型。

对外返回只带正式引用、可读错误和恢复入口，不泄漏庞大内部 DTO。持久恢复操作读取明确 descriptor，复用原 ResultStore/领域采用能力，不接受任意 Python 反序列化。

## DBOS 步骤边界

准备、语言计算、采用分为明确步骤。计算步骤默认 retries_allowed=False；既有模型执行器的 max_attempts/备用路线/结构化重试已组成一次预算。SQL 采用和可重建的确定性步骤允许按错误分类有限重试。

DBOS 已记录的成功步骤不会再次调用模型；业务结果已经提交但步骤 checkpoint 尚未完成时，通过 producer_key 返回原结果。外部模型已成功而进程在任何本地持久记录前退出的窗口仍可能重发，不能以 DBOS 事务保证宣传提供方收费 exactly-once。这个事实只影响该故障窗口，不扩大为普遍重复调用。

已发布描述在 DBOS 清理历史之前必须被正式结果引用或保留至工作/恢复窗口结束；CAS GC 不得把正在恢复的描述当无主临时文件删除。本轮沿现有内容 owner/slot 引用建立保留关系，不创建另一种文件存储。

## 迁移位置

`models/operations.py`、`models/executor.py`、`models/prompts.py`、`content/results.py`、`content/schema.py`、`cognition/writing/{generation,drafts,tasks}.py`、认知标注任务和 Job workflow。ConceptAnnotationTask 位于 cognition/annotation_tasks.py，通用 TaskCatalog 由 runtime/task_catalog.py 显式装配。

## 完成行为

未知时间域在发送前失败；Prompt 更新不改变已排队任务；成功计算后的链接/来源/采用错误有真实可读恢复引用；恢复后不改变原输入来源或重复正式结果；凭据内容不进入冻结产物；当前新 head 不被旧草稿/旧标注覆盖。
