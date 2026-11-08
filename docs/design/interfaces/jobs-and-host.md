# 长期宿主与 Job 的使用接口

## 程序入口

```python
from nous.cognition import AfterWrite
from nous.cognition.maintenance.contracts import MaintenanceOutcome
from nous.runtime.host import run_host

async def application(runtime):
    cognition = await runtime.cognition("research")
    outcome = await cognition.records.create_journal(
        markdown,
        after=AfterWrite(annotation="run", retrieval="refresh", execution="background"),
    )
    pending = list(outcome.jobs)
    seen = set()
    while pending:
        ref = pending.pop(0)
        if ref.workflow_id in seen:
            continue
        seen.add(ref.workflow_id)
        result = MaintenanceOutcome.model_validate(await runtime.jobs.wait(ref))
        pending.extend(result.related_jobs)
    return await cognition.records.read(outcome.record.revision_ref)

current = run_host(locator, application)
```

`run_host` 是同步应用根入口，拥有一个 asyncio.Runner 和本进程的 DBOS 生命周期；应用协程与持久工作共享 Runtime。普通 `async with open_instance(locator)` 可保存内容、登记及控制 Job，不启动持久工作。已有异步应用将主协程交给其根入口的 `run_host`。

## 公开值与方法

`JobRef` 保存 instance_id 和 workflow_id。`JobView` 包含工作种类、状态、队列、边界输入、创建/开始/完成时间、结果、错误、失败步骤与重试来源。通用 Job 结果是 JSON 对象；认知维护调用者使用 `MaintenanceOutcome.model_validate(result)` 读取 state、正式 proposal/annotation 或快照引用、覆盖代次及 related_jobs。

`JobOperations.list(kind?, status?, limit, cursor)`、`read(ref)`、`wait(ref, timeout?)`、`cancel(ref)`、`resume(ref)`、`retry_failed(ref, from_step)` 是完整控制入口。工作引用按实例、应用和已登记工作名核验。

DBOS 的 ENQUEUED/PENDING/SUCCESS/ERROR/CANCELLED 与业务结果分别呈现。成功可能返回 superseded；取消记录后，实际资源结束仍由宿主等待。

`wait` 超时或调用者取消只结束本次等待；显式 cancel 才取消 Job。等待不占业务执行名额。`resume` 将已取消或超出恢复尝试的工作放回原队列。ERROR 使用 `retry_failed` 从实际失败步骤 fork，保留原错误与已完成步骤。并发重试同一失败工作得到同一后继；后继再次失败时再对后继发起重试。需要使用新 Prompt 时创建新的标注请求。

## CLI

```sh
nous journal create --file ./notes.md --background --refresh
nous journal annotate "jrn(...)@1" --background --task annotate-concepts
nous note annotate "note(...)@1" --background --refresh
nous job list
nous job read JOB
nous job run --until-idle
nous job wait JOB
nous job cancel JOB
nous job resume JOB
nous job retry JOB --from-step FAILED_STEP
nous runtime run
```

`--background` 只选择 AfterWrite.execution，不把 CLI 编排成另一个队列。`--inline` 显式前台，二者冲突时报输入错误。正常 deferred 检索或已入队维护返回 success；明确前台后续失败沿既有 partial 退出码 3。

`job run --until-idle` 打开一个持锁 Runtime 并启动持久宿主，处理当前可运行工作及其即时后续；没有可运行工作且本实例实际业务阶段全部退出时结束。未来延迟工作可保持 queued/delayed，输出其数量/状态，不为了等待远期节点一直占锁。`runtime run` 持续运行直至正常停止，不自行转为操作系统服务或启动第二个 worker 进程。

短保存命令结束后，queued 工作可以等下一次 run。`runtime run` 和 `job run --until-idle` 保持实例和数据锁，由同一 Runtime 执行来自普通本地 CLI 的请求；读取、写入、检索和编辑都继续使用原有 CLI 与业务操作。相对路径以客户端当前目录解析，编辑器在客户端终端启动。`job` 查询与控制仍使用受限 DBOSClient。SDK 的 `run_host` 可在应用组合根通过 `service` 注册本地 CLI 服务；普通 `open_instance` 仍要求取得独占锁。

