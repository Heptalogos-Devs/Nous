# HTTP、MCP 与 gRPC

宿主按实例的 `transports.http`、`transports.mcp` 和 `transports.grpc` 配置显式启动监听器。三种入口调用同一应用操作，凭据和 TLS 配置见 [宿主与后台操作](jobs-and-host.md#httpmcp-与-grpc)。每个请求携带 Bearer 凭据；非回环监听使用 TLS。

## HTTP

- `GET /operations` 列出操作；`GET /operations/{name}` 返回请求、结果和事件的 JSON Schema。
- `POST /calls` 接收 `{"operation": "...", "request": {...}}`，返回 `{"value": ...}`。可选 `timeout` 以秒指定本次期限；可选 `select` 是 JSON Pointer 数组，非空时 `value` 为指针到所选值的对象。
- `POST /calls/stream` 接收同一请求，以 SSE 依次发送 `event` 消息以及最终 `result` 或 `error`。事件包含操作 ID 和序号。
- `POST /content?name=文件名` 流式接纳请求正文并返回 Artifact；`GET /content/{完整Artifact引用}` 流式读取该内容。

调用还可通过 `inputs` 按键传入小型内容：每项包含 base64 `data`、`name` 和可选 `source`，应用请求中的 `ExternalInput.key` 使用相同键。较大内容先通过 `/content` 接纳，再在请求中使用返回的正式引用。普通调用的 JSON 请求体有大小限制，内容上传使用流式入口。

`select` 的指针相对操作结果本身，例如 `"/evidence_ref"` 或 `"/selections/0/body_text"`；省略或传空数组返回完整结果。指针语法在操作前校验，结果没有指定路径时返回 `input.invalid_selection`；如果操作已提交成果，错误保留其正式引用。流式调用的事件保持完整，仅最终结果按指针筛选。

```sh
curl -H "Authorization: Bearer $NOUS_AGENT_ACCESS_TOKEN" http://127.0.0.1:8000/operations
curl -H "Authorization: Bearer $NOUS_AGENT_ACCESS_TOKEN" -H "Content-Type: application/json" \
  -d '{"operation":"retrieval.query","request":{"text":"取消后如何释放资源？"}}' \
  http://127.0.0.1:8000/calls
```

端口替换为宿主实际监听值。失败响应包含共同错误对象；上游限流返回 HTTP 429，并在服务提供等待时间时附带 `Retry-After`。客户端断连会取消当前前台调用。

## MCP

MCP 使用 Streamable HTTP，端点为 `/mcp`。工具目录公开检索、材料读取与跟随、阅读整理、Work、反馈和写作等完整操作；每个工具使用对应应用请求 Schema，并以 `structuredContent.value` 返回同一结果。可选工具参数 `_select` 接收 JSON Pointer 数组，与 HTTP 的 `select` 使用相同语义。请求 Schema 非对象时，工具参数使用 `request` 字段承载该请求。请求方提供 progress token 时，应用事件以进度通知的 message 发送。

`content.upload` 接收文件名和 base64 正文，返回正式 Artifact；`content.download` 返回指定 Artifact 的媒体类型及 base64 正文。MCP 内联内容默认上限为 512 KiB；大内容使用 HTTP 或 gRPC 流式交付。

## gRPC

服务名为 `nous.transport.Application`，消息合同见 [application.proto](../../../src/nous/transports/application.proto)。消息使用 protobuf `BytesValue`；其中请求和结果载荷是 UTF-8 JSON 字节，保留整数和坐标精度。认证通过 `authorization: Bearer ...` 元数据传入。

- `Describe` 接收 `{"name":"操作名"}`；省略 name 列出操作。
- `Call` 使用与 HTTP 相同的 operation、request、inputs、timeout 和可选 select JSON。
- `StreamCall` 按顺序返回事件，最后返回结果或错误；RPC deadline 与调用期限取较早者。
- `Upload` 的首条消息是 `{"name":"文件名","source":null}` 元数据，后续消息直接携带内容字节；返回正式 Artifact。
- `Download` 接收 `{"ref":"完整Artifact引用"}`；首条响应是 Artifact、文件名、媒体类型和字节长度的 JSON，后续响应为内容字节。

客户端取消会结束当前调用并等待其资源退出。已提交的成果保持可读取；连接中断后客户端依据返回的正式引用决定是否继续。
