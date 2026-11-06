# 长期宿主与 Job 的使用接口

## 程序入口

```python
async with open_instance(locator, workers=True) as runtime:
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
        result = await runtime.jobs.wait(ref)
        pending.extend(result.related_jobs)
    current = await cognition.records.read(outcome.record.revision_ref)
```

`workers=False` 为默认，不启动 DBOS 执行器，但能登记工作并读取状态。`workers=True` 在同一实例宿主中启动持久维护。任一业务资源只由 Runtime 装配一次；外部已存在 worker 可以正常调用绑定服务。

## 公开值与方法

```python
class JobRef(CognitiveValue):
    instance_id: str
    workflow_id: str

class JobView(CognitiveValue):
    ref: JobRef
    kind: Literal["annotation", "refresh"]
    status: str                 # 保留明确映射的 DBOS 原状态
    namespace_ref: NamespaceRef
    target_revision: RecordRevisionRef | None
    created_at: datetime | None
    started_at: datetime | None
    completed_at: datetime | None
    result: JobOutcome | None
    error: OperationIssue | None

class JobOutcome(CognitiveValue):
    state: Literal["ready", "skipped", "superseded", "published_not_current"]
    result_refs: tuple[str, ...]
    proposal_result: ResultRef | None
    annotation_ref: AnnotationRef | None
    activation_state: Literal["active", "inactive"] | None
    related_jobs: tuple[JobRef, ...]
    captured_generation: int | None
    covered_generation: int | None
```

`JobOperations.list(kind?, status?, limit, cursor)`、`read(ref)`、`wait(ref, timeout?)`、`cancel(ref)`、`resume(ref)`、`retry_failed(ref, from_step)` 为完整入口。直接按引用查看/等待的工作必须核验 instance 与 application 归属。普通列表不默认返回巨大 inputs/outputs；详情按需显示可读目标与结果引用。

DBOS 的 ENQUEUED/PENDING/SUCCESS/ERROR/CANCELLED 等与业务结果分别呈现，不复制持久 job.status 到应用表。任务成功可能是 superseded，用户需要看到其实际影响。取消返回“取消请求已记录/后续执行停止”的真实状态，资源实际结束由宿主执行能力负责，不能把数据库 CANCELLED 误呈现为进程已退出。

`wait` 的调用超时/取消只停止本次等待，不自动 cancel Job。需要取消工作调用显式 cancel。等待不占用业务执行槽，避免 max_in_flight=1 死锁。`resume` 用于已取消或超过恢复尝试上限的工作，默认通过 `queue_name` 重新入原队列，不绕过队列并发限制。`ERROR` 失败终态使用显式 `retry_failed`，依据公开 `DBOSClient.fork_workflow(workflow_id, start_step, queue_name=原队列)` 从失败步骤产生新的 JobRef，保存 `forked_from` 关系；按实际步骤编号选择，不能从起点盲目重发已成功的模型请求。换新参数/Prompt 用明确的新维护请求。

`retry_failed` 在执行前检查原工作确为适用的失败终态、步骤仍与冻结合同相容、已保存计算结果是否可复用以及是否允许再次调用外部模型。若失败源于来源/版本冲突，返回业务错误或 superseded，而不是通过 fork 重试覆盖新 head。旧 Job 保留原错误，新 Job 返回新工作身份；不显示为原工作已恢复。`rewind_workflow` 会修改原步骤记录，因此本轮不用作普通用户的失败重试入口。进程崩溃后的 DBOS 自动恢复无需用户手动发出 `resume`。

## CLI

```sh
nous journal create --file ./notes.md --background --refresh
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

`job run --until-idle` 打开一个持锁 Runtime 并启动 workers，处理当前可运行工作及其即时后续；没有可运行工作且本实例实际业务阶段全部退出时结束。未来延迟工作可保持 queued/delayed，输出其数量/状态，不为了等待远期节点一直占锁。`runtime run` 持续运行直至正常停止，不自行转为操作系统服务或启动第二个 worker 进程。

短保存命令结束后，queued 工作可以等下一次 run。持续 Runtime 占用实例锁时，其他本地认知写命令仍按既有合同返回 instance.in_use；本轮没有任意远程 RPC。程序宿主在同进程使用 Runtime 执行业务；独立 CLI 仅 Job 状态/控制可以使用受限 DBOSClient。

## 活动实例上的控制客户端

`open_job_client(locator)` 复用共同 InstanceLocator、配置、资源绑定和 SQLite 文件身份核验，但不打开第二个业务 Runtime、不启动恢复/迁移、不抢实例锁。它仅操作 DBOS 的 job 状态及明确控制，不暴露任意 workflow_name 入队或认知 SQL。使用 retry_connection_errors=False 和有界等待。

本地客户端启动时核验配置和数据库 metadata 属于该安装；路径不符拒绝。读取 schema 版本不改写现有实例。普通错误使用共同 NousError 和 CLI JSON 出口。

SQLite 对 attributes 过滤的限制要在接口中尊重。第一版提供 kind/status 和 SDK 正式支持的分页；不承诺任意 Namespace/全文字段过滤。确需客户端过滤时返回实际扫描范围与 continuation，不谎称过滤后的全库总数。

## 呈现与诊断

默认输出工作名、文稿标题、状态、必要短引用、已完成结果和下一步。后台成功登记输出“正文已保存；维护已排队”；失败工作显示真实阶段/错误及 read/retry 操作，取消后可 resume；排队时仍需明确 worker 是否运行。没有配置模型时，主动要求 annotation 的行为明确失败或拒绝，不能伪造成功。

runtime check 包含 worker 是否启用/运行、队列可用性、当前运行/近期失败摘要和检索代次。worker 未启动是正常状态，不标成整个 Nous 不可用。DBOS 明细通过 jobs owner 查询，不让 presentation 或 cognition 访问系统表。

中文/英文文案经共同 presentation，stdout 保留正文/机器结果，日志到 stderr。JobRef 是运行工作引用；不要改名 WorkRef，以免侵占后续 WorkContext 的认知语义。
