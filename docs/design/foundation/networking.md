# 网络客户端与连接策略

## 选择与所有权

采用 HTTPX 与所选 Provider SDK 实际要求的成熟客户端。Network owner 负责连接配置、客户端生命周期和池复用；Provider adapter 负责协议字段和响应解释。

本模块依赖已经解析的网络设置和生命周期，不在构造时读取全局环境。Trace 使用标准 API，可选埋点不影响请求完成。

## 资源和配置

同一兼容连接配置复用客户端；代理、证书或连接池配置不同的资源使用对应客户端。禁止为每次生成建立并关闭一套 HTTP 客户端，也不把所有服务强行放到一个永远无法独立关闭的全局客户端。

配置包括代理、CA、trust_env、连接/read/pool 超时、连接上限。TLS 校验默认开启。使用者已经通过配置指定的代理和证书传给 SDK，不让底层再独立读取另一套隐含代理设置。

Provider 的 endpoint 和协议方言属于连接资源。通用模型可以连接 OpenAI-compatible 网关，也可以使用已支持的原生协议；网关名称不进入 Journal、配置字段语义或领域模型。

## 期限和重试

实际网络期限取调用剩余总期限与网络分项期限。取消必须到达网络等待并关闭未消费响应。流式响应由异步上下文管理器拥有，调用结束或取消后关闭。

基础 HTTP transport 默认不做隐藏多次模型 POST 重试。生成重试由 ModelExecutor 统一决定。非模型的安全读取可由其具体操作选择重试，仍受同一总期限限制。

## 错误与观测

连接、TLS、超时、限流与协议错误保留可用状态及 provider request_id。响应正文只在确有需要时保存安全摘要，凭据不进入日志。HTTPX/SDK 原生 instrumentor 能满足关联时直接采用。

遥测 exporter 自己的传输由观测装配管理，避免与业务网络相互依赖。

## 完成条件

真实连接复用、代理与超时生效、取消流关闭、客户端正确释放；同一失败不被多层各自重试。依据：[HTTPX Clients](https://www.python-httpx.org/advanced/clients/)、[Async Support](https://www.python-httpx.org/async/)。
