# 精确来源、正式归属与依赖冻结

## 正式值

内容的坐标和认知归属各有所有者。`ContentSelection` 表示某个不可变 Artifact 的范围；`RevisionSelection` 在它之上保存具体文稿修订的认知含义。

```python
class RevisionSelection(CognitiveValue):
    revision: RecordRevisionRef
    selections: tuple[ContentSelection, ...]  # 至少一项，允许不连续

class SourceBinding(CognitiveValue):
    key: str
    selections: tuple[ContentSelection, ...]
    owner_revision: RecordRevisionRef | None
    usage: Literal["reference", "current_basis", "inspiration"]
    name: str | None
```

SourceBinding 继续为冻结后的存储值，不再让外部输入类型决定是否保留 owner_revision。RecordView、SearchPassage/DeliveredSelection 和选段结果应方便地返回 RevisionSelection，不让调用者拆掉 revision 后再尝试恢复。通用 Wave 仍返回 CandidateDescriptor；认知 retrieval 在自己拥有正式文稿 metadata 的位置构造 RevisionSelection。

## 一个共同解析能力

`cognition/sources.py` 的 SourceResolver 集中来源准备，使用 RecordReader、ArtifactStore 与 MediaPreparer。媒体 owner 检查真实范围，内容 owner 负责字节和表示映射；它不依赖 CLI、Wave 私有 SQL 或 DBOS。

```python
class SourceResolver:
    async def prepare(
        self, namespace: NamespaceRef, inputs: Sequence[SourceInput],
        *, prefix: str = "S", reserved_keys: Sequence[str] = (),
        reject_journals: bool = False,
        usage: Literal["reference", "current_basis", "inspiration"] = "reference",
        preserve_keys: bool = False,
    ) -> PreparedSources: ...

    async def validate_in(
        self, tx: ReadTx, namespace: NamespaceRef, sources: Sequence[SourceBinding]
    ) -> None: ...
```

`SourceResolver.prepare` 返回的 `PreparedSources` 同时包含正式 `bindings` 和供语言规划使用的 `SelectedInputs`，按逻辑槽连接两者。写入采用正文坐标；语言请求保留实际消费表示。消费后的正式来源由 SourceResolver 按逻辑槽与生产者登记的坐标映射解析，不能从后续消费记录推测坐标。

使用共同的键分配规则，保持输入组、顺序、usage、显示名称和不连续范围。source:S1:C1 的局部键是这次输入目录中的键，不承担认知身份。

## 解析规则

正式 RecordRef 在一次一致读取中解析为精确 revision；RevisionRef 保持原版本；RevisionSelection 核对其每个内容选区实际属于该修订正文或拥有明确原文映射的已采用表示。拒绝跨实例、跨域或伪造的 body 绑定。

裸 Artifact/ContentSelection 保持合法通用输入。对于确为本域已采用文稿正文的 artifact，查询正式绑定补全 owner_revision；只能依据 artifact 身份和正式绑定，不能按相同 CAS digest、相同文本、名称或任意祖先猜测。映射唯一时补全；有多个正式 owner 时要求提供 RevisionSelection，不任取 current 或第一条记录。

外部原材料没有正式文稿归属时仍可作为 reference/inspiration。`current_basis` 表示可追踪的当前认识依赖，需要正式修订 owner；无法确定该语义时给出 `cognition.current_basis_owner_required`，引导调用者使用确切 RevisionSelection 或普通引用。以后 Resource 有正式版本能力时由该 owner 扩展，不在本轮造一个外部版本库。

raw Journal Artifact/ContentSelection 也要识别其已采用身份，journalize 按现有意图合同返回 `cognition.writing_intent_required`；rewrite/reflect 是明确入口。Note 引用了 Journal 的事实仍按 Note 自身语义使用，不沿任意血缘做“污染”分类。

## 范围校验与提交

准备时由 ArtifactStore 校验 artifact、实例身份、文本范围和 UTF-8 边界，MediaPreparer 使用确定性 inspect 校验媒体页、帧、时间和裁剪范围。文本按行的输入归一成原件绝对 UTF-8 半开区间；多选区分别保留。媒体校验不创建转换产物或触发语言解释。

大文件读取、媒体探测与解析在事务外；短事务重新检查正式 owner、不可变 body 绑定以及需要修改的可变前置条件。不可变源正文已经冻结，提交不再读一遍整个文件。调用者直接提供 SourceBinding 也必须经过该能力，不能成为绕过范围校验的后门。

`validate_sources_in` 不再只是确认 Artifact 行存在；它对“这些规范化选区对应哪个正式修订”负责。底层 content 不查询 cognition 表，跨领域归属在 cognition 完成。采用表示选区时，准备结果同时保留实际消费的表示及明确的原文映射；SourceBinding 用于认知依赖的 selections 归一到所声明 owner 的正文坐标，模型 actual_inputs 继续保存真正消费的表示。不能在采用时丢弃表示，再靠内容相似度找回原文。

## current_basis 的失效

保留 `reference/current_basis/inspiration` 区分。`correction` 对被纠正的 exact revision 登记失效，当前后代沿正式 revision DAG 计算 needs_review；普通编辑、归档、后续现实状态或新反思不声明旧认识错误。

RevisionRef、RevisionSelection、唯一可归属的 body ContentSelection 表示相同依赖时，得到相同 target_revision_id 和失效结果。更新依据后通过新 revision 完成复核，精确旧版仍可阅读。

来源范围和依赖范围不能无意扩大：不连续片段不转为 min/max 的连续区间；表示的原始页/时段也不能替换成整份媒体。字段在 input planning → actual_inputs → DraftContext → adoption → CorpusCapture → QueryResult 的每一步都有原样或明确变换后的消费者。

## 来源独立性

原始文稿各修订共用既有 source-origin；同一材料再次摘要不增加独立见证。保留 reference/current_basis 的正式 DAG 和 inspiration 的独立用途，解释性结构贡献继续按 SupportGroup 规则进入 Wave。来源解析不改变 VCP 数学或把用户反馈变为全局真实性分。

## 完成检查

直接用查询返回的 RevisionSelection 创建新 Journal，再更正其 current_basis，默认查询正确排除待复核文稿，include-review 可查看。对同一范围换用行与字节输入，结果与表示关联一致。越界输入在保存前拒绝；多归属不默选。手写、模型草稿、人工覆盖和反馈编辑都走同一解析与采用路径。
