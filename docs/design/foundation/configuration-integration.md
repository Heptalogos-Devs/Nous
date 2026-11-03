# 配置变更的共同应用操作

## 目标与现有接缝

首轮 Configuration 已提供定义、分层合并、保存和快照。当前 CLI 的 `offline_change` 与 bootstrap 分别组织路径/时间绑定，并在构造后回填 callback。本轮把这项完整操作收回应用层；CLI 和运行中的管理入口共用它。

本专题保留 [配置机制](configuration.md) 与 [配置变更](configuration-changes.md) 的作用域、原子文件替换、期望/有效代和失败语义。本轮用以下明确 owner 接入取代临时的 Embedding 全拒绝与可变回调。

## 职责与接口

Configuration 内部保留纯的 definition/catalog、source resolver、snapshot 和 TOML store。按修改原因拆分已有大文件，避免让所有拆分文件共享整个 `self`。公共读取、解释和变更结果不重造多套 DTO。

应用层提供 `ConfigurationOperations`，具有 `update`、`unset`、`remove_resource`、`reload` 和 `explain/status`。它接收实例绑定、时间域 owner、Embedding 空间 owner、配置存储与快照、日志级别应用能力。依赖在构造时明确提供。

离线 `open_configuration_operations(locator)` 只取得需要的实例锁和文件能力；普通字段在数据库不可读时仍能保存。确需时间/Embedding 绑定时才打开受实例资源身份核验的数据库。在线入口借用已打开资源，不重新拿实例锁、不再打开另一个 Runtime。

这两个资源取得方式提供同一操作实现。不要分别写 offline-update 和 runtime-update 两套判断。所需 owner 暂不可用时，只拒绝受其绑定影响的修改。

## 无环启动

先使用纯 resolver 得到候选配置，解析路径和日志，再打开数据库。数据库提供给 TimeDomainRegistry 与 EmbeddingSpaceRegistry。完成资源绑定核对后，构造 ConfigurationOperations，发布 Runtime。纯 resolver 不持有数据库；TimeDomainRegistry/EmbeddingSpaceRegistry 不依赖 ConfigurationOperations。

配置的基本字段由各 owner 的纯定义提供。纯错误和 ResourceId 移到不加载 runtime/observability 的下层；包 `__init__` 不通过重导出引发反向导入。时间域配置与持久域共用 kind/scale/epoch 语义校验。

## 一次修改的顺序

候选构造与形状校验 → 按变化确定要询问的 owner → 只读绑定核对 → 原子保存期望文件 → 需要且可立即采用的绑定提交 → 日志等明确 live 修改 → 发布新快照。

文件写入与 SQL 不伪装为同一事务。沿用已完成的失败合同：保存前失败不写文件；保存后采用失败保留旧快照，报告已保存并等待重启。在线修改含 restart 字段时整项等待重启，不拼接多代资源。

配置变更同时涉及时间与 Embedding 时，分别调用对应 owner 的纯校验和已用绑定检查。时间域按现行合同在配置采用时登记；Embedding 新空间在首次正式向量采用时登记，配置保存本身不提前将它标成已使用。需要一起提交的数据库采用共用调用者的 WriteTx，各参与者不自行 commit。文件仍由 Configuration 持有并报告实际状态。

## Embedding 绑定接入

空间 owner 解析候选中的空间、模型与连接身份。没有使用过的新空间允许正常创建和修改；首次保存正式向量时在同一事务登记完整空间合同并写入向量关联。已使用 ID 的模型部署、维度、模态、编码、归一化变化被拒绝；新空间使用新 ID。

普通模型显示信息、未被向量空间使用的文本模型配置可以正常修改。相关模型条目、空间条目的增删，以及环境/手改文件带来的变化均通过同一 owner 核对。Configuration 不再硬编码 `embedding.spaces.*` 与 `models.*.model_name` 的专有规则。

配置只是本地声明；网关别名背后的实际部署不由本地文件证明。空间中的 deployment_revision 是操作者维护的版本断言，运行结果保留服务实际报告的模型标识。绑定细节见 [Embedding](../models/embeddings.md)。

## 网络资源采用

连接创建参数（代理、CA、trust_env、连接池）及 Provider endpoint/protocol/credential_ref 采用 restart。逐请求网络超时、模型选取、input_formats/output_modes/compatibility、任务参数、Prompt 内容和用途在下一操作生效；模型准备从本次快照构造相应 Profile，复用既有兼容连接。这样当前进程无需建设通用客户端热替换框架。CLI 每次重新打开实例时自然采用保存配置。

任务内保持同一配置快照；两阶段媒体处理不会在中间突然采用新参数或换模板。不同阶段可用不同的明确任务分配，均从根快照解析。

## 同步迁移

迁移 bootstrap 和 CLI 的内联编排、现有测试中的直接 callback 回填、相关帮助和设计说明；删除被取代路径。配置路径、类型、描述、单位、默认、nullable、作用域和生效方式仍由正式定义导出。

迁移只处理受本轮能力影响的职责。已有实例锁、路径绑定和正确的保存失败合同继续复用。
