# 模型、媒体与内容通路实施计划

## 最终成果

在已有运行基础上交付正式 `nous tools`。使用真实本地网关与资料，完成文本/结构化生成、Prompt 管理、图片/PDF/音视频准备、三种媒体解释策略、CAS 和数据库结果保存、原文选段，以及真实文本 Embedding。

使用者能接纳一个文件，取得可读内容地址；按配置进行一次真实处理；看到正文、实际覆盖和结果地址；关闭并重新打开后仍能读取原件、描述、结构化结果和来源。所有正式入口消费同一内核。

Note、Journal、journalize、Dream、Steward 的认知机制、Wave、完整 Namespace、DBOS、HTTP/MCP、远程 SDK、本轮未需要的格式和网关运维平台不进入施工范围。

## 输入与授权

阅读 [架构](../ARCHITECTURE.md)、[配置接入](../design/foundation/configuration-integration.md)、[内容记录](../design/foundation/content-records.md)、[模型执行](../design/models/execution.md)、[媒体解释](../design/media/analysis.md)、[工具 CLI](../design/interfaces/tools-cli.md)。各专题分别维护详细合同，计划只安排依赖与实际改动。

当前基础定位基准为 429c841，开始时读取最新文件，不回退工作。用户会同步新的讨论和边界要求，优先合入当前负责专题。现行基础可复用，错误职责直接重构。

本机 Docker New API 已授权用于真实调用；按 [本地网关](../development/local-gateway.md) 取得已存在地址与可用 Key、选择可用模型。没有金额批准前置；不要重建网关、轮换无关凭据或修改上游渠道。

## 目标职责位置

| 位置 | 本轮责任与迁移 |
|---|---|
| src/nous/application/configuration.py | 完整配置变更用例，迁出 CLI/bootstrap 的重复绑定编排 |
| src/nous/configuration/ | 保留纯定义、来源解析、快照与文件保存，按职责整理 service.py |
| src/nous/models/ | 资源、任务、Prompt、真实执行、Embedding 空间 owner |
| src/nous/network/、credentials/ | 连接配置与凭据读取，不认识媒体/认知领域 |
| src/nous/storage/ | 现有数据库与 CAS，正式内容引用 |
| src/nous/content/ | Artifact、ProcessingResult、可读地址、结果采用与读取 |
| src/nous/media/ | 选区、各模态准备与 MediaAnalysis，复用 GenerationOperations |
| src/nous/runtime/ | 显式装配、执行、期限、关闭，保持当前 instance/paths 合同 |
| src/nous/observability/ | 轻量埋点 API 与 SDK 装配分离，增加真实处理阶段关联 |
| src/nous/transports/cli/ | 只负责命令适配和呈现，接入 tools 命令 |

这些位置按实际高内聚责任落地，不为每个词建立一层 service/repository/manager。纯共同值和错误可放 src/nous/common；不要用新通用容器接替全局 app。

## 共同合同先落实

明确 ArtifactRef/ResultRef、BlobRef、SourceSelection、PreparedSource、GenerationResult/producer 和 ConfigurationOperations 的实际类型。依赖这些值的模块引用唯一维护位置。

配置 resolver 在数据库前可用；配置操作在绑定 owner 构造后可用。当前时间域形状与正式域的校验合一。基础模块通过 logging/轻量 OTel API 埋点，不导入完整 SDK 装配。

业务用例使用一次根 Execution 操作；嵌套阶段继承名额、快照与期限。新增模型、媒体和内容消费者不各写一份取消/线程调度。

## 内容接纳与读取

实现 CAS 流式 prepare/publish/attach/read，处理 temp/content 跨卷。ContentStore 的内容引用与 Artifact 元数据保持一份正文映射。

接入 ArtifactStore、ResultStore 及 art/res 短地址，CLI `content add/read/export` 可真实使用。显式初始化收集所需表；普通打开仍不创建空数据库或绕过既有路径绑定。此次本地开发使用新指定的 `temp/nous-model-media` 实例完成初始化，避免把新增表暗中写进旧实例。旧开发格式不兼容时明确报出，不建立内部升级链，也不删除其它实例；与用户正在使用的当前开发实例需合并时按其实际授权处理。

