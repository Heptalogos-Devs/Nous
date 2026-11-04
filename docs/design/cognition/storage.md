# 认知存储、引用和短事务

## 存储方式

沿用现有 SQLite/SQLAlchemy Core、CAS 和内容 schema。本轮只新增真实认知状态，不引入 Event Sourcing、通用 UnitOfWork 层、代理仓储或第二个数据库。

表名可按项目规范调整，下面字段含义及约束必须保持。JSON 仅用于需要完整保存而不作为查询边的类型化值；可反向访问的关系使用有索引的表。

| 数据 | 必要字段与约束 |
|---|---|
| cognition_namespaces | id, name UNIQUE, content_generation >=0；初始化 default |
| written_records | id, namespace_id, kind(note/journal), address UNIQUE, head_revision_id, archived, recall；kind 不原地改写 |
| written_revisions | id, record_id, ordinal, parent_id?, title, body_artifact_id, purpose?, author/viewpoint, cognitive_times, change_kind/reason, origin_result_id?, recorded_at；UNIQUE(record_id,ordinal) |
| revision_sources | revision_id, key, ordinal, content Artifact/selection, cognitive_target_revision?, usage, name；UNIQUE(revision_id,key)，target 反向索引 |
| journal_drafts | id, address, namespace_id, result_id UNIQUE, body_artifact, sources/context, target_revision?, adopted_revision?；只有成功草稿 |
| concepts / concept_revisions | stable id/namespace/address/head；exact label/description/aliases/author/times、可选 producer_result/源正文引用，ordinal 唯一 |
| annotations | id/address, record_revision_id, parser_profile, producer_result?, 只保存已采用的不可变标注；提案留在普通 ProcessingResult |
| revision_annotation_heads | record_revision_id PRIMARY KEY, annotation_id；唯一可变活动指针，与不可变正文/标注分开 |
| annotation_bindings | annotation_id, local_key, concept_revision_id；local key 唯一 |
| annotation_occurrences / paths / path_steps | exact body block/selector、绑定和有序重复出现；用顺序/外键约束 |
| cognition_feedback | target 的公开类型化值、category/value/comment/author/time、applied_revision? |
| basis_invalidations | exact source revision/selection、纠正的新 revision 与原因；用于计算显式依赖的待复核 |

不把整个 Journal 正文/向量复制到这些表。每个 owner 提供纯 metadata，bootstrap 显式注册。基础 Artifact 的 owner/slot 仍是字节引用的唯一来源。

## 跨 owner 事务

手写文稿：内容发布 → WriteTx → 无来源首稿使用 Artifact.register_in；有来源或修订基准时使用 ResultStore.adopt_in → revision/source/namespace 更新 → commit → record_completed。人工结果的 source 输入保留各自 usage，编辑基准使用 revision_base；原模型稿直接采用时复用原输出 Artifact。

模型草稿：共同 produce → ResultStore.prepare_outputs → WriteTx → ResultStore.adopt_in + DraftDescriptor → commit。之后正式采用草稿在另一个事务内；草稿成功不依赖采用成功。

标注采用：准备 proposal/bindings → WriteTx → expected revision/annotation 检查 + 新 ConceptRevision + Annotation + head + generation → commit。

反馈更正：WriteTx → records.append_in + Feedback.applied_revision + 依据失效声明 + 必要的 generation → commit。

参与者接收已有 WriteTx，不重新打开写事务。地址生成使用已有 new_address/insert_address 能力；需要复用其合理功能时改为公开的窄接口，不从 content._records 私有模块跨域偷用。

## 引用一致性

Namespace、对象 kind 和 exact revision 的归属在一次操作边界及采用中核实。SQLite 外键/唯一键维护真实关系。body Artifact 和 ConceptRevision 均不可变；head 指针、archive/recall、active_annotation 在明确 owner 内修改。

精确来源目标只能指已经存在的版本，因此 revision 依赖自然向已提交内容指向；跨文稿循环的旧/新版本引用仍按版本 DAG 展开，不能把 logical 地址存成会随将来变化的边。

## 读取规模

记录列表分页，来源沿已有 ProvenanceReader 的相关祖先查询；认知 source bindings 提供认知意义与目标版本，再调用内容 provenance。不要复制 content 的来源递归。

捕获 CorpusView 在一个短 ReadTx 批量取得当前身份和不可变引用，结束事务后才读取大正文和向量。所有对象的 generation 与 refs 来自同一个一致视图。不要一边读 current 指针一边跨多次事务拼出混合代次。

## 旧数据

内部 schema 直接迁移到目标结构，授权开发实例可以重建；不建设旧文件读写兼容。正式 revision 历史是此后产品功能，与维护旧内部 schema 是两回事。

创建时预先分配文稿与 revision ID，使用 SQLite 支持的 deferred 外键或同事务先建 identity 再设置 head；事务提交后每个正式文稿必须有合法 head。不得为了构造顺序取消外键。

Namespace generation 仅表示 current-retrieval 定义的投影相关变化。新建未进入 recall 的 Note 不推进；创建/修改正文的操作仍返回实际存储结果和 not_in_scope。
