# fdb39e4 结构审查与本轮决定

## 结论与范围

审查基于 Nous `fdb39e433126abe94d3231292eabb0c857772ec2`，与前次 d86c737 比较并沿当前调用链阅读。没有运行本地测试、查询 CI 或把用户的验证报告替代源码判断。本轮保留已建立的运行/模型/内容基础，先修正跨 owner 接缝，再进入 Native；不重建整个应用。

以下是源码事实与据此作出的设计决定。审查不涉及 New API 转写补丁和用户独立样本生成工具。

## 最新修复应继续保留

| 已观察改动 | 设计意义 |
|---|---|
| Execution 按 owner 保存上下文，前台操作有范围完成信号 | 避免不同执行器共享名额；关闭等待真正结束的范围 |
| 完整 SDK response.state/finish_reason 判断及 Direct API | 保持流完成真实性，协议反馈交给依赖 |
| PDF 页缓存先检查再渲染，原件级并发协调 | 避免先做昂贵工作再查缓存 |
| 选区按模态验证 | 防止不适用字段被静默丢弃 |
| Embedding 响应与 index 类型检查 | 保护逐项向量对应 |
| CAS 导出实际 digest/长度与取消清理 | 保持交付内容真实性 |

来源：[执行器](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/runtime/execution.py)、[模型执行](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/models/executor.py)、[媒体准备](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/media/preparation.py)、[CAS](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/storage/content.py)。

## 需要直接重构的接缝

### 语言调用仍进入确定性准备

MediaPreparer 持有 ModelExecutor，`_audio_source` 直接转写并采用结果；GenerationOperations 的准备也会到达该方法。与现行“普通准备不触发额外语言阶段”不一致。

决定：转写组织进入应用语言阶段，准备器只输出确定性计划和真实表示。模型客户端和结果采用仍复用共同 owner。

### CLI 预览和 probe 具有独立业务实现

`prompt_preview` 固定 MIME 列表、访问 preparer、合并全部批次并另建 Compiler；`model_probe` 在复制配置中插入 probe-internal 任务。这些不是简单参数转换，后续 HTTP/MCP 会被迫复制。

决定：共同应用接口提供 preview/probe，传明确选择，不伪造配置代；CLI 仅呈现。纯模板预览本来就可以不选模型，不能把它与真实 task preview 混为一种承诺。

来源：[CLI tools](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/transports/cli/tools.py#L120-L350)。

### 来源数据形状缺少实际 part 与原件坐标的区分

PreparedSource 只有一个 artifact/selection，却可同时装页图和页文本；PDF 路径选 outputs 最后一个，分段音频/transcript 使用原媒体坐标。`_batch` 又追加原件 S1 和实际来源列表。这里需要调整共同数据形状，不能只在 Wave 去重后补偿。

决定：InputGroup/parts 各自指向实际正文，origin 指回原件和变换；实际消费与来源血缘分开。相同原文不同实际使用保留，独立支持按根集合计算。

来源：[输入值](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/models/contracts.py)、[来源记录](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/content/contracts.py)、[媒体页与音轨](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/media/preparation.py#L475-L740)。

### 单次上限没有限制全操作物化量

prepare 返回整份输入所有 batches 的 BinaryContent；GenerationOperations 再为每批预编译后才开始请求。每批大小受限，整个长文件仍累计全部字节。视频每次采样从头解码一次，放大连续处理成本。

决定：计划/引用与实际物化分离，一次只物化有界批次；复用 FFmpeg 一次/分片采样。现有成功页缓存和引用不丢。

### 通用模型执行混入任务业务

Executor 按 information-extraction/passage-selection 校验局部键，GenerationOperations 再解释同一专名输出。新增认知任务会继续扩大这个分支。

决定：TaskDefinition 拥有来源语义和结果解释；Executor 只负责协议与形状，统一采用由应用完成。

### 多路线与配置采用需要收紧

准备阶段对全部候选窗口/输出/时限取最小值；fallback 未给 preset 时使用空预设，与继承合同不同。网络连接持有启动 Credential 引用，但引用对象的 name/path 缺少相同重启声明；Provider 网络合并用 exclude_none 丢失显式清空语义。

决定：每条路线独立准备，主请求不受备用窗口提前压缩；凭据绑定和连接同生命周期；resolver 区分缺失/null。文本/媒体 token 启发式保留为估计，不以压缩字节换算假定精确模态成本。

来源：[请求准备](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/models/executor.py#L60-L325)、[网络](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/network/__init__.py)、[配置定义](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/models/configuration.py)。

### 共用基础还存在局部绕过

内容 owner 自行取 datetime.now；媒体直接拿 CAS path_for；文件写入和发布各有局部 shield。单个操作看起来很小，多消费者出现后会复制生命周期判断。

决定：Clock、borrow_local 与 Execution 的明确阻塞等待机制集中承担责任。无需抽象成万能基础服务。

### Schema 装配存在隐藏的导入依赖

bootstrap 从 models.embedding_store 导入 metadata 并命名为 content_metadata；该模块在 content 的 MetaData 上追加向量表。结果是内容建表通过模型模块间接装配。增加 Wave 后不能继续通过另一个上层模块连带注册所有表。

决定：组合根显式收集各 owner 的纯表定义，以 SQLAlchemy 支持的共同 MetaData 解析跨 owner 外键。保持一份表定义，消除依赖某个业务模块导入顺序的装配。来源：[bootstrap imports](https://github.com/Heptalogos-Devs/Nous/blob/fdb39e433126abe94d3231292eabb0c857772ec2/src/nous/runtime/bootstrap.py#L12-L30)。

## 严重程度与顺序

这些问题集中在方法所有权和输入合同，基础没有发展成需要整体废弃的服务网。来源形状、语言准备、共同入口和有界物化在 Native 接入前完成；其余共用参数/时间/尝试规则在相应消费者迁移时同步完成。不要先写 Wave 的补偿包装再回头整理。

文档按本包的当前规则原地替换。现行测试保护的有效场景迁到新接口；旧结构专属支撑删除。用户继续独立运行和压力验证。
