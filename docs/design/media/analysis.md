# 媒体语言阶段与部分成果

## 完整操作

MediaAnalysis 接收实际输入、策略、问题和选区，建立一次 Execution 根操作。它冻结描述与结构化任务，使用 GenerationOperations.prepare_input 取得共同 RequestGroup，然后按组组织语言阶段。

构造依赖是内容 owner、确定性 MediaPreparer、GenerationOperations 与 Execution。确定性准备器无模型依赖；MediaAnalysis 不创建 Provider 客户端或自己的路线准备器。

## 三种策略

- description_only：确定性表示 → 描述任务 → 采用描述。
- direct_structured：确定性表示 → 指定结构化任务 → 采用结果。
- describe_then_structure：描述阶段成功即采用；后续阶段同时取得其需要的原媒体表示与已保存描述，采用最终结果。

每个阶段调用共同 run_group。所选路线不支持音频且已配置转写时，prepare_group 才执行转写；原生主路线成功时不会触发文本备用的转写。原生音频、转写文字与媒体描述的表示身份分别记录，preview 不发语言请求。

## 不同模型的输入能力

按阶段选择实际表示。描述阶段可以用原生 PDF，结构化阶段可以用同页图和文本；它们都绑定同一原件和页范围。没有必要把全部阶段和全部备用模型格式取交集。无法保持请求内容时明确报告该阶段能力不足，不能悄悄只发描述。

两阶段共享配置和模板快照、根 deadline、实际覆盖；各自的模型尝试额度由任务定义。成功转写、描述和结构化都有独立 producer 与输入 part 映射。

## 复用与缓存

`description_result` 必须匹配请求的原件/选区、描述任务/合同、实际表示与必要参数。允许当前最终阶段采用另一种确定性表示，只要来源和覆盖等价；核对由共同来源能力完成，不能仅比较整个 DTO 是否字面相同。

确定性转换可以自动 exact 复用；语言结果仅显式复用。重用既有描述后不再次触发描述模型。

## 逐批提交

一次只物化有界批次。每个成功阶段通过统一 ResultStore 采用，向上累计完成 refs。失败返回已完成 refs、failed_stage 和 unfinished_coverage，保留根取消语义。原件已接纳也返回其标识符。

结果读回通过 ResultStore 的公开 view/provenance；CLI 不查询私有关系表。数据库失败不重做模型请求。本轮没有后台恢复承诺；后续 DBOS 将使用明确的批次业务键复用已采用结果。
