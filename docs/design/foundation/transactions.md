# 完整操作的事务责任

## 一项操作一个提交者

完整业务用例打开 WriteTx。所有需要一起提交的参与者接收同一个事务，负责自己的数据变化，提交与回滚由用例的事务上下文完成。参与者不 commit、不另开会逃出外层回滚的事务。

时间域登记、Journal 修订、来源关系、内容引用和明确维护入队使用同一事务参与结构。写事务在读取修改依据前取得 SQLite 写入资格，使用共同 Database 的 BEGIN IMMEDIATE 入口；不要把业务进程内写锁当成 DBOS 独立连接也遵守的锁。

## I/O 分界

模型生成、网络请求、媒体转换、向量计算和大字节写入在短事务外完成。业务拥有者读取需要的输入，准备产物，再在短事务中重新检查受影响的正式状态并采用。

普通操作不强制携带通用 operation UUID 或永久 request/response receipt。重复投递按实际业务身份处理，例如宿主原 event_id、导入运行的单元身份、持久工作 ID。操作诊断 ID 不负责业务去重。

## 修订与时间

各领域决定同一对象的修订以及预期旧版本。共同记录能力以后可复用 identity/revision/支持关系，但数据库层不以一个统一枚举解释所有认知变化。

recorded_at 由实际写入时的 UTC 时钟提供。确有历史顺序要求的正式提交由相应记录能力分配递增序号；查询句柄、配置读取和日志写入不冒充认知变更。时间域坐标不代替系统提交次序。

## 持久入队接入

本轮使用 DBOS 3.4.0 的 `DBOSClient.enqueue_in_transaction`，传入当前事务中的 SQLAlchemy Connection。同一 SQLite 文件保存应用与 DBOS 系统表。异步连接通过官方支持的 `AsyncConnection.run_sync` 取得绑定到同一事务的同步 connection。

该方法不提交也不重试调用者事务；事务提交后才可以等待其 handle。不要在写事务里等待 worker 完成。不要为此再建立应用 outbox 与轮询搬运器。

正式接入见 [DBOS 与 Runtime](dbos-integration.md)。显式维护入队失败使同一业务事务回滚；不返回虚假的已排队结果。标注与刷新使用各自工作身份；队列 deduplication 的 return-existing 策略不用于事务内合并。

## 完成条件

同一事务的多项变化一起提交或回滚；失败后连接可继续正常使用；模型等待不占写锁。引入 DBOS 的那次施工验证业务写入与入队的共同提交，除此不重复测试数据库本身的 ACID 实现。

依据：[DBOS Client transaction enqueue](https://docs.dbos.dev/python/reference/client#enqueue_in_transaction)。
