# 稳定身份、操作目标与版本前置条件

## 目标与基线缺陷

`Runtime.cognition()` 按 ID 缓存服务，却让 reader、concepts 和 retrieval 保存完整 Namespace(name, generation)。这些字段改变后，调用判断仍使用旧名称。CLI 又把完整 expected 当成实际操作目标。两者都在入口到 owner 之间丢掉了用户选择的对象。

本专题把定位、稳定身份、可变描述和并发前置条件分开，结果由事务中的正式对象决定。

## Namespace 合同

```python
class NamespaceRef(CognitiveValue):
    instance_id: str
    id: str

class Namespace(CognitiveValue):
    ref: NamespaceRef
    name: str
    content_generation: int

async def Runtime.cognition(namespace: str | NamespaceRef | None = None) -> CognitiveServices: ...
async def NamespaceOperations.resolve(selector, *, tx: ReadTx | None = None) -> NamespaceRef: ...
async def NamespaceOperations.read(ref: NamespaceRef) -> Namespace: ...
```

Namespace 是一次读取的值；CognitiveServices 长期持有 NamespaceRef 和实际服务对象，不能把读取时的 name/generation 用作后续授权或视图判断。缓存按稳定 ID，名称改动无须遍历并刷新缓存。

公开字符串 selector 由 NamespaceOperations 统一解释，内部用 typed NamespaceRef；已保存的 default_namespace 绑定稳定 ID。保留现有名称/ID输入便利性时，也必须经过同一个解析器；名称不得与可识别内部 ID 产生歧义。这里不建立永久名称别名或重命名历史兼容。

绑定操作缺省使用 bound ref。确有显式 namespace 参数的入口先解析为 ref 再比较；推荐从绑定的公开操作删除重复 namespace 参数，所有内部 `_in` 参与者显式接受 namespace_id。一次修改同步迁移 CLI、reader、concepts、retrieval、annotation、feedback 和示例，避免长期保留 bound/unbound 两套公开规则。

重命名 `research → science` 后，旧句柄仍属于同一 ID。新建的 research 是另一个 ID；通过旧句柄传入新 research 名称必须拒绝，不能静默操作 science。读取句柄的名称通过 Namespace owner 返回当前值。

## 文稿修改合同

```python
async def replace(target: RecordTarget, markdown: str, *, expected: ExpectedRevision | None = None, ...) -> RecordOutcome: ...
async def apply(target: RecordTarget, changes, *, expected: ExpectedRevision | None = None, ...) -> RecordOutcome: ...
async def archive(target: RecordTarget, *, expected: ExpectedRevision | None = None, ...) -> RecordOutcome: ...
async def restore(target: RecordTarget, *, expected: ExpectedRevision | None = None, ...) -> RecordOutcome: ...
async def set_recall(target: RecordTarget, enabled: bool, *, expected: ExpectedRevision | None = None, ...) -> RecordOutcome: ...
```

ExpectedRevision 接受精确 RevisionRef、精确字符串或正 ordinal。省略 expected 仅在 target 自带精确 revision 时成立。逻辑 target 未指定 expected 时返回 `input.expected_required`，不暗中读取最新 head 充当用户基准。

事务中的共同顺序：解析 target → 核验实例/Namespace/种类 → 解析 expected → 比较 record identity → 读取 head → 比较 exact expected → 应用变更。跨文稿返回 `cognition.target_mismatch`；同文稿过期返回 `cognition.revision_conflict`，带 current 与 caller expected。校验失败不产生正文、状态或 generation 变化。

Expected 为整数时只在 target 的 revision 集合内解释。完整引用包含对象身份时必须核对，不能将其截成 ordinal 后继续。target 本身也精确时，显式 expected 必须指同一精确版本，否则报输入冲突。

CLI 保留 ref 和 expected 两个参数交给共同方法，不再做 `expected = ref.split(...)`，也不重复 SQL。编辑器固定打开时的精确基准；草稿采用固定 draft.base_revision；这些值最终进入同一提交核心。

## 原子性与来源

模型、编辑器和 CAS 准备发生在事务外。最终 target/expected/head 关系在 WriteTx 内重新核验；八个并发修改同一版本只有一个可成功。草稿采用和 feedback resolution 与该 revision 在同一事务完成。错误冲突保留已完成草稿和人工文件。

可读 identifier 保留本轮已有的六词、65 位编码；UUID 继续用于内部稳定身份。不要为了修复目标错误恢复旧 address/proquint 双轨，也不要用显示标题或正文 hash 判断对象同一性。

## 修改位置

`cognition/contracts.py`、`scope.py`、`services.py`、`writing/{reading,records,operations}.py`、`concepts.py`、`retrieval/operations.py`、`runtime/bootstrap.py`、`transports/cli/{cognition,concepts,journalize,retrieval,feedback}.py`。同时调整默认域配置的读写、包导出及进程内示例。

完成时检查的是同一操作在重命名、名称再利用、跨对象 expected、过期 expected、精确引用和整数 expected 下的实际目标；不为每种字符串写独立规则。
