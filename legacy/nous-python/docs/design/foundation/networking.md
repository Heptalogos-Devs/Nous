# 连接资源与模型协议

ModelConnections 负责复用正式 Provider 的 SDK/HTTP 连接，Credentials 负责实际凭据引用。当前锁定的 HTTPX2、Pydantic AI 和官方 SDK 继续承担协议与网络；无需因名称改变重写网络机制。

网络覆盖由配置定义者 NousConfig.provider_network 解析为完整 NetworkConfig：缺失字段继承，proxy_url/ca_file 显式 null 清空，非空标量显式 null 报错。稀疏覆盖在配置快照序列化时保持其字段存在性。连接创建参数、endpoint/protocol/credential 和实际 credential 位置按 restart；本次请求超时从操作快照读取。ModelConnections 不自行再读另一组环境或重复默认值。

代理、证书和连接池相同的连接按实际 Provider 生命周期复用。模型名只作为请求数据。compatibility 仅表示 SDK 支持的协议能力，不进入认知业务，也不根据某厂商名字创建特殊逻辑。

SDK 隐式重试关闭，模型请求共同策略负责已知尝试。取消关闭响应流；服务错误保留状态和 request_id，实际凭据不进入普通结果/日志。所有已创建客户端归组合根关闭，构造失败的部分资源在本层清理。

转写、Embedding 与 Chat 可以是同一标准兼容协议的不同端点，复用同一连接。New API 容器和管理端点、开发语音合成工具在生产调用路径之外。
