# Managed OpenClaw

[返回合同目录](../../INDEX.md)

## 所有权

Nous 提供唯一的长期认知。官方 Client 的 `./managed` 子入口提供框架无关 prepare/apply/report/close；`packages/openclaw` 只映射原生循环、受众和实际结果。Heptalogos 可以装载同一发行物，其重建、数据库及旧 AIRuntime 不是 Nous 集成的前提。

Subject 是独立持久个体，创建可省略 Seed；没有 Seed 不生成个人经历、默认人格或可写 SOUL 镜像。初始源文档的保存/adoption 与后续 Self 形成分别处理。Self/Social/LanguageConvention/Motivation 的完整对象功能按长期设计继续实现。

## 实际调用

每次原生模型提交在 convertToLlm 后、provider stream 前运行 managed binding。首轮、工具后续、重试、压缩后的下一轮、原生子 Agent 和完成回告均经过该入口。必需模式不允许 context-engine registry、logical-turn、assemble 或其他 harness 失败后退回 legacy。安装器将修正入口应用到哈希固定的 OpenClaw 2026.9.1；漂移/部分补丁拒绝安装。

`CognitionUsage` 从实际执行者、用途、当前意图、参与者与受众形成。它进入已有 Query binding 和 ContextCompiler track；不授予管理权限。adapter 从原生可信 chatType/channel/sender/destination 获取来源，未解析的人物保留 unresolved；不自行分配 Entity。身份、principal、Subject 与受众不从模型工具参数取得。

Host 保有历史、工具 schema、输出预留和 provider controls。renderer 只在干净的调用输入上应用一个 Nous slice，固定指引说明认知、证据和归属的使用方式；authority 不转换为管理/工具权限。slice 不持久写入 transcript。原生 system prompt 不再加载 SOUL/IDENTITY/USER/MEMORY/BOOTSTRAP 为第二份当前认知。

## 同步与预算

prepare 复用 BuildManagedContext；apply 成功后才推进本 binding 的 cursor。RESET 替换全部 slice；APPEND 仅接受相同 track/epoch 和连续 revision。受众、用途、执行者、WorkContext 或内容变化会改变 scope/RESET。并行 worker 分开保存临时状态。

Host 按所选模型的上下文窗口、输出预留、原生完整输入估计剩余预算，再取 cognition cap 的较小值。当前 token 计量是 OpenClaw native estimate；Nous 文本字节上限用于有余量的输入控制，不声称精确 tokenizer。renderer 核验最终输入，超预算停止调用。

WorkContext 和 identity/situational guidance 必须完整；不足时拒绝。Memory/Schema/Episode/Journal/Material 等可选语义单元整段省略并报告，不截断条件。REQUIRED 意味着认知 owner/capability 必须可用，不意味着无相关认知的新主体必须凭空有一段记忆。Kernel 核验必需 Memory 的进程和 Subject 能力；来源读取或传输失败照实际错误处理。

## 原生循环与结果

OpenClaw 继续拥有工具执行、渠道、Computer use、skills、子 Agent 和 transcript。压缩显式委派 `delegateCompactionToRuntime`；其输入不含 Nous slice，下一轮重新应用。默认 memory flush 对主体实例关闭，避免第二套可写认知。

原生 session extension 仅保存 Subject/nativeSession/NousSession 身份，不保存 cognition 或 cursor。重启复用未关闭的对应 Session；native reset 使用新 Nous Session。子 Agent 同 Subject、独立 Session/binding，以隔离任务交接，不能经模型要求 fork 父历史。

记录实际 user/model/tool 事件为 Observation；工具结果包含结构、details 与 isError，并在原生结果发出后立即报告，覆盖终止工具。工具接触可 admit 成为经历；普通 transcript 不自动成为 accepted Memory。模型输出标为 generation/model_output，不声称渠道已发送。流式失败/取消报告实际 invocation outcome。

ReportUse 仅把实际输入的 exact cognition refs 记为 presented。模型产生引用或文本不证明其内部因果、选用或真实外发。Heptalogos 后续对实际行为/外发许可及 effect/messaging outcome 负责，不能以 best-effort message hook 替代正式许可入口。

普通聊天“记住/忘记”不拥有认知 mutation/purge 能力。工具可见性、执行政策、凭据和 OS 范围由宿主明确配置；插件不向普通主体暴露 Core discovery 管理凭据。运维 OpenClaw 使用独立实例及权限。

## 发行与验证

[包说明](../../../../packages/openclaw/README.md) 是安装/配置命令的唯一完整入口。`openclaw:pack` 生成 bundled 插件及安装器 tarball，宿主原样安装；`openclaw:prepare` 使用同一产物安装到开发实例。

`smoke:openclaw` 使用真实模型 gateway 与正式安装的原生 Gateway，核验最终 provider 输入、工具后续、RESET、原生 compact、子 Agent、真实 Material 回流、必需 Core 不可用时零发送及恢复。准备/应用日志不冒充已传输证明。未测试的渠道、Computer use 平台和语言人格质量不由该接线场景推定。
