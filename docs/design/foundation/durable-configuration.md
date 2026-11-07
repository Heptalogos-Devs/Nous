# 持久维护配置与生效规则

字段由所属模块通过 ConfigModel/setting 注册，Schema、目录、解释和 CLI 说明从定义生成。用户可按实际负载调整队列与执行资源。

| 字段 | 初值 | 生效方式 | 含义 |
|---|---:|---|---|
| `jobs.annotation_concurrency` | 2 | next worker start | 标注队列工作并发上限 |
| `jobs.refresh_concurrency` | 1 | next worker start | 刷新队列工作并发上限 |
| `jobs.poll_interval_seconds` | 1.0 | next worker start | SDK 队列轮询间隔，正有限数 |
| `jobs.max_executor_threads` | 8 | restart | DBOS 自有执行池上限 |
| `jobs.database_pool_size` | 4 | restart | DBOS 同步连接池上限 |
| `jobs.workflow_timeout_seconds` | null | next enqueue | 可选持久 start-to-completion 期限 |
| `cognition.maintenance.execution` | inline | next operation | AfterWrite.default 的执行方式 |

队列并发与 Runtime.execution.max_in_flight 是不同资源层次；业务阶段仍要进入共同执行准入。Job 等待不占该业务槽。配置更新不在同一 patch 中偷偷增大模型尝试预算或改变向量空间。

CLI `runtime run`、`job run --until-idle` 与 SDK `run_host` 显式启动持久宿主；普通打开、状态查询和保存可以登记工作，但不启动工作者。

SQLite 文件路径、busy_timeout、synchronous、日志级别、shutdown_grace、凭据和网络继续使用现有字段。DBOS application_name/executor_id 根据稳定实例身份派生，应用版本由 SDK 的部署机制处理。

DBOS 连接不读取 DBOS_SYSTEM_DATABASE_URL 来覆盖正式实例配置；应用明确传 system_database_engine。持久宿主不额外启用 SDK OTLP 导出，业务日志使用实例会话。

自动恢复最多尝试 20 次，这是静态 workflow 合同的一部分。DBOS 3.4 在注册时固定该值，由合同变更管理。

超时字段为现实单调时钟/SDK 持久 wall-time deadline 的运行政策，与 occurred/observed/formed/valid/recorded 无关。排队时间与执行时间分别显示，不能把 SDK start-to-completion 当包含所有排队时间的保证。

CLI `config explain` 应能说明每项当前值、来源和生效方式。新 field 的验证与快照冻结复用共同配置能力；运行中不能在每次读取队列属性时重建全配置。
