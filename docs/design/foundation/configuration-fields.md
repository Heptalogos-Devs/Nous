# 配置字段目录与初始值

## 定义来源

类型、默认、范围、description、unit、scope、exposure、apply_mode、impact 和只读/秘密元信息由负责模块的纯 Pydantic 定义提供。目录和帮助从同一定义导出，不维护第二份手写运行时表。

本文件记录当前基础默认。模型/媒体新增字段及改变说明见 [模型与媒体字段](configuration-model-media.md)。这些初始值服务正常起步，可以按实际使用更新。

## 实例、执行与观测

| 字段 | 类型 / 默认 | 生效与含义 |
|---|---|---|
| instance.id / layout | init 生成 ID 与选定布局 | 只读身份，new_binding |
| instance.name | string=default | next_operation |
| paths.data/content/index_dir | nullable path，按布局解析 | new_binding，需重定位 |
| 其他 paths.*_dir | nullable path，按布局解析 | restart |
| runtime.shutdown_grace_seconds | number=30，>=0 | next_operation |
| execution.max_in_flight | int=16，>=1 | restart |
| execution.max_blocking_workers | int=4，>=1 | restart |
| execution.default_timeout_seconds | nullable number=null，非空>0 | next_operation，根操作总期限 |
| logging.level | DEBUG/INFO/WARNING/ERROR=INFO | live |
| logging.file_enabled | bool=true | restart |
| logging.max_file_bytes / backup_count | int=10485760 / 5 | restart |
| tracing.enabled / sample_ratio | true / 1.0 | restart；ratio 0..1 |
| tracing.export / otlp_endpoint | none / null | restart；otlp 必须指定 URL |
| tracing.export_timeout_seconds | number=5，>0 | restart |
| metrics.enabled / export | true / none | restart |
| metrics.interval_seconds / export_timeout_seconds | 60 / 5 | restart |
| metrics.otlp_endpoint | nullable URL=null | restart |
| localization.language / timezone | zh-CN / UTC | next_operation，呈现专用 |

## 数据库与时间

| 字段 | 定义 |
|---|---|
| database.filename | data 根相对文件名，默认 nous.sqlite3；new_binding |
| database.busy_timeout_ms | int=5000，>=0；restart |
| database.synchronous | FULL/NORMAL，默认 FULL；restart，disruptive |
| database.foreign_keys / journal_mode | 固定 true / WAL；只读语义 |
| time.domains.<id>.kind | utc/linear/opaque；已登记后解释固定 |
| time.domains.<id>.tick_seconds | linear 为正有理数字符串；opaque 必须为空 |
| time.domains.<id>.epoch | 可读起点声明，改变坐标意义需新 ID |
| time.domains.<id>.name | 显示名，可变 |

utc 固定为 Unix epoch 微秒。模型/媒体的局部时段另有 Selection 值，不强行注册为真实世界时钟。

## 网络与模型

network 的 proxy_url/ca_file 默认 null、trust_env=false、max_connections=20，均 restart；connect/read/write/pool timeout 默认 10/120/30/10 秒，可在下一操作读取。Provider 连接覆盖同类字段，协议/endpoint/credential_ref 改动为 restart。

credentials 支持 env/file；providers、models、presets、tasks 无默认付费服务。具体必需字段由 [模型资源](../models/resources.md) 定义。task media_strategy 为三种现行策略，缺省不隐式处理媒体。

model_execution.max_attempts=2、timeout_seconds=120、max_input_tokens=16384、max_output_tokens=2048、structured_retries=0，属于每个语义阶段的技术限制，可在明确作用域内选择更低值；实例参数可按实际资料提高。max_cost 默认 null，本轮不建设费用执行体系。

prompt.cache_entries=128、prompt.bytecode_cache=true，restart。embedding.spaces 无默认空间，用户先 probe 后明确维度与编码，再按 owner 合同采用。

## 字段维护

资源映射、数组、联合类型均提供完整字段说明。新增字段接入真实消费者；保持普通、进阶与内部呈现层级，不把未实现功能写成已可用。标准常量不要伪装成任意可调参数。
