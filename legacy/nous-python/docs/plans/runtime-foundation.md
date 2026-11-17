# 运行基础实施计划

## 本次交付

完成可独立使用的 Nous 运行基础：实例与跨平台路径、启动关闭、完整配置机制、错误/日志/Trace/指标、多轴时间值、进程内执行、SQLite 连接与事务、本地化及本地管理 CLI。

本次只执行本计划。内容存储、DBOS 队列、网络 Provider、真实模型生成、认知对象、Wave 索引、HTTP/MCP 服务和远程 SDK 不在本次施工范围。模型与 Embedding 的纯配置定义和危险修改说明属于配置系统本次范围。

## 输入文档

先读 [架构](../ARCHITECTURE.md)、[配置](../design/foundation/configuration.md)、[字段目录](../design/foundation/configuration-fields.md)、[变更](../design/foundation/configuration-changes.md)、[时间](../design/foundation/time.md)和[本地管理](../design/interfaces/local-management.md)。按当前工作继续读 instance、paths、bootstrap、lifecycle、errors、logging、tracing、metrics、execution、database、transactions、localization 各专题。

模型目录中的 resources、embeddings 为配置字段和未来绑定提供合同，不授权开始真实模型执行。sources 原文可用于恢复动机；已退役文档不作为本次结构约束。

## 首先确定真实职责位置

目标代码职责为 runtime、configuration、observability、storage、presentation，以及 models 中的纯配置值。沿既有仓库定位能够独立复用的服务，采用后直接接入新调用链。对耦合严重或接口错误的部分重写，迁移受影响消费者并删除淘汰路径。

不要先审计全仓，也不依据文件行数强拆模块。代码组织服务于完整职责；少量内聚函数可以同处一个模块，文档专题无需逐文件映射代码。

## 可独立推进的工作

**实例与共同值**先确定 InstanceLocator、ResolvedPaths、错误数据、TimePoint/TemporalValue/域定义和配置元信息。它们的定义由明确负责人维护，避免各分支自创字段。

**配置和呈现**建立纯定义目录、各来源合并、null/unset、schema 导出、explain、写文件和生效机制。同步接入本地 config 操作，用户能直接观察结果。

**生命周期与观测**在共同值稳定后组装 stdlib 日志、OpenTelemetry、执行资源和关闭顺序。基础模块直接埋点，不依赖服务化 Trace。

**数据库与时间域绑定**消费路径及配置，接入非 legacy SQLite 事务和真实域登记；bootstrap 在数据库打开后检查绑定，然后发布运行时。

这些工作可以按独立责任并行；共享合同的变更由同一负责人统筹。最终通过同一个 Runtime 和 CLI 接通，而非各自留下演示程序。

## 完整操作链

从 `nous init` 创建指定 portable 实例开始。读取字段目录及来源解释，修改日志级别和一个高级执行参数。运行 `runtime check`，实际连接数据库、读取实例与域定义，生成真实日志和 Trace，再正常关闭。重新打开，保存值保持。

在同进程的 Runtime 中运行一次配置更新，旧操作继续原快照，新操作使用新快照。包含 restart 字段时保存但不混用新旧资源。修改已登记时间域的坐标定义被拒绝，新增域 ID 正常登记。

无模型、无 OTLP 服务和无已有认知内容时，这条链路仍完整可用。

## 重要检查

配置检查关注来源优先级、数组替换、显式 null、作用域、危险绑定和快照一致性。时间检查关注不同时域、未知/无界、精度、补录与单调时钟。运行检查关注实际启停、启动失败清理、取消、单写入占有、事务回滚和日志关联。

使用成熟依赖验证本项目接入，不做逐字段无意义测试，不运行强制 TDD，不研究 SQLite 断电安全。修复发生的问题后运行受影响操作，已经回答的问题不重复累计验证。

Linux、Windows、macOS 的路径设计都要成立；实际运行记录明确已执行的平台。没有对应环境时说明未验证，不假报通过，也不为此新增永久 CI/审查制度。

## 实现边界与清理

保留与本次无关的用户工作。需要重建开发数据库时仅操作明确指定的开发实例。旧内部接口没有兼容承诺时，迁移真实消费者并删除别名、桥接和专属脚手架，不建立双轨版本。

配置目录不能宣称未实现能力可用；未来方法不放成功空实现。无需启用开机自启动、安装系统服务或配置公共网络端口。

## 完成与报告

交付上述操作可用的代码、准确的用户命令、当前 design 文档修订及 CURRENT 更新。报告实际完成链路、必要运行结果、仍有的具体阻塞。未实现的下一计划保持未开始。

本计划完成后停止，不自动展开模型、DBOS、Wave 或认知领域。
