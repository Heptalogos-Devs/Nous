# DBOS 3.4 接入依据与已作出的取舍

这里记录会改变实际施工的方法依据。研究基线为DBOS tag `3.4.0`，当前文档可能继续变化；恢复包保留该已选版本，不把新版本API静默写入本轮。

## 接入事实

| 依据 | 对本项目的直接影响 |
|---|---|
| DBOSClient.enqueue_in_transaction接受调用者SQLAlchemyConnection/Session，要求是系统数据库，调用者提交；AsyncConnection.run_sync是正式桥接 | 同一SQLite文件、同一WriteTx，无自制outbox，无独立enqueue事务 |
| 3.x队列通过register_queue登记，Queue不能按旧例直接构造 | 更新施工代码与初始化顺序，采用当前公开接口 |
| 类方法/配置实例支持class_name与instance_name，实例必须在恢复前注册 | 一个显式绑定的NousMaintenance实例，不使用全局Runtime查找 |
| 3.4 BackgroundEventLoop在launch调用线程已有loop时会采纳该loop | launch放在无运行loop的同步启动上下文 |
| 3.4内部异步支持会设置其所在loop的默认executor | 隔离DBOS调度loop；业务不直接进入DBOS全局异步执行 |
| destroy停止SDK后台资源，但不替外部业务资源完成清理 | bridge停止派发、实际业务退出、再destroy；独立完成信号 |
| DBOSPortableJSONSerializer提供公开JSON边界 | payload与checkpoint只传JSON/引用，不pickle Runtime |
| transaction enqueue不支持deduplication return-existing策略 | 不用宽泛队列去重吞掉更新generation；同一业务ID有独立复用语义 |
| SQLite attributes containment过滤不受支持 | 不承诺任意Job属性过滤，不查询SDK私有表绕过 |
| SQLite reset/truncate路径作用于整个文件/表集 | 同业务库绝不暴露reset系统数据库作为产品清理 |
| 官方 workflow management 把 resume 限于取消、恢复次数超限和排队中工作；失败步骤重试提供 fork/rewind | 对 ERROR 使用公开 fork_workflow 从失败步骤建立新工作，不将 resume 的返回当作实际重试；保留原工作与原队列并发约束 |

## 已选方案与排除的替代

同一循环直接运行DBOS最少代码，但会将其循环/executor管理与既有Runtime混合。另一个进程独立open_instance会与当前实例锁、业务所有权冲突。选定单进程、DBOS自有调度循环、业务回到原循环的有限接缝，隔离工作机制且复用现有服务。

DBOS datasource可为其自己的事务模式提供checkpoint；当前Nous已经拥有SQLAlchemy Core事务参与者、来源与结果采用。为两个维护工作迁移全领域到另一个session/transaction模型收益不足。本轮使用官方事务入队与现有业务幂等，领域数据继续归原owner。

单一“保存后全流程”workflow会让标注失败阻断正文索引。分离annotation/refresh，保留初次文本可查询与标注完成后的再次刷新。没有人工补偿队列，后续刷新在状态改变的事务登记。

## 重放中的故障窗口

| 故障位置 | 处理 |
|---|---|
| 正文事务提交前 | 文稿和入队共同回滚 |
| 正文事务提交后、请求返回前 | 精确revision和已登记Job存在，读取事实，不重新生成正文 |
| 模型已完成且步骤结果已记录 | 直接恢复计算描述，后续采用不重发 |
| 正式产物已采用、步骤完成记录尚未写入 | 同一producer/adoption键返回原正式结果 |
| 提案生成后正文head改变 | 保留提案，结果superseded，不覆盖新head |
| refresh捕获G后发生G+1 | 保留G快照，后续需求继续，不无限追赶 |
| 外部请求成功但没有任何本地持久结果 | 可能需要重新请求；不以数据库exactly-once替代提供方语义 |
| 用户取消与提交同时发生 | 以实际提交结果呈现；已提交事实不回滚 |

## 一手资料

- [DBOS 3.4.0源码](https://github.com/dbos-inc/dbos-transact-py/tree/3.4.0)
- [3.4 client事务入队](https://github.com/dbos-inc/dbos-transact-py/blob/3.4.0/dbos/_client.py)
- [3.4生命周期与executor](https://github.com/dbos-inc/dbos-transact-py/blob/3.4.0/dbos/_dbos.py)
- [3.4事件循环](https://github.com/dbos-inc/dbos-transact-py/blob/3.4.0/dbos/_event_loop.py)
- [3.4队列](https://github.com/dbos-inc/dbos-transact-py/blob/3.4.0/dbos/_queue.py)
- [3.4 SQLite实现](https://github.com/dbos-inc/dbos-transact-py/blob/3.4.0/dbos/_sys_db_sqlite.py)
- [3.4序列化](https://github.com/dbos-inc/dbos-transact-py/blob/3.4.0/dbos/_serialization.py)
- [官方Client参考](https://docs.dbos.dev/python/reference/client)
- [官方 Workflow Management（resume、fork、rewind 语义）](https://docs.dbos.dev/python/tutorials/workflow-management)
- [官方类与生命周期](https://docs.dbos.dev/python/reference/dbos-class)
- [官方工作流与步骤](https://docs.dbos.dev/python/reference/decorators)
- [官方配置](https://docs.dbos.dev/python/reference/configuration)
- [官方事务/datasource说明](https://docs.dbos.dev/python/reference/datasources)

实现只import公开API；源码研读用于理解生命周期与限制，不将SDK私有属性变成Nous运行接口。版本、连接或loop行为改变时先核对本项目实际接缝，再改变选择。
