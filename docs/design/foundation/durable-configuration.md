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

`workers` 是本次打开 Runtime 的明确运行选择，CLI runtime run/job run 与 SDK workers=True 启用；不因执行一次 status/create 自动启动永久 worker。无需新增通用服务管理平台。

SQLite 文件路径、busy_timeout、synchronous、日志级别、shutdown_grace、凭据和网络继续使用现有字段，不在 jobs 下复制另一套同义配置。DBOS application_name/executor_id 根据稳定实例身份派生，workflow_contract_version 来自本轮代码合同，不给普通用户一个可随意改动导致工作失联的文本配置。

DBOS 连接不读取 DBOS_SYSTEM_DATABASE_URL 来覆盖正式实例配置；应用明确传 system_database_url/engine。关闭重复 SDK OTLP/控制台 exporter，默认日志通过既有会话。需要独立导出时沿现有观测配置接入，不添加第二套 OTel 私有全局状态。

自动恢复最多尝试 20 次，这是静态 workflow 合同的一部分。DBOS 3.4 在注册时固定该值，由合同变更管理。

超时字段为现实单调时钟/SDK 持久 wall-time deadline 的运行政策，与 occurred/observed/formed/valid/recorded 无关。排队时间与执行时间分别显示，不能把 SDK start-to-completion 当包含所有排队时间的保证。

CLI `config explain` 应能说明每项当前值、来源和生效方式。新 field 的验证与快照冻结复用共同配置能力；运行中不能在每次读取队列属性时重建全配置。
