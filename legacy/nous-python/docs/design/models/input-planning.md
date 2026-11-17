# 一次语言任务的输入规划

## 语义单位

TaskDefinition 声明输入方式 `joint` 或 `independent`。普通 summarize、extract-information、select-passages、custom-reading 缺省为 joint：输入数组共同回答同一个问题。两个小文件的比较必须让同一次请求看到两份文件。

`independent` 用于调用者明确选择的语义单元，例如离散页或时段。一个语义单元可能需要多个物理表示批次，它们共同进入同一次请求。MediaAnalysis 返回总覆盖和各单元结果；没有选择独立模式时不会把一项比较拆成互不知情的回答。

输入 owner 先从原始选择及任务声明的领域范围形成 SemanticUnit，再为每个单元选择路线和物理表示。默认每个所选范围保持完整，任务可声明领域分段，调用者显式范围优先。独立单元分别处理所选材料，`context` 槽随每个单元保留；媒体操作的独立范围也使用同一规则验证和分组。

一次 joint 请求放不下已知硬限制时，在模型调用前返回具体输入范围、限制和可用做法。只有任务自己定义了有语义的分解/汇总策略时才能执行它；本轮不为任意问题自动建立 MapReduce、Agent 工作流或默默截断原文。

## 共同值与操作

```text
InputPlan:
  task_mode
  selected_inputs  # 原件、离散选区、逻辑槽、用途和已确认的 SourceFacts
  representation_plans
  route_plans[]    # 每项持有原始 SemanticUnit 与该路线的物理表示
  requested_coverage

plan_inputs(inputs, frozen_task, settings, options) -> InputPlan
semantic_units(selected_inputs, mode, ranges) -> SemanticUnit[]
for_route(route_plan, accepted_formats, settings) -> RoutePlan
materialize(route_plan, limit) -> scoped PreparedBatch
compile(reading, frozen_resource, output_contract) -> CompiledPrompt
```

InputPlan 由 `models/input.py` 的完整规划能力拥有，GenerationOperations 使用它；MediaAnalysis 显式组织语言阶段并复用它。外部输入由 InputReader 接纳，媒体表示走 MediaPreparer，正文选区走 ArtifactStore 的共同范围能力；跨内容/媒体的检索批量准备由 application.preparation.TextPreparation 组织。

TaskDefinition.text_input_mode 区分 lossless 与 structured。全文 rewrite 和 edit_target 采用 lossless；定义行、CRLF、间隙与尾部随实际输入保留。structured 任务可以附带块键与标题路径，其 TextView 仍持有完整所选原文。

SelectedInputs 按逻辑槽保留原件、用户顺序中的离散 ContentSelection、role 和显示名称，SourceFacts 分别保存声明 MIME 与探测到的类型和已知形状。模型可见的名称进入请求身份；每一物理批次保留原始选择与该批范围，切换路线从这些值重新规划。

逻辑输入组保持用户输入顺序。固定局部键在计划形成后唯一分配，预览和实际执行一致；物化不重新编号。group 保存片段关系，不把一份 PDF 页图和文字当成两份独立来源。

媒体准备保留已知的原件范围与每个实际表示对应的选区；来源用途来自输入组的明确角色。共同编译在 user 消息的对应来源旁说明这些事实，例如源音视频的已知时长、实际提供的时段或采样画面，以及 PDF 页码。未取得的信息不作推断。转写、转换和备用路线沿用实际原件及选区，系统提示词保持独立于本次材料；请求预览和执行使用同一说明。

## 表示选择与两阶段任务

首选路线直接物化已形成的计划；切换路线时 `GenerationPreparation.prepare_group` 使用 input owner 的 `for_route`，按同一语义组的原件与选区重新规划。原生 PDF、原生视频、页图、采样视频帧、音频与转写文字可以随路线改变；格式转换由 MediaPreparer 完成。

输入含视频时，先按已声明的能力和 MIME 选择支持原生视频的路线，并保留这些路线之间的配置顺序；其余路线仍可在需要时使用。路线能否使用以实际准备和协议校验为准，不根据模型名称猜测能力。

选定路线成功时不准备其他路线的媒体或转写。ModelExecutor 只有在需要切换时才回调同一个 prepare_group；已知格式、能力或额度不足会跳过该路线并保留原因。文本主模型无法消费图片或缺少可用转写时，仍能选择有相应原生能力的备用模型。未选择的路线保持 deferred 状态。

描述与结构化分别调用共同生成操作，消费保存的原始分批选区。第二阶段从原件规划本路线表示，并显式附加已采用的 DESCRIPTION Artifact。配置的转写是所选路线需要时执行的语言前置阶段，结果独立采用；备用路线尚未选择时不运行它。

preview 不发语言请求。所选路线依赖转写时返回 requires_transcription、路线与任务配置；可确定执行时展示一个实际准备值，后继路线列为 deferred_routes。task preview、run、probe 和媒体阶段共用同一选择与准备操作。

## 有界工作

计划只保存引用、格式、选区、转换参数和范围。MediaPreparer 的 batch session 在需要下一组时才转换；当前请求完成后释放其字节。joint 任务可以依次物化组成当前请求的若干来源，但物化总量仍受请求真实字节/尺寸限制。

Root deadline 包含接纳、排队、转换、语言请求和采用。成功转换和各成功语言阶段已采用的引用保留。任一失败返回这些引用和未完成范围；保存失败发生在请求尝试之外。

## 完成场景

A 写“周五发布”，B 写“已改到周一”，对二者的比较在同一次真实请求中出现；只返回两个独立摘要不算完成。三个互不相关的音频时段在明确 per-batch 下产生三份可读结果；第二批失败仍能读取第一批。纯预览与实际 show-request 能解释同一组来源、实际表示和模板版本。

## 原生视频

路线明确接收视频 MIME 时，MediaPreparer 优先准备原生视频：可直接发送的完整原件保留原 Artifact；时间选区或排除音轨时用 FFmpeg 生成 MP4，并把原始 start_ms/end_ms、实际音轨策略及转换引用一起保存。选区产物是原视频的一次表示，不增加独立来源。

coverage 使用 native_video 和 sampling=provider；实际发送的是一个视频文件，不登记为本地选定的若干帧。上游如何采样以实际服务合同为准。当前本地文件使用有界 data URI；超出 max_inline_bytes 返回范围建议，尚未接入远端 Files 上传/URL托管。普通 route 只接受图片时仍按现有有界抽帧规划处理；fallback 选中新路线后按同一原件和选区重规划。
