# 事务、持久作业与索引

## 采用同一物理 SQLite 文件

默认每个 Nous 实例使用一个 `data/authority.sqlite`。领域表与 DBOS 的系统表逻辑上分属各自所有者，物理上使用同一 SQLite 文件，从而使用 DBOS 公共接口 `DBOSClient.enqueue_in_transaction` 在同一次提交中接纳后续工作。

该接口接受 SQLAlchemy Connection/Session，并明确要求连接指向 DBOS system database。异步应用通过 `AsyncConnection.run_sync` 把当前事务的底层连接交给它。DBOS 不在此接口中提交或回滚，完整业务操作决定提交。这消除了自行拼接第二个队列、分布式事务或数据库间补偿的需要。

领域表选择不与 DBOS 表名冲突的业务命名。DBOS 表结构由其自己的迁移维护；应用只调用公开 API，不手改调度表。使用 SQLite WAL、本地磁盘、foreign_keys 与合理 busy_timeout；实例锁保证一个 active Host。普通读请求和网络工作仍然并发。

## SQLAlchemy 的职责

使用 SQLAlchemy 2 Core、异步 Engine 与 aiosqlite。当前工程大量手写 SQL 和事务经验可以按明确 owner 改编，避免建立泛型 ORM Repository。

事务设施只有几个必要入口：

```text
Authority.read(ctx) -> ReadView / read connection
Authority.write(ctx, operation_id, operation_kind, scope) -> WriteTx
WriteTx.connection -> 当前 SQLAlchemy AsyncConnection（仅领域内部）
WriteTx.enqueue(job_spec) -> JobRef
WriteTx.result(value) -> 完整业务回执
```

WriteTx 负责一次 transaction、操作幂等、commit_seq、记录时间和最终提交。Receipt 保留用户需要的正式结果；重复相同 operation 返回原结果，改变载荷明确冲突。操作身份按 caller+operation+scope 定义，不能把所有重复文件或所有同 Subject 工作都塞入同一个去重键。

SQLite 显式事务设置使用 SQLAlchemy 官方说明的一种方式：关闭 driver 的隐式 BEGIN，并由 begin 事件发送 BEGIN；业务 writer engine 发送 `BEGIN IMMEDIATE`，reader engine 发送普通 `BEGIN`。DBOS 自己的同步 engine 维持其正式实现，只配置同一文件、busy_timeout 与 foreign_keys。不要同时混用 driver `autocommit=False` 与这套显式 BEGIN 方案。

独立读/写 engine 是连接策略；它们面对同一数据模型，没有独立读写状态副本。数据库写锁只覆盖 SQL 的短事务。模型、下载、FFmpeg、embedding、压缩、native 索引构建均在事务外。

## 原子入队的实际接线

目标接线等价于：

```python
async with writer.connect() as conn:
    async with conn.begin():
        # 在同一 conn 修改领域结果、回执和本领域后续任务引用。
        handle = await conn.run_sync(
            lambda sync_conn: dbos_client.enqueue_in_transaction(
                sync_conn,
                {"workflow_name": "form_memory", "queue_name": "formation",
                 "workflow_id": durable_job_id},
                request_reference,
            )
        )
# 到此才允许等待任务结果；事务内不 get_result。
```

正式实现由 `platform/execution` 包含这段 DBOS 接线，领域只传 typed JobSpec。Workflow 参数传持久请求引用、scope 和必要小值；原媒体、模型正文、向量、schema 全文不重复放进队列。使用该稳定 DBOS 版本提供的 JSON 序列化选择，不能从用户 Bundle 反序列化任意 Python pickle。

为所有任务保留可查询的 scope、提交者、业务操作与 owned-parent 关联；调度状态、重试次数、sleep 和执行续接由 DBOS 拥有。应用数据库不复制一套通用 pending/running/retrying 状态机。导入文件成功、认知采纳、向量采用等是独立业务事实，不能改成 DBOS step 输出的唯一副本。

采用 DBOS Python 3 的公开 API：启动后注册数据库队列，使用按队列名入队及持久 schedule；不使用旧 `Queue(...)` 构造器、`@DBOS.scheduled`、`@DBOS.transaction` 或旧 FastAPI 构造参数。普通用例的事务设施继续是唯一入口；workflow step 调用同一用例，靠业务 operation id 识别已经完成的采纳。

## 一个完整的材料形成过程

