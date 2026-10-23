# 源码复用与改编记录

日期：2026-10-23。组件的复制改编保留各自来源许可，整体产品发行许可证由项目另行确定。研究克隆与发布源码包位于 `temp/repos`，统一版本见 [sources.lock.json](sources.lock.json)，临时内容可由锁定清单恢复。

## M0 的实际来源

| 本地来源与锁定 commit | 许可 | Nous 落点 | 改编内容 |
| --- | --- | --- | --- |
| `temp/repos/Nous-previous/crates/persistence/src/mutations.rs`，`412b062082afb75ccfae4c3f69fc606d3e3cc862` | Apache-2.0 | `crates/nous-storage/src/transaction.rs`、`crates/nous-app/src/lib.rs` | MutationEnvelope 的统一提交结构；改用 SQLite `BEGIN IMMEDIATE`、完整 OperationReceipt、实例 commit_seq 和同连接入队 |
| `temp/repos/apalis-sqlite/src/persist.rs`、`examples/basic.rs`，`9c87189360b0e212b23821a1a177e9cbe2ad1d4a` | MIT | `crates/nous-storage/src/lib.rs`、`transaction.rs`、`examples/transaction_recovery.rs` | 同连接 push_tasks、嵌入迁移和进程恢复 Worker；保留 `crates/nous-storage/LICENSE.apalis` |
| `temp/repos/rmcp/examples/servers/src/counter_streamhttp.rs` 与官方 stdio 示例，`2a401c4ca3eb5880744d2e70fad8073a7fcfb2bc` | Apache-2.0 | `crates/nous` 的 MCP transport | 官方 SDK 服务、Axum 挂载和 CancellationToken 生命周期；工具委派到同一 App |

SQLx、Tokio、Axum、Clap、Reqwest、tracing 与 tempfile 通过 Cargo 依赖使用其正式实现。ObjectStore 的暂存、原子发布与文件生命周期直接使用 tempfile，Nous 负责材料身份、内容摘要、来源和权威引用。

## M1 的实际来源

| 本地来源与锁定 commit | 许可 | Nous 落点 | 改编或使用方式 |
| --- | --- | --- | --- |
| genai 0.7.0 发布源码 `temp/repos/genai-0.7.0`，`8fc13af4db80acd0771c87f6180d79b83c9a898d` | MIT OR Apache-2.0，采用 Apache-2.0 | `crates/nous-app/src/model.rs` 与 `src/prompts` | 改编官方 target resolver 与结构化输出用法，SDK 通过发布依赖使用；显式目标、版本 Prompt、输入源版本、预算与结果采纳由 Nous 管理 |
| 旧 Nous `crates/retrieval/src/mechanisms/lexical.rs`，`412b062082afb75ccfae4c3f69fc606d3e3cc862` | Apache-2.0 | `crates/nous-retrieval/src/lib.rs` | 改编词项 schema、文档替换、范围过滤和候选读取；保留 `crates/nous-retrieval/licenses/Nous-LICENSE` |
| Tantivy 0.27，`67889269cae67063c372bc236b102d632a021568`；tantivy-jieba 0.21，`882dcc4304e415879b9e0169ff439a4806a2bf8e` | MIT | `crates/nous-retrieval` | 直接使用发布依赖；研究 PreparedCommit 原子覆盖发布，以及中文、英文 tokenizer 集成 |

## 已研究的领域合同

A-Memorix 的 RequestContext、身份解析端口、Namespace 授权、检索范围集合与工具合同用于确定 Nous 的消费者需求。M1 已建立 Namespace、Subject、Material、Observation、Memory 与模型调用的统一认知链路；M2 继续按 [合同清单](a-memorix-contracts.json) 承接 A-Memorix 的完整功能、维护、导入和公共接口。

VCP 与旧 Nous reference 已复制改编到 `crates/nous-vcp`：AGS/几何、传播资产、Sense/Pipeline/Projection、完整 RiverMemo/Thinking，以及独立旧 reference/DTSC 基线。实际原文件映射、固定 commit、原始哈希、许可与修改边界集中于 [组件来源记录](../crates/nous-vcp/UPSTREAM.md)。PR #56 与高级产品能力继续按 [能力续接清单](../docs/能力续接清单.md) 推进。

真实向量 adapter `crates/nous-retrieval/src/vector.rs` 从旧 Nous 的 VCP 双索引及 DenseGeneration 复制改编，直接使用 USearch 2.26.2 原生 HNSW。合法快照、代际与外部身份映射由调用方供给；双索引和元数据完整发布，加载时校验后原生搜索。实际 API、来源、锁定源码和独立测试结果集中于 [检索组件来源记录](../crates/nous-retrieval/UPSTREAM.md#frozen-vector-generations)。

`crates/nous-retrieval/src/serving.rs` 继续改编旧 Nous 的 generation/graph/observation/lane/readout，将合法来源快照接到完整几何、传播资产、真实 ANN 和单观察多路 RiverMemo 查询。来源根与候选/Tag 身份分开，同材料多次派生共享图证据，成组资产可持久重开；实际合同与独立测试见 [Serving 来源记录](../crates/nous-retrieval/UPSTREAM.md#serving-snapshots-and-graph-assets)。

## 后续复制改编边界

[能力续接清单](../docs/能力续接清单.md) 是后续来源与实施映射的维护位置：master 的媒体派生、结构化领域、managed binding/ProjectionPlanner、NousQL、reference/Native/DTSC；PR #56 的持久 ContextView、来源与独立采纳组；VCP 的 AGS、传播资产、Pyramid/Sense/Fusion/双场、RiverMemo/Thinking 共同续接。已有数值基线与当前上游算法分别保留版本、原始输入、expected 和来源，研发先做本地提取与验证。
