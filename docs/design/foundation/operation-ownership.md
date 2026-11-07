# 共同操作与模块所有权

## 依赖方向

入口只解析与呈现。应用操作封装完整用户动作；能力 owner 保存自己的规则、状态和事务责任。基础设施提供完整公共能力，使用者不访问其内部字段补齐功能。

```text
CLI / 以后 HTTP、MCP、SDK
          ↓
GenerationOperations / MediaAnalysis / EmbeddingOperations / WaveOperations
          ↓
输入规划、模型请求、媒体表示、内容/来源/正文、向量、时间
          ↓
配置快照、Execution、Database、CAS、网络、轻量观测

Python Wave → PyO3 边界 → Rust retrieval 核心 → VCP、Tantivy、USearch
```

媒体准备不导入模型 executor。MediaAnalysis 可以调用完整 GenerationOperations；GenerationOperations 可以消费确定性媒体准备。这两个模块的依赖方向明确，包的 __init__ 不相互导入所有服务。content 不依赖 media/models/wave。

## 当前结构的具体调整

| 所有者 | 实现位置及完整职责 | 迁出的职责 |
|---|---|---|
| ArtifactStore | `content/artifacts.py`：接纳、元信息、正文/选区、borrow/export | 不遍历处理关系、不解析 PDF |
| ResultStore | `content/results.py`：采用、get/read_view，组合来源读取 | 不执行模型与媒体 |
| 来源查询 | `content/provenance.py`：输入关系的有界祖先查询 | 不复制全库关系到 Wave |
| TextView | `content/text.py`：Markdown 结构、块位置、精确读取 | 模型任务不再自行 split/parse |
| 媒体准备 | `media/preparation.py`：识别、表示计划、逐批物化与 text_view | Wave 不自建 PDF 文本结果 |
| 语义输入规划 | `models/input.py`：一次任务材料集合与请求分组 | CLI、preview、MediaAnalysis 不各写一套 |
| 请求策略 | `models/policy.py`：尝试分类、期限、退避及 attempt 观测 | 各端点保留自身请求编码 |
| 向量业务 | `application/embedding.py` + `models/embeddings.py`：复用/采用与批次执行 | run/ensure 不各有一套批次规则 |
| 表定义 | `content/schema.py`、`models/embedding_schema.py`、`wave/schema.py` | 表不放在 service/repository 文件内 |
| Wave 快照 | `wave/snapshots.py`：构建、发布、打开、身份与引用保存 | 不负责模型调用细节 |
| Wave 查询 | `wave/query.py`：查询输入、时间/来源范围、结果物化 | 不手写底层数值排序 |
| Wave 入口 | `wave/operations.py`：完整用例，组合上述深模块 | 删除混在其中的 SQL、数组编解码大段 |
| Native | 见 [核心](../wave/engine.md) | Python binding 不组织算法 |

这些模块按实际数据和生命周期承担职责，不通过 mixin 或共享巨大 self 分散实现。内容读取与结果采用分别由 artifacts/results 模块提供，来源查询和正文结构通过各自公开能力消费。

同一个公开动作可以由一个应用操作调用数个深模块；新增模块必须拥有规则、数据或资源生命周期。纯粹重复签名的转发、DTO 来回转换不形成独立层。目录按 content/media/models/wave 等稳定主题组织，不增加 controller/service/manager/repository 四层目录。

## 共用能力不足时的处理

需要 PDF 搜索文字，完善 MediaPreparer.text_view；需要正文块，完善 TextView；需要向量批次，完善 EmbeddingExecutor；需要请求诊断，完善 request attempt；需要来源，完善 ResultStore 的来源 owner。所有消费者一起迁入，删除其局部实现。

配置 field 的范围与生效由 owner 定义，统一配置系统收集。所有实际请求在一次 ConfigSnapshot 下工作。没有运行消费者的配置不提前加入菜单；已在公开配置中的参数必须有可说明的实际作用。

## 事务与资源

完整采用操作决定 WriteTx，各参与 owner 接收同一事务；模型、转换与大文件工作都在事务外。纯表 metadata 可在一个基础注册器中汇合，但 schema 只能做声明；组合根显式导入并创建全部声明，避免服务模块副作用。

复用 Execution 的根操作、嵌套上下文、期限、阻塞工作和取消。局部 owner 决定文件/句柄何时释放，Execution 保证 worker 真正退出。协议 SDK 的隐式重试关闭，保留唯一实际尝试策略。

代码长度由职责衡量；明显长到混合装配、算法、协议、存储时在这些职责处拆分。开发检查的警告和拒绝阈值由 [scripts/check/config/check.toml](../../../scripts/check/config/check.toml) 定义；文件仍按职责拆分，也不把十行纯值对象拆成新包。
