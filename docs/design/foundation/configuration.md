# 配置定义、解析与有效快照

## 责任

Configuration 提供字段定义、类型校验、分层解析、来源解释、保存值和有效快照。定义归各实际 owner，纯解析在数据库与网络建立之前可用。一次修改所需的跨模块采用由 [ConfigurationOperations](configuration-integration.md) 组织。

采用 Pydantic 2、pydantic-settings 与 tomlkit。字段类型、默认和约束来自唯一 Pydantic 定义；补充元信息通过 Field/json_schema_extra 的 x-nous 提供，目录/Schema/帮助由它生成。

## 独立维度

字段必须表达类型与约束、缺失是否允许、nullable、单位和范围；以及允许的 instance/namespace/task/call scope、standard/advanced/developer/internal exposure、live/next_operation/restart/new_binding apply_mode、ordinary/disruptive/data_bound impact、只读身份与秘密属性。

普通用户也会选择危险的向量配置。呈现层级不决定数据绑定；高级设置也应有充分默认、描述、修改后果和示例。固定协议语义/SQLite 外键要求不是调优开关。

## 来源与覆盖

启动范围由低到高：包内默认 → 实例 TOML → 显式环境 → 显式启动参数。业务范围再叠加允许的 Namespace、任务、本次调用设置。实例资源上限继续约束下层，不通过普通覆盖扩大。

NOUS_INSTANCE 只定位配置目录。环境值使用 NOUS__SECTION__FIELD，来源记录精确键。动态资源使用小写 ASCII slug；真实模型名保留原样。不得让 Provider SDK 独立读取另一套网关/代理设置绕过 Configuration。

对象按字段递归合并，数组整项替换，资源 map 按 ID 覆盖。未知字段返回错误；协议扩展仅在明确的受控参数区由 adapter/library 解释。删除资源先由对应 owner 核对任务、fallback 和已用资产引用。

## 缺失与清空

缺失表示继承，0/false/空字符串/空列表各有本身语义。null 只允许 nullable 字段。TOML 使用 `[config] null_fields=["network.proxy_url"]` 表达本层显式清空，同一层同时写值与 null_fields 是错误。

unset 删除本层值和对应 null 标记，恢复继承。资源删除使用明确操作，不能用 null 含混替代。

## 读取、目录与版本

catalog/schema/explain/resolve/validate 使用共同定义。有效快照在公开语义上不可变，包括嵌套容器；提供类型化读取时不得让调用者取得可以修改原快照的 dict/list。

快照保留值、版本及每个有效字段的来源。期望版本与生效版本分开，restart pending 时可不同。运行中的操作固定一份快照；内部任务参数来自它与合法调用覆盖，不反复读取全局配置。

纯 validate 说明实际做过的检查。必须访问正式数据库的时间/向量绑定检查在对应阶段执行；未执行不标作通过。config show --effective 离线显示 prospective 值，不冒充已运行进程。

## 文件与数据库

本轮实例设置使用 TOML，包内默认及 Prompt 资源随代码提供。数据库保存正式身份/资产绑定，作用是维护持久语义，不镜像整份 TOML。

保存使用 tomlkit 与同目录原子文件替换，保留非目标字段的注释。文件、数据库绑定、资源生效是可区分的事实，具体状态见 [变更](configuration-changes.md)。

新增字段从负责模块定义并接到真实调用点；目录能解释尚未接入资源而不伪报功能 ready。初始参数可按实际资料调整，配置能力不被命令行当前呈现范围限制。
