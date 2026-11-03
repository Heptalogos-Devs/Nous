# 操作、阻塞计算与资源关闭

## 当前保留能力

ExecutionManager 负责根操作名额、配置快照、截止时间、运行范围与实际阻塞工作。继续使用标准 asyncio、ThreadPoolExecutor 和 contextvars。

保留 fdb39e4 的 owner 隔离：同一 ExecutionManager 内嵌套操作继承上下文；A→B→A 中 B 有独立名额，回到 A 复用 A。前台按作用域完成信号结束，spawn 任务按实际 Task 生命周期结束。spawn 清空操作继承而保留日志/Trace 上下文。

已接纳操作在停止接纳后的宽限期内可完成嵌套阶段和清理；新根操作被拒绝。OperationContext 不保存全局服务容器。

## 共同阻塞操作

ExecutionManager.blocking(action, *args, on_cancel=None, **kwargs) 管理实际 worker。等待方收到取消后先调用一次同步、轻量的 on_cancel（若有），等待真实 worker 完成，再传播原 CancelledError。收尾异常附在原取消上；重复取消也继续等待。

Native 使用 on_cancel=token.cancel。CAS 的文件操作和发布复用同一等待能力，文件句柄、临时路径和发布锁保持到 worker 退出。能在 worker 内完成打开/关闭的操作优先使用这一方式。

## 请求与并行层次

顶层应用操作获得一次并发名额，阶段沿用剩余总期限。阶段 timeout 可以更短，不能延长根期限。业务通过 TaskGroup 管理需要共同结束的子任务；不将普通 create_task 的继承上下文当成脱离父范围的许可，独立后台动作使用 spawn。

Native 所有请求共用一个有界 Rayon 池；Python 的 worker 只承载跨语言调用，不为每个调用新建 Rust 池。选择合理池数而不是逐核嵌套线程爆炸。Native token 仅在循环/批次安全点生效，不伪造杀线程。

## 生命周期范围

执行资源可在同进程独立构造多个 owner；完整 Runtime 的日志/SDK 装配仍按一个主要实例运行的当前部署合同。执行隔离修复不自动授权共享全局日志的多实例服务。真正需要多实例 Host 时再修改观测装配合同。

关闭依次停止新入口、结束操作/子进程/Native 实际计算、关闭模型连接与存储、刷新观测、释放锁。独立操作异常不取消无关请求。沿用现有正常停止与启动失败清理能力。
