# 认知文稿的所有权与基础接入

cognition 管理 Note/Journal 身份、版本、来源、概念绑定、反馈和知识范围；content 管理实际字节、正文结构、处理结果和生成血缘；models 管理语言任务与请求；Wave 管理可重建检索资产；Native 负责数值计算。程序在正式采用时作决定，语言模型提供文字或结构化提案。

Note 和 Journal 共用有限的 WrittenRecords 保存机制，保持各自的产品语义和召回规则。`writing/records.py`、`reading.py` 负责精确写入和读取，`writing/operations.py` 完成跨 owner 的提交与后续维护；`writing/generation.py`、`drafts.py` 和 `tasks.py` 负责写作产品与草稿。`annotation/` 拥有提案与采用，`retrieval/` 拥有正式视图捕获、投影和查询，`feedback.py` 保存实际交付与意见，`scope.py` 和 `basis.py` 负责域身份、代次和依据变化。

完整操作通过 Application.call 使用 CallContext 与同一配置快照。模型、媒体转换和检索构建在事务外完成；跨 owner 采用使用已有 WriteTx。内容 bytes 先发布，再与结果、来源或正式修订关系一起提交。

GenerationOperations.compute 完成当前语言调用和任务解释，返回 `CompletedTask(facts, outputs)`。ResultFacts 包含原始选择、实际输入、请求、生产者、覆盖及映射；ResultStore 负责共同成果采用。写作和标注将各自的领域上下文与模型成果一起保存，再执行草稿组装或标注采用。

普通维护在正文提交后运行。标注提案固定精确正文与候选；采用时核验当前正文和活动标注，过时提案继续可读。刷新捕获正式视图、复用不变投影并发布完整快照。详见[维护架构](maintenance-architecture.md)、[来源解析](source-resolution.md)和[当前检索](current-retrieval.md)。
