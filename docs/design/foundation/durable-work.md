# 持久工作与业务操作

## 选择

采用 DBOS Python 3.4.0 提供持久工作、队列、取消、延迟和恢复。具体装配与事务接口见 [DBOS 接入](dbos-integration.md)；后续工作见[架构重构计划](../../plans/architecture-rework.md)。定时触发可由外部宿主调用。

DBOS 系统数据库与业务表使用同一 SQLite 文件。工作拥有者决定内容、输入、结果和重试是否合理；DBOS 负责通用执行机制。

## 接入方式

应用组合根注册命名 workflow 函数并显式绑定所需业务操作。公开 payload 只含可持久序列化的值和正式引用，不包含 Runtime、SQLAlchemy Connection、HTTP 客户端或模型实例。

显式宿主在启动时注册命名工作流及已构造的领域操作。DBOS 异步工作流和业务阶段在宿主同一循环执行；独立 Job 客户端可以在宿主运行时控制工作，不另开业务 Runtime。

事务内入队按 [事务专题](transactions.md)执行。方法对当前 SQLAlchemy transaction 生效；不要再复制队列状态到一套应用任务总表。

## 结果与状态

业务结果以所属对象保存。公开入口使用 runtime.jobs、JobRef 和 nous job，保留 WorkContext 的认知含义。任务 handle 返回当前状态、结果引用及真实错误。恢复、等待和继续使用同一个 handle；普通用户看到可读工作名和进展，不必拆内部 request ID 再猜调用方式。

DBOS 工作记录可按其保留策略处理。正式采用的模型输出、认知或内容引用保持独立生命周期，不能随任务历史清理一起消失。

## 重试与取消

DBOS 重试用于该工作声明的可重试步骤。模型执行器内部尝试已计入本次预算，不能通过 workflow 无界重试重新刷新预算。已采用结果由业务键识别，恢复时复用真实结果。

外部模型可能已经收到一次失败或超时请求。DBOS 的数据库执行保证不被描述成提供方收费 exactly-once。没有拿到结果的请求状态按实际信息保留；是否重试由预算与工作政策决定。

任务取消后停止后续采用或新调用；已经提交的正式结果保留。关闭宿主先停止接纳，等待或取消实际任务并等资源退出，再关闭 DBOS 及数据库资源，详见[执行与资源关闭](execution.md)。

## 实施依据与验证

使用 [DBOS Client](https://docs.dbos.dev/python/reference/client) 的事务入队和 [Queues](https://docs.dbos.dev/python/tutorials/queue-tutorial) 原有能力。首个消费者为独立的标注与刷新工作：保存事务可分别登记两者，标注成功激活后在采用事务登记新代次刷新；标注失败不阻断正文检索。具体合同见 [持久维护](../cognition/durable-maintenance.md)，不建立只回传字符串的永久演示任务。

验证真实入队、状态、结果读取、重启继续、一次失败恢复及业务写入回滚时不入队。只验证本项目的集成责任。
