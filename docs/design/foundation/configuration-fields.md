# 配置字段目录与初始默认值

## 字段描述合同

运行目录从模块的 Pydantic 字段导出。每项 `FieldDefinition` 包含 `path`、JSON Schema 类型、`required`、`nullable`、默认值或默认计算规则、`description`、`unit`、取值约束、允许 scopes、exposure、apply_mode、impact、secret 标志和可选示例。

动态资源使用 `models.<resource-id>` 等路径模板，并为具体资源实例展开查询。联合对象使用 discriminator。角色资源之间的交叉引用由相应配置 owner 校验，Configuration 汇总结果。

以下默认值是本次设计选择，用于开工，不是测量所得最优值。后续根据实际使用更新字段定义与本表。时间单位包含在字段名和目录中；内部统一换算为对应值类型。

## 运行、日志与执行

| 字段 | 类型、缺省与范围 | 呈现 / 效果 |
|---|---|---|
| instance.id | init 生成的 UUID；必需、只读 | internal / new_binding |
| instance.name | string；默认 `default` | standard / next_operation |
| instance.layout | platform\|portable；init 决定 | advanced / new_binding |
| paths.*_dir | 路径；缺失按布局计算 | advanced / restart；data/content/index 需重定位 |
| runtime.shutdown_grace_seconds | number，30，>=0 | advanced / next_operation |
| execution.max_in_flight | integer，16，>=1 | advanced / restart |
| execution.max_blocking_workers | integer，4，>=1 | advanced / restart |
| execution.default_timeout_seconds | nullable number，null，>0 时为默认期限 | advanced / next_operation |
| logging.level | DEBUG\|INFO\|WARNING\|ERROR，INFO | standard / live |
| logging.file_enabled | boolean，true | advanced / restart |
| logging.max_file_bytes | integer，10485760，>=1024 | advanced / restart |
| logging.backup_count | integer，5，>=1 | advanced / restart |
| tracing.enabled | boolean，true | advanced / restart |
| tracing.sample_ratio | number，1.0，0..1 | advanced / restart |
| tracing.export | none\|stderr\|otlp，none | advanced / restart |
| tracing.otlp_endpoint | nullable URL，null；otlp 时必需 | advanced / restart |
| tracing.export_timeout_seconds | number，5，>0 | developer / restart |
| metrics.enabled | boolean，true | advanced / restart |
| metrics.export | none\|otlp，none | advanced / restart |
| metrics.interval_seconds | number，60，>0 | developer / restart |
| metrics.otlp_endpoint | nullable URL，null；otlp 时必需 | advanced / restart |
| metrics.export_timeout_seconds | number，5，>0 | developer / restart |
| localization.language | zh-CN\|en，zh-CN | standard / next_operation |
| localization.timezone | IANA zone，UTC | standard / next_operation |

同一实例入口的 `--language` 或显示时区仅影响本次呈现，不改变持久时间。采样与导出关闭时，普通日志仍工作。

## 数据库与时间

| 字段 | 合同 | 呈现 / 效果 |
|---|---|---|
| database.filename | 相对 data 根文件名，`nous.sqlite3` | advanced / new_binding |
| database.busy_timeout_ms | integer，5000，>=0 | advanced / restart |
| database.synchronous | FULL\|NORMAL，FULL | developer / restart，disruptive |
| database.foreign_keys | 固定 true，只读目录项 | internal |
| database.journal_mode | 固定 WAL，本地文件模式 | internal |
| time.domains.<id>.kind | utc\|linear\|opaque | advanced / new_binding |
| time.domains.<id>.tick_seconds | 正有理数；linear 时必需 | advanced / new_binding |
| time.domains.<id>.epoch | 可选可读说明；不可据此自动换算 | advanced / new_binding |
| time.domains.<id>.name | 可读名称 | standard / next_operation |

内置 `utc` 域使用 Unix epoch 微秒：每 tick 为 1/1,000,000 秒。应用耗时使用 `monotonic_ns`，不注册成可以持久比较的认知域。`tick_seconds` 描述坐标尺度，不是世界与现实的运行速率。

## 网络、凭据和模型

以下纯字段首轮可解析；实际资源在模型计划中接入。

| 字段 | 默认与约束 | 效果 |
|---|---|---|
| network.proxy_url | nullable URL，null | restart |
| network.trust_env | boolean，false；使用正式配置明确代理与证书 | restart |
| network.ca_file | nullable path，null；默认系统/库信任库 | restart |
| network.connect_timeout_seconds | number，10，>0 | next_operation |
| network.read_timeout_seconds | number，120，>0 | next_operation |
| network.write_timeout_seconds | number，30，>0 | next_operation |
| network.pool_timeout_seconds | number，10，>0 | next_operation |
| network.max_connections | integer，20，>=1 | restart |
| credentials.<name> | env/file 可判别引用，见 credentials 专题 | next_operation |
| providers.<id> | protocol、endpoint、credential_ref；无默认真实服务 | next_operation |
| models.<id> | provider、实际模型名、能力声明；无默认付费模型 | next_operation；向量绑定另见空间合同 |
| presets.<id> | 生成参数、能力兼容的扩展参数 | next_operation |
| tasks.<id> | model、preset、prompt、fallback、budget | next_operation |
| model_execution.max_attempts | integer，2，>=1，实例上限 | next_operation |
| model_execution.timeout_seconds | number，120，>0 | next_operation |
| model_execution.max_input_tokens | integer，16384，>=1，实例上限 | next_operation |
| model_execution.max_output_tokens | integer，2048，>=1，实例上限 | next_operation |
| model_execution.structured_retries | integer，0，>=0 | next_operation |
| prompt.cache_entries | integer，128，>=0 | restart |
| prompt.bytecode_cache | boolean，true | restart |
| embedding.spaces.<id> | 显式空间描述，无默认值 | new_binding |

任务可以选择更低的预算。模型窗口和协议实际限制继续参与判断。输入估计、提供方实际用量和费用估计分别返回，不把字符数冒充真实 token 用量。

媒体策略是任务配置的枚举：`description_only / direct_structured / describe_then_structure`；未启用媒体任务时没有隐式媒体调用。具体解析器参数在媒体专题建设时补入其 owner，基础配置只承载已确定字段。

## 固定常量与可调参数

协议版本、坐标解释、精确 ID 规则、SQLite 外键启用等确定性语义不是调优开关。它们可以在只读目录中解释。排序权重、预算、缓存容量、时限、路径和可选能力的启停从正式配置读取。

## 字段维护

新增业务参数时在负责模块声明并接入真实读取点。目录、帮助和管理接口从同一模型生成；用户配置示例只展示常用部分。不要为了让配置看起来完整而预建未决定算法或认知对象的数百字段。
