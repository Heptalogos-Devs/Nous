# DBOS 3.4 与显式宿主

## 采用范围

采用 `dbos==3.4.0`，更新 pyproject/锁文件，不自动升级到不同 API 的版本。使用公开 DBOS、DBOSClient、DBOSConfiguredInstance、workflow/step、队列、portable JSON 和事务入队。DBOS 负责持久工作及步骤记录；认知与内容 owner 继续负责正式状态和结果。无需本轮改成 Datasource ORM、独立 PostgreSQL 或新任务平台。

持久宿主在自身应用循环启动 DBOS，异步工作流和业务阶段共享该循环。

## 宿主与注册

`run_host(locator, application)` 是同步入口，使用 asyncio.Runner 建立一个循环、打开 Runtime，并在运行中的该循环同步调用 DBOS.launch。普通 `open_instance` 不注册工作流、不启动 DBOS，也不改变调用者事件循环的默认线程池。

一个进程只运行一个 Nous 持久宿主。启动前检查 DBOS 单例和已有注册，发现外部所有权时返回 `job.host_in_use`，不销毁外部实例。3.4 的只读所有权检查集中在 jobs/runtime；宿主正常关闭时清理自己创建的注册。

`cognition/maintenance/durable.py` 在宿主启动时注册 `nous.annotation` 与 `nous.refresh`，并以稳定实例身份构造 `NousMaintenance` 配置实例。业务依赖在注册前完整构造。队列 `nous-annotation` 和 `nous-refresh` 使用 DBOSClient.register_queue，容量来自 jobs 配置。导入普通 Runtime 或 Job 客户端不会执行工作流装饰器。

持久载荷只包含 JSON 值、业务计划与正式引用，不携带 Runtime、连接、Task、客户端或 Native 对象。业务阶段直接使用所属 Runtime 的服务。

## SQLite 与事务

DBOS 系统表与应用表使用实例已经绑定的同一 SQLite 物理文件。数据库路径、URI escaping、busy timeout、外键和 synchronous 策略均由 Database 的共同能力提供；不能由 jobs 自读环境拼第二条路径。

应用 AsyncEngine 保留 SQLAlchemy Core 的 ReadTx/WriteTx，写操作在任何依据读取之前 BEGIN IMMEDIATE。采用 SQLAlchemy 的显式事务控制方案时，将 sqlite3/aiosqlite 的隐式 BEGIN 关闭，再由 begin hook 按读/写模式发送 BEGIN 或 BEGIN IMMEDIATE。必须删除旧 autocommit=False 工厂与新 BEGIN 混用；PRAGMA 在事务外设置。writer_lock 可以用于本应用公平串行，但 SQLite 的实际写锁负责与 DBOS 的独立连接协调。

DBOS 使用同步 Engine，不能把 AsyncEngine.sync_engine 当普通同步连接跨线程使用。同步引擎由 storage 层工厂按同一路径和配置建立，连接政策兼容 DBOS 的 SQLite 用法；SDK 的迁移通过其公开 DBOS.migrate 在业务事务外完成。若传 system_database_engine，所有者负责最终 dispose，SDK 和 client 关闭时不能提前销毁共享资源。

默认新实例初始化时在已有实例锁下创建应用表并运行 DBOS 官方迁移。正常 open 只检查已初始化状态，`run_migrations=False`；业务表或必需列缺失时提示在新目录初始化，业务 schema 完整而 DBOS 系统表缺失时提示运行初始化。对已有实例执行初始化时，先验证业务表及列，再运行 DBOS 官方迁移。DBOS 自己维护系统 schema 版本，Nous 不复制迁移 SQL。

**共享数据库中禁止调用 DBOS.reset_system_database 或 truncate 清理。** 3.4 的 SQLite reset 路径可能删除整个文件或遍历应用表。需要测试清空时只使用明确可丢弃的新实例目录，不把该接口接入产品命令。

## 同事务登记

