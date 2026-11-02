# 源码复用与改编记录

当前产品由 Python 应用与 `native/` 独立 Cargo workspace 构成。锁定来源见 [sources.lock.json](sources.lock.json)；本地参考源码只用于研究和按实际需要采用。

| 来源 | 当前落点 | 采用内容与许可记录 |
|---|---|---|
| VCPToolBox `d1b40c863273cbbba082fb7657671424768c4a53` | `native/vcp` | AGS、传播资产、Sense/Pipeline/Projection、RiverMemo/Thinking。保留 CC BY-NC-SA 4.0；原文件映射、哈希与调整见 [VCP 来源](../native/vcp/UPSTREAM.md)。 |
| 旧 Nous `412b062082afb75ccfae4c3f69fc606d3e3cc862` | `native/vcp/src/baseline` | 独立 reference/DTSC 历史算法，保留 Apache-2.0 与原始来源清单。 |
| 旧 Nous 同一版本的词项、DenseGeneration 与 VCP 双索引 | `native/retrieval` | Tantivy schema、授权候选集合、USearch 代际、图资产和来源映射；保留 Apache-2.0。见 [检索来源](../native/retrieval/UPSTREAM.md)。 |
| Tantivy 0.27、tantivy-jieba 0.21、USearch 2.26.2 | `native/retrieval` | 直接使用成熟依赖的中文词项索引与 HNSW 检索，锁定版本及源码见组件清单。 |
| 现有 NousQL parser | `native/nousql-syntax` | 纯语法解析与手册；名称、时间和查询计划由 Python owner 绑定。 |

Python 管理领域、材料、来源、授权、模型调用、事务、持久任务及公开接口；PyO3 传递授权后的冻结计算输入。原生发行包携带所采用的 VCP 与 Apache-2.0 许可证，源目录保留详细来源记录。

A-Memorix 的 RequestContext、Namespace、身份、检索范围和管理工具用于核对真实消费者需求。[合同清单](a-memorix-contracts.json)保存已读取的公开协议。画像读取、图搜索、修订与统计由 Nous 领域服务提供，Python SDK、HTTP、CLI 和 MCP 消费原生操作。

Bundle 的来源和采用责任见 [Bundle 来源清单](bundle-export-sources.json)。当前实现由 `src/nous/data_management` 管理原件、逻辑对象、安装映射和事务。

SQLAlchemy/aiosqlite 与 DBOS 共用同一 SQLite 权威文件；Pydantic AI、官方模型 SDK 与 HTTPX 提供模型及网络基础能力。具体职责见[架构](../docs/python-native-plan/design/architecture.md)、[事务](../docs/python-native-plan/design/transactions.md)与[源码采用](../docs/python-native-plan/design/adoption.md)。旧 Rust 应用及其研究记录可从 Git 历史读取。
