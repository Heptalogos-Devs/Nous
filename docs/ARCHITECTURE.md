# Nous 应用与 Wave Native 架构

## 产品能力与本次范围

Nous 提供独立的内容处理与长期认知能力。实例、配置、CAS、Artifact、ProcessingResult、模型、Prompt、媒体和 Embedding 是共同基础。语义输入、逐批表示、来源与正文、请求诊断和 Rust retrieval 各有明确负责模块；Journal 草稿与标注通过这些能力进入 Wave 实验，见 [主计划](plans/architecture-repair.md)。

本轮建立的 Wave 集合是检索输入与可重建投影。Journal 的目标语义与修订已在 cognition 专题定义；完整认知对象由后续领域实现。已存在的 Artifact 可以成为当前候选；将来 Journal 提供相同的精确内容引用即可接入。

## 调用方向

```text
CLI / 后续 HTTP、MCP、SDK
  ↓
完整应用操作：配置、生成、媒体解释、向量、Wave
  ├─ 内容接纳/读取/结果采用 → ArtifactStore、ResultStore → CAS、Database
  ├─ 语言阶段 → 任务合同 + 冻结 Prompt → ModelExecutor → 连接与凭据 → 网关
  ├─ 确定性准备 → MediaPreparer → Pillow、PDFium、FFmpeg
  └─ Wave → 内容/向量 owner、确定性媒体抽文 → 冻结资产 → Rust Native
                                           └─ retrieval Engine / Snapshot
                                               ├─ 词项/ANN 索引
                                               ├─ VCP 数值闭包
                                               └─ Nous 数值扩展

组合根：生命周期与依赖装配
共同基础：配置快照、Clock、Execution、Database、CAS、错误、轻量观测
```

MediaPreparer 不持有模型执行器；ModelExecutor 不读取业务数据库。应用操作可以调用其它具有明确业务作用的操作，但不相互递归、不从整个 Runtime 查找依赖。CLI 不创建候选配置或另一套模型/模板编译路径。

## 状态所有权

| 状态 | 所有者 |
|---|---|
| 配置定义、来源、保存值和有效代 | Configuration |
| 配置修改的绑定与资源采用 | ConfigurationOperations，调用相应 owner |
| 不可变字节及 owner/slot 引用 | ContentStore |
| 原件、派生物与实际处理输入输出 | ArtifactStore、ResultStore |
| 精确输入分组与表示选择 | 共同输入能力与确定性媒体能力 |
| 一次语言任务与结果采用 | GenerationOperations（包括转写）；多阶段由 MediaAnalysis 组织 |
| 模型请求、尝试、流与响应 | ModelExecutor，协议适配由成熟依赖承担 |
| 模板内容与输入输出合同 | PromptCatalog 与具体任务定义 |
| 向量空间、已采用向量及输入映射 | Embedding owner |
| Wave 集合定义、来源映射、活动快照 | Wave owner |
| 冻结矩阵、索引和单次查询观测 | Native，均为可重建计算资产 |
| 实际认知、关系和修订 | 后续认知领域；Native 不写认知 |

## 事务与副作用

耗时解码、模型调用、索引构建与 CAS 发布在短写事务外。正式采用由一个用例开启 WriteTx，结果、引用、空间/快照关系由参与者在同一事务完成。保存失败不重新执行已成功的模型调用。Native 只接收已明确的数组/快照，不打开业务 SQLite，也不从 Python 回调逐条加载数据。

## 资源与并发

CPython 3.14、uv 与当前成熟依赖继续使用；Rust 通过 PyO3/maturin 接入。现行 Native 工作区是复用来源，不是不可变接口。计算脱离 Python 后使用自有不可变内存和一个实例级有界 Rayon 池。

Execution 控制根操作、截止时间和实际 worker 生命周期。Native 合作取消由 Rust token 实现，取消后等待实际计算退出再释放资产。不存在独立 Native 守护进程或另一套任务调度器。

数据库、内容、模型与 Wave 分别使用已解析配置。Rust 算法参数由其真实配置类型导出 schema/defaults，Python 配置目录承接；不保留两套手写默认值。

## 开发边界

现有 VCP 数值模块与 DTSC 保留，本轮补齐可复用检索核心、候选池及多路径/支持组实验；不恢复 retired Node/JS 运行壳和旧 Nous 服务。DBOS 留给实际持久后台任务，完整 NousQL 与认知对象不作为本轮前提。

现行代码可以按真实用途重新组织。没有外部兼容承诺的接口直接迁移，旧路径和专属支撑随替换删除。临时样本和诊断留在 temp，网关补丁与独立语音工具保持独立。

## 深模块与正文结构

具体迁移位置见 [操作所有权](design/foundation/operation-ownership.md)。Rust retrieval 拥有 Engine/Snapshot 与查询业务，PyO3 只做语言边界。纯 schema 模块由组合根显式装配，业务服务不通过隐含导入注册表。

Journal 正文、版本、概念标注、关系结构和支持来源的目标分工见 [Journal](design/cognition/journal.md)。本轮以共同 TextView、现有 ProcessingResult 和 Wave 运行实验；完整 Journal owner 不作为修复工作的前置。
