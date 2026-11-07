# 持久认知维护的实际使用

本文通过 CLI 与程序内 SDK 操作正式文稿及持久维护。新实例初始化会同时建立业务表和 DBOS 系统表。

## 不需要模型的起点

沿现有安装方式初始化一个新实例，配置实例profile和默认Namespace。先通过 Note/Journal 手写、read、replace、history、archive/restore 和 lexical query 确认完整基本使用。已有开发数据不得自动清空；结构不兼容的开发实例明确使用新目录。

对一个目标文稿修改时，CLI ref 指目标，expected 指该文稿的已知版本。误填另一篇文稿必须报目标冲突且两篇均不变。名称改名与重新使用不会改变已经取得的域句柄归属。

## 短命令保存并运行待办

配置 `cognition.annotation.task` 指向的标注任务（缺省为 `annotate-concepts`，合同为 `concept-annotation`）和需要的检索空间；模型访问继续用统一网关。示例：

```sh
nous journal create --file ./journal.md --background --annotate --refresh
nous job list
nous job read JOB_REFERENCE
nous job run --until-idle
nous read 'jrn(...)@1' --sources
nous query '文稿所讨论的问题'
```

保存先返回正式 revision 和 Job 引用。没有 worker 时工作是 queued；job run 占有同一实例并处理真实维护。自动标注失败会保留已保存正文，独立刷新仍可让正文检索。通过 job read 查看 ERROR 及失败步骤，再用 `job retry JOB --from-step STEP` 创建接续工作；CANCELLED 使用 resume。更新 Prompt 后要采用新 Prompt 则发起新的维护请求。

## 持续宿主

普通程序用 `async with open_instance(locator)` 取得可嵌入 Runtime，执行文稿和查询操作。需要持久维护时，从应用根入口调用同步 `run_host(locator, application)`，其中 `application(runtime)` 是异步函数，使用同一 Runtime 写作、读取、查询并运行后台维护。CLI `runtime run` 持续运行这一显式宿主。宿主持有应用循环和实例占有，其他进程的普通认知修改继续受实例锁限制。

另外一个终端可以 `job list/read/wait/cancel/resume/retry`，这些操作使用受限管理客户端，不再启动另一个业务 Runtime。wait 超时只结束等待。

## 关闭后继续

运行维护并在有实际工作的节点正常停止。宿主先等待已接纳工作，宽限后取消并等待真实退出，再结束 DBOS 和业务资源。未完成 Job 保留已记录步骤；再次 run_host 或 runtime run 继续。普通 open_instance 不启动队列。已采用结果仍是原引用，旧正文的后台结果不会覆盖新 head。

Job的生成步骤已经完成时，后续采用失败再恢复不能重新发送模型请求。进程在外部响应尚未有本地记录时退出属于另一窗口，诊断要如实记录。

## 原文与来源使用

在Journal中使用普通Markdown引用定义、代码块、CRLF和不连续来源。全文rewrite输入保持原文。查询返回RevisionSelection可以直接作为新的reference/current_basis；纠正其依据后，默认current查询按needs_review规则处理，精确旧引用继续可读。

编辑、来源与模型结果使用实际可读标识符；命令行含空格的完整标识符整体引用。内部UUID与诊断字段按需展开，不占据默认正文输出。

## 观察结果

runtime check 反映宿主状态、当前维护问题与检索代次。Job 成功结果还包括领域 state：superseded 表示保留结果但没有覆盖当前状态，published_not_current 表示构建了精确快照但期间有新写入。

完整示例见[同宿主保存与查询](../examples/durable-cognition/host-use.py)。操作诊断使用 `runtime logs --operation`；wait 的调用超时只结束等待，用户取消 Job 与停止宿主分别保留其持久含义。
