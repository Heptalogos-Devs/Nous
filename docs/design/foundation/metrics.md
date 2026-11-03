# 指标与运行状态

## 责任

指标表达随时间累积的运行量和耗时。可用状态由实际资源 owner 提供，管理入口读该状态；指标后端不可用时仍能查看实例。

采用 OpenTelemetry Metrics API/SDK。首轮实现内存仪表与可选 OTLP 导出，不自建时序数据库、告警平台或仪表盘。

## 首批指标

operation.count 和 operation.duration 记录操作名、结果分类。runtime.in_flight、execution.queue_wait 记录受管理的当前工作。database.transaction.duration 和 database.error.count 记录实际事务与失败类别。后续模型 owner 添加请求数、延迟、实际 token 和估计费用。

指标 label 使用有限集合，例如组件、操作、结果和已配置资源 ID。用户问题、来源路径、对象引用、operation_id、trace_id 进入日志或 Trace，不作为无限增长的指标 label。

provider 未给出的 token/费用保持未知，不填 0。模型费用估计带价格来源和时间，不描述成提供方账单。

## 配置与启停

指标启停、导出端点和间隔从正式配置读取。SDK 在启动装配时构造，最后关闭。关闭导出后本地状态仍可用，不需要网络才能报告进程是否就绪。

每项指标在实际操作位置维护，不能只注册名称而永远不产生数据。首轮完成配置与数据库路径上的少量指标即可；未实现的模型和索引不显示虚假零值健康。

## 验证

完成一次 runtime check 后能观察实际操作量和耗时；禁用遥测不会改变结果；失败正确增加对应分类；本地状态不依赖遥测连接。来源：[OpenTelemetry Python instrumentation](https://opentelemetry.io/docs/languages/python/instrumentation/)。
