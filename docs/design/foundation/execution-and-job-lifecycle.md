# 公开操作、Runtime 观测与实际资源结束

## 完整操作负责执行边界

完整公开应用操作进入 ExecutionManager，拥有一个根 operation、配置快照、期限、并发准入和关闭责任。同 manager 的嵌套阶段继承；事务参与方法 `_in` 不再创建独立根。CLI 只解析与呈现，删除 CLI 独有的执行补偿包装。

保留 owner 隔离：A→B→A 时回到 A 继续 A 的上下文，不能让不同 manager 的名额和配置混用。spawn 为独立工作范围，不把普通 create_task 的继承上下文当作永久后台许可。

Job 等待、停止和诊断不能占用其所等待业务所需的普通执行名额，尤其 max_in_flight=1。将这些有限控制操作作为明确的无业务准入等待范围，保留超时/观测；不额外建设优先级调度器。

## 观测资源由被调用 Runtime 决定

ExecutionManager 绑定一个轻量观测作用域提供者，内容包括 instance_id、startup_id、该实例的 OTel provider/metrics 和日志归属。每次根 operation 进入时设置，退出时通过 token 恢复调用者上下文。

保留外部 W3C Trace parent；队列工作通过明确持久化的 trace carrier 或 link 连接发起操作，不把启动 Runtime 的 span 当作数日父 span。当前 operation_id 与持久 job_id、DBOS step 标识分别使用。

先创建异步 worker、再把 Runtime 注入 worker 的调用必须具有相同实例日志和有效 Trace。日志标准入口仍为 logging/structlog，底层模块不反向依赖 Runtime 服务容器。完整 Runtime 继续采用一个进程一个主要实例的部署合同，多个 Namespace 共用它；本轮不构建多实例全局日志路由平台。

## 期限

根期限覆盖准入等待和工作；阶段可缩短，不能延长。真实期限使用单调时钟，认知时间域不能拿来管理网络/进程超时。工作线程的计算不会被强行杀死；期限到达后请求取消并等待资源真正退出，最终仍返回 timeout，不能因后来计算完成变成 success。

进入提交前检查已到期/取消状态；已经完成的事务保存准确结果。取消发生在提交期间时由共同事务机制表达实际结果与已完成引用，不试图用撤销已提交认知补偿取消。

## 线程与子进程

沿用 `ExecutionManager.blocking`：等待被取消后请求一次合作取消，屏蔽后续取消直到真实 worker 退出，再传播原取消。Native 使用已有 token，CAS/解析持有的文件和临时路径保留到 worker 结束。

将 AudioVisual.process 的弱清理路径收拢到共同受管子进程能力。成功完成 communicate 后取得结果；取消/超时先终止，给已有配置决定的有限退出等待，仍存活则 kill；等待进程退出并排空管道后才返回。重复取消不能中断该清理。正常退出与 terminate 的竞争按实际 returncode 处理，不吞掉主体错误。

一次处理的 FFmpeg/ffprobe 资源必须由启动它的操作拥有，Runtime 关闭等其清理完成。Windows 使用对应进程终止行为，Unix 不无条件发送 Windows 不支持的信号；不创建新的机器管理服务。

## Job 与 Runtime 的关系

DBOS 生命周期在 jobs owner。它的 workflow/step 调度循环与 Nous 业务循环分开；桥接每个实际业务阶段时进入 Runtime 新的根操作。持久工作在休眠、排队、checkpoint 和等待期间不持有业务事务或执行槽。

运行桥接只传播明确的 job/step 关联、trace carrier、剩余期限和不可变参数，不复制 DBOS 私有 ContextVar 或运行中的 Nous OperationContext。Runtime 的业务对象不会跨循环直接调用。

桥接使用独立的“取消请求”和“业务任务实际完成”信号。`run_coroutine_threadsafe` 返回 future 的 cancelled 状态不能充当资源结束通知；实际 Task 的 finally/done 才发布完成。取消发生在排队到目标循环之前时，也要使任务不开始或开始后完成清理。

## 关闭顺序

停止新的外部业务请求和 bridge 新派发；给实际业务阶段退出机会；超出宽限期后请求取消，并等待其真实资源结束。DBOS 不再能通过 bridge 启动新的业务副作用。之后停止/销毁 DBOS，关闭只用于 Job 管理的客户端及同步数据库资源，再结束普通执行资源、模型连接、业务数据库、观测与实例锁。

DBOS.destroy 不提供对所有外部业务资源的强制中断承诺。不要先 destroy/关数据库再等业务线程。宿主正常退出造成的未完成工作保留可恢复状态；用户显式 cancel 才改变持久 Job 的取消意图。

## 完成行为

配置、生成、媒体、Wave、认知以及 Job 进入的阶段遵守同一执行合同。老 worker 不丢实例/Trace。缓存命中长文不卡事件循环。取消两次后子进程已回收、Native/文件 worker 已结束。已保存但维护失败的 partial 在返回、根日志、runtime logs/check 中一致。
