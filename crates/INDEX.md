# Rust Crate Index

[Rust 实现](README.md) · [仓库地图](../INDEX.md)

| Crate | Responsibility | Entry | Important dependencies | Read when |
| --- | --- | --- | --- | --- |
| `core` | IDs, exact refs, query DTOs, shared semantic errors and provenance primitives | `crates/core/src/lib.rs` | serde, uuid, chrono | Changing shared contracts or identity |
| `protocol` | Generated Rust Protobuf/tonic bindings | `crates/protocol/src/generated/` | prost, tonic | Inspecting wire bindings; edit `proto/` instead |
| `persistence` | PostgreSQL pool, transactions, fresh schema, mutation envelopes, receipts and projection watermarks | `crates/persistence/src/lib.rs` | sqlx, core | Changing durable persistence mechanics |
| `object-store` | Immutable blob/object storage mechanism | `crates/object-store/src/lib.rs` | OpenDAL | Changing artifact storage |
| `configuration` | Typed registry, snapshots, overrides and configuration receipts | `crates/configuration/src/lib.rs` | persistence, core | Changing configuration resolution |
| `subject` | Subject identity, capability and Cognitive Seed adoption | `crates/subject/src/lib.rs` | persistence, configuration, runtime, object-store | Changing Subject lifecycle or seed source |
| `material` | Artifact/Observation/DerivedRepresentation, selected text and source lineage | `crates/material/src/lib.rs` | persistence, runtime, object-store | Changing evidence admission or materialization |
| `memory` | Memory, CognitiveSchema, Episode, Journal, Tag, AssociationEvidence and Authority/query operations | `crates/memory/src/lib.rs` | material, persistence, runtime | Changing cognitive Authority, Episode or Memory query |
| `runtime` | Session, ResidentSet, WorkContext, UseEvent, QueryPlan, lane contract and fixed fusion | `crates/runtime/src/lib.rs` | persistence, configuration, core | Changing Runtime or query semantics |
| `retrieval` | Owner-fed lexical/dense/topology projections, bounded model material and generations | `crates/retrieval/src/lib.rs` | material, memory, runtime, persistence, object-store | Changing Serving mechanics or experimental lanes |

`crates/protocol/src/generated/` is derived output. Change canonical `.proto` sources and regenerate.

## 修改一项功能时的模块入口

| 功能 | 内部层次 | 从哪里开始 |
| --- | --- | --- |
| Runtime policy | `policy/` 拥有 execution、episode、maintenance；Query policy 随 `query/` 的绑定合同 | [Runtime](runtime/src/lib.rs) / [Policy](runtime/src/policy/mod.rs) |
| WorkContext | 同一 owner 下分开 contract/validation、read、mutation、lifecycle；anchor 存储只在 mutation 中实现 | [WorkContext](runtime/src/work_contexts/mod.rs) |
| Serving assets | `assets/` 负责 current/history 的 build、install/open 和 reclamation；共享文件机械在同目录 | [资产生命周期](retrieval/src/assets/mod.rs) |
| Retrieval algorithms | `mechanisms/` 放原生索引与 Wave；`concept/` 放共享 concept generation/lane；`vcp/` 放 adapter/generation/lane/readout；`policy/` 只拥有配置 | [Retrieval](retrieval/src/lib.rs) / [数值 reference](retrieval/src/reference/mod.rs) |
| PostgreSQL projection | `projection/` 提供 text/concept/topology 输入与 invalidation；`historical/` 提供过去 Authority binding/projection，均由语义 owner 调用 | [Persistence](persistence/src/lib.rs) |

Rust 私有算法 unit tests 可以使用同模块内的 `#[cfg(test)]`；独立的 query unit support 在 `retrieval/tests/unit/`，真实数据库轨迹在 Kernel `tests/`。测试和 helper 与生产源码共用长度门禁。

- [Proquint 地址编码](persistence/src/proquint.rs)
- [Rust 实现](README.md)
