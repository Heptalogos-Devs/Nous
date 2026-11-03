# 模型与 Prompt 实施计划

## 开始条件

运行基础完成；文字任务可先推进，持久媒体结果依赖已经完成的内容存储。只有明确需要长任务恢复的调用才接入 DBOS，普通前台生成无需先转成持久工作。

本计划单独启动。它交付模型基础能力及管理操作，不包含完整 journalize、Dream、Steward 认知流程。

## 负责专题

先读 [凭据](../design/foundation/credentials.md)、[网络](../design/foundation/networking.md)、[资源](../design/models/resources.md)、[Prompt](../design/models/prompts.md)、[执行](../design/models/execution.md)、[预算](../design/models/budgets.md)、[fallback](../design/models/fallback.md)、[缓存](../design/models/caching.md)、[向量空间](../design/models/embeddings.md)。

## 完整建设路径

资源配置先接入真实客户端和凭据解析。Provider 通过正式网关或已支持协议工作；相同连接复用，网络策略从共同配置取得。

PromptCatalog 加载包内与用户模板，用 Jinja 生成完整有角色的消息。管理端能查看、编辑、校验和预览。首个真实调用可采用管理员的文本整理 Prompt；它是 Prompt 试运行能力，不提前创建 Journal 对象。

ModelExecutor 执行已经准备的请求，统一结果、用量、流、预算和备用尝试。结构化生成使用成熟库输出能力，配一个确实需要的简单结果合同验证集成；不为测试创建专属生产导出口。

EmbeddingExecutor 接入真实文本向量，并完整落实空间定义、维度检查与绑定。多模态 embedding 在已配置模型和实际协议支持时接入；不以文本描述向量冒充视觉向量。Wave 索引和跨空间迁移继续由后续检索计划负责。

## 管理与消费

CLI 增加模型资源列表、Prompt 查看/预览、明确的模型试运行和 Embedding probe。所有操作调用同一内核。试运行展示真实模型、请求/输出及用量；敏感输入按显式诊断策略处理。

配置或 status 不隐式发出付费请求。真实模型验证仅使用用户提供的连接、凭据与预算，不替用户注册服务或消耗未授权费用。

## 完成条件

真实文字与一个结构化任务、Prompt 预览对应实际输入、流取消、一次显式 fallback、预算总计、实际 embedding 及绑定拒绝均有结果。无模型配置时基础管理可用。未获得真实服务条件的部分准确列明，不能以 Mock 通过宣称完成真实调用。

完成本计划后再依据相应认知专题接入 journalize、Steward 和 Dream。新增语义改变当前基础时，直接重构负责者。
