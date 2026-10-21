# 当前施工

## Objective

先完成Nous Wave与Heptalogos的公共工程底座，再恢复全部特有认知与生活语义。保留既有能力和有效算法，减少自有机械、重复状态、内部转换和维护成本。23类基础能力按实际依赖组织为少量普通库，各产品独立持有状态。

## Decisions

- 需求以工作区Specs/项目宪纲和当前用户修正为准，现行架构/详规是施工入口；原Vault和旧设计副本退出当前权威。
- 两产品、两数据库；公开contracts/client/platform独立构建和打包，Hept不编译相邻Nous源码。
- 通用能力优先直接依赖成熟库；有价值源码先复制再在owner改编。OpenClaw原生执行/历史/工具与DBOS持久工作继续采用。
- 普通短事务直接Kysely，事务client显式传入当前操作；只有真正持久工作使用官方datasource/checkpoint。无第二队列/租约/工作流引擎。
- 配置、权限、来源、时间、费用与trace从第一条真实链进入正式结构。NousQL/VCP/Native、认知五轴与Hept生活能力继续保全。
- 只保留少量真实贯通场景与必要规则；删除临时开发支架和被替代形状测试。新下载/缓存/实验在temp，已有data研究资料就地读取。

## Current Work


当前先闭合 Hept Task → 受限 Worker → Nous 当前来源与上下文 → OpenClaw 原生模型/工具 → 选定成果发布 → Subject 正式读取。逐调用权限、时间、费用与实际接触沿同一身份保存；取消阻止后续效果并保留已发生事实。完整 Gateway 制品和空启动验证同时推进。

接着完成真实外部 ClockSource 连接、完整媒体与扩展管理、受限诊断与 Web/i18n、全部 owner 数据生命周期和跨平台发行。正式 SDK 发行源及 Hept 正式 lock 尚待收口。完整 NousQL/VCP/认知与生活语义在基础底座完成后继续。

### 已通过的基础链

基础能力已落共享普通库：Bootstrap/目录/OS服务、受保护引导凭据、pg/Kysely/官方datasource、配置稀疏覆盖与历史、当前认证/委派/Secret、Undici网络、Pino/OTel、Prompt资源与模型五族。各产品仍持有自己的迁移、状态和组合根。

空安装真实链已经通过：CLI init/auth init → Host/HTTP → SDK状态与空Subject → 配置CAS/原提交重读/持久继承 → Secret轮换/撤销 → 停止/重启 → 正式登录与凭据撤销。本地managed PostgreSQL已通过显式官方平台取包、SCRAM初始化、嵌套管理连接、pg_ctl正常停止与持久重启。离线Recovery使用Better Auth原生hash/internalAdapter，在同一事务修复选定管理员并撤销其人类会话，实际新密码登录通过。

初始化和停止使用Node24原生AsyncDisposableStack按owner释放资源，初始化失败真实验证无遗留数据库连接。普通事务明确传client到来源、材料、配置和原生Better Auth guard；专用LISTEN/Clock authority session由同一storage建立，不占业务池。精确时间保留整微秒，金额为十进制Money。Actor/Consumer独立，稳定Ref保留selector.pointer。Subject持续授权以真实用户为parent、实际issuer为审计来源，临时CLI登录过期不结束已明确开启的主体；当前撤销仍生效。

模型平台已通过正式Auth/Settings/Secret/Network/Material/Blob/Encounter/Time的五族实际HTTP与官方DBOS场景，包括工具循环、流、embedding批并行、重排/转录/语音、费用保全及unknown不重发。Hept分段Clock已通过单连接池、wall回退/mono单调、离线/暂停/变速、高水位恢复、DST和真实DBOS期限重投影。

Nous已替换旧ModelResource/Invoice与硬编码Prompt，Provider/Model/Role/价目/祖先limits分别通过正式Settings采用，18个既有职责及native_agent接缝与15份资源已接实际SDK messages。原认知长场景通过来源撤权、组事务回滚、种子并发、Selector.pointer、结构错误输出与费用保全；安装级模型测试保存独立ModelArtifact，不创建主体。另一个正式五族场景以15次实际HTTP贯通工具热配置、流、批并行、DBOS恢复、当前来源重验与调用数据导出删除恢复，恢复不产生新Provider调用。实际终止时间接收已覆盖进行中撤委派、原时间回执丢确认和管理身份重采用，正在闭合原生Task授权与选定成果交付。正式HTTP/CLI已接Provider/Model/Role、Prompt、网络、时间与调用ledger；两产品Client统一采用生成操作和纯HTTP元数据。

Worker连接已通过正式HTTP/SDK与单连接PostgreSQL场景：受限Host创建metadata-only子委派，真实Consumer选择、私有来源读取、并发/丢确认幂等、principal限制与当前撤权均由原Auth/source owner执行；子key不进入HTTP结果或工作检查点。Hept Task正在贯通本地Worker、远端cognitionAuthority与原生四语义入口。取消后实际调用保持completed/time/cost，原件与ContextView定位认知采用pending；撤权后Nous晚到原件的私有接收仍待完整消费proof单元，不放宽当前选择或重发模型。

原生历史 UUID、归档来源和晚到结果已沿实际 owner 验证；完整 Gateway 安装与发行闭包继续施工。


详细类型、源码采用、配置和顺序见工作区Specs/设计/基础设施/施工总览.md及Specs/施工顺序.md。

模型与脚本发行仍为本地预发布制品；正式registry或GitHub发行源及Hept正式lock收口尚未完成。共同生成器在platform/devtools中复用固定上游模板，支持联合类型、原字段名、流式BodyInit与参数接口命名隔离。
