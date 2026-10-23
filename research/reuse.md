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

结构化认知已从 M 的 `core/cognition.rs`、`memory/domain.rs`、`service/tag.rs`、Episode partition 与 Journal/Schema 规则复制改编到 `nous-domain::structured` 和 `nous-app::structured::lineage`，精确原文件 SHA-256 与 Apache-2.0 许可见 [领域来源记录](../crates/nous-domain/UPSTREAM.md)。来源闭包、范围、历史与修订写入复用现有 SQLx 事务。

结构化模型边界从 M 的 typed schemas/consolidation/concept/longitudinal prompts，以及 P 的 `createCognition.formChanges` 来源目录和独立变更组改编。P Prompt 先复制再按当前领域合同调整；来源版本、12 个原文件摘要和两种许可保存在 [模型来源清单](../crates/nous-app/src/model/structured-sources.json)。模型输出使用显式逐字段引用键解析，实际 provider Schema 从领域/模型 DTO 生成；领域采纳复用共享账本、持久队列和事务。

[能力续接清单](../docs/能力续接清单.md) 是后续来源与实施映射的维护位置：master 的媒体派生、结构化领域、managed binding/ProjectionPlanner、NousQL、reference/Native/DTSC；PR #56 的持久 ContextView、来源与独立采纳组；VCP 的 AGS、传播资产、Pyramid/Sense/Fusion/双场、RiverMemo/Thinking 共同续接。已有数值基线与当前上游算法分别保留版本、原始输入、expected 和来源，研发先做本地提取与验证。

## 基础服务的实际消费链

本轮在已克隆的 MaiBot `c602a2780f4f4dda4a6be46d67abc1cfb1ad420c` 与旧 Nous `412b062082afb75ccfae4c3f69fc606d3e3cc862` 上追踪公共接口和实际消费者，结论用于 [基础服务设计](../docs/基建实施规划.md)。以下为源码事实及具体采用边界。

| 实际消费链 | 统一入口承担的责任 | Nous 采用方式 |
| --- | --- | --- |
| `services/memory_flow_service.py:421` → `LLMServiceClient.generate_response` → `LLMOrchestrator._execute_request` → client registry → `OpenaiClient._execute_chat_request` → usage recorder | 业务给角色与输入；公共层选模型、管理 SDK client、参数、重试和规范结果 | 复制角色资源解析、客户端生命周期和单一重试 owner；调用前持久预约，成功/失败/不确定都有账本 |
| `maisaka/builtin_tool/send_emoji.py:329` → `PromptManager.get_prompt/render_prompt` → `common.prompt_i18n.load_prompt_template`；WebUI edit/activate → cache revision | 默认/自定义/语言资源、模板克隆、变量渲染、管理与失效 | Prompt owner 保存产品历史、原子激活并返回实际正文/摘要；实际调用绑定不可变快照 |
| `web_search/provider.py:90` → `WebSearchService.execute` → `TavilyAdapter` → `HTTPAdapter.post` → `common.http_client` | 配置冻结、协议适配、TLS、超时、受限流读取、原文与预览结果 | 受管 Reqwest 连接池统一请求策略；模型端点与任意资料 URL 使用各自目标政策 |
| `agent/messaging.py:31` → `message_service` → `common.message_repository.find_messages` → `get_db_session` | 查询合同、时间字段、数据库连接、返回 SessionMessage | 异步仓储与冻结 ReadView；错误保留类型和原因，业务事务内复用当前事务 |
| `AgentSession.save` / `AgentHistory.append` → `common.async_work.run_sync_work` → `agent.persistence.write_json` | 取消时等待真实线程结束、锁生命周期、同目录临时文件、原子替换、Windows 文件语义 | 统一阻塞工作收尾与 Objects.publish；数据库权威、回执和持久任务仍同事务提交 |
| 模型配置修改 → `ConfigManager.reload_config` → `ExpressionVectorService.on_config_reload` → 后台补建 → `EmbeddingServiceClient.embed_texts` | 配置通知、资源签名变化、任务唤醒、批量向量调用 | 配置 owner 处理快照/生效/通知；作业和调用服务统一执行；领域只保留投影与采纳规则 |

`common.paths` 区分程序只读资源和实例目录，且不导入配置、日志或数据库。`common.i18n.I18nManager` 以 ContextVar 支持请求级 locale、统一目录缓存和格式化。这两点分别进入实例与语言服务。

源码也暴露了需要改动的责任边界：`message_repository` 将数据库故障转为空结果；部分异步消费者自己决定是否放入线程；`utils_file.save_binary_to_file` 先写外部路径再另行提交数据库；模型费用只在成功后记录；配置与部分 Prompt 逻辑依赖可变全局状态、调用栈或分散文件写入。Nous 的公共接口负责这些生命周期和失败语义。

旧 Nous 的可直接改编文件包括 `crates/configuration/src/{registry,service,snapshot}.rs`、`crates/persistence/src/mutations.rs`、`crates/runtime/src/lib.rs::capture_semantic_in`、`crates/core/src/time.rs`、`crates/object-store/src/lib.rs::{put_chunks,reference_guard,stream,read_range}`，以及 PromptRegistry、模型角色资源图与 managed binding。配置保持 active/desired，单次提交捕获同一语义时间，流式对象在完整发布后才登记权威引用。

MaiBot 主仓库许可为 GPL-3.0，`src/llm_models/LICENSE` 为 MIT；实际复制按文件归属保留许可。旧 Nous 相关 Rust 服务沿其 Apache-2.0 来源归档。源码现成的升级适配服务于其已发布产品；Nous 当前内部结构直接更新。