正式结果采用将结果、输出 Artifact 与内容引用同事务提交；跨 owner 的采用使用 `adopt_in(tx)`，不在现有事务内再开结果事务。开始即接入这条原子路径，不在后面以若干相互调用的补偿服务修补。

## 网络、Prompt 与真实文字生成

同时完成 Credentials、网络连接与成熟模型客户端。发现本地网关上的可用模型，填写真实实例配置。产品代码中无 New API Docker/管理 API 依赖。

PromptCatalog/Compiler 接入包内与实例资源，任务合同与字段定义保持一份。preview、run 共用确定编译结果。先让 summarize 和一个结构化任务经真实网关运行，再把请求与输出通过正式 ResultStore 保存。

根据 [fallback](../design/models/fallback.md) 和 [额度](../design/models/budgets.md) 接通流、有限尝试、取消、用量。阶段重试计数与两阶段正常调用分开，费用限制不启用。

此时完整链为：输入 → CAS → 编译 → 网关 → 输出 CAS → 短事务 → 结果读取。Provider 返回成功而保存失败时只处理采用失败，不再请求模型。

## 媒体准备与三种解释

按图片、PDF、音频、视频专题分别采用 Pillow、PDFium、FFmpeg/ffprobe 与已存在 SDK 的真实媒体输入。

MediaAnalysis 显式组织策略与分批，复用 GenerationOperations。原始媒体与描述同时参与 describe_then_structure 的第二阶段；只有转换为转写的路径才按其实际表示工作。

完整范围超过单次限制时分批并明确 coverage；中间成功结果可继续读取。完成 PDF 选页、视频采样和声音处理，不能通过只发文件路径或只看第一帧报告多模态成立。

必要外部可执行文件通过当前环境正常发现或安装，并把配置与安装说明写清；依赖安装不变成权限/测试治理。自带二进制分发以后按实际发行需要处理。

## Embedding 与配置采用

实际文本 Embedding 使用统一连接。先 probe 确认返回形状，再以显式空间执行、保存、重开读取。首次向量采用同事务登记绑定，随后普通配置修改向量语义会由 owner 拒绝。

删除 Configuration 中临时的空间全拒绝和模型名判断。共享配置操作调用实际绑定能力，离线与运行内管理一致。provider/source 相关普通改动按现行生效语义处理。

## 清理与文档

迁移真实生产者、消费者、配置目录、导出和相关用户说明，删除重复编排、旧 callback 回填、被替代内部方法和仅服务这些方法的测试支撑。

现有 tests/python/test_database_contention.py 的独立 PRAGMA/底层争用情境迁到实际内容保存/结果采用通路后删除。其它仍保护完整功能的测试保留或按新入口迁移，不为测试数量清仓。

只迁入本轮真正使用的 legacy 独立能力，不恢复旧服务/SDK/Native 运行栈。CURRENT 仅更新真实完成能力和剩余项，不列讨论归档文件。

## 通路验证与独立验收

按 [通路说明](../development/model-media-walkthrough.md) 走实际 CLI/应用操作：真实材料、Prompt、模型、内容和数据库共同参与。Coding Agent 可用正式命令调试实际故障；用户做独立验证。咨询/审查者不另跑本地测试或把 CI 状态当交付判断。

不新增测试 DSL、评测平台、dev 框架或 CI 流程。已有 pytest 只保留入口通路与高价值高风险回归；不要为字段、helper 或库内部功能逐项造测试。

## 完成条件

正式 tools 命令可用；真实文字/结构化/媒体/Embedding 至少各有相应实际结果；所有范围与未覆盖内容准确；原件/派生/请求可按引用读回；配置与重启行为一致；取消和部分成功清楚；无需 Note/Journal 或持久队列才能运行。

某项实际模型能力不可取得时报告真实缺项，已完成的内容处理仍交付；继续完成其它已授权功能，不能把整个任务降格成 mock 演示。实现者的总结记录实际使用与结果，用户验收结论另由用户决定。
