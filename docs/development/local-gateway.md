# 本地 Docker New API 接入

## 本次实际环境

用户使用本机 Docker 部署的 New API，并已授权 Coding Agent 直接完成本项目的连接与真实调用。网关位置、容器名、端口和 Key 不在本包中猜测；从实际运行环境取得。费用无需重复确认，金额上限不启用。

本文件是开发环境接入指南。生产代码只使用通用 protocol/base URL/credential_ref/model 配置，不内置 Docker、New API 后台 API 或厂商分支。

## 定位现有部署

在目标开发机使用 `docker ps`、`docker port <container>` 和必要的 `docker inspect` 读取现有映射。查看与当前项目有关的本地配置、Compose 信息或已有受控环境，确定该 Coding Agent 所处网络中可以到达的地址。

宿主机可达的 127.0.0.1 与另一个容器内的 127.0.0.1 含义不同。根据实际部署使用端口映射、已配置容器网络或宿主地址；不为解决连通性在 Nous 内增加代理服务。

复用现有 New API，不自行升级、重建、清库或修改其它应用渠道。

## 取得调用凭据

优先复用该环境已有可用于本项目的模型 API Key。可从已知本地配置、环境或已授权控制台/API 取得；无需要求用户在聊天里再粘贴一次。

需要新项目 Key 时，使用现有登录 Session 或管理 Access Token 调用部署版本支持的令牌管理接口。管理凭据与用于 `/v1` 调用的模型 API Key 是不同用途；管理请求可能要求 `New-Api-User` 与真实用户相符。创建、读取完整值的细节以正在运行的版本为准，不根据列表中的掩码拼接密钥。

已安装的官方 newapi 工具/skill 如果能完成当前操作可以直接使用，不为获取 Key 另建插件。只有真实访问能力缺失才报告所缺连接；不猜默认管理员密码，不清空网关身份。取得后写入忽略的开发实例 credentials 文件或已存在环境引用，不放进产品模板。

## 模型发现与配置

对实际 `/v1/models` 进行明确查询，得到此 Key 可访问的模型。选择文字、视觉/音频或转写、结构化和文本向量所需资源，在实例 TOML 配置其用途。不要把网关模型名写进 Python 分支或公共默认配置。

模型目录只列名称。用本轮正式工具对实际需要的能力完成调用，并保存看到的形状与结果。原生视频、PDF 或多模态向量如果协议没有支持，采用规格中的通用准备或明确报告具体缺项。

配置示例见 [models.toml](../examples/models.toml)。endpoint 指实际 `/v1` 根，只写一次；实际 Key 使用 file/env 引用。调整本地参数服务于真实资料的大小，不因本包中的初始默认阻塞已经授权的操作。

## 不作为产品行为的操作

定位 Docker、取得管理 Token、创建网关 Key 都是本地接入步骤。Nous CLI 不增加 `newapi login`、渠道管理、余额查询或供应商特殊命令。

## 官方依据

[部署与起步](https://docs.newapi.pro/zh/docs)、[管理鉴权](https://docs.newapi.pro/zh/docs/api/management/auth)、[API Key](https://docs.newapi.pro/zh/docs/guide/feature-guide/user/token)、[官方 coding-agent 工具](https://docs.newapi.pro/zh/docs/skills/newapi)、[模型接口](https://docs.newapi.ai/en/docs/guide/feature-guide/user/api)。
