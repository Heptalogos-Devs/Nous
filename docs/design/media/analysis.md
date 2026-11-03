# 媒体解释与派生操作

## 一次完整操作

`MediaAnalysis.analyze(input, strategy, options, context) -> AnalysisResult` 接纳原件或解析已有 ArtifactRef，选择实际范围，准备媒体，调用语言阶段，提交结果并交付可读内容。

它依赖 ArtifactStore、MediaPreparer、GenerationOperations 和 ResultStore。它不拿走 Database 引擎，也不创建 Provider 客户端。GenerationOperations 是通用的“准备明确语言任务—执行—采用结果”用例；普通 model run 也使用它。

## 三种策略

**description_only**：准备实际输入 → 执行 describe-media → 保存描述 Artifact 和结果。

**direct_structured**：准备实际输入 → 执行 extract-information 或用户明确合同 → 保存已校验结构化结果。

**describe_then_structure**：准备实际输入 → 执行并提交描述 → 将同一准备输入与已保存描述一起交给结构化任务 → 保存最终结果，并关联描述结果。

已有描述通过 `--description-result res:…` 显式提供。程序检查原件、选区、实际表示与覆盖是否匹配当前请求；匹配后复用。用于图像/PDF/视频的第二阶段仍带实际可用媒体表示；需要转写的音频则明确使用实际转写及来源，不能拿某个“描述”字符串替代缺失能力。

## 批次和结果集合

原件过大时按准备专题分批。每批各自完成策略并返回 ProcessingResult，顶层 AnalysisResult 含原件、范围、按顺序的 batch results、已完成描述及 overall outcome。

默认交付有页/时段标识的结果集合，不在技术上限之外偷偷拼接全部媒体为一个请求。用户需要再做跨批综合时，可把已保存文本结果交给普通 summarize 任务；这不是完成每个批次的强制第二套审查。

## 部分完成

成功阶段立刻采用。后续失败或取消，保持已提交结果，并在错误信息/JSON 中返回 `completed_result_refs`、失败阶段和未完成范围；CLI 返回非零。下一次可以显式读取/复用已经成功的描述或转换结果。

没有持久 running 记录、自动恢复承诺或重复业务队列。原件保存成功而解释失败时，原件地址继续有效。数据库采用失败在保存边界解决，不自动重跑模型。

## 原文选段与预实验

同一 GenerationOperations 可以运行 summarize、extract-information、select-passages。这用于观察当前 Prompt/模型/来源结构的实际效果。select-passages 的输出由程序回取原文并保留精确段落映射。

本轮不存在“自动把摘要作为 Journal”或“把提取项写成 Memory”的隐式行为。后续认知领域消费这些已经完整的基础能力，再定义其形成与采用机制。

## 返回结构

默认展示用户请求、实际覆盖、正文、短结果地址、可展开原件/描述位置和必要的实际模型。结构化模式提供同一信息的类型化结果。

内部转换参数、所有 attempts、Schema 和完整请求通过 result read 的详情展开，不一开始把全部技术字段填进模型工具正文。
