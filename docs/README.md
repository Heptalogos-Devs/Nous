# Nous 设计与开发文档

Nous 当前建设独立认知服务的运行基础。开发以本目录中的当前设计和对应实施计划为依据。

## 阅读路径

先阅读 [整体架构](ARCHITECTURE.md)，再读取所执行计划及其列出的专题。当前施工位置见 [当前工作](CURRENT.md)。实现中新出现的普通细节由实现者决定；涉及对象语义、所有权、数据归属或公开操作的实质变化，先提出具体修改，再更新负责文档。

`design/` 保存当前合同；`plans/` 规定本次实际施工；`sources/` 保留讨论原文；`legacy/` 是退役资料；`agent/` 是面向使用 Nous 的 Agent 的指南。原文中的提案和旧决定不覆盖当前设计。

## 基础专题

| 专题 | 文档 | 本次定位 |
|---|---|---|
| 实例 | [instance](design/foundation/instance.md) | 首轮实施 |
| 逻辑与物理路径 | [paths](design/foundation/paths.md) | 首轮实施 |
| 启动装配 | [bootstrap](design/foundation/bootstrap.md) | 首轮实施 |
| 生命周期 | [lifecycle](design/foundation/lifecycle.md) | 首轮实施 |
| 配置机制 | [configuration](design/foundation/configuration.md) | 首轮实施 |
| 字段与初始目录 | [configuration-fields](design/foundation/configuration-fields.md) | 首轮实施 |
| 配置变更 | [configuration-changes](design/foundation/configuration-changes.md) | 首轮实施 |
| 错误 | [errors](design/foundation/errors.md) | 首轮实施 |
| 日志 | [logging](design/foundation/logging.md) | 首轮实施 |
| Trace | [tracing](design/foundation/tracing.md) | 首轮实施 |
| 指标 | [metrics](design/foundation/metrics.md) | 首轮实施 |
| 时间 | [time](design/foundation/time.md) | 首轮实施 |
| 进程内执行 | [execution](design/foundation/execution.md) | 首轮实施 |
| 数据库 | [database](design/foundation/database.md) | 首轮实施 |
| 事务 | [transactions](design/foundation/transactions.md) | 首轮实现事务边界；持久入队在后续接入 |
| 本地化 | [localization](design/foundation/localization.md) | 首轮实施 |
| 内容存储 | [content-storage](design/foundation/content-storage.md) | 内容与持久工作交付 |
| 持久工作 | [durable-work](design/foundation/durable-work.md) | 内容与持久工作交付 |
| 凭据 | [credentials](design/foundation/credentials.md) | 模型交付的前置 |
| 网络 | [networking](design/foundation/networking.md) | 模型交付的前置 |

## 模型专题

[模型资源](design/models/resources.md)、[Prompt](design/models/prompts.md)、[模型执行](design/models/execution.md)、[预算](design/models/budgets.md)、[fallback](design/models/fallback.md)、[缓存](design/models/caching.md)、[向量空间](design/models/embeddings.md)分别维护自己的规则。

模型相关字段先由纯配置定义提供，使首轮能校验、编辑和解释完整配置；真实 Provider、Prompt 执行和 Embedding 调用按模型计划接入。未接通的执行能力必须在状态中如实显示。

## 实施与使用

[本地管理入口](design/interfaces/local-management.md)规定首轮 CLI 的可执行操作。[运行基础](plans/runtime-foundation.md)、[内容与持久工作](plans/storage-and-jobs.md)、[模型运行](plans/model-runtime.md)分别授权一项完整交付。

[实例配置示例](examples/nous.toml)、[模型配置示例](examples/models.toml)用于说明正式配置形状。示例中的本地网关地址和模型名由操作者替换，不作为默认可用服务。

## 文档维护

一条规则只有一个定义位置，其他专题链接引用。规则、接口或默认值改变时，同时修改其实际消费者和相应使用例子。原文保留在 `sources/`，已失效说明从当前设计退出。`CURRENT.md` 只更新当前目标、有效决定和未完成能力。

专题短小可以保留。随着细节增长，在原职责内补全，不因当前篇幅小而合并不同主题。文档拆分不要求代码按文件一一拆分。
