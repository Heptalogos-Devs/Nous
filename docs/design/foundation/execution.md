# 进程内执行、期限与取消

## 范围

执行基础提供受管理的异步并发、总期限、取消、阻塞工作与关闭协作。它不定义业务工作流，不替代后续 DBOS 持久执行。

采用标准 asyncio 的 TaskGroup、timeout/timeout_at、Semaphore 及 to_thread。只有外部库或平台需要不同执行机制时在该边界使用成熟能力，不建设第二套通用调度器。

## 操作上下文

`OperationContext` 只携带 operation_id、操作名、必要调用范围、配置快照引用和可选的单调截止期限。日志/Trace 关联通过 contextvars 传播。具体领域对象与全部服务不塞进上下文。

入口可用 `execution.operation(name, settings, deadline?)` 建立上下文，内部操作继承剩余期限；子调用可以缩短，不能重新获得完整总超时。公开标识由程序生成，普通用户无需手写 UUID。

## 并发和取消

同一操作内部用 TaskGroup 组织需要共同完成的子任务。一个子任务失败只按该业务原本的原子性/部分成功合同影响兄弟任务；不让一个无关用户请求失败取消整个服务中其他请求。长期服务 supervisor 在请求边界收集独立失败。

有界 Semaphore 控制整体资源进入。排队时间计入总期限。达到期限或用户取消后停止开始新子任务，向已开始的可取消任务传播；CancelledError 原样穿过无关层。

## 阻塞调用

同步文件、库操作或 Native 计算使用标准 ThreadPoolExecutor，并以 max_blocking_workers 限制实际 worker 数，不堵塞事件循环。通过 run_in_executor 提交时复制所需 contextvars；若采用 to_thread，则由唯一运行时配置对应默认 executor。等待者取消后，容量和资源引用在实际 future 完成时才释放，不能因 async 等待提前退出就宣称线程空闲。`to_thread` 的等待取消不保证线程停止；任务负责者保留实际运行与资源引用直至完成。长计算需要协作取消或已有子进程机制，不伪造安全杀线程。

媒体子进程的 stdin/stdout/stderr、退出和取消归相应媒体能力；执行基础提供公共生命周期方法。真正的持久重试由 DBOS 处理。

## 失败与重试

基础执行不重试任意 callable。网络执行、模型尝试、持久工作分别在自己的负责层定义重试策略，并共享同一总期限。取消与输入错误没有自动重试。

## 首轮完成条件

并发实际受限；排队和执行共同消耗期限；取消后没有孤儿 asyncio task；阻塞工作持有的资源不提前关闭；一个独立操作失败不结束整个运行时。来源：[asyncio tasks](https://docs.python.org/3.14/library/asyncio-task.html)。
