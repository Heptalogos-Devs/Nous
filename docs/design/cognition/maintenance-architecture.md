# 文稿维护的所有权与程序内使用

Note 和 Journal 的身份、不可变修订、正文、来源和召回资格由认知领域拥有。写作、标注、反馈与检索各提供完整操作；Runtime 构造这些 owner，CLI、SDK 和宿主通过同一 `Application.call` 入口调用。

```text
CLI / SDK / 宿主 → Application.call(操作定义, 类型化请求)
  ├─ RecordOperations → WrittenRecords / RecordReader → 内容与数据库
  ├─ WritingOperations → GenerationOperations → Result / Draft → 正式采用
  ├─ AnnotationOperations → 提案 / 标注采用
  ├─ FeedbackOperations → 实际交付与意见
  └─ CognitiveRetrieval → 目录捕获 / 投影 → Wave / Native
```

`RecordOperations` 负责输入、正文短事务以及提交后的维护；`WrittenRecords` 不依赖标注或检索。`WritingOperations` 保存模型成果、组装完整草稿，再由文稿 owner 正式采用。标注 owner 保存提案并核对活动指针；检索 owner 捕获当前正式视图并发布快照。

## 程序入口

```python
from nous.cognition.writing.contracts import AddNote, AfterWrite, ReadRecord
from nous.cognition.writing.operations import ADD_NOTE, READ_RECORD
from nous.content.contracts import InlineText
from nous.runtime.bootstrap import open_instance

async with open_instance(instance) as runtime:
    note = await runtime.app.call(
        ADD_NOTE,
        AddNote(body=InlineText(text="一次实际经历"), remember=True,
                after=AfterWrite(annotation="skip")),
    )
    view = await runtime.app.call(READ_RECORD, ReadRecord(target=note.commit.revision_ref))
```

请求的 namespace 可为名称、稳定 `NamespaceRef` 或默认值。精确目标携带域身份时由共同 resolver 使用并核验；改名不改变已有引用归属。完整示例见 [host-use.py](../../examples/journal-maintenance/host-use.py)。

## 提交与维护

`CommittedRecord` 返回稳定引用、精确修订、提交代次、标题、是否变化和召回资格。`RecordOutcome` 组合该提交与 `MaintenanceOutcome`、问题列表。正文提交后执行标注和刷新，维护失败不撤销正文。

`AfterWrite` 的 `annotation` 为 default、skip 或 run；`refresh` 为 defer 或 run，`mode` 选择检索模式，`execution` 为 wait 或 background。default 由配置和召回资格确定。没有宿主能力时，明确 background 在写入前拒绝；容量不足或运行期停止发生在正文提交后时，维护返回 not_started。background 返回当前 Runtime 的操作引用，进程退出后过期。相同草稿再次采用返回已有修订，不重复启动维护。

标注与刷新共用同一维护操作；标注失败后仍可执行请求的刷新。生成期间正文或活动标注变化时，旧提案仍可读，但不能覆盖当前状态。详细状态见[提交后维护](durable-maintenance.md)，检索发布见[当前知识视图](current-retrieval.md)。

## 代码归属

`cognition/writing/records.py` 和 `reading.py` 分别负责事务内正式写入与精确读取；`drafts.py` 保存完整草稿及采用关系，`generation.py` 组合写作成果。`annotation/` 拥有计划、提案和采用；`maintenance/` 负责提交后选择与当前运行期任务；`retrieval/` 负责轻量目录、变化材料展开、投影和查询；`feedback.py` 保留实际交付与处理关系。

跨 owner 原子采用时，参与者接收已有 `WriteTx`；网络、媒体转换和 Native 构建在事务外完成。文稿、结果、草稿、标注和快照各自有明确保存点与读取入口。
