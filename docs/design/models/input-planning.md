# 一次语言任务的输入规划

## 语义单位

TaskDefinition 声明输入方式 `joint` 或 `independent_batches`。普通 summarize、extract-information、select-passages、custom-reading 缺省为 joint：输入数组共同回答同一个问题。两个小文件的比较必须让同一次请求看到两份文件。

`independent_batches` 仅用于调用者明确接受各批独立答案的操作，例如逐段转写、逐时段描述。MediaAnalysis 的返回同时带总覆盖和分批结果。CLI 可提供 `--per-batch` 显式选择；没有选择时不能把一项比较静默改成多次互不知情的回答。

一次 joint 请求放不下已知硬限制时，在模型调用前返回具体输入范围、限制和可用做法。只有任务自己定义了有语义的分解/汇总策略时才能执行它；本轮不为任意问题自动建立 MapReduce、Agent 工作流或默默截断原文。

## 共同值与操作

```text
InputPlan:
  task_mode
  ordered_source_groups
  representation_plans
  request_groups[]  # 每组引用实际共同参与的一组来源/范围
  requested_coverage

plan_inputs(inputs, frozen_task, settings, options) -> InputPlan
materialize(request_group, route, context) -> scoped ReadingInput
compile(reading, frozen_resource, output_contract) -> CompiledPrompt
```

InputPlan 由 `models/input.py` 的完整规划能力拥有，GenerationOperations 使用它；MediaAnalysis 显式组织语言阶段并复用它。文件接纳走 ArtifactStore.accept，媒体表示走 MediaPreparer，正文分块走 TextView。CLI 不自己遍历全部批次拼 Prompt。

逻辑输入组保持用户输入顺序。固定局部键在计划形成后唯一分配，预览和实际执行一致；物化不重新编号。group 保存片段关系，不把一份 PDF 页图和文字当成两份独立来源。

## 表示选择与两阶段任务

每条实际路线独立取得可用的确定性表示。首选模型接受原生 PDF 时可以保留 PDF，备用模型需要页图时按同一原件与选区准备；参数记录本次实际采用的表示。格式转换由 MediaPreparer 完成，执行器不自建媒体适配器。

描述与结构化可以使用不同模型，各自从原件规划表示。第二阶段显式附加已保存的描述；不能直接假定第一阶段的 PNG/WAV 对第二个模型也合法。额外转写是已规划的语言阶段，不属于 fallback 的隐式格式转换。

preview 不发语言请求。依赖转写但尚无转写产物时，展示“需要转写”及实际缺项，不能以空文本假装可执行。task preview、run 和 show-request 使用同一输入计划与编译路径。

## 有界工作

计划只保存引用、格式、选区、转换参数和范围。MediaPreparer 的 batch session 在需要下一组时才转换；当前请求完成后释放其字节。joint 任务可以依次物化组成当前请求的若干来源，但物化总量仍受请求真实字节/尺寸限制。

Root deadline 包含接纳、排队、转换、语言请求和采用。成功转换和各成功语言阶段已采用的引用保留。任一失败返回这些引用和未完成范围；保存失败发生在请求尝试之外。

## 完成场景

A 写“周五发布”，B 写“已改到周一”，对二者的比较在同一次真实请求中出现；只返回两个独立摘要不算完成。三个互不相关的音频时段在明确 per-batch 下产生三份可读结果；第二批失败仍能读取第一批。纯预览与实际 show-request 能解释同一组来源、实际表示和模板版本。
