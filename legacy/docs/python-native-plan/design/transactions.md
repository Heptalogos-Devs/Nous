# 事务、持久作业与索引

## 权威事务

每个 Nous 实例使用一个 `data/authority.sqlite`。SQLAlchemy 2 Core、aiosqlite 与 SQLite WAL 提供读快照和短写事务。读连接使用 `BEGIN`，写连接使用 `BEGIN IMMEDIATE`；同一次业务修改共享一个 `WriteTx`。

```python
async with authority.write(ctx) as tx:
    source = await material.admit_text_in(tx, ctx, document, lease)
    record = await cognition.adopt_in(tx, source.context, command)
    await enqueue_index_in(tx, ctx)
    return record
```

`Authority.read(ctx)` 返回 `ReadView(connection, commit_seq)`。`Authority.write(ctx)` 返回携带连接、认证上下文、提交序号和记录时间的 `WriteTx`。写事务成功退出才提交；异常回滚领域修改、时钟与同事务接纳的作业。业务函数直接返回领域结果，提交由事务上下文成功退出决定。

模型、下载、FFmpeg、Embedding 与索引构建在写事务外执行。取得结果后，业务用例重新核对实际来源授权和预期修订，在一次短事务中采纳结果及安排后续工作。参与者接收已有事务，不自行提交。

材料、Observation、Representation、Region、认知修订及已采用向量均在 Namespace 内解释身份。认知修订键为 `(namespace_id, id, revision)`，向量键为 `(namespace_id, reference, space)`。SDK 和 HTTP 读取显式选择 Namespace；来源授权与短引用使用同一绑定域。

## 原子持久工作

DBOS 系统表与领域表位于同一 SQLite 文件。`tx.enqueue(JobSpec(...))` 通过 `AsyncConnection.run_sync` 将当前连接传给 DBOS 的公开 `enqueue_in_transaction`，由外层业务事务统一提交。

`JobSpec.scope` 由业务明确设置。`nous_job_links` 保存作业身份、提交者、Namespace、业务范围、父工作及提交序号，调度状态由 DBOS 拥有。提交者使用稳定 `workflow_id` 重复接纳同一命令时返回原作业；相同身份携带不同命令或范围时发生冲突。作业参数传持久请求引用和必要小值。

DBOS 负责领取、暂停、重试、取消与重启接续。导入单元输出、文件接纳资格、认知修订和向量采用由各业务表保存。

## 业务幂等

普通创建、修订和退役以事务提交为准。外部重复投递使用真实事件身份：实际接触按 `(namespace_id, caller_id, external_event_id)` 唯一，Host 事件按 `(namespace_id, caller_id, event_id)` 唯一；重复输入重新核对当前来源权限，改变实际内容时返回冲突。

导入请求保存自己的接纳状态与结果；同一处理运行和单元的输出具有唯一约束。文件清单按范围、文件身份及处理策略判断接纳资格。新的真实接触保留独立 Observation，即使读取同一原件或产生相同文字。

## 索引与备份

认知与来源修改在同一事务接纳索引维护任务。索引由正式认知、材料与已采用向量生成；查询先确定已授权候选，再进入 Native 检索，结果物化时复核当前来源状态。

安装备份使用 SQLite online backup API，内容文件按该快照保留的真实引用枚举。DBOS 状态与已提交 WAL 随同一数据库保存。Namespace Bundle 保存当前正式对象、精确来源和修订；接收域内的身份由 Namespace 限定，工作运行按业务创建本地执行身份。

应用表使用当前唯一 SQLAlchemy metadata 初始化。开发数据采用当前 Schema 重建，DBOS 独立维护其系统表。
