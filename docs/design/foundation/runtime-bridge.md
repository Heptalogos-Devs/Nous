# 调度循环与业务循环的有限交接

## 责任

`jobs/bridge.py` 仅负责一次DBOS步骤调用如何进入现有Nous事件循环，如何把结果或错误交回，以及如何在取消和宿主关闭时等到真实业务结束。它不决定业务重试、序列化版本、文稿身份、事务、模型路线或下一项工作。

这是 [DBOS接入](dbos-integration.md) 已选择的单进程双循环结构。Nous的AsyncEngine、HTTP连接、asyncio锁、ExecutionManager和Native资源只在原业务循环使用。DBOS调度循环持有workflow及checkpoint，业务服务不执行DBOS异步API。

## 生命周期与接口

```python
# 目标接口示意；T的持久边界值由workflow负责转为JSON。
class RuntimeBridge:
    async def call(self, operation, payload, *, association, deadline=None): ...
    async def quiesce(self, *, grace_seconds: float): ...
```

call从DBOS循环使用。operation是组合根绑定的有限业务阶段函数，不能是任意字符串命令或从Runtime查找服务的路由；payload为已冻结值。quiesce由Nous原循环的宿主关闭调用。禁止在Nous事件循环中对跨线程Future调用同步result/join。

运行实例只创建一个bridge。维护workflow类持有该bridge及已绑定的维护阶段；恢复前完成绑定。普通前台调用直接进入同一业务阶段，无须跨循环绕行。

## 一次派发的控制关系

桥为每次派发维护很小的进程内Flight，包含：取消请求标记、目标Task（仅目标循环访问）、独立完成Future，以及发起关联。它只存在于实际运行期，不建SQL记录或重试/租约状态。

1. 调度侧检查桥仍接纳，通过 `call_soon_threadsafe` 将启动动作投递到Nous循环。
2. Nous循环再次检查接纳状态与取消标记。在已经取消或关闭时不启动业务，直接完成Flight；否则创建业务Task并登记。
3. Task在清楚的上下文中进入所属ExecutionManager.operation，绑定instance/startup、job、阶段及Trace信息，随后调用现有业务方法。
4. 完成回调只在实际Task结束后设置Flight的独立完成Future，收集结果或原始异常，释放登记；设置时处理取消和完成竞争，且只设置一次。
5. DBOS步骤等待这个独立完成信号并取得结果。成功checkpoint发生在调度侧，业务侧只返回正式结果或恢复引用。

不把DBOS私有ContextVar、正在运行的Nous OperationContext、锁、连接或Token跨循环传递。目标Task使用明确建立的上下文，Trace carrier/Job关联由公开值恢复；不要隐式复制整个调用方context，使DBOS以为业务代码也处于同一个步骤上下文。

## 取消请求与完成信号分开

不能用 `run_coroutine_threadsafe(...).cancel()` 返回的Future终态作为业务资源结束依据。等待侧取消时：记录原CancelledError → 请求目标Task取消 → 继续屏蔽重复取消并等待独立完成信号 → 传播原取消。Task尚未创建时，取消标记使启动回调拒绝工作；已经在执行时，由业务自己的finally、Execution.blocking和子进程owner完成收尾。

独立完成Future不能被等待者的取消传播所取消。可用 `asyncio.shield(asyncio.wrap_future(done))` 等标准机制；每次等待都针对未被取消的完成对象，重复取消不终止等待。业务异常通过完成值或exception交回，不让回调失败遮蔽实际业务错误。

DBOS异步步骤需要对用户cancel及时响应时采用官方 `preemptible=True`。DBOS取消步骤后，通过上述路径请求业务停止；原子提交可能已经完成，结果保留实际提交的引用。Job状态已CANCELLED与本机资源仍在清理是两项信息，接口不得声称远程cancel返回即所有资源已经消失。

## 期限与重启

本次进程内派发可使用明确的单调期限或剩余时长；跨进程持久payload不得保存一次启动的monotonic绝对值。DBOS持久workflow期限采用其公开参数；业务阶段根据仍有效的剩余期限和当前Execution限制建立实际期限，两者取更紧者。

Job等待超时仅结束调用者的等待，只有显式cancel改变持久取消意图。等待和控制不占业务并发槽，max_in_flight=1时必须仍能运行所等待的阶段。

## 正常关闭

宿主先关外部入口并令bridge不再接纳新派发，在Nous循环保留已登记Task的完成等待。宽限结束后请求取消，随后一直等到实际资源退出。关闭门建立后DBOS可能已经领取的新工作得到中断，不进入新业务副作用，也不伪报完成。

宿主关闭使用中断当前执行的语义，不逐项调用DBOS.cancel_workflow把待恢复工作永久标为用户取消。被中断的步骤应沿SDK的asyncio取消语义退出并在下次启动恢复。此项在实际3.4集成中验证，不能把通用业务Exception当成shutdown并记录ERROR后假称自动恢复。

所有Flight实际结束后，在不会阻塞Nous循环的同步生命周期上下文销毁DBOS；再关闭client、同步engine、业务执行、连接、存储、观测及实例锁。不得访问DBOS私有task集合、私有loop或stop_events完成关闭。

## 验证范围

只保护实际接缝：正常返回/异常；目标Task创建前取消；执行中重复取消；取消与提交竞争；关闭后拒绝派发；阻塞worker和子进程退出后才发完成信号；重开后工作继续；等待不抢占最后一个业务名额。测试不用泛化任意Actor/RPC框架，也不把所有业务方法改成远程命令。
