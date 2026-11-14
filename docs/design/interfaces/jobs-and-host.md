# 宿主与运行期操作

## 程序入口

```python
from nous.cognition.writing.contracts import AfterWrite, CreateJournal
from nous.cognition.writing.operations import CREATE_JOURNAL
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import open_instance
from nous.runtime.operations import WAIT_OPERATION, SelectOperation


async def application(locator):
    async with open_instance(locator, allow_background=True) as runtime:
        outcome = await runtime.app.call(
            CREATE_JOURNAL,
            CreateJournal(
                body=InlineText(text="正文"),
                after=AfterWrite(annotation="run", refresh="run", execution="background"),
            ),
        )
        if outcome.maintenance.execution == "started":
            completed = await runtime.app.call(
                WAIT_OPERATION, SelectOperation(ref=outcome.maintenance.operation_ref)
            )
            return outcome.commit, completed
        return outcome.commit, outcome.maintenance
```

普通 `open_instance(locator)` 提供可嵌入的 Runtime。`allow_background=True` 允许该作用域持有后台维护，退出时等待或取消仍在执行的工作。同步应用可使用 `run_host(locator, application)` 运行同一 Runtime 和应用循环；`runtime run` 在该作用域提供本地服务。

## 后台操作

正文事务提交后，维护可以在调用中等待，也可以在已启用后台能力的 Runtime 内启动。没有宿主的短命令显式请求后台时，在写入前返回 `execution.background_unavailable`。提交后若宿主停止接纳或后台容量已满，正文继续可读，维护结果报告 `not_started`。

`RunningOperationRef` 包含实例、启动和操作身份，仅在当前运行期有效。`runtime.operations.list/read/wait/cancel` 查看、等待或取消当前进程持有的工作；完成后保留有限数量的短终态。`wait` 被取消或超时只结束等待，显式 `cancel` 才取消后台操作。

退出或中断后，已保存的原件、结果、草稿、修订和快照按正式引用读取。未完成维护可以用新的明确请求重新发起。

## CLI

在一个终端运行宿主，在另一个终端发起操作：

```sh
nous runtime run
nous operations list
nous operations describe records.create_journal
nous journal create --file ./notes.md --annotate --background --refresh
nous runtime operations list
nous runtime operations read <操作标识>
nous runtime operations wait <操作标识>
nous runtime operations cancel <操作标识>
```

`--background` 选择后台维护；不使用该选项时，明确请求的维护在本次调用中等待。`--inline` 与 `--background` 不能同时使用。正文已保存而后续维护失败时，结果保留正文引用和具体问题，CLI 使用部分成功退出状态。

## 本地传输和文件

本地服务只绑定 `127.0.0.1` 的临时端口。POSIX 使用用户私有目录和端点文件；Windows 使用当前用户作用域的 DPAPI 保护端点密钥。双方通过随机挑战和 HMAC 认证，认证完成前不发送业务请求。连接丢失后不自动重试写入。

客户端在请求作用域内声明外部输入并交付文件或流；相对路径由客户端解释，宿主分配的暂存位置不成为正式来源名称。请求期限覆盖容量等待、输入准备、传输及业务执行。运行期等待和取消使用独立的连接容量。客户端断开时宿主取消该调用并等待其资源退出；宿主关闭后清除本次输入暂存。内容导出由客户端向目标目录写入临时文件并在完成后交付，已保存的 Artifact 独立保留。

SDK 的文件输入使用有界文件读取进程。取消只终止相关读取，关闭时等待全部受控进程和阻塞工作实际退出。日志和正式结果保持实例关联，不接管宿主进程的 root logging。

## HTTP、MCP 与 gRPC

`runtime run` 同时启动实例配置中明确启用的监听器。每个监听器配置地址、端口和 `credentials` 中的凭据引用；默认均未启用，地址默认为本机回环。端口设为 0 时由系统分配，启动后显示实际地址。

```toml
[credentials.agent-access]
kind = "env"
name = "NOUS_AGENT_ACCESS_TOKEN"

[transports.http]
enabled = true
host = "127.0.0.1"
port = 0
credential_ref = "agent-access"
```

`transports.mcp` 和 `transports.grpc` 使用同一配置形状。非回环地址需同时设置 `tls_certfile` 与 `tls_keyfile`；相对路径以实例目录为基准。客户端提供 Bearer 凭据；配置只保存引用，凭据值由环境或受管理文件提供。

HTTP 提供 `/operations`、`/operations/{name}`、`/calls`、`/calls/stream` 和内容上传下载。MCP 以可发现工具提供面向 Agent 的完整应用操作和内容交付。gRPC 的 `nous.transport.Application` 提供描述、调用、事件流及内容流操作。三者使用同一 Application 请求、结果、事件与错误合同；取消和期限传递到当前调用。宿主退出前停止接入并等待正在使用的资源退出。

程序内宿主可在 `open_instance(..., allow_background=True)` 作用域中使用 `serve_configured_transports(runtime, locator)`，或分别借用传输的 `serve_http`、`serve_mcp`、`serve_grpc` 上下文管理器。

具体路径、消息和内容流格式见[远程协议](remote-protocols.md)。