1. 流式接收文件，写入临时区域并由 content owner 原子发布不可变字节。计算内容身份，保持输入顺序与媒体类型；不要以文件名唯一化所有资料。
2. 在接纳事务中保存 Material、InputSource/Catalog、真实 Import 或 Contact、业务请求与 DBOS 作业。字节文件本身由临时 lease 保护；事务失败释放未引用发布结果，清理不影响其他引用。
3. Worker 读取固定请求及授权范围，取得有效 Prompt、模型与参数快照，事务内预约预算并保存调用意图，然后释放数据库锁。
4. 执行真实 Provider 请求，持久保存已完成返回及 usage。第二段媒体转述复用第一段已保存正文，并同时再次传入原媒体。
5. 短采纳事务重新取得必要修订与来源权限，采纳认知、保存 UnitOutput/派生关系、结算费用，并原子安排 embedding 与维护义务。
6. 索引任务读取正式认知与已采用向量构建新代际。完成后在事务中发布可用水位。CLI/SDK 能等待 accepted、formed、indexed 等真实用户选择的完成条件；不能只把队列接纳当作形成完成。

Provider 请求遇到“服务端可能已接受但响应丢失”时，调用账本保存未知状态及请求身份。已获得的付费结果优先复用；有服务端幂等/结果查询能力则利用。外层 workflow、Provider SDK、HTTP transport 不同时各自无限重试。

## 幂等、去重与取消

OperationId 处理同一个用户操作的重复提交；File dedupe 处理一个范围下相同文件的接纳资格；embedding 缓存处理同一内容与模型空间的已采用向量。这三者保持不同的键和事实。

继承交接中的 UnitCatalog、UnitOutput、FileSuccess、ManifestHead 语义。Catalog 和 Output 是业务事实；再次查看相同媒体仍可产生新的 Contact。恢复 bundle 中的历史输出不生成假的历史 Job。Required 向量策略按每个真实 UnitOutput 判断，索引 owner 再报告当前实际 coverage。

DBOS 分区并发按实际共享资源设置，网络形成可并行，短 SQL 写入由 SQLite 串行。不要把一个 Subject 的所有任务都单线程化，令媒体、查询、维护互相等待；只有具有真正顺序约束的用例使用分区。

owned 子任务随父操作取消；多个调用共同等待的共享 embedding/下载工作只撤销当前等待者。未引用的可选工作可以取消，已经产生的正文、向量和调用费用保留实际状态。取消与事务提交并发时，由已经提交的事实决定结果，入口不虚报“从未发生”。

## 查询与索引

Python 先建立已授权的 ReadView，确定 Subject、来源、认知种类、历史截点、时间域和允许图节点；再把被允许的候选/图选择交给 Rust。新索引的发布使用代际身份与完整对象列表，不让 native 读取权威 SQLite。

Tantivy/USearch/图资产都是派生状态。查询需要的最小水位尚未满足时，返回明确的等待/部分可用状态或由请求选择等待；精确引用直接读权威数据。查询中缺向量可提交共享工作，但创建工作有明确预算与语义，不把普通状态读取变成模型调用。

权限和删除必须作用于 native 候选与图传播的输入，最终物化再检查当前授权及请求的历史规则。仅在最终正文输出时过滤，无法阻止受限图节点影响推断。去权/清除同时使相关 context、cache、向量与索引失效；原始多模态根、派生与关系按照 material owner 的生命周期处理。

## 备份、恢复与升级

安装备份使用 SQLite online backup API 获取数据库一致副本，内容 owner 按该副本的真实引用枚举不可变文件。DBOS 状态在同一库中随备份保存；向量/索引可按覆盖信息保存或重建。备份过程不直接复制一个正在写入的裸 SQLite 文件并漏掉 WAL。

Subject Bundle 导出逻辑对象、来源、修订和明确的未完成业务，而非 DBOS 内部表、Python 执行堆栈或机器密钥。导入产生新的本地执行身份；历史已完成事实保持原有来源，不假造重放调用。

应用 schema 使用 Alembic 正式迁移，DBOS 使用自己的迁移。初始化和升级在 worker 启动前完成。旧本地 Rust 实例的数据目前留在原机；如用户实际提供 bundle/数据库副本，建立针对该具体已知版本的导入转换，在新实例验证，保留原件。不要为不存在的历代内部 API 开发永久兼容服务。
