# 候选评价和重排

`models.evaluate` 接收问题、候选键及文本、评价标准、用途和统一尺度。尺度由调用者声明分类标签或数值上下限；模型返回候选键和值，程序检查这些值并为所有条目附上同一个尺度。结果保留实际输入、模型和用量。多个批次分别保留各自的实际键、模型、耗时和用量；未返回的候选保持未评价。

`models.rank` 使用相同候选合同。配置为生成任务时按任务 Schema 返回顺序；配置为 `operation = "rerank"` 时，显式声明模型的 `rerank` 能力和 `http-rerank` 连接协议，发送问题和候选文本，按返回的序号映射到原键。协议分数保留为 `rerank_relevance_score`，不同请求或服务的分数需要各自解释。

专用端点使用运行时现有 HTTP 连接、凭据、期限和尝试记录。单次排序必须处于 `evaluation.max_candidates` 和 `evaluation.max_characters` 以内，超过时返回输入限制错误；评价可以按这些上限分批。配置示例见 [models.toml](../../examples/models.toml)。