```python
async with database.write() as tx:
    committed = await records.create_in(tx, ...)
    job = await jobs.enqueue_in(
        tx, kind="annotation", workflow_id=stable_job_id, payload=payload
    )
# 只有提交后，job 才可作为已登记工作返回或等待。
```

这段 run_sync 与当前 async transaction 使用同一数据库连接，不是把 write 丢到另一个线程。lambda 内只调用事务入队，不进行文件、模型、独立连接查询或等待 handle。入队失败使本次业务事务回滚；不在 except 中吞错然后宣称后台会补。

不要使用 queue 的 `deduplication_id + duplication_policy="return-existing"` 来实现事务内合并，3.4 明确不支持该组合。`workflow_id_reuse_policy` 是另一项语义；本轮仅对同一确定业务事件使用相同 workflow_id，不能用同一个 ID 表达不同参数。

## 业务幂等与调度重复

标注计算键在创建请求时确定；刷新工作键由实际写入或标注采用及代次确定。一次恢复复用原计算和工作身份。手工明确重新维护创建新的请求身份，保留此前工作结果。

恢复步骤可能重复进入一个已经提交成功的业务方法，因此 ResultStore 和正式采用按工作/步骤业务键查找同一结果。键相同、内容合同不同报冲突，不静默返回别人的结果。Job ID 不能替代文稿 revision identity。

## JSON 与部署

DBOS runtime 和 client 使用一致的 DBOSPortableJSONSerializer；持久载荷、步骤返回与结果在边界转为 JSON 和正式引用。正文、模型输出与模板存于内容存储。

application version 由 DBOS 的部署机制确定，executor_id 根据实例身份稳定派生；项目不维护内部工作流版本链。开发期更换内部结构时使用明确选择的新实例。

## 恢复、失败重试和工作身份

区分崩溃后自动恢复、用户取消后的继续以及业务失败后的重试：

- DBOS 按已注册名字/应用版本恢复未完成的工作；重复执行必须经过业务采用幂等校验。进程退出不是用户 cancel，不能在关闭时把所有持久工作写成 CANCELLED。
- `DBOSClient.resume_workflow(..., queue_name=原队列)` 只用于允许恢复的取消/超出恢复次数状态；明确传队列可保留原队列并发控制，避免立即执行绕过限制。
- `ERROR` 的步骤失败由 Jobs owner 使用公开 fork 从实际失败步骤继续，复用已完成步骤；重试身份和重复请求规则见 [Job 操作](../interfaces/jobs-and-host.md)。
- 若旧工作本身已与新修订不兼容，报告 superseded 或明确要求新的维护请求，不能通过修改冻结 payload 偷偷重试。

用户 `wait` 是对已提交工作状态的读取，不进入工作执行的并发名额；`cancel` 的 DBOS 状态变更和 Runtime 业务任务实际结束分别报告。资源实际结束由 ExecutionManager 跟踪，包括工作流 Task 的最后检查点。

## 启动与关闭

启动依次取得实例锁并核验资源、构造业务能力和 Job 客户端、检查框架所有权、注册已绑定工作流，在应用循环 launch，随后运行应用协程。

关闭时停止新业务接纳，按宽限期等待或取消已接纳任务，直至子进程、Native 和托管线程实际结束。随后通过 ExecutionManager 的独立线程池调用 DBOS.destroy，应用循环继续运行以完成 SDK 收尾；最后关闭客户端、同步引擎、业务资源与实例锁。用户取消与宿主停止分别使用 DBOS 取消和可恢复中断语义。

## 限制与错误处理

SQLite 不支持 DBOS attributes containment 过滤；Job 列表使用公开可用过滤与有界分页，不直接查询系统私有表假装完整能力。控制客户端关闭无限连接错误重试，连接错误由共同执行期限与错误合同处理。

本轮不要求 Conductor、Kafka、Cron 或云账户。业务日志按实例归属，SDK 不额外启用 OTLP 导出；启停不重设主应用的全局 OTel provider。恢复日志保留 job_id 与步骤，正文/凭据不默认输出。
