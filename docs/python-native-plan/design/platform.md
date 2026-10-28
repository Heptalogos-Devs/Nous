# 公共基础能力与第三方采用

## 先采用成熟实现

| 能力 | 默认选择 | Nous 保留的语义 |
|---|---|---|
| HTTP / 模型传输 | HTTPX、所选官方 Provider SDK | 网络配置、来源访问范围、超时/取消与预算关系 |
| 模型生成 | Pydantic AI slim 原生 Provider、typed output、直接请求 | Role、调用身份、Prompt版本、实际输入、付费结果采纳 |
| 特有多模态 embedding / rerank | Provider 官方 Python SDK及既有成熟调用代码 | 输入parts、精确区域、向量空间、usage与来源 |
| 数据校验 | Pydantic 2 / pydantic-settings | 配置继承、角色能力、业务不变量 |
| 模板 | Jinja2 | Prompt资源、历史、权限、变量目录与激活 |
| SQL | SQLAlchemy 2 Core / Alembic / SQLite | 事务中心、修订、接触与来源关系 |
| 作业 | DBOS Python 3 / SQLite | 请求与任务的业务归属、形成条件、owned取消 |
| API | FastAPI / Uvicorn | 正式领域操作、认证、错误、可用性 |
| CLI | Typer / Rich | 高密度可读结果、完整参数、--json、stderr日志 |
| MCP | 官方 Python SDK | 现有工具、资源、实际scope及上下文能力 |
| 观测 | logging / structlog / OpenTelemetry | 业务事件、模型费用、来源与调用关联 |
| 文件 / 媒体 | pathlib、tempfile、标准库hash、Pillow、FFmpeg/ffprobe | Material、Artifact、区域、失效资源保全与派生谱系 |
| 目录 / 密钥 | platformdirs、keyring、环境SecretRef | 安装实例、运行用户、凭据用途和管理权限 |
| 国际化 / 时间 | 保留Fluent资源并用Python实现；Babel、zoneinfo、datetime、python-dateutil | 产品语言、日期精度、时间域、五轴和知情截点 |

库的稳定补丁版本在云端生成 lock；只安装当前实际使用的 Provider extras。模型服务优先选择 Pydantic AI 的 typed Provider 与结构化结果支持，不同时堆叠另一套 LiteLLM Router、LangChain Runtime 和自行开发的 Agent框架。现有兼容网关可作为 OpenAI-compatible Provider 连接。

## 模型与网络

Provider 保存协议、endpoint、SecretRef、网络策略；Model 保存实际模型名、模态、上下文和参数能力；Route 保存一套可选择的参数；Role保存routes、Prompt与预算。继续沿用交接分支已经建立的资源图语义，Python采用现成SDK完成底层协议。

Pydantic AI 的 Provider 可能依赖不同版本的 HTTPX 家族客户端；按已安装版本接受的 client 类型创建并注入，不把另一个类强制塞入所有SDK。网络服务统一的是配置、身份、代理、CA、超时、并发和生命周期；具体 transport 由SDK成熟实现承担。保存对云端 HTTPS 代理、环境证书与 trust_env 的支持。

结构化形成使用明确的 Pydantic 输出类型，普通描述保留自由文本。三种解释策略使用同一模型工作服务：description_only、direct_structured、describe_then_structure。二段策略第二次同时输入原媒体/区域与第一段真实文本。不要用第一段文本替代实际图像而声称完成二段路径。

文本、视觉、图文联合向量保持模型、维度、归一化、space、producer与内容身份。Ark 特有向量协议先采用官方 SDK 和 MaiBot 当前调用代码；现有 `models/ark_embedding.rs`、`embedding_request.rs` 作为参数与响应语义来源。现成SDK未暴露的字段在该Provider模块改编，其他领域仍使用完整Embedding结果。

调用入口先验证角色能力和资源预算，形成实际请求快照；调用发生在数据库事务外。SDK retries、workflow retries与路线切换统一由模型服务策略安排，费用账本记录每次真实尝试。已成功的自由描述、结构化结果和向量被重复任务引用，避免为恢复重复付费。

## Prompt 与上下文

保留当前独立 Prompt 文件、目录和模板变量。MiniJinja→Jinja2 逐份查看实际语法，采用等价模板功能；用真实调用输入预览找到变量、空白或转义差异。模板默认资源随Python包发布，用户覆盖和修订保存于实例，编辑/激活通过同一管理API。

Prompt有稳定ID、用途、语言、版本、变量schema、默认资源和覆盖来源。人类可列表、读、编辑、预览、激活和恢复。一次调用记录确切Prompt版本、变量、媒体访问、最终messages和有效模型配置，密钥不进入该快照。

