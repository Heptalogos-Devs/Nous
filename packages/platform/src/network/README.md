# 网络底座

`createNetwork()` 组合 Undici 8.10.0、Node 24 原生 Fetch/Web Streams、ipaddr.js 和 LRUCache。设置由同一产品的 `Settings` 实例持有，网络层维护实际连接与流的生命周期。

```ts
const network=createNetwork({
  product,installationId,processBootId,settings,authorize,
  resolveResourceNetwork:resources.networkSelection,
  withMaterial:secrets.withSecret,
});
await network.withClient(ctx,{
  resource:{
    resourceRef,operation:'resources.read',profileId:'default',
    allowedOrigins:['https://example.org'],allowedPathPrefixes:['/documents'],
    methods:['GET','HEAD'],addressScope:'public',credentialOrigins:[],
  },
},async client=>{
  const response=await client.fetch('https://example.org/documents/a');
  // 在回调中消费或保全完整body；返回Response前关闭客户端会取消这条流。
  await preserve(response.body);
});
```

资源 owner 从已登记资源及当前权限建立 `ResourceNetworkScope`。`operation` 是宿主指定的真实操作；API body 与模型参数不直接提交这个内部授权对象。Subject/Role 配置范围须与真实 ctx.subjectId 一致。

`client.fetch` 接收 Node 原生 Request/FormData/ReadableStream，返回原生 Response。每次实际 dispatch 重新检查权限及秘密代际，跨 origin 去掉 Authorization/Cookie；标准 body 消费、clone、取消与背压继续生效。响应上限作用于真正消费的字节，超限报告 `network.response_too_large`。

`client.webSocket()` 异步核验后创建 Undici 原生 WebSocket，支持受限 headers、协议、取消和接收字节上限。SOCKS5 沿 Undici 的原生实验性 API；当前 SDK 在 IP 目标的 TLS SNI 上会产生 Node 弃用提示。

`networkSettingsModule` 注册到 `platform.network`，消费者为 `network.factory`。`metadata` 给出实际 resourceRef、profileId、scope 和修订向量。新客户端取得新设置快照；旧池由 LRU/设置通知退出，实际使用中的池等待所属客户端关闭。

`network.closeResource(ref)` 按稳定对象身份关闭所有修订的连接；`closeScope(scope)` 包括 Subject 下的 Role，供删除/禁用 owner 调用；`close()` 取消本实例的请求和 WebSocket 并关闭全部池。

代理的 NO_PROXY 选择直接改编 Undici 同版本私有路由函数，保留 MIT 来源及许可证，并与 EnvHttpProxyAgent 共用同份环境快照。实际直连使用真实 connector lookup 检查 IP。远端 DNS 的代理线路须由 `authorizeProxyRoute` 对其真实服务端 ACL 作出授权；无法确认时返回 `network.scope_unavailable`。

代理 SecretRef 内容使用 `{username,password}` JSON；CA、客户端证书、私钥分别采用 `network.ca`、`network.client_certificate`、`network.client_key`，代理采用 `network.proxy`。`withMaterial` 必须维持 SecretRegistry 回调边界。池缓存包含真实 generation，每次 dispatch 仍重新取得当前代际；材料在池关闭后离开使用回调。

`inspect` 只输出 profile 标识、代理模式、TLS 验证状态和连接计数。`probe` 的 DNS 分支明确输出本机解析结果；TLS/HTTP 使用资源授权的 HEAD 请求。模型生成测试由模型 owner 记录预算。

`tests/network-database.test.ts` 用真实 PostgreSQL Settings 和本地 HTTP 验证版本接缝、流及取消。TLS、CA Secret、HTTP/HTTPS CONNECT、认证 SOCKS5、WebSocket、NO_PROXY 与 connector DNS 的完整实测见施工包研究记录。

TLS/代理秘密绑定真实 `NetworkProfile` Ref。`withMaterial` 给出该 Profile 的 resourceRef 和当前消费资源 useRef；SecretRegistry 对这些 purpose 使用 `resolveUse:network.credentialTarget`。后者从 `resolveResourceNetwork(ctx,useRef)` 重读资源，验证其当前 Profile 和具体秘密/purpose，再返回资源本来的 operation/target。不同模型或渠道可共享一份 Profile 秘密，Subject 无需额外 network.use。每次派发核验当前 owner，资源范围或 Profile 已改变时返回明确冲突，调用者建立新客户端。
