# 本地管理与工具入口

## 已有管理命令

init、instance show、config list/show/explain/set/unset/remove-resource/validate/schema、runtime check、time parse 保留首轮语义。命令消费共同应用操作，输出可读文字或 JSON，日志写 stderr。

本地配置目录通过 --instance / NOUS_INSTANCE / 平台默认位置选择。离线 show --effective 表示 prospective 值，不能报告另一个进程的内存快照。实例被占用时按已有锁规则处理，不启动第二个写入者。

## 配置修改接入

set/unset/remove-resource/reload 使用 [共同配置用例](../foundation/configuration-integration.md)。CLI 负责选择离线资源上下文和解析参数，不持有时间域/Embedding/路径绑定规则。运行内管理使用同一实现。

字段目录从纯定义导出，standard/advanced/developer/internal 分层展现。普通输出包括类型、默认、可空性、范围、作用域、生效与危险影响，不隐藏 Embedding 的数据绑定含义。

## 新增正式工具

本轮新增 [nous tools](tools-cli.md)，用于模型、Prompt、媒体、CAS 及结果。实例管理和内容工具共用 Runtime/Execution，不另建一个“实验运行时”。工具配置与产品配置相同。

runtime check 只检查真实已组装资源与能力，不隐式发起模型请求。模型 probe/run 是用户明确操作；本轮本地开发调用已授权，具体操作无须再次费用确认。

## 用户呈现

未知或未配置的能力准确显示。实际 Key 不打印；连接资源名和 endpoint 可以用于本地排查。具体路径、领域名和模型名保持原文，本地化只影响操作说明、状态和错误呈现。

CURRENT 只记录能力与当前工作，不作为讨论原文同步目录。