本地服务只绑定 `127.0.0.1` 的临时端口；实例路径的摘要定位端点文件。POSIX 使用用户私有目录（0700）和文件（0600）；Windows 使用当前用户作用域的 DPAPI 加密端点密钥。双方使用独立随机挑战和端点密钥作 HMAC 认证，客户端在验证宿主前不发送命令；没有网络监听地址配置，也不接受非回环连接。单条消息上限 64 MiB，最多 16 个并行请求。客户端断开时宿主取消对应操作并等待实际工作释放资源；连接丢失后不自动重试，因为写入可能已经提交。正常退出清除端点；崩溃后实例锁由操作系统释放；陈旧端点的端口不可用时客户端可离线打开，下一宿主取得锁后原子替换端点。若端口已被其他程序占用，认证失败并停止，不会自动重试写入。

握手在业务名额之外进行，未认证连接有数量与期限约束；迟到的文件或输入答复不能用于其他请求。宿主只在命令实际读取时请求客户端 stdin 或文件，相对路径和符号链接由客户端解析。文件先在客户端读取到仅该用户可访问的本地暂存目录，宿主正常结束时清除；同实例崩溃后由下一持锁宿主清理。用户选定的输出文件也由客户端写入，内容导出先校验并以不覆盖已有文件的方式发布。慢文件或管道受执行超时、客户端断开和宿主关闭约束，不占用宿主事件循环。stdout、stderr、模型 delta 与 `--show-request` 按产生顺序交付，编辑器仍在客户端执行。

SDK 直接接纳调用方路径时，Runtime 使用有界、可复用的文件读取进程；文件打开或读取阻塞不会阻塞事件循环。取消只终止相关读取进程，正常关闭会等待并回收全部进程；父进程崩溃时子进程检测管道关闭并退出。

Windows 的 DPAPI 路径需在真实 Windows 用户账户中验证，包括不同用户无法解密、并发编辑和异常退出后的端点恢复。

## 活动实例上的控制客户端

`open_job_client(locator)` 复用共同 InstanceLocator、配置、资源绑定和 SQLite 文件身份核验，但不打开第二个业务 Runtime、不启动恢复/迁移、不抢实例锁。它仅操作 DBOS 的 job 状态及明确控制，不暴露任意 workflow_name 入队或认知 SQL。使用 retry_connection_errors=False 和有界等待。

本地客户端启动时核验配置和数据库 metadata 属于该安装；路径不符拒绝。读取 schema 版本不改写现有实例。普通错误使用共同 NousError 和 CLI JSON 出口。

SQLite 对 attributes 过滤的限制要在接口中尊重。第一版提供 kind/status 和 SDK 正式支持的分页；不承诺任意 Namespace/全文字段过滤。确需客户端过滤时返回实际扫描范围与 continuation，不谎称过滤后的全库总数。

## 呈现与诊断

默认输出工作名、文稿标题、状态、必要短引用、已完成结果和下一步。后台成功登记输出“正文已保存；维护已排队”；失败工作显示真实阶段/错误及 read/retry 操作，取消后可 resume；排队时仍需明确 worker 是否运行。没有配置模型时，主动要求 annotation 的行为明确失败或拒绝，不能伪造成功。

runtime check 包含 worker 是否启用/运行、队列可用性、当前运行/近期失败摘要和检索代次。worker 未启动是正常状态，不标成整个 Nous 不可用。DBOS 明细通过 jobs owner 查询，不让 presentation 或 cognition 访问系统表。

中文/英文文案经共同 presentation，stdout 保留正文/机器结果，日志到 stderr。JobRef 是运行工作引用；不要改名 WorkRef，以免侵占后续 WorkContext 的认知语义。
