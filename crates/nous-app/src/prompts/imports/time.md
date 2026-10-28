提取原文明确表达的事件时间。只使用提供的 reference_time 解析相对时间；缺少可靠参考时，相对时间字段返回 null。绝不以机器当前时间补足参考。
时间点填 event_time，时间段填 event_time_start/event_time_end 或两项 time_range。保留可确定的日期、时区与粒度，不补造缺失的时区或日期。
没有可靠时间时，时间及粒度字段返回 null。time_confidence 为 0 到 1 的已知置信度或 null。按指定 schema 只输出 JSON。
