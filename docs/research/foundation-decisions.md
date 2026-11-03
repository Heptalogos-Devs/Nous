# 基础服务研究与技术选择

## 资料与决定的来源

项目资料以已保存的 `docs/sources/001/01.md`、`02.md`、`03.md` 和本轮用户讨论为基础。仓库本次核实的 docs 目录包括 sources、legacy、agent；新增两轮原文尚未在该目录出现。本次没有阅读产品代码，也不据旧实现推断已完成能力。

用户明确确定的内容包括专题文档、设计持续可改、基础服务前置、两阶段日志、配置的多个维度和分层呈现、危险参数需慎重、跨平台路径、多轴时间以及模型任务保持专一。

本包据此做出的具体工程选择包括：首轮范围、配置 null/unset 与快照规则、不可变数据绑定、SQLite 非 legacy 事务、时间不确定性数据形状、直接采用 OTel API、Jinja 编译合同、后续 DBOS 同库事务入队。它们是本次设计结论，不是代码已经具备的事实。

外部技术资料于 2026-11-03 检索。采用当前稳定接口，实施时在项目版本线内锁定实际依赖；文档不以网页出现的补丁版本代替项目锁文件。

## 配置机制

Pydantic/pydantic-settings 已提供类型、字段约束及多来源读取。Nous 应维护自己的覆盖范围、有效值来源、生效时机和绑定影响，避免重复实现解析器。TOML Kit 保留编辑时的注释和布局。

选定：类型与元信息在各 owner 的纯配置模型声明，统一合并并生成目录。TOML 显式清空用 null_fields，缺失继续继承。数组整体替换。已有数据的身份字段走 new_binding 规则。

资料：[Pydantic Settings](https://docs.pydantic.dev/latest/concepts/pydantic_settings/)、[Pydantic JSON Schema](https://docs.pydantic.dev/latest/concepts/json_schema/)、[TOML Kit](https://tomlkit.readthedocs.io/en/latest/)。

## 启动、日志与 Trace

标准 AsyncExitStack 能组合异步资源关闭；logging 处理器提供 stderr、文件和轮转。OpenTelemetry 把埋点 API 与应用 SDK/导出分开，适合基础模块不依赖业务 TraceService 的要求。

选定：先 stderr，随后正式本地日志和 OTel SDK；模块直接埋点，组合根管理输出。默认日志有操作关联，完整 Span 导出按配置启用。本地运行不要求遥测后端在线。

资料：[contextlib](https://docs.python.org/3.14/library/contextlib.html)、[logging handlers](https://docs.python.org/3.14/library/logging.handlers.html)、[structlog](https://structlog.org/en/stable/standard-library.html)、[OpenTelemetry](https://opentelemetry.io/docs/languages/python/instrumentation/)。

## 时间

Python 提供独立的单调计时及带时区日期处理。zoneinfo 在缺少系统 IANA 数据的平台需要 tzdata；跨平台项目应显式依赖它。

选定：单调时间用于进程期限，UTC 用于实际记录，独立 domain+ticks 用于认知坐标。Occurred/observed/formed/valid/recorded 的归属不同，入站时间不替代未知业务时间。未知边界与无界边界分别编码；粗粒度事件采用可能时间区间。

资料：[time](https://docs.python.org/3.14/library/time.html)、[datetime](https://docs.python.org/3.14/library/datetime.html)、[zoneinfo](https://docs.python.org/3.14/library/zoneinfo.html)。

## 数据库与持久执行

SQLAlchemy 文档说明 SQLite 驱动默认 legacy 事务模式对 SELECT/DDL 等处理特殊，并提供现代事务模式配置。DBOS 提供在调用者 SQLAlchemy 事务中入队的能力，必须使用同一系统数据库，异步连接用 run_sync 桥接。

选定：SQLite/SQLAlchemy Core/aiosqlite；明确非 legacy 模式，短事务由完整操作拥有。后续 DBOS 同文件接入，不另加应用 outbox 搬运。DBOS Client 当前对类方法入队有限制，计划采用命名普通函数入口。

资料：[SQLAlchemy SQLite](https://docs.sqlalchemy.org/en/20/dialects/sqlite.html)、[DBOS Client](https://docs.dbos.dev/python/reference/client)、[DBOS Queues](https://docs.dbos.dev/python/tutorials/queue-tutorial)。

## 模型、Prompt 与缓存

Pydantic AI 已提供原生模型请求、消息、输出和 fallback 能力。Jinja 支持 StrictUndefined 和 bytecode cache。模型提供方的前缀缓存依据最终请求和实际缓存命中情况，不能用应用 Session 代替测量。

选定：统一 ModelExecutor；任务定义小而明确；Jinja 渲染有角色的 Markdown；先准备确定请求再执行。模型备用、修复和重试共享预算；流输出已经交付后不静默换模型拼接。模板、Provider 前缀和业务产物复用分别管理。

资料：[Direct requests](https://ai.pydantic.dev/direct/)、[Output](https://ai.pydantic.dev/output/)、[FallbackModel](https://ai.pydantic.dev/api/models/fallback/)、[Jinja](https://jinja.palletsprojects.com/en/stable/api/)、[OpenAI 提示实践](https://developers.openai.com/api/docs/guides/prompt-engineering)、[OpenAI reasoning 实践](https://developers.openai.com/api/docs/guides/reasoning-best-practices)、[OpenAI 缓存](https://developers.openai.com/api/docs/guides/prompt-caching)、[Claude 缓存](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)。

## Embedding 的数据绑定

更换编码模型通常需要从原输入重新生成向量。向量维度只描述形状，不能定义整个空间含义；模型、输入预处理与查询/文档规则需要共同记录。

选定：显式空间 ID 与描述绑定。首次资产采用后禁止同 ID 改语义字段，普通配置更改不能完成迁移。开发期允许暂停语义检索后重建，不引入零停机双写要求。

资料：[Pydantic AI embeddings](https://pydantic.dev/docs/ai/guides/embeddings/)、[Qdrant embedding 模型更换](https://qdrant.tech/documentation/tutorials-operations/embedding-model-migration/)。资料中的 Qdrant 迁移机制未作为本项目选型。

## 其他成熟基础

逻辑路径复用 platformdirs 与 pathlib；跨平台本地实例占有采用 filelock 原生锁；网络采用 HTTPX/官方 SDK；本地化采用 gettext。

资料：[platformdirs](https://platformdirs.readthedocs.io/en/stable/api.html)、[filelock](https://py-filelock.readthedocs.io/en/stable/concepts.html)、[HTTPX](https://www.python-httpx.org/advanced/clients/)、[gettext](https://docs.python.org/3.14/library/gettext.html)。

## 本次停止点

本轮已经可以指导运行基础施工。认知对象表结构、Wave 索引更新、正式向量迁移、完整远程权限与业务媒体处理由后续专题继续设计，不影响当前实例、配置、时间、观测和数据库能力建设。