Context不是Prompt拼字符串的附属品。context owner保留当前身份、用途、听众、WorkContext、ResidentSet、来源范围、选区、预算与prepared/admitted/used的实际关系。SDK每个实际模型调用前准备；模型返回、工具续轮与压缩后更新按现有正式合同迁移。worker、世界作者、维护者和主体保持消费者身份，后台取得不直接形成主体知识。

## 配置来源与生效

各模块声明一份typed配置，composition注册。默认值→部署文件→Instance→Namespace→Subject→实际允许的角色覆盖，保留每项来源。当前分支按配置key整体替换，继续采用此规则；完整父对象key只由一个owner注册。例如 `integrations.amemorix.imports` 是 `integrations.amemorix` 内的字段，不再独立注册重叠key。

管理UI可对对象内字段做patch，但交由其owner构造完整候选值后验证，存储仍是一份该key的覆盖。数组整体替换；clear恢复继承。跨字段与下级范围约束由该配置owner检查，单次变更原子写入历史。

active、desired和restart-required有实际含义。Live配置先建立新资源快照，在调用边界切换；旧网络client有活跃调用时由引用/任务完成后关闭。Restart配置保持desired直到资源成功重建。Prompt版本按调用开始时确定。配置保存失败与实际应用失败分别记录，不能显示一个尚未被运行资源使用的“已生效”值。

安装bootstrap只含必要启动项（instance目录、监听、受保护凭据引用、日志位置）；常规模型、检索、角色与维护配置通过正式配置服务管理。CLI离线编辑使用相同schema，不能维护另一个默认值集合。

## 身份、时间和来源

Namespace、Subject、Caller、Consumer、Material、Observation、CognitiveRef保持各自含义。恢复同一Subject保留身份，分支Subject获得新身份并引用合法的历史。用户实际数据与资料范围经所有正式入口一致处理，MCP工具不能通过模型参数扩大授权。

Nous负责认知时间：五轴、已知/未知精度、历史截点、时间域和宿主提供的CognitiveClock。Hept继续拥有世界推进与日历。现实服务时间、队列timeout和模型费用周期不使用虚拟世界速率。

交接实际遇到了无offset日期，Python时间owner必须表达 calendar-date、local-date-time、zoned-instant与unknown等精度。date不能靠补午夜UTC变成事实；timezone只能来自明确输入或已授权域配置。模型原始表达和解析状态保留，查询决定怎样匹配该精度。普通状态读取不推进主体时间。

来源清除同时处理派生、context、检索与缓存；系统保留必要身份与明确删除事实，不能从历史bundle、日志或模型快照重新泄出已清除正文。真实授权数据的导入和机器间移动按对应操作处理。

## 运维、CLI、MCP与界面

默认一个服务进程运行DBOS与HTTP/MCP，CLI在线管理走同一服务；离线初始化、备份检查、路径设置等只在对应模式获得实例锁。`mcp stdio`连接已配置服务；MCP标准输出只发协议，日志写stderr。正式SDK继续复用当前 `sdk/python` 的 client、managed、imports、files、sources和A-Memorix模块。

CLI默认给人类和模型读内容、短引用、状态和下一项可用操作，`--json`输出严格对象；不要输出整份数据库行、签名或内部owner解释。query/search支持分批展开，避免为省token删掉查找下一页与精确引用能力。

Fluent/i18n覆盖CLI、错误、配置说明与管理页面；日志code和领域身份不随界面语言改变。Web管理按当前宪纲保留完整目标，复用MaiBot/A-Memorix已有管理消费者和组件；本轮不另建Node业务后端。浏览器通过同一HTTP contract管理配置、Prompt、模型、记忆、来源、任务、导入和恢复。

## 可观测性与资源生命周期

logging→structlog统一字段，OpenTelemetry按现成FastAPI/HTTP/DBOS接线传播trace。关键关联为operation、namespace、subject、invocation、job、source；正文默认不写普通日志，授权诊断从调用账本读取。

业务错误类别保持现有InvalidArgument/Forbidden/Conflict/Unavailable等语义及领域code，入口映射成对应协议。日志只在负责处理失败的边界记录stack，避免每个模块重复异常包装。

启动用AsyncExitStack和TaskGroup按依赖创建/关闭资源：配置/凭据→DB→native索引→模型client→DBOS→API。停止先停止接纳新写与取消可选后台工作，等待已有短事务及实际I/O收尾，关闭client、native reader和DB。使用成熟进程/服务管理和现有权限处理来源，不能把每种平台写成完整自制守护程序。
