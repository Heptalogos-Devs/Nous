# 模型资源与任务分配

## 责任

模型资源描述可以调用什么、具有哪些能力以及任务选择什么资源。它属于共享模型基础。Journal、Steward 等领域定义自己的任务语义，并消费模型能力。

本专题纯配置在首轮登记；实际客户端和能力探测在模型计划中接入。没有配置模型是合法状态，确定性基础能力仍可使用。

## 资源定义

`ProviderConfig`：provider ID、实际 API protocol、endpoint、credential_ref，以及对应网络配置。协议必须明确，不能由模型显示名称猜测。第一实现支持正式 OpenAI-compatible 网关；其他协议复用 Pydantic AI 已有 adapter，按实际配置启用。

`ModelConfig`：资源 ID、provider ID、实际 model_name、明确的部署/版本信息，以及 text、image、audio、video、stream、structured_output、tools、embedding 等真实能力声明。保存上下文窗口和可用输出上限，未知能力保持 unknown；未声明支持时不凭名称猜测。

`PresetConfig`：温度、推理参数、输出预算、超时及协议支持的扩展参数。不支持的参数在准备阶段报错或按明确支持策略省略并解释，不能静默当作已生效。

`TaskAssignment`：任务 key、主模型、preset、Prompt 资源、fallback 候选、预算和媒体处理策略。任务名称表示用途，如 journalize；system/user 等表示消息角色，两者分别使用。

## 配置键与形状

首轮纯配置合同使用以下键。Provider、Model、Preset、Task 的 ID 都是配置内 slug；实际模型名保持 provider 原样。

| 对象 | 必需项 | 可选项及缺省 |
|---|---|---|
| providers.<id> | protocol、endpoint | credential_ref=null，网络覆盖只允许网络专题字段 |
| models.<id> | provider、model_name | capabilities=[]，context_window=null，max_output_tokens=null，deployment_revision=null |
| presets.<id> | 无，允许空预设 | temperature、top_p、reasoning 等在实际协议支持时提供；缺失交给实际模型默认并记录 |
| tasks.<id> | model、prompt | preset=null，fallback=[]，budget={}，media_strategy=null |

首个可执行 protocol 是 `openai-chat-completions`。后续采用的 SDK adapter 可以注册自己的明确 protocol 名，未知 protocol 在资源执行准备时显示 unavailable；配置层保留字符串但不会假造支持。capabilities 枚举只表达已经确认的能力，空集合表示未声明，不推断为全部能力。

fallback 每项为 `{model, preset?}`，按顺序使用；preset 缺失继承该任务的预设并重新校验兼容性。budget 可包含 max_attempts、timeout_seconds、max_input_tokens、max_output_tokens 和可选金额限制。资源引用必须在同一解析快照中存在。

首轮校验配置形状及 Provider/Model/Preset/Task 引用。Prompt 文件与具体任务输入合同尚未接入时，输出明确说明执行准备检查未运行；不把纯配置通过写成模型已可调用。模型计划接入 Prompt 后再完成对应检查。

## 确定与发现

程序根据任务需要与资源能力选择兼容模型。资源配置需要人或可信管理来源确认。`model probe` 是显式操作，可查看实际连接和输出维度；它可能产生费用，初始化、status、配置读取不隐式调用模型。

文本生成能力与混合媒体 embedding 各有协议。官方库已经支持的部分直接使用。特有能力只有在真实消费者需要时采用官方 SDK 接入，不复制整个生成执行器。

## 版本与变更

文本生成资源的更换影响之后的操作；已准备或已发生请求记录真实资源。模型 API 的别名可能指向不同部署，长期向量空间因此需要单独的[绑定合同](embeddings.md)。

资源删除前检查被哪些任务和备用链引用。配置目录能够解释资源不足、缺少凭据和未启用功能。显示名称可变，实际调用身份必须保持真实。

## 接口

```text
ModelRegistry.list() -> ModelResourceInfo[]
ModelRegistry.resolve(task, config_snapshot) -> ResolvedModelPlan
ModelRegistry.probe(resource, explicit_budget) -> ProbeResult
```

resolve 只选择并校验，不联网。probe 通过相同 ModelExecutor/EmbeddingExecutor 执行真实请求并返回用量，不建设旁路 SDK 客户端。

来源：[Pydantic AI Models](https://ai.pydantic.dev/models/overview/)、[Direct requests](https://ai.pydantic.dev/direct/)。
