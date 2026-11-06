# 模块职责与调用路径收敛

## 目标拓扑

```text
进程内宿主 / CLI / 后续远程适配
    → 完整应用操作
        → 文稿、Concept、Annotation、Feedback、当前认知查询
        → 共同生成、输入准备、Embedding、媒体解释、Wave
        → Job 登记/控制（仅明确持久工作）
    → 各领域 owner 的短事务参与能力
        → Database / Artifact / ResultStore / CAS

DBOS 调度循环 → RuntimeBridge → 原 Nous 循环中的同一业务操作
```

领域语义和正式状态不下沉到 DBOS、Native 或 transport。Runtime 是生命周期与依赖装配根，不是服务定位器；业务类通过构造得到所需能力，不接收整个 Runtime 再随意查找。

## 需要修改的依赖

- `content/preparation.py → media` 的跨域编排迁至 `application/preparation.py`。content 保持原件与原文机制；media 继续使用 content，不形成环。
- ConceptAnnotationTask 的认知合同位于 `cognition/annotation_task.py`；组合根把定义注册进通用 TaskCatalog。models executor 不识别 Journal/Concept 任务名。
- `WritingOperations._freeze`、`RecordReader.select_sources`、各入口的 SourceBinding 分支集中到 cognition 来源 owner。
- namespace 名称匹配从 Reader/Concept/Retrieval 中移除，使用共同 stable ref。
- ResultStore、ArtifactStore、Wave 的公开“完整读取/描述”补齐执行与批量合同，消费者不直接取其 SQL/Native 内部结构。
- 配置 Snapshot 的冻结在定义者完成，不在 cache、media、Wave 各加局部副本。

## 新模块的实际责任

| 位置 | 唯一职责 | 不应持有 |
|---|---|---|
| `jobs/contracts.py` | JobRef、JobView、持久关联等公开值 | Runtime、模型客户端 |
| `jobs/operations.py` | 入队适配、状态、等待、取消、恢复 | 文稿修改规则、第二套任务状态 |
| `jobs/runtime.py` | DBOS 装配、注册、启动/关闭、版本与队列 | Journal/Concept SQL |
| `jobs/bridge.py` | 跨循环派发与真实完成/取消信号 | 任意命令路由、存储状态机 |
| `cognition/maintenance/operations.py` | 标注准备、采用条件、刷新意图 | DBOS 私有表与调度循环 |
| `jobs/maintenance.py` | 两个固定 DBOS workflow 的步骤组织 | SQL、媒体转换、直接模型协议 |
| `cognition/sources.py` | 正式来源归属和冻结 | Markdown parser、队列 |
| `application/preparation.py` | 内容与媒体的成批输入协作 | 认知真值、正文身份 |

这些位置可依实际内聚性合并相邻短文件，不为一个动作建立 repository/service/manager 三层转发。大文件按独立修改责任拆分，不能机械按行数切碎。

## 正式保存和维护的界面

WrittenRecords 仍返回 CommittedRecord。RecordOperations 组织事务参与者、草稿采用关联、反馈解决及明确维护入队。维护如何执行通过 AfterWrite 选择；保存核心不持有 AnnotationOperations、Wave 或 DBOS。

前台维护与持久维护都调用同一认知维护能力。只有持久分支经过 DBOS；没有为前台另写一套 SQL。反过来，普通创建、精确读取和 lexical 查询不被强制包装成 workflow。

## 删除范围

删除被新源解析取代的私有解析分支、CLI expected 重定向、content→media 旧准备模块、通用 tasks 内认知专用类、重复 per-candidate 快照加载及仅为旧接口服务的测试/导出。删除后更新实际注册、配置、示例和 docs，不保留 deprecated 别名和双轨 fallback。

保留有用的现有能力：领域 revisions、内容血缘、historical 精确读取、VCP/SupportGroup、模型按路线准备与可配置备用路线。合法备用模型是产品资源选择，不能与为掩盖内部错误临时发起的静默降级混为一谈。

## 代码审查应回答的实际问题

追踪一个用户动作，确认目标身份、来源、配置、实际语言输入、采用引用与结果在各层都保留其含义。发现“丢掉字段 → 根据字符串猜回”“为了下游接口反复转换”“第三处复制相同规则”时修改定义者。检查方法是否真正拥有完整行为，而非用一个新 facade 遮住旧的耦合。

维护成本包括新测试、脚本、配置项和文档。本轮不新增永久探针平台、泛化事件总线、通用恢复账本或所有模块必须继承的万能 Memory/Manager。
