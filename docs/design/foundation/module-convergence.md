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

普通 open_instance → Runtime 与受限 Job 客户端
显式 run_host → 同一循环的 DBOS 与实际领域操作
```

领域语义和正式状态不下沉到 DBOS、Native 或 transport。Runtime 是生命周期与依赖装配根，不是服务定位器；业务类通过构造得到所需能力，不接收整个 Runtime 再随意查找。

## 依赖方向

- `application/preparation.py` 负责内容和媒体的跨域编排。content 保持原件与原文机制；media 使用 content。
- ConceptAnnotationTask 位于 `cognition/annotation/task.py`；组合根将领域定义装入 TaskCatalog。models executor 不识别 Journal/Concept 任务名。
- `cognition.sources` 固定正式来源和模型原选择，写作不从返回的物理表示补猜 owner。
- Namespace 名称通过 NamespaceOperations 解析，长期句柄持有 stable ref。
- ResultStore、ArtifactStore、Wave 的公开“完整读取/描述”补齐执行与批量合同，消费者不直接取其 SQL/Native 内部结构。
- 配置 Snapshot 的冻结在定义者完成，不在 cache、media、Wave 各加局部副本。

## 模块的实际责任

| 位置 | 唯一职责 | 不应持有 |
|---|---|---|
| `jobs/contracts.py` | JobRef、JobView、持久关联等公开值 | Runtime、模型客户端 |
| `jobs/operations.py` | 入队适配、状态、等待、取消、恢复 | 文稿修改规则、第二套任务状态 |
| `jobs/runtime.py` | 通用 DBOS 生命周期适配和受限客户端 | 认知 payload/outcome、Journal/Concept SQL |
| `runtime/host.py` | 显式应用循环、宿主启动和停止顺序 | 领域采用规则 |
| `cognition/maintenance/operations.py` | MaintenancePlan、标注采用、刷新意图与跨 owner 事务 | DBOS 私有表与调度循环 |
| `cognition/maintenance/durable.py` | 显式注册两个认知 workflow 及真实步骤 | 媒体转换、直接模型协议 |
| `models/generation/` | 完整生成、成功响应恢复与结果采用 | 认知业务字段 |
| `cognition/sources.py` | 正式来源归属和冻结 | Markdown parser、队列 |
| `application/preparation.py` | 内容与媒体的成批输入协作 | 认知真值、正文身份 |

这些位置可依实际内聚性合并相邻短文件，不为一个动作建立 repository/service/manager 三层转发。大文件按独立修改责任拆分，不能机械按行数切碎。

## 正式保存和维护的界面

WrittenRecords 返回 CommittedRecord。RecordOperations 持有完整 WriteTx，组织事务参与者、草稿采用关联、反馈解决及维护入队。维护 owner 一次解释 AfterWrite 为 MaintenancePlan；保存核心不持有 AnnotationOperations、Wave 或 DBOS。

前台维护与持久维护都调用同一认知维护能力。只有持久分支经过 DBOS；没有为前台另写一套 SQL。反过来，普通创建、精确读取和 lexical 查询不被强制包装成 workflow。

## 共用能力

领域 revisions、内容血缘、historical 精确读取、VCP/SupportGroup、按路线规划和可配置备用模型各自保留真实责任。结果采用复用已发布输出；坐标由生产者登记；Wave 用同源清单、不可变单元及组件引用管理资产。请求备用模型属于明确资源政策。

## 代码审查应回答的实际问题

追踪一个用户动作，确认目标身份、来源、配置、实际语言输入、采用引用与结果在各层都保留其含义。发现“丢掉字段 → 根据字符串猜回”“为了下游接口反复转换”“第三处复制相同规则”时修改定义者。检查方法是否真正拥有完整行为，而非用一个新 facade 遮住旧的耦合。

维护成本包括新测试、脚本、配置项和文档。本轮不新增永久探针平台、泛化事件总线、通用恢复账本或所有模块必须继承的万能 Memory/Manager。
