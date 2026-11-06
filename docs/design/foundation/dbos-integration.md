# DBOS 3.4 与既有 Runtime 的接入

## 采用范围

采用 `dbos==3.4.0`，更新 pyproject/锁文件，不自动升级到不同 API 的版本。使用公开 DBOS、DBOSClient、DBOSConfiguredInstance、workflow/step、队列、portable JSON 和事务入队。DBOS 负责持久工作及步骤记录；认知与内容 owner 继续负责正式状态和结果。无需本轮改成 Datasource ORM、独立 PostgreSQL 或新任务平台。

DBOS 调度循环与 Nous 业务循环隔离。源码依据及接口细节见[研究记录](../../research/dbos-3.4-integration-notes.md)。

## 一个进程、一个实例宿主、两个循环

Nous Runtime 持有实例锁、业务 asyncio loop、数据库 AsyncEngine、模型 HTTP 客户端、Native Engine、ExecutionManager 和领域服务。DBOS 单例及其同步数据库连接由 jobs/runtime 持有，其异步 workflow 在 DBOS 自己的后台 loop 运行。

启动前检查进程内是否已有 DBOS 单例；已有实例由原宿主继续持有，Nous 报告 `job.host_in_use`。DBOS 3.4 没有公开的单例存在性查询，必要的检查集中在 jobs/runtime。关闭只销毁该 Runtime 实际创建并仍持有的单例。

从没有运行中 asyncio loop 的同步启动上下文调用 DBOS.launch，例如在 `asyncio.to_thread` 的启动函数中调用。这样 3.4 的 BackgroundEventLoop 不会把 Nous loop 采纳为主循环。不要在 Nous loop 内直接调用 DBOS 的异步执行 API；3.4 的内部异步支持会替换所用 loop 的默认 executor，这也是选择隔离的具体原因。

所有真实业务经 [RuntimeBridge](runtime-bridge.md) 回到 Nous loop。桥只认识绑定好的函数和序列化值，业务函数通过构造依赖得到服务。workflow 参数不得包含 Runtime、Connection、Lock、Task、客户端、Pydantic 的可变运行对象或 Native 对象。

## DBOS 注册

静态 `@DBOS.dbos_class()` 类 NousMaintenance 继承 DBOSConfiguredInstance，组合根为当前实例构造一次。`config_name`/`instance_name` 使用稳定实例身份；startup_id 只表示一次启动。注册的 workflow 名为 `NousMaintenance.annotation`、`NousMaintenance.refresh`，class_name 为 `NousMaintenance`，实际目录由 jobs/maintenance.py 统一持有。函数名、参数序列化和步骤顺序组成待恢复工作的执行合同。

注册装饰器、配置实例及服务绑定必须在 launch 前完成，不能按请求/Namespace 动态创建工作类。一个 Runtime 的多个 Namespace 共用两个 workflow，通过 payload 的 namespace_ref 区分。

队列为 `nous-annotation` 与 `nous-refresh`，使用 3.4 的 register_queue 公共接口；不能用旧教程里的 `Queue("name")`。运行期注册在 launch 后；为避免恢复时配置缺失，可在 schema 已建立后通过 DBOSClient.register_queue 预先登记，再启动 DBOS。两种方式必须使用同一配置和 application_name。

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

自动维护 ID 从精确 revision/提交事件、工作种类、冻结配方或刷新 profile/目标代次形成稳定键。一次事务重试复用该键。手工明确重新维护创建新的请求身份，保留此前工作结果。

恢复步骤可能重复进入一个已经提交成功的业务方法，因此 ResultStore 和正式采用按工作/步骤业务键查找同一结果。键相同、内容合同不同报冲突，不静默返回别人的结果。Job ID 不能替代文稿 revision identity。

## JSON 与版本

DBOS runtime 和 client 使用一致的 DBOSPortableJSONSerializer；持久 payload、步骤返回与结果显式使用 JSON 标量、数组、对象和正式引用的 JSON 表示。大型正文、模型输出和模板存 CAS，checkpoint 只保存稳定内容引用和必要值，不用 pickle 保存 Python 类实例。

`application_version` 使用显式的 workflow 合同版本（初值 `nous-durable-cognition-1`），不以每次进程随机值或任意源码 hash 破坏重启恢复。仅内部实现变化且步骤序列/语义兼容时保留版本；破坏重放合同的变更在发布前排空或明确迁移未完成工作，不能自动把旧工作重放到不同步骤序列。不开启 Enterprise auto_upgrade，不建立内部 v1/v2 兼容链。

`executor_id` 使用同一单宿主实例稳定值以便本地恢复；startup_id 仍只用于一次启动诊断。系统只运行当前实例、当前支持合同的工作；不接管其他安装的共享数据。

## 恢复、失败重试和工作身份

区分崩溃后自动恢复、用户取消后的继续以及业务失败后的重试：

- DBOS 按已注册名字/应用版本恢复未完成的工作；重复执行必须经过业务采用幂等校验。进程退出不是用户 cancel，不能在关闭时把所有持久工作写成 CANCELLED。
- `DBOSClient.resume_workflow(..., queue_name=原队列)` 只用于允许恢复的取消/超出恢复次数状态；明确传队列可保留原队列并发控制，避免立即执行绕过限制。
- `ERROR` 的步骤失败由 Jobs owner 使用公开 fork 从实际失败步骤继续，复用已完成步骤；重试身份和重复请求规则见 [Job 操作](../interfaces/jobs-and-host.md)。
- 若旧工作本身已与新修订不兼容，报告 superseded 或明确要求新的维护请求，不能通过修改冻结 payload 偷偷重试。

用户 `wait` 是对已提交工作状态的读取，不进入工作执行的并发名额；`cancel` 的 DBOS 状态变更和 Runtime 业务任务实际结束分别报告。跨循环桥的 Future 显示 cancelled 不表示持有的数据库/文件/子进程已可关闭。

## 启动与关闭

启动顺序：取得实例锁并核验资源 → 打开业务基础 → 建立 Job client/同步引擎并验证 DBOS schema → 装配 Runtime 与维护 owner → 建立 bridge → 注册 DBOS 类/实例、配置队列 → 在同步启动上下文 launch → 宣布 workers ready。

短命令 workers=False 只持有 Job client，不构造/launch DBOS 单例。工作可以被持久登记，等待下一次 worker 宿主运行。

关闭顺序按 [执行与生命周期](execution-and-job-lifecycle.md)：先停止新的 bridge 业务派发，再结束实际业务任务，最后 destroy DBOS 和关闭依赖。DBOS.destroy 只能在同步生命周期线程调用，避免阻塞 Nous loop；它开始前不得留下仍依赖业务数据库的无主任务。

## 限制与错误处理

SQLite 不支持 DBOS attributes containment 过滤；Job 列表使用公开可用过滤与有界分页，不直接查询系统私有表假装完整能力。控制客户端关闭无限连接错误重试，连接错误由共同执行期限与错误合同处理。

本轮不要求 Conductor、Kafka、Cron 或云账户。DBOS 日志接入既有 logging，关闭重复 OTLP/控制台输出，启停不会重设主应用的全局 OTel provider。恢复日志保留 job_id 与步骤，正文/凭据不默认输出。
