# 源码复用与改编记录

日期：2026-10-23。Nous 本批沿用 Apache-2.0。研究源码先克隆到 `temp/repos`，统一版本见 [sources.lock.json](sources.lock.json)；临时克隆可由锁定清单恢复。

## M0 的实际来源

| 本地来源与锁定 commit | 许可 | Nous 落点 | 改编内容 |
| --- | --- | --- | --- |
| `temp/repos/Nous-previous/crates/persistence/src/mutations.rs`，`412b062082afb75ccfae4c3f69fc606d3e3cc862` | Apache-2.0 | `crates/nous-storage/src/transaction.rs`、`crates/nous-app/src/lib.rs` | MutationEnvelope 的统一提交结构；改用 SQLite `BEGIN IMMEDIATE`、完整 OperationReceipt、实例 commit_seq 和同连接入队 |
| `temp/repos/apalis-sqlite/src/persist.rs`、`examples/basic.rs`，`9c87189360b0e212b23821a1a177e9cbe2ad1d4a` | MIT | `crates/nous-storage/src/lib.rs`、`transaction.rs`、`examples/transaction_recovery.rs` | 同连接 push_tasks、嵌入迁移和进程恢复 Worker；保留 `crates/nous-storage/LICENSE.apalis` |
| `temp/repos/rmcp/examples/servers/src/counter_streamhttp.rs` 与官方 stdio 示例，`2a401c4ca3eb5880744d2e70fad8073a7fcfb2bc` | Apache-2.0 | `crates/nous` 的 MCP transport | 官方 SDK 服务、Axum 挂载和 CancellationToken 生命周期；工具委派到同一 App |

SQLx、Tokio、Axum、Clap、Reqwest、tracing 与 tempfile 通过 Cargo 依赖使用其正式实现。ObjectStore 的暂存、原子发布与文件生命周期直接使用 tempfile，Nous 负责材料身份、内容摘要、来源和权威引用。

## 已研究的领域合同

A-Memorix 的 RequestContext、身份解析端口、Namespace 授权、检索范围集合与工具合同用于确定 Nous 的消费者需求。当前 Namespace、Subject、Material 和 Observation 依宪纲组织；A-Memorix 完整功能和公共接口继续按 [合同清单](a-memorix-contracts.json) 实施。

VCP、旧 Nous 检索实现与 PR #56 保留为算法和产品研究源码。后续每项复制改编在真正接入时补充文件、commit、许可与产品落点；TagMemo、RiverMemo 和高级认知按 [基建实施规划](../docs/基建实施规划.md) 推进。
